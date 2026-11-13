// SPDX-License-Identifier: Apache-2.0

/** Browsers: fetch the wasm file next to the glue module (bundlers rewrite the URL). */

import init from "../pkg/storage_formats.js";

let ready: Promise<unknown> | undefined;

/** Instantiate the wasm module once. */
export function loadWasm(): Promise<unknown> {
  ready ??= init();
  return ready;
}
