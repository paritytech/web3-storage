// SPDX-License-Identifier: Apache-2.0
//
// Pull one `<file>:<Contract>` entry out of resolc's combined.json into a clean
// `{ abi, bin }` artifact the app and scripts import. Usage:
//   node extract.mjs <combined.json> <File.sol:Contract> <out.json>

import { readFileSync, writeFileSync } from "node:fs";

const [, , combinedPath, key, outPath] = process.argv;
if (!combinedPath || !key || !outPath) {
  console.error("usage: extract.mjs <combined.json> <File.sol:Contract> <out.json>");
  process.exit(1);
}

const combined = JSON.parse(readFileSync(combinedPath, "utf8"));
const entry = combined.contracts?.[key];
if (!entry) {
  console.error(`combined.json is missing ${key} — known keys: ${Object.keys(combined.contracts ?? {}).join(", ")}`);
  process.exit(1);
}

const bin = entry.bin.startsWith("0x") ? entry.bin : `0x${entry.bin}`;
writeFileSync(outPath, `${JSON.stringify({ abi: entry.abi, bin }, null, 2)}\n`);
