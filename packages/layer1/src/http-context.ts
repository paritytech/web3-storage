// SPDX-License-Identifier: Apache-2.0

import { HttpError } from "@web3-storage/core";

/**
 * Run `fn`; rethrow a provider HTTP error as `<action>: <status> <details>`
 * so callers can match on the status (e2e tests match `Upload failed: 403`).
 * Other errors pass through unchanged.
 */
export async function withHttpContext<T>(action: string, fn: () => Promise<T>): Promise<T> {
  try {
    return await fn();
  } catch (err) {
    if (err instanceof HttpError) {
      throw new HttpError(err.status, `${action}: ${err.status} ${withoutStatus(err)}`);
    }
    throw err;
  }
}

/** `<path>: <status> <body>` (from `providerFetch`) without the status. */
function withoutStatus(err: HttpError): string {
  const m = /^(\S+): (\d+) ([\s\S]*)$/.exec(err.message);
  return m && Number(m[2]) === err.status ? `${m[1]}: ${m[3]}` : err.message;
}
