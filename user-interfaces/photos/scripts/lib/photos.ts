// SPDX-License-Identifier: Apache-2.0

// Photos-contract specifics: load the compiled artifact and read `libraryOf`
// unsigned via the `ReviveApi.call` runtime API (no signature, no gas) — the
// same state-detection read the UI will use.

import assert from "node:assert";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { decodeFunctionResult, type Abi } from "viem";
import { asHex, hexToBytes, type ChainSigner, type ParachainApi } from "@web3-storage/sdk";
import { callContract, decodeContractEmitted, encodeCall } from "@web3-storage/sdk/revive";

/** Default artifact path: `<app>/src/contract/Photos.json` (produced by `build:contract`). */
export const ARTIFACT_URL = new URL("../../src/contract/Photos.json", import.meta.url);

export interface Artifact {
  abi: Abi;
  bin: Uint8Array;
}

export async function loadArtifact(url: URL = ARTIFACT_URL): Promise<Artifact> {
  let raw: string;
  try {
    raw = await readFile(fileURLToPath(url), "utf8");
  } catch {
    throw new Error(`Photos.json not found at ${fileURLToPath(url)} — run \`pnpm --filter @web3-storage/photos build:contract\` first (needs solc + resolc on PATH).`);
  }
  const json = JSON.parse(raw);
  return { abi: json.abi as Abi, bin: hexToBytes(json.bin) };
}

export interface LibraryState {
  driveId: bigint;
  rootCid: `0x${string}`;
  exists: boolean;
}

/**
 * Read `libraryOf(user)` unsigned. `user` is the H160 the contract keys
 * libraries by (the user's substrate-mapped account). `origin` is any account
 * to attribute the dry-run to.
 */
export async function readLibraryOf(
  api: ParachainApi,
  contractAddressBytes: Uint8Array,
  userH160: string,
  origin: string,
  abi: Abi,
): Promise<LibraryState> {
  const inputData = encodeCall(abi, "libraryOf", [userH160]);
  const res: any = await api.apis.ReviveApi.call(
    origin,
    asHex(contractAddressBytes), // dest: SizedHex<20>
    0n,
    undefined, // gas_limit: node default (read)
    undefined, // storage_deposit_limit: none
    inputData,
    { at: "best" },
  );
  if (!res?.result?.success) {
    throw new Error(`ReviveApi.call(libraryOf) reverted: ${JSON.stringify(res?.result?.value ?? res)}`);
  }
  const decoded = decodeFunctionResult({
    abi,
    functionName: "libraryOf",
    data: asHex(res.result.value.data),
  }) as unknown as [bigint, `0x${string}`, boolean];
  return { driveId: decoded[0], rootCid: decoded[1], exists: decoded[2] };
}

/**
 * Anchor the drive's metadata Merkle root on-chain via `setRoot(rootCid)` — a
 * signed write keyed by `signer`'s library. Asserts the contract's `RootUpdated`
 * event fired. `rootCid` is the client-computed root as `0x`-prefixed 32-byte hex.
 */
export async function anchorRoot(
  api: ParachainApi,
  signer: ChainSigner,
  contractAddressBytes: Uint8Array,
  rootCid: `0x${string}`,
  abi: Abi,
): Promise<void> {
  const data = encodeCall(abi, "setRoot", [rootCid]);
  const r = await callContract(api, signer, contractAddressBytes, data);
  const logs = decodeContractEmitted(r.events, api, asHex(contractAddressBytes), abi);
  assert.ok(
    logs.some((l) => l.eventName === "RootUpdated"),
    "setRoot landed on-chain but no RootUpdated event was decoded — likely ABI/bytecode drift between Photos.json and the deployed contract",
  );
}
