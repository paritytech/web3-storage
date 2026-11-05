// SPDX-License-Identifier: Apache-2.0

/** Node (and Vitest): read the wasm file from disk. */

import { readFile } from "node:fs/promises";

import init from "../pkg/bucket_tree.js";

let ready: Promise<unknown> | undefined;

/** Instantiate the wasm module once. */
export function loadWasm(): Promise<unknown> {
  ready ??= readFile(new URL("../pkg/bucket_tree_bg.wasm", import.meta.url)).then((bytes) =>
    init({ module_or_path: bytes }),
  );
  return ready;
}
