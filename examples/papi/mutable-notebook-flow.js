/**
 * MutableNotebook end-to-end demo.
 *
 * Deploys the contract, edits one file three times, fetches every revision
 * back by CID, and asserts byte-equality with the original uploads. Doubles
 * as the integration test for the notebook library — a UI imports the same
 * `NotebookClient` and follows the same call shape.
 *
 * Usage: node mutable-notebook-flow.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  hexToBytes,
  makeSigner,
  parseProviderClientArgs,
  READ_OPTS,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";
import { ensureAccountMapped } from "./sc-api.js";
import { toHex } from "./common.js";
import { CONTRACT_KEY, NotebookClient } from "./notebook.js";

const { chainWs, providerUrl, providerSeed, clientSeed } = parseProviderClientArgs();

const HERE = dirname(fileURLToPath(import.meta.url));
const CONTRACT_JSON = resolve(HERE, "../contracts/build/combined.json");

const UNIT = 10n ** 12n;
const FILE_KEY = "hack.md";

async function main() {
  console.log("=== MutableNotebook e2e ===");
  console.log(" chain    :", chainWs);
  console.log(" provider :", providerUrl, `(${providerSeed})`);
  console.log(" client   :", clientSeed);

  const { papi, api } = await connect(chainWs);
  try {
    await waitForChainReady(api);
    await waitForBlockProduction(api);
    await waitForNextBlock(papi);

    const provider = makeSigner(providerSeed);
    const author = makeSigner(clientSeed);

    console.log("\n[setup] provider + Revive account mapping…");
    await ensureProviderRegistered(api, provider, providerUrl, {
      pricePerByte: 1n,
      maxDuration: 100_000,
    });
    await ensureSoleAcceptingProvider(api, provider);
    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, author);

    const combined = JSON.parse(await readFile(CONTRACT_JSON, "utf8"));
    const entry = combined.contracts?.[CONTRACT_KEY];
    if (!entry) {
      throw new Error(
        `combined.json missing ${CONTRACT_KEY} — run \`bash examples/contracts/build.sh\` first`
      );
    }
    const abi = entry.abi;
    const bytecode = hexToBytes(entry.bin);
    console.log("  bytecode:", bytecode.length, "bytes");

    // Bucket name: globally unique, append head block to survive reruns.
    const head = await api.query.System.Number.getValue(READ_OPTS);
    const bucketName = `notebook-${Number(head)}`;

    console.log(`\n[1/6] Deploying MutableNotebook + initialize('${bucketName}', …)`);
    const client = await NotebookClient.deploy({
      api,
      signer: author,
      providerUrl,
      providerPublicKey: toHex(provider.publicKey),
      abi,
      bytecode,
      bucketName,
      maxBytes: 1n << 20n, // 1 MiB
      duration: 50, // blocks
      pricePerByte: 1n,
      value: 5n * UNIT,
    });
    console.log("  contract :", client.address);
    console.log("  s3Bucket :", client.s3BucketId.toString());

    // Three revisions of the same file. Edits land mid-document so CDC's
    // structural sharing actually gets exercised (the demo isn't testing
    // dedup ratios — that's covered by the provider integration tests —
    // but using realistic edits keeps the example honest).
    const v1 = encode(
      "# hack.md\n\nfirst draft.\n\n- bullet one\n- bullet two\n"
    );
    const v2 = encode(
      "# hack.md\n\nfirst draft, with a fix.\n\n- bullet one\n- bullet two\n"
    );
    const v3 = encode(
      "# hack.md\n\nfirst draft, with a fix.\n\n- bullet one\n- bullet two\n- bullet three\n"
    );

    console.log("\n[2/6] createFile('hack.md') — revision 1");
    const r1 = await client.createFile(FILE_KEY, v1, "text/markdown");
    console.log("  cid =", r1.cid, "  size =", r1.size);

    console.log("\n[3/6] updateFile — revision 2 ('typo fix')");
    const r2 = await client.updateFile(FILE_KEY, v2, "text/markdown", 1, "typo fix");
    console.log("  cid =", r2.cid, "  size =", r2.size);

    console.log("\n[4/6] updateFile — revision 3 ('add bullet')");
    const r3 = await client.updateFile(FILE_KEY, v3, "text/markdown", 2, "add bullet");
    console.log("  cid =", r3.cid, "  size =", r3.size);

    console.log("\n[5/6] History");
    const history = client.historyFromBatches(
      [r1.events, r2.events, r3.events],
      FILE_KEY
    );
    assert.equal(history.length, 3, "expected 3 history entries");
    for (const log of history) {
      if (log.eventName === "FileCreated") {
        console.log(
          `  rev 1  ${log.args.cid}  (initial, by ${log.args.author})`
        );
      } else if (log.eventName === "FileUpdated") {
        console.log(
          `  rev ${log.args.newRevision}  ${log.args.newCid}  ` +
            `(was ${log.args.oldCid.slice(0, 10)}…)  "${log.args.commitMessage}"`
        );
      }
    }

    console.log("\n[6/6] Fetch each revision by CID and assert byte-equal");
    const fetched1 = await client.fetchBytesByCid(r1.cid);
    const fetched2 = await client.fetchBytesByCid(r2.cid);
    const fetched3 = await client.fetchBytesByCid(r3.cid);
    assert.deepEqual(fetched1, v1, "v1 bytes mismatch");
    assert.deepEqual(fetched2, v2, "v2 bytes mismatch");
    assert.deepEqual(fetched3, v3, "v3 bytes mismatch");
    console.log("  all three revisions resolvable post-pointer-flip ✓");

    // The current S3 pointer flipping to v3 is observable through the
    // provider's S3 GET path (which reads via the on-chain index): a fetch
    // by key returns v3's bytes.
    const byKey = await client.fetchCurrentBytes(FILE_KEY);
    assert.deepEqual(byKey, v3, "current-key fetch != v3 bytes");
    console.log("  current-key fetch returns v3 bytes ✓");

    console.log("\n✅ MutableNotebook flow completed (PASS)");
  } finally {
    papi.destroy();
  }
}

function encode(str) {
  return new TextEncoder().encode(str);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
