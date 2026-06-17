/**
 * Browser-clean library for talking to the MutableNotebook example contract.
 *
 * Designed so a UI can `import` this file directly — no Node-only deps
 * (`node:fs`, `process.argv`, etc.) — and inject the same PAPI client,
 * `PolkadotSigner`, contract ABI/bytecode, and provider HTTP URL that the
 * CLI demo passes in.
 *
 * Provides a `NotebookClient` class for stateful use (UI) plus the underlying
 * standalone helpers (demo + tests). Reads / writes byte content via the
 * provider's S3 HTTP API; reads / writes the mutable pointer + history via
 * the `MutableNotebook` contract through `pallet_revive`.
 */

import { keccak256, toHex } from "viem";

import {
  callContract,
  decodeContractEmitted,
  deployContract,
  encodeCall,
  h160ToSubstrate,
  negotiatePrecompileTerms,
} from "./sc-api.js";

/** Contract key inside `examples/contracts/build/combined.json`. */
export const CONTRACT_KEY = "MutableNotebook.sol:MutableNotebook";

/** keccak256(utf8(key)), matching the contract's `keyHash` topic. */
export function keyHash(key) {
  return keccak256(toHex(new TextEncoder().encode(key)));
}

/** Upload bytes to the provider's S3 HTTP API. Returns `{ cid, size, etag }`. */
export async function uploadObject(
  providerUrl,
  bucketId,
  key,
  content,
  contentType = "application/octet-stream"
) {
  const url = `${providerUrl}/s3/${bucketId}/object?key=${encodeURIComponent(key)}`;
  const resp = await fetch(url, {
    method: "PUT",
    headers: { "content-type": contentType },
    body: content,
  });
  if (!resp.ok) {
    throw new Error(`upload ${key} failed: ${resp.status} ${await resp.text()}`);
  }
  const body = await resp.json();
  return {
    cid: body.data_root,
    size: Number(body.size),
    etag: body.etag,
    leafIndex: Number(body.leaf_index),
  };
}

/** Fetch bytes by CID. Works for any historical revision still resident on
 * the provider, regardless of which key currently points at it. */
export async function fetchByCid(providerUrl, cid) {
  const url = `${providerUrl}/content?data_root=${encodeURIComponent(cid)}`;
  const resp = await fetch(url);
  if (!resp.ok) {
    throw new Error(`fetch ${cid} failed: ${resp.status} ${await resp.text()}`);
  }
  return new Uint8Array(await resp.arrayBuffer());
}

/** Fetch the *current* bytes pointed at by an S3 key. */
export async function fetchByKey(providerUrl, bucketId, key) {
  const url = `${providerUrl}/s3/${bucketId}/object?key=${encodeURIComponent(key)}`;
  const resp = await fetch(url);
  if (!resp.ok) {
    throw new Error(`fetch ${key} failed: ${resp.status} ${await resp.text()}`);
  }
  return new Uint8Array(await resp.arrayBuffer());
}

/** Filter a `Revive.ContractEmitted` batch down to the file-related events
 * for one `key`, in arrival order. */
export function decodeFileEvents(events, addressBytes, abi, key) {
  const target = keyHash(key).toLowerCase();
  const decoded = decodeContractEmitted(events, null, addressBytes, abi);
  return decoded.filter(
    (log) =>
      ["FileCreated", "FileUpdated", "FileDeleted"].includes(log.eventName) &&
      log.args.keyHash?.toLowerCase() === target
  );
}

/** Stateful handle: bundles api + signer + contract address + ABI + bucket
 * id so the UI can pass a single object around instead of threading args.
 * Construct via `NotebookClient.deploy(...)` or `NotebookClient.attach(...)`. */
export class NotebookClient {
  constructor(opts) {
    this.api = opts.api;
    this.signer = opts.signer;
    this.providerUrl = opts.providerUrl;
    this.abi = opts.abi;
    this.address = opts.address;
    this.addressBytes = opts.addressBytes;
    this.s3BucketId = opts.s3BucketId;
  }

  /** Deploy a fresh notebook. Negotiates agreement terms with the provider
   * (owner = contract's substrate-mapped account), then redeems them on-chain
   * inside `initialize`. `value` funds the agreement reserve. */
  static async deploy({
    api,
    signer,
    providerUrl,
    providerPublicKey,
    abi,
    bytecode,
    bucketName,
    maxBytes,
    duration,
    pricePerByte,
    value,
  }) {
    const deployed = await deployContract(api, signer, bytecode);
    const contractAccount = h160ToSubstrate(deployed.addressBytes);
    const signed = await negotiatePrecompileTerms(providerUrl, contractAccount, {
      maxBytes,
      duration,
      pricePerByte,
    });
    const initData = encodeCall(abi, "initialize", [
      bucketName,
      providerPublicKey,
      signed.terms,
      signed.signature,
    ]);
    const r = await callContract(api, signer, deployed.addressBytes, initData, {
      value,
    });
    const created = api.event.S3Registry.S3BucketCreated.filter(r.events);
    if (created.length !== 1) {
      throw new Error(
        `expected 1 S3BucketCreated event, got ${created.length}`
      );
    }
    return new NotebookClient({
      api,
      signer,
      providerUrl,
      abi,
      address: deployed.address,
      addressBytes: deployed.addressBytes,
      s3BucketId: created[0].s3_bucket_id,
    });
  }

  /** Re-bind to an already-deployed notebook. The UI's "attach to existing"
   * flow gives the user a way to paste a contract address; `s3BucketId` is
   * cached on first use (from the `NotebookInitialized` event) so we ask the
   * caller to supply both for now. */
  static attach({ api, signer, providerUrl, abi, address, addressBytes, s3BucketId }) {
    return new NotebookClient({
      api,
      signer,
      providerUrl,
      abi,
      address,
      addressBytes,
      s3BucketId,
    });
  }

  /** Upload `content` and register it as a new file. Returns the assigned
   * revision (always 1) and the contract event batch. */
  async createFile(key, content, contentType = "application/octet-stream") {
    const { cid, size } = await uploadObject(
      this.providerUrl,
      this.s3BucketId,
      key,
      content,
      contentType
    );
    const data = encodeCall(this.abi, "createFile", [
      key,
      cid,
      BigInt(size),
      contentType,
    ]);
    const r = await callContract(this.api, this.signer, this.addressBytes, data);
    return { revision: 1, cid, size, events: r.events };
  }

  /** Upload `content` and bump the revision. `expectedRevision` is the
   * revision the caller read before editing — mismatched values revert with
   * `StaleRevision`, surfacing concurrent edits. */
  async updateFile(
    key,
    content,
    contentType,
    expectedRevision,
    commitMessage = ""
  ) {
    const { cid, size } = await uploadObject(
      this.providerUrl,
      this.s3BucketId,
      key,
      content,
      contentType
    );
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

  async deleteFile(key) {
    const data = encodeCall(this.abi, "deleteFile", [key]);
    return callContract(this.api, this.signer, this.addressBytes, data);
  }

  /** Current bytes pointed at by `key`. */
  fetchCurrentBytes(key) {
    return fetchByKey(this.providerUrl, this.s3BucketId, key);
  }

  /** Bytes for an arbitrary CID (current or historical). */
  fetchBytesByCid(cid) {
    return fetchByCid(this.providerUrl, cid);
  }

  /** Decode this contract's events out of a per-tx event batch. */
  decodeEvents(events) {
    return decodeContractEmitted(events, null, this.addressBytes, this.abi);
  }

  /** Reconstruct the history of `key` from a list of per-tx event batches.
   * For the live demo each batch comes straight from a `createFile` /
   * `updateFile` call; a UI attaching to an existing contract would seed
   * this list by backfilling past blocks (TODO: helper for that). */
  historyFromBatches(eventBatches, key) {
    return eventBatches.flatMap((events) =>
      decodeFileEvents(events, this.addressBytes, this.abi, key)
    );
  }
}
