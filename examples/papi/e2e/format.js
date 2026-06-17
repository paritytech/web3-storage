// SPDX-License-Identifier: Apache-2.0

/**
 * Tiny helper extracted so both common.js and e2e/helpers.js can format
 * PAPI dispatch errors without creating a circular dependency.
 */

export function formatDispatchError(err) {
  if (!err || typeof err !== "object") return String(err);
  let s = err.type ?? "DispatchError";
  if (err.value?.type) s += `::${err.value.type}`;
  if (err.value?.value?.type) s += `::${err.value.value.type}`;
  return s;
}
