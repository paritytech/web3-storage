// SPDX-License-Identifier: GPL-3.0-only

/**
 * Browser port of the MutableNotebook PAPI library. Mirrors
 * `examples/papi/notebook.js` so the UI and the CLI demo speak to the same
 * contract + provider endpoints in the same way. Pluggable
 * `api`/`signer`/`providerUrl` so this file is wallet-agnostic.
 */

import { ss58Address } from "@polkadot-labs/hdkd-helpers";
import type { PolkadotSigner } from "polkadot-api";
import {
  decodeEventLog,
  encodeFunctionData,
  keccak256,
  type Abi,
  type DecodeEventLogReturnType,
} from "viem";

export const CONTRACT_KEY = "MutableNotebook.sol:MutableNotebook";

// PAPI is heavily generic. Rather than ship typed bindings here (which would
// require a circular dep on @polkadot-api/descriptors), use a structural any
// for the typed-api handle and let callers' usages type-check at call sites.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type TypedApi = any;

const DEFAULT_GAS_LIMIT = {
  ref_time: 1_000_000_000_000n,
  proof_size: 4_000_000n,
};
const DEFAULT_STORAGE_DEPOSIT_LIMIT = 10n ** 18n;

// ─────────────────────────────────────────────────────────────────────────────
// Encoding utilities
// ─────────────────────────────────────────────────────────────────────────────

export function toHex(bytes: Uint8Array | ArrayLike<number>): `0x${string}` {
  const arr = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let out = "0x";
  for (const b of arr) out += b.toString(16).padStart(2, "0");
  return out as `0x${string}`;
}

export function hexToBytes(hex: string): Uint8Array {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

/** keccak256(utf8(key)), matching the contract's indexed `keyHash` topic. */
export function keyHash(key: string): `0x${string}` {
  return keccak256(toHex(new TextEncoder().encode(key)));
}

/** Substrate account `AccountId32Mapper` assigns to an unmapped H160: 20
 * address bytes + 12 bytes of `0xEE`. Used as the `owner` of negotiated
 * agreement terms when a contract redeems them. */
export function h160ToSubstrate(addressBytes: Uint8Array): {
  publicKey: Uint8Array;
  address: string;
} {
  const publicKey = new Uint8Array(32).fill(0xee);
  publicKey.set(addressBytes, 0);
  return { publicKey, address: ss58Address(publicKey) };
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider HTTP
// ─────────────────────────────────────────────────────────────────────────────

export interface UploadResult {
  cid: `0x${string}`;
  size: number;
  etag: string;
}

export async function uploadObject(
  providerUrl: string,
  bucketId: bigint,
  key: string,
  content: Uint8Array | Blob,
  contentType = "application/octet-stream",
): Promise<UploadResult> {
  const url = `${providerUrl}/s3/${bucketId}/object?key=${encodeURIComponent(key)}`;
  const resp = await fetch(url, {
    method: "PUT",
    headers: { "content-type": contentType },
    body: content as BodyInit,
  });
  if (!resp.ok) {
    throw new Error(`upload ${key} failed: ${resp.status} ${await resp.text()}`);
  }
  const body = await resp.json();
  return {
    cid: body.data_root,
    size: Number(body.size),
    etag: body.etag,
  };
}

/** Reassemble bytes by CID. Works post-pointer-flip for any historical
 * revision still resident on the provider. */
export async function fetchByCid(
  providerUrl: string,
  cid: string,
): Promise<Uint8Array> {
  const resp = await fetch(
    `${providerUrl}/content?data_root=${encodeURIComponent(cid)}`,
  );
  if (!resp.ok) {
    throw new Error(`fetch ${cid} failed: ${resp.status} ${await resp.text()}`);
  }
  return new Uint8Array(await resp.arrayBuffer());
}

export async function fetchByKey(
  providerUrl: string,
  bucketId: bigint,
  key: string,
): Promise<Uint8Array> {
  const url = `${providerUrl}/s3/${bucketId}/object?key=${encodeURIComponent(key)}`;
  const resp = await fetch(url);
  if (!resp.ok) {
    throw new Error(`fetch ${key} failed: ${resp.status} ${await resp.text()}`);
  }
  return new Uint8Array(await resp.arrayBuffer());
}

interface NegotiateRequest {
  owner: string;
  max_bytes: string;
  duration: number;
  price_per_byte: string;
  replica_params: null;
  bucket_id: null;
}

interface SignedTerms {
  terms: {
    max_bytes: string | number;
    duration: number;
    price_per_byte: string | number;
    valid_until: number;
    nonce: string | number;
    bucket_id: number | null;
    replica_params: {
      sync_balance: string | number;
      min_sync_interval: number;
      sync_price: string | number;
    } | null;
  };
  signature: string;
}

/** POST /negotiate on the provider node, then shape the response for the
 * PrimitiveAgreementTerms ABI struct. */
export async function negotiatePrecompileTerms(
  providerUrl: string,
  owner: { publicKey: Uint8Array; address: string },
  {
    maxBytes,
    duration,
    pricePerByte,
  }: { maxBytes: bigint; duration: number; pricePerByte: bigint },
) {
  const req: NegotiateRequest = {
    owner: owner.address,
    max_bytes: maxBytes.toString(),
    duration,
    price_per_byte: pricePerByte.toString(),
    replica_params: null,
    bucket_id: null,
  };
  const resp = await fetch(`${providerUrl}/negotiate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(req),
  });
  if (!resp.ok) {
    throw new Error(
      `/negotiate failed: ${resp.status} ${await resp.text().catch(() => "")}`,
    );
  }
  const signed: SignedTerms = await resp.json();
  const t = signed.terms;
  const rp = t.replica_params;
  return {
    terms: {
      owner: toHex(owner.publicKey),
      maxBytes: BigInt(t.max_bytes),
      duration: Number(t.duration),
      pricePerByte: BigInt(t.price_per_byte),
      validUntil: Number(t.valid_until),
      nonce: BigInt(t.nonce),
      hasReplicaParams: rp != null,
      replicaParams: {
        syncBalance: BigInt(rp?.sync_balance ?? 0),
        minSyncInterval: Number(rp?.min_sync_interval ?? 0),
        syncPrice: BigInt(rp?.sync_price ?? 0),
      },
      hasBucketId: t.bucket_id != null,
      bucketId: BigInt(t.bucket_id ?? 0),
    },
    signature: signed.signature.startsWith("0x")
      ? (signed.signature as `0x${string}`)
      : (`0x${signed.signature}` as `0x${string}`),
  };
}

// ─────────────────────────────────────────────────────────────────────────────
// Revive helpers
// ─────────────────────────────────────────────────────────────────────────────

interface TxResult {
  // Loose type — PAPI's event shape; we only ever filter through .filter()
  // accessors or scan for ContractEmitted, both of which work duck-typed.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  events: any[];
}

async function submitTx(
  tx: { signSubmitAndWatch: (signer: PolkadotSigner) => unknown },
  signer: PolkadotSigner,
  label: string,
): Promise<TxResult> {
  console.log(`[${label}] submitting…`);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const observable = tx.signSubmitAndWatch(signer) as any;
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error(`${label}: timeout after 180s`)),
      180_000,
    );
    const sub = observable.subscribe({
      next: (ev: {
        type: string;
        found?: boolean;
        ok?: boolean;
        events?: unknown[];
        dispatchError?: unknown;
      }) => {
        console.log(`[${label}] event:`, ev);
        if (ev.type === "txBestBlocksState" && ev.found) {
          clearTimeout(timeout);
          sub.unsubscribe();
          if (ev.ok === false) {
            reject(
              new Error(
                `${label} dispatch failed: ${JSON.stringify(ev.dispatchError)}`,
              ),
            );
          } else {
            resolve({ events: (ev.events ?? []) as TxResult["events"] });
          }
        }
      },
      error: (err: unknown) => {
        clearTimeout(timeout);
        console.warn(`[${label}] error:`, err);
        reject(new Error(`${label} stream error: ${err}`));
      },
    });
  });
}

export async function deployContract(
  api: TypedApi,
  signer: PolkadotSigner,
  bytecode: Uint8Array,
): Promise<{ address: `0x${string}`; addressBytes: Uint8Array; events: unknown[] }> {
  const tx = api.tx.Revive.instantiate_with_code({
    value: 0n,
    weight_limit: DEFAULT_GAS_LIMIT,
    storage_deposit_limit: DEFAULT_STORAGE_DEPOSIT_LIMIT,
    code: bytecode,
    data: new Uint8Array(),
    salt: undefined,
  });
  const result = await submitTx(tx, signer, "Revive.instantiate_with_code");
  const instantiated = api.event.Revive.Instantiated.filter(result.events);
  if (instantiated.length !== 1) {
    throw new Error(
      `expected 1 Revive.Instantiated event, got ${instantiated.length}`,
    );
  }
  // PAPI 2.x wraps each filter match as { original, payload }.
  const contractField = instantiated[0].payload?.contract ?? instantiated[0].contract;
  const addrBytes: Uint8Array =
    typeof contractField === "string" ? hexToBytes(contractField) : contractField;
  return { address: toHex(addrBytes), addressBytes: addrBytes, events: result.events };
}

export async function callContract(
  api: TypedApi,
  signer: PolkadotSigner,
  contractAddressBytes: Uint8Array,
  data: Uint8Array,
  { value = 0n }: { value?: bigint } = {},
): Promise<TxResult> {
  const tx = api.tx.Revive.call({
    // `dest` is SizedHex<20> in the descriptor — a hex string, not Uint8Array.
    dest: toHex(contractAddressBytes),
    value,
    weight_limit: DEFAULT_GAS_LIMIT,
    storage_deposit_limit: DEFAULT_STORAGE_DEPOSIT_LIMIT,
    data,
  });
  return submitTx(tx, signer, "Revive.call");
}

export function encodeCall(abi: Abi, functionName: string, args: unknown[]): Uint8Array {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return hexToBytes(encodeFunctionData({ abi, functionName, args } as any));
}

/** Find `Revive.ContractEmitted` events emitted by `addressBytes` and decode
 * them against `abi`. Returns viem-decoded `{ eventName, args }` records. */
export function decodeContractEmitted(
  api: TypedApi,
  events: unknown[],
  addressBytes: Uint8Array,
  abi: Abi,
): DecodeEventLogReturnType[] {
  const out: DecodeEventLogReturnType[] = [];
  const matched = api.event.Revive.ContractEmitted.filter(events);
  for (const m of matched) {
    const payload = m.payload ?? m;
    const contractField = payload.contract;
    const emitterBytes: Uint8Array =
      typeof contractField === "string" ? hexToBytes(contractField) : contractField;
    if (!bytesEq(emitterBytes, addressBytes)) continue;
    const dataField = payload.data;
    const dataBytes: Uint8Array =
      typeof dataField === "string" ? hexToBytes(dataField) : dataField;
    const topicsRaw = (payload.topics ?? []) as Array<string | Uint8Array>;
    const topicsBytes = topicsRaw.map((t) =>
      typeof t === "string" ? hexToBytes(t) : t,
    );
    try {
      out.push(
        decodeEventLog({
          abi,
          data: toHex(dataBytes),
          topics: topicsBytes.map(toHex) as [signature: `0x${string}`, ...args: `0x${string}`[]],
        }),
      );
    } catch {
      // unknown event (not in this ABI) — skip
    }
  }
  return out;
}

function bytesEq(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

// ─────────────────────────────────────────────────────────────────────────────
// NotebookClient
// ─────────────────────────────────────────────────────────────────────────────

export interface FileEvent {
  eventName: "FileCreated" | "FileUpdated" | "FileDeleted";
  // viem decodes named args into this object
  args: Record<string, unknown>;
}

export class NotebookClient {
  constructor(
    readonly api: TypedApi,
    readonly signer: PolkadotSigner,
    readonly providerUrl: string,
    readonly abi: Abi,
    readonly address: `0x${string}`,
    readonly addressBytes: Uint8Array,
    readonly s3BucketId: bigint,
  ) {}

  static async deploy(opts: {
    api: TypedApi;
    signer: PolkadotSigner;
    providerUrl: string;
    providerPublicKey: `0x${string}`;
    abi: Abi;
    bytecode: Uint8Array;
    bucketName: string;
    maxBytes: bigint;
    duration: number;
    pricePerByte: bigint;
    value: bigint;
  }): Promise<NotebookClient> {
    console.log("[deploy] step 1: instantiate contract");
    const deployed = await deployContract(opts.api, opts.signer, opts.bytecode);
    console.log("[deploy] contract instantiated at", deployed.address);
    const contractAccount = h160ToSubstrate(deployed.addressBytes);
    console.log("[deploy] step 2: negotiate terms with provider", contractAccount.address);
    const signed = await negotiatePrecompileTerms(opts.providerUrl, contractAccount, {
      maxBytes: opts.maxBytes,
      duration: opts.duration,
      pricePerByte: opts.pricePerByte,
    });
    console.log("[deploy] terms signed:", signed);
    console.log("[deploy] step 3: call initialize()");
    const initData = encodeCall(opts.abi, "initialize", [
      opts.bucketName,
      opts.providerPublicKey,
      signed.terms,
      signed.signature,
    ]);
    const r = await callContract(opts.api, opts.signer, deployed.addressBytes, initData, {
      value: opts.value,
    });
    console.log("[deploy] initialize landed; events:", r.events);
    const created = opts.api.event.S3Registry.S3BucketCreated.filter(r.events);
    if (created.length !== 1) {
      throw new Error(`expected 1 S3BucketCreated event, got ${created.length}`);
    }
    const bucketId = created[0].payload?.s3_bucket_id ?? created[0].s3_bucket_id;
    return new NotebookClient(
      opts.api,
      opts.signer,
      opts.providerUrl,
      opts.abi,
      deployed.address,
      deployed.addressBytes,
      bucketId,
    );
  }

  static attach(opts: {
    api: TypedApi;
    signer: PolkadotSigner;
    providerUrl: string;
    abi: Abi;
    address: `0x${string}`;
    s3BucketId: bigint;
  }): NotebookClient {
    return new NotebookClient(
      opts.api,
      opts.signer,
      opts.providerUrl,
      opts.abi,
      opts.address,
      hexToBytes(opts.address),
      opts.s3BucketId,
    );
  }

  async createFile(
    key: string,
    content: Uint8Array | Blob,
    contentType = "application/octet-stream",
  ): Promise<{ revision: number; cid: `0x${string}`; size: number; events: unknown[] }> {
    const { cid, size } = await uploadObject(this.providerUrl, this.s3BucketId, key, content, contentType);
    const data = encodeCall(this.abi, "createFile", [key, cid, BigInt(size), contentType]);
    const r = await callContract(this.api, this.signer, this.addressBytes, data);
    return { revision: 1, cid, size, events: r.events };
  }

  async updateFile(
    key: string,
    content: Uint8Array | Blob,
    contentType: string,
    expectedRevision: number,
    commitMessage: string,
  ): Promise<{ revision: number; cid: `0x${string}`; size: number; events: unknown[] }> {
    const { cid, size } = await uploadObject(this.providerUrl, this.s3BucketId, key, content, contentType);
    const data = encodeCall(this.abi, "updateFile", [
      key,
      cid,
      BigInt(size),
      contentType,
      expectedRevision,
      commitMessage,
    ]);
    const r = await callContract(this.api, this.signer, this.addressBytes, data);
    return { revision: expectedRevision + 1, cid, size, events: r.events };
  }

  fetchCurrentBytes(key: string): Promise<Uint8Array> {
    return fetchByKey(this.providerUrl, this.s3BucketId, key);
  }

  fetchBytesByCid(cid: string): Promise<Uint8Array> {
    return fetchByCid(this.providerUrl, cid);
  }

  decodeEvents(events: unknown[]): FileEvent[] {
    return decodeContractEmitted(this.api, events, this.addressBytes, this.abi) as unknown as FileEvent[];
  }

  /** Replay history of `key` from a list of per-tx event batches. The UI
   * appends a batch after every successful create/update tx. */
  historyFromBatches(eventBatches: unknown[][], key: string): FileEvent[] {
    const target = keyHash(key).toLowerCase();
    const out: FileEvent[] = [];
    for (const events of eventBatches) {
      for (const log of this.decodeEvents(events)) {
        if (
          ["FileCreated", "FileUpdated", "FileDeleted"].includes(log.eventName) &&
          (log.args.keyHash as string)?.toLowerCase() === target
        ) {
          out.push(log);
        }
      }
    }
    return out;
  }
}
