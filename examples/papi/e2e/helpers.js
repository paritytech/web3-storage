/**
 * E2E test utilities for the web3-storage test suite.
 *
 * Provides `runTest`, `runSuite`, and `submitTxExpectFailure` so each
 * workflow file can focus on scenario logic.
 */

import { formatDispatchError } from "./format.js";
import { READ_OPTS } from "@web3-storage/sdk";

// ── Test runner primitives ──────────────────────────────────────────────────

/**
 * Run a single test function and print PASS/FAIL.
 * Returns `{ name, passed, error? }`.
 */
export async function runTest(name, fn) {
  const t0 = Date.now();
  try {
    await fn();
    const ms = Date.now() - t0;
    console.log(`  ✅ PASS  ${name}  (${ms}ms)`);
    return { name, passed: true };
  } catch (err) {
    const ms = Date.now() - t0;
    console.log(`  ❌ FAIL  ${name}  (${ms}ms)`);
    console.log(`          ${err.message || err}`);
    if (err.stack) {
      const firstFrame = err.stack.split("\n").find((l) => l.includes("file://"));
      if (firstFrame) console.log(`         ${firstFrame.trim()}`);
    }
    return { name, passed: false, error: err };
  }
}

/**
 * Run an ordered array of `{ name, fn }` tests, print a summary table,
 * and set `process.exitCode = 1` if any test failed.
 *
 * @param {string} suiteName  — banner label
 * @param {Array<{name: string, fn: Function}>} tests
 * @param {object} ctx  — shared context passed to each `fn(ctx)`
 */
export async function runSuite(suiteName, tests, ctx) {
  console.log(`\n${"=".repeat(70)}`);
  console.log(`  ${suiteName}`);
  console.log(`${"=".repeat(70)}\n`);

  const results = [];
  for (const { name, fn } of tests) {
    const result = await runTest(name, () => fn(ctx));
    results.push(result);
  }

  // ── Summary ──
  const passed = results.filter((r) => r.passed).length;
  const failed = results.filter((r) => !r.passed).length;
  console.log(`\n${"─".repeat(70)}`);
  console.log(
    `  ${suiteName}: ${passed} passed, ${failed} failed, ${results.length} total`
  );
  console.log(`${"─".repeat(70)}\n`);

  if (failed > 0) {
    console.log("  Failed tests:");
    for (const r of results.filter((r) => !r.passed)) {
      console.log(`    - ${r.name}: ${r.error?.message || "unknown"}`);
    }
    process.exitCode = 1;
  }
  return { passed, failed, total: results.length };
}

// ── Failure assertion ───────────────────────────────────────────────────────

/**
 * Submit a transaction and assert it fails with a dispatch error whose
 * stringified representation contains `expectedError`.
 *
 * Works by catching the error thrown by `submitTx` (which already calls
 * `signAndSubmit` and throws on `!result.ok`). If the tx unexpectedly
 * succeeds, an assertion error is thrown.
 */
export async function submitTxExpectFailure(tx, signer, expectedError, label) {
  try {
    const observable = tx.signSubmitAndWatch(signer);
    const result = await new Promise((resolve, reject) => {
      let done = false;
      let sub;
      const cleanup = () => {
        done = true;
        clearTimeout(timer);
        if (sub) sub.unsubscribe();
      };
      const timer = setTimeout(() => {
        if (!done) {
          cleanup();
          reject(new Error(`${label}: timed out after 180s`));
        }
      }, 180_000);
      sub = observable.subscribe({
        next: (ev) => {
          if (done) return;
          if (ev.type === "txBestBlocksState" && ev.found) {
            cleanup();
            resolve(ev);
          }
        },
        error: (err) => {
          if (done) return;
          cleanup();
          reject(err);
        },
      });
    });
    if (result.ok) {
      throw new Error(
        `${label}: expected dispatch failure containing "${expectedError}", but tx succeeded`
      );
    }
    // Dispatch error present — check that it matches.
    const errStr = formatDispatchError(result.dispatchError);
    if (!errStr.includes(expectedError)) {
      throw new Error(
        `${label}: expected error containing "${expectedError}", got "${errStr}"`
      );
    }
    return result;
  } catch (err) {
    // submitTx-style wrappers and PAPI can throw various errors. Check if the
    // error message itself contains what we're looking for.
    if (err.message && err.message.includes(expectedError)) {
      return; // expected failure — success
    }
    // Re-throw: either the tx succeeded unexpectedly or the error didn't match.
    throw err;
  }
}

// ── Shared setup helper ─────────────────────────────────────────────────────

/**
 * Standard preamble for every E2E workflow: connect, wait for chain, return
 * `{ papi, api }`.
 */
export async function setupChain(chainWs) {
  const { connect, waitForChainReady, waitForBlockProduction, waitForNextBlock } =
    await import("@web3-storage/sdk");
  const { papi, api } = await connect(chainWs);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);
  return { papi, api };
}

/**
 * Grab the free balance for an account.
 */
export async function getFree(api, who) {
  const acc = await api.query.System.Account.getValue(who.address, READ_OPTS);
  return acc.data.free;
}
