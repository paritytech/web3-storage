// SPDX-License-Identifier: GPL-3.0-only

/**
 * The product account a TrUAPI host gives photos, as a PAPI signer.
 *
 * Transactions go through the host's `createTransaction`, which needs every
 * extension already encoded, so PAPI's nonce and common-extension enhancers
 * fill them first. The host writes them in the order it receives them, so they
 * are sent in the order the runtime declares. Fee estimation builds a
 * fake-signed transaction locally and never reaches the host. Raw bytes (the
 * provider's request `Authorization`) go through `signRaw`, which the host
 * wraps in `<Bytes>…</Bytes>`; the provider verifier accepts that form.
 */

import {
  asProductId,
  type CallErrorValue,
  type HexString,
  type ProductAccountId,
  type TxPayloadExtension,
} from '@parity/truapi'
import { getClientSync } from '@parity/truapi/sandbox'
import {
  withCommonExtensions,
  withNonce,
  type CommonEnhancersSpecs,
  type CommonSignerTxCreator,
} from '@polkadot-api/signers-common'
import { decAnyMetadata, unifyMetadata } from '@polkadot-api/substrate-bindings'
import { getFakeTxCreator, type TxCreator } from 'polkadot-api/tx-creator'
import { fromHex, toHex } from 'polkadot-api/utils'

/** Must match the `--product-id` the host runs with; the host signs for no other. */
const PRODUCT_ID = 'photos.dot'

const PRODUCT_ACCOUNT: ProductAccountId = {
  dotNsIdentifier: asProductId(PRODUCT_ID),
  derivationIndex: { tag: 'Index', value: 0 },
}

export interface HostAccount {
  publicKey: Uint8Array
  txCreator: CommonSignerTxCreator
}

/** Whether photos runs inside a TrUAPI host. */
export function isInHost(): boolean {
  return getClientSync() !== null
}

export async function getHostAccount(): Promise<HostAccount> {
  const client = requireClient()

  const account = await client.account.getAccount({ productAccountId: PRODUCT_ACCOUNT })
  if (account.isErr()) throw hostError('getAccount', account.error)
  const publicKey = fromHex(account.value.account.publicKey)

  const createWithHost: TxCreator<[]> = async (input) => {
    const result = await client.signing.createTransaction({
      signer: PRODUCT_ACCOUNT,
      genesisHash: input.context.genesisHash as HexString,
      callData: input.callData as HexString,
      extensions: inDeclaredOrder(input),
      txExtVersion: input.txExtVersion ?? 0,
      contacts: [],
    })
    if (result.isErr()) throw hostError('createTransaction', result.error)
    return result.value.transaction
  }
  const hostCreator = withNonce(publicKey)(withCommonExtensions(createWithHost))
  const fakeCreator = getFakeTxCreator(publicKey)

  const signBytes = async (data: Uint8Array): Promise<Uint8Array> => {
    const result = await client.signing.signRaw({
      account: PRODUCT_ACCOUNT,
      payload: { tag: 'Bytes', value: { bytes: toHex(data) as HexString } },
    })
    if (result.isErr()) throw hostError('signRaw', result.error)
    return fromHex(result.value.signature)
  }

  const create: TxCreator<CommonEnhancersSpecs> = (input, options, bindings, mockedSignature) =>
    (mockedSignature ? fakeCreator : hostCreator)(input, options, bindings, mockedSignature)

  return { publicKey, txCreator: Object.assign(create, { publicKey, signBytes }) }
}

/**
 * The input's extensions in the order the runtime declares them. PAPI's
 * enhancers append the nonce first, and the runtime signs and reads extension
 * bytes in its own order.
 */
function inDeclaredOrder(input: Parameters<TxCreator>[0]): TxPayloadExtension[] {
  const metadata = unifyMetadata(decAnyMetadata(input.context.metadata))
  const declared = metadata.extrinsic.extensionsByVersion[input.txExtVersion ?? 0] ?? []
  return declared.flatMap(({ identifier }) => {
    const extension = input.extensions.find(({ id }) => id === identifier)
    if (!extension) return []
    return [
      {
        id: extension.id,
        extra: extension.extra as HexString,
        additionalSigned: extension.additionalSigned as HexString,
      },
    ]
  })
}

function requireClient() {
  const client = getClientSync()
  if (!client) throw new Error('Not running inside a TrUAPI host')
  return client
}

function hostError(method: string, error: CallErrorValue<unknown>): Error {
  return new Error(`host ${method} failed: ${JSON.stringify(error)}`)
}
