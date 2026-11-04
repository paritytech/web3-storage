// SPDX-License-Identifier: Apache-2.0

/**
 * Local reproduction of paritytech/web3-storage#523: the manual challenge
 * response path (`fetchChallengeProof` -> `GET /mmr_proof`) proves the leaf
 * against the bucket's current MMR, not the commitment the challenge cites.
 *
 * Needs `just start-chain` and `just start-provider` (provider = Alice).
 *
 *   node --import tsx repro-523.ts setup     # Bob: bucket, upload A, challenge leaf A at commitment C1
 *   node --import tsx repro-523.ts advance   # Bob: upload another chunk -> commitment C2
 *   node --import tsx repro-523.ts respond   # Alice: respond the same way the provider UI does
 *   node --import tsx repro-523.ts status    # is the challenge still open?
 *
 * State is kept in .repro-523.json next to this file.
 */
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import {
  Alice,
  Bob,
  createBucketViaApi,
  disconnectApi,
  getApi,
  isProviderRegistered,
  registerProviderViaApi,
} from "@web3-storage/test-helpers";
import {
  challengeOffchain,
  fetchChallengeProof,
  fetchCheckpointSignature,
  respondToChallenge,
  submitClientCheckpoint,
  uploadChunk,
} from "@web3-storage/sdk";

const PROVIDER_URL = process.env.PROVIDER_URL ?? "http://127.0.0.1:3333";
const STATE_FILE = join(dirname(fileURLToPath(import.meta.url)), ".repro-523.json");

interface State {
  bucketId: string;
  challengeId: { deadline: number; index: number };
  uploads: number;
}

function loadState(): State {
  if (!existsSync(STATE_FILE)) throw new Error(`No ${STATE_FILE}; run "setup" first`);
  return JSON.parse(readFileSync(STATE_FILE, "utf8"));
}

function saveState(s: State): void {
  writeFileSync(STATE_FILE, JSON.stringify(s, null, 2));
}

async function setup(): Promise<void> {
  const api = getApi();
  if (!(await isProviderRegistered(Alice.address))) {
    console.log("Registering Alice as provider...");
    await registerProviderViaApi(Alice);
  }

  const { bucketId } = await createBucketViaApi(Bob);
  console.log(`Bucket ${bucketId} created by Bob, primary provider Alice`);

  const upload = await uploadChunk(PROVIDER_URL, bucketId, `repro-523 chunk A @ ${Date.now()}`, Bob);
  console.log("Commitment C1 signed by Alice:", {
    mmr_root: upload.commit.mmr_root,
    start_seq: upload.commit.start_seq,
    leaf_count: upload.commit.leaf_count,
    leaf_indices: upload.commit.leaf_indices,
  });

  const checkpoint = await fetchCheckpointSignature(PROVIDER_URL, bucketId, Bob);
  await submitClientCheckpoint(api, Bob, Alice, bucketId, checkpoint);
  console.log("C1 checkpointed on-chain");

  const challengeId = await challengeOffchain(api, Bob, Alice, bucketId, {
    leafIndex: upload.commit.leaf_indices[0],
    mmrRoot: upload.commit.mmr_root,
    startSeq: upload.commit.start_seq,
    leafCount: upload.commit.leaf_count,
    providerSignature: upload.commit.provider_signature,
  });
  console.log("Challenge created against C1:", challengeId);

  saveState({ bucketId: bucketId.toString(), challengeId, uploads: 1 });
}

async function advance(): Promise<void> {
  const s = loadState();
  const bucketId = BigInt(s.bucketId);
  const upload = await uploadChunk(PROVIDER_URL, bucketId, `repro-523 chunk ${s.uploads} @ ${Date.now()}`, Bob);
  console.log("New commitment signed by Alice:", {
    mmr_root: upload.commit.mmr_root,
    start_seq: upload.commit.start_seq,
    leaf_count: upload.commit.leaf_count,
  });
  saveState({ ...s, uploads: s.uploads + 1 });
}

async function respond(): Promise<void> {
  const api = getApi();
  const s = loadState();
  const challenge = await api.query.StorageProvider.Challenges.getValue(
    s.challengeId.deadline,
    s.challengeId.index,
    { at: "best" },
  );
  if (!challenge) {
    console.log("Challenge is not open (already answered or resolved).");
    return;
  }
  console.log("Challenge cites commitment:", {
    mmr_root: challenge.commitment.mmr_root.asHex?.() ?? challenge.commitment.mmr_root,
    start_seq: challenge.commitment.start_seq,
    leaf_count: challenge.commitment.leaf_count,
  });

  // Same calls the provider UI makes in respondToChallenge().
  const proof = await fetchChallengeProof(api, PROVIDER_URL, s.challengeId, Alice);
  console.log(`MMR proof from GET /mmr_proof has ${proof.value.mmr_proof.items.length} item(s)`);
  try {
    await respondToChallenge(api, Alice, s.challengeId, proof);
    console.log("Response ACCEPTED: challenge defended.");
  } catch (e) {
    console.log("Response REJECTED:", (e as Error).message);
  }
}

async function status(): Promise<void> {
  const api = getApi();
  const s = loadState();
  const open = await api.query.StorageProvider.Challenges.getValue(
    s.challengeId.deadline,
    s.challengeId.index,
    { at: "best" },
  );
  console.log(open ? `Challenge ${JSON.stringify(s.challengeId)} is OPEN` : "Challenge is closed");
}

const commands: Record<string, () => Promise<void>> = { setup, advance, respond, status };
const cmd = process.argv[2];
if (!cmd || !commands[cmd]) {
  console.error(`Usage: node --import tsx repro-523.ts <${Object.keys(commands).join("|")}>`);
  process.exit(1);
}
try {
  await commands[cmd]();
} finally {
  disconnectApi();
}
process.exit(0);
