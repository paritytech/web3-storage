import {
  fetchCheckpointSignature,
  submitClientCheckpoint,
  uploadChunk,
} from "./api.js";
import { connect, makeSigner } from "./common.js";

const CHAIN_WS = "ws://127.0.0.1:2222";
const PROVIDER_PRIMARY_URL = "http://127.0.0.1:3333";
const BUCKET_ID = 0n;

async function main() {
  const payload = `Hello, Web3 Storageeeeeeeeeeeeeeeeeeeeeeeeee! [${new Date().toISOString()}]`;
  const { data, commit } = await uploadChunk(PROVIDER_PRIMARY_URL, BUCKET_ID, payload);
  console.log("  Uploaded %d bytes, mmr_root=%s", data.length, commit.mmr_root);

  // submit checkpoint
  const { papi, api } = await connect(CHAIN_WS);
  try {
    const client = makeSigner("//Bob");
    const primary = makeSigner("//Alice");
    const ck = await fetchCheckpointSignature(PROVIDER_PRIMARY_URL, BUCKET_ID);
    await submitClientCheckpoint(api, client, primary, BUCKET_ID, ck);
    console.log("  Checkpoint submitted (chain MMR root:", ck.mmr_root, ")");
  } finally {
    papi.destroy();
  }
}

main().catch((err) => {
  console.log("Error:", err);
});
