// SPDX-License-Identifier: GPL-3.0-only

/**
 * A PAPI `JsonRpcProvider` that reaches one chain through the TrUAPI host's
 * chain service instead of a WebSocket of its own.
 *
 * The host's chain calls mirror the new JSON-RPC spec one to one, so each
 * `chainHead_v1_*`, `chainSpec_v1_*` and `transaction_v1_*` request becomes the
 * matching call and each follow item the matching event. A follow is
 * identified to PAPI by the TrUAPI subscription id, which is the value the
 * host expects back as `followSubscriptionId`. The host can deliver an
 * operation's events before the reply that starts it, while PAPI drops events
 * for an operation it has not been told about, so they are held until then.
 * The host serves only chains its stack declares, so `genesisHash` must be one
 * of them.
 */

import type {
  CallErrorValue,
  HexString,
  OperationStartedResult,
  RemoteChainHeadFollowItem,
  RuntimeType,
  StorageQueryType,
  Subscription,
} from '@parity/truapi'
import { getClientSync } from '@parity/truapi/sandbox'
import type { JsonRpcProvider } from 'polkadot-api'

type JsonRpcRequest = Parameters<ReturnType<JsonRpcProvider>['send']>[0]
type JsonRpcMessage = Parameters<Parameters<JsonRpcProvider>[0]>[0]
type JsonRpcId = JsonRpcRequest['id']

/** The part of a host call's `ResultAsync` this provider uses. */
interface HostCall<Response> {
  match(ok: (response: Response) => void, error: (error: CallErrorValue<unknown>) => void): unknown
}

const INTERNAL_ERROR = -32603
const METHOD_NOT_FOUND = -32601

const STORAGE_QUERY_TYPES: Record<string, StorageQueryType> = {
  value: 'Value',
  hash: 'Hash',
  closestDescendantMerkleValue: 'ClosestDescendantMerkleValue',
  descendantsValues: 'DescendantsValues',
  descendantsHashes: 'DescendantsHashes',
}

export function createHostChainProvider(genesisHash: HexString): JsonRpcProvider {
  const client = getClientSync()
  if (!client) throw new Error('Not running inside a TrUAPI host')
  const chain = client.chain

  return (onMessage: (message: JsonRpcMessage) => void) => {
    const follows = new Map<string, Subscription>()
    const broadcasts = new Set<string>()
    /** Operations whose start PAPI has been told about, keyed by follow and operation id. */
    const startedOperations = new Set<string>()
    /** Items of operations whose start PAPI has not been told about yet. */
    const heldItems = new Map<string, RemoteChainHeadFollowItem[]>()

    const respond = (id: JsonRpcId, result: unknown) => onMessage({ jsonrpc: '2.0', id: id ?? null, result })
    const fail = (id: JsonRpcId, code: number, message: string) =>
      onMessage({ jsonrpc: '2.0', id: id ?? null, error: { code, message } })
    const followEvent = (subscription: string, result: unknown) =>
      onMessage({ jsonrpc: '2.0', method: 'chainHead_v1_followEvent', params: { subscription, result } })

    /** Answer `id` with `toResult` of the host's response, or with its error. */
    const forward = <Response>(
      id: JsonRpcId,
      call: HostCall<Response>,
      toResult: (response: Response) => unknown,
    ) => {
      call.match(
        (response) => respond(id, toResult(response)),
        (error) => fail(id, INTERNAL_ERROR, JSON.stringify(error)),
      )
    }

    const emitOperationItem = (subscriptionId: string, key: string, item: RemoteChainHeadFollowItem) => {
      if (isFinal(item)) startedOperations.delete(key)
      followEvent(subscriptionId, toFollowEvent(item))
    }

    /** Answer `id` with an operation's start, then release the items that arrived first. */
    const forwardOperation = (
      id: JsonRpcId,
      followSubscriptionId: string,
      call: HostCall<{ operation: OperationStartedResult }>,
    ) => {
      call.match(
        ({ operation }) => {
          respond(id, toOperationResult(operation))
          if (operation.tag !== 'Started') return
          const key = `${followSubscriptionId}:${operation.value.operationId}`
          startedOperations.add(key)
          const held = heldItems.get(key) ?? []
          heldItems.delete(key)
          for (const item of held) emitOperationItem(followSubscriptionId, key, item)
        },
        (error) => fail(id, INTERNAL_ERROR, JSON.stringify(error)),
      )
    }

    const follow = (id: JsonRpcId, withRuntime: boolean) => {
      let subscriptionId = ''
      const subscription = chain.followHeadSubscribe({ request: { genesisHash, withRuntime } }).subscribe({
        next: (item) => {
          if (item.tag === 'Stop') follows.delete(subscriptionId)
          const operationId = operationIdOf(item)
          if (operationId === undefined) return followEvent(subscriptionId, toFollowEvent(item))
          const key = `${subscriptionId}:${operationId}`
          if (startedOperations.has(key)) return emitOperationItem(subscriptionId, key, item)
          heldItems.set(key, [...(heldItems.get(key) ?? []), item])
        },
        error: () => {
          if (follows.delete(subscriptionId)) followEvent(subscriptionId, { event: 'stop' })
        },
      })
      subscriptionId = subscription.subscriptionId
      follows.set(subscriptionId, subscription)
      respond(id, subscriptionId)
    }

    const handle = ({ id, method, params = [] }: JsonRpcRequest) => {
      switch (method) {
        case 'chainHead_v1_follow':
          return follow(id, params[0])
        case 'chainHead_v1_unfollow': {
          follows.get(params[0])?.unsubscribe()
          follows.delete(params[0])
          return respond(id, null)
        }
        case 'chainHead_v1_header': {
          const [followSubscriptionId, hash] = params
          return forward(id, chain.getHeadHeader({ genesisHash, followSubscriptionId, hash }), (response) => response.header ?? null)
        }
        case 'chainHead_v1_body': {
          const [followSubscriptionId, hash] = params
          return forwardOperation(id, followSubscriptionId, chain.getHeadBody({ genesisHash, followSubscriptionId, hash }))
        }
        case 'chainHead_v1_storage': {
          const [followSubscriptionId, hash, items, childTrie] = params
          const queryItems = (items as Array<{ key: HexString; type: string }>).map((item) => ({
            key: item.key,
            queryType: STORAGE_QUERY_TYPES[item.type] ?? 'Value',
          }))
          // PAPI sends `null` for no child trie; the optional field takes `undefined`.
          const request = { genesisHash, followSubscriptionId, hash, items: queryItems, childTrie: childTrie ?? undefined }
          return forwardOperation(id, followSubscriptionId, chain.getHeadStorage(request))
        }
        case 'chainHead_v1_call': {
          const [followSubscriptionId, hash, fn, callParameters] = params
          const request = { genesisHash, followSubscriptionId, hash, function: fn, callParameters }
          return forwardOperation(id, followSubscriptionId, chain.callHead(request))
        }
        case 'chainHead_v1_unpin': {
          const [followSubscriptionId, hashOrHashes] = params
          const hashes = Array.isArray(hashOrHashes) ? hashOrHashes : [hashOrHashes]
          return forward(id, chain.unpinHead({ genesisHash, followSubscriptionId, hashes }), () => null)
        }
        case 'chainHead_v1_continue': {
          const [followSubscriptionId, operationId] = params
          return forward(id, chain.continueHead({ genesisHash, followSubscriptionId, operationId }), () => null)
        }
        case 'chainHead_v1_stopOperation': {
          const [followSubscriptionId, operationId] = params
          return forward(id, chain.stopHeadOperation({ genesisHash, followSubscriptionId, operationId }), () => null)
        }
        case 'chainSpec_v1_genesisHash':
          return forward(id, chain.getSpecGenesisHash({ genesisHash }), (response) => response.genesisHash)
        case 'chainSpec_v1_chainName':
          return forward(id, chain.getSpecChainName({ genesisHash }), (response) => response.chainName)
        case 'chainSpec_v1_properties':
          return forward(id, chain.getSpecProperties({ genesisHash }), (response) => JSON.parse(response.properties))
        case 'transaction_v1_broadcast':
          return forward(id, chain.broadcastTransaction({ genesisHash, transaction: params[0] }), (response) => {
            if (response.operationId !== undefined) broadcasts.add(response.operationId)
            return response.operationId ?? null
          })
        case 'transaction_v1_stop': {
          broadcasts.delete(params[0])
          return forward(id, chain.stopTransaction({ genesisHash, operationId: params[0] }), () => null)
        }
        default:
          return fail(id, METHOD_NOT_FOUND, `${method} is not served by the host`)
      }
    }

    return {
      send(request) {
        try {
          handle(request)
        } catch (error) {
          fail(request.id, INTERNAL_ERROR, error instanceof Error ? error.message : String(error))
        }
      },
      disconnect() {
        for (const subscription of follows.values()) subscription.unsubscribe()
        follows.clear()
        for (const operationId of broadcasts) chain.stopTransaction({ genesisHash, operationId })
        broadcasts.clear()
      },
    }
  }
}

function operationIdOf(item: RemoteChainHeadFollowItem): string | undefined {
  switch (item.tag) {
    case 'OperationBodyDone':
    case 'OperationCallDone':
    case 'OperationStorageItems':
    case 'OperationStorageDone':
    case 'OperationWaitingForContinue':
    case 'OperationInaccessible':
    case 'OperationError':
      return item.value.operationId
    default:
      return undefined
  }
}

/** Whether `item` is the last event of its operation. */
function isFinal(item: RemoteChainHeadFollowItem): boolean {
  switch (item.tag) {
    case 'OperationBodyDone':
    case 'OperationCallDone':
    case 'OperationStorageDone':
    case 'OperationInaccessible':
    case 'OperationError':
      return true
    default:
      return false
  }
}

function toOperationResult(operation: OperationStartedResult) {
  return operation.tag === 'Started'
    ? { result: 'started', operationId: operation.value.operationId }
    : { result: 'limitReached' }
}

function toRuntime(runtime: RuntimeType | undefined) {
  if (!runtime) return null
  if (runtime.tag === 'Invalid') return { type: 'invalid', error: runtime.value.error }
  const { apis, ...spec } = runtime.value
  return { type: 'valid', spec: { ...spec, apis: Object.fromEntries(apis.map((api) => [api.name, api.version])) } }
}

function toFollowEvent(item: RemoteChainHeadFollowItem) {
  switch (item.tag) {
    case 'Initialized':
      return {
        event: 'initialized',
        finalizedBlockHashes: item.value.finalizedBlockHashes,
        finalizedBlockRuntime: toRuntime(item.value.finalizedBlockRuntime),
      }
    case 'NewBlock':
      return {
        event: 'newBlock',
        blockHash: item.value.blockHash,
        parentBlockHash: item.value.parentBlockHash,
        newRuntime: toRuntime(item.value.newRuntime),
      }
    case 'BestBlockChanged':
      return { event: 'bestBlockChanged', ...item.value }
    case 'Finalized':
      return { event: 'finalized', ...item.value }
    case 'OperationBodyDone':
      return { event: 'operationBodyDone', ...item.value }
    case 'OperationCallDone':
      return { event: 'operationCallDone', ...item.value }
    case 'OperationStorageItems':
      return { event: 'operationStorageItems', ...item.value }
    case 'OperationStorageDone':
      return { event: 'operationStorageDone', ...item.value }
    case 'OperationWaitingForContinue':
      return { event: 'operationWaitingForContinue', ...item.value }
    case 'OperationInaccessible':
      return { event: 'operationInaccessible', ...item.value }
    case 'OperationError':
      return { event: 'operationError', ...item.value }
    case 'Stop':
      return { event: 'stop' }
  }
}
