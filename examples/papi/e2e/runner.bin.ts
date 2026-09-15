#!/usr/bin/env node

// SPDX-License-Identifier: Apache-2.0

/**
 * Sequential orchestrator for the E2E test suite.
 *
 * Runs each numbered workflow file as a child process, collects exit codes,
 * and prints a summary table at the end.
 *
 * Usage: npx tsx e2e/runner.bin.ts [chain_ws] [provider_url]
 */

import { execFileSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

// Discover workflow files: XX-*.ts in this directory, sorted by name.
const files = readdirSync(__dirname)
  .filter((f) => /^\d{2}-.*\.ts$/.test(f))
  .sort();

if (files.length === 0) {
  console.error("No workflow files found in", __dirname);
  process.exit(1);
}

console.log(`\n${"═".repeat(70)}`);
console.log("  E2E Test Suite — %d workflows", files.length);
console.log("  Chain: %s", CHAIN_WS);
console.log("  Provider: %s", PROVIDER_URL);
console.log(`${"═".repeat(70)}\n`);

const results = [];

for (const file of files) {
  const label = file.replace(/\.ts$/, "");
  const t0 = Date.now();
  console.log("▶ Running %s …", label);
  try {
    execFileSync(
      process.execPath,
      // Workflows import the raw-TS @web3-storage/sdk, so each child needs
      // the tsx loader just like the justfile invocations do.
      ["--import", "tsx", resolve(__dirname, file), CHAIN_WS, PROVIDER_URL],
      {
        stdio: "inherit",
        timeout: 10 * 60 * 1000, // 10 minutes per workflow
      }
    );
    const ms = Date.now() - t0;
    results.push({ label, passed: true, ms });
    console.log("✅ %s passed (%ds)\n", label, Math.round(ms / 1000));
  } catch (err) {
    const ms = Date.now() - t0;
    results.push({ label, passed: false, ms });
    console.log("❌ %s FAILED (%ds)\n", label, Math.round(ms / 1000));
  }
}

// ── Summary table ───────────────────────────────────────────────────────────

const passed = results.filter((r) => r.passed).length;
const failed = results.filter((r) => !r.passed).length;

console.log(`\n${"═".repeat(70)}`);
console.log("  RESULTS: %d passed, %d failed, %d total", passed, failed, results.length);
console.log(`${"═".repeat(70)}`);

const maxLen = Math.max(...results.map((r) => r.label.length));
for (const r of results) {
  const icon = r.passed ? "✅" : "❌";
  const time = `${Math.round(r.ms / 1000)}s`.padStart(5);
  console.log(`  ${icon} ${r.label.padEnd(maxLen)}  ${time}`);
}
console.log();

if (failed > 0) process.exit(1);
