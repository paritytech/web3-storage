import { test as base, expect, type Page } from "@playwright/test";
import { firstMatch } from "@web3-storage/sdk";
import { getApi, getClient, getBestBlockNumber, disconnect, type ParachainApi } from "./chain-api";

const PROVIDER_HEALTH_URL =
  process.env.PROVIDER_HEALTH_URL ?? "http://127.0.0.1:3333/health";

const DEFAULT_MIN_BLOCK_TIMEOUT = process.env.CI ? 60_000 : 60_000;

export async function waitForConnection(page: Page, timeout = 30_000): Promise<void> {
  await expect(page.getByTestId("block-number")).toBeVisible({ timeout });
}

/**
 * Wait for the chain to produce enough blocks so mortal transactions have a
 * valid era. Submitting at block 1 on a freshly started --dev chain yields
 * "Stale" errors.
 *
 * Reads the chain's *best* block directly (not the UI's finalized badge): a
 * mortal tx is valid against the best chain, and best blocks advance ~6s/block
 * regardless of finality lag — keying this off the finalized head made it flake
 * on slow runners where finality trails well behind block production. The
 * `_page` arg is kept for call-site compatibility but is unused.
 */
export async function waitForMinBlock(
  _page: Page,
  minBlock = 3,
  timeout = DEFAULT_MIN_BLOCK_TIMEOUT,
): Promise<void> {
  await firstMatch(
    getClient().bestBlocks$,
    (blocks) => (blocks[0]?.number ?? 0) >= minBlock,
    { timeoutMs: timeout, description: `best block >= ${minBlock}` },
  );
}

/**
 * Smoke-test liveness assertion shared by all three UIs: the block-number badge
 * is present, and the chain's *best* block advances within `timeout`. Keyed off
 * best blocks (not the UI's finalized badge), so it won't flake when finality
 * lags behind block production (~6s/block).
 */
export async function expectBestBlockToAdvance(page: Page, timeout = 30_000): Promise<void> {
  await expect(page.getByTestId("block-number")).toBeVisible();
  const first = await getBestBlockNumber();
  await firstMatch(
    getClient().bestBlocks$,
    (blocks) => (blocks[0]?.number ?? 0) > first,
    { timeoutMs: timeout, description: `best block > ${first}` },
  );
}

export async function probeProviderHealth(): Promise<boolean> {
  try {
    const res = await fetch(PROVIDER_HEALTH_URL);
    return res.ok;
  } catch {
    return false;
  }
}

export interface LocalPageFixtureOptions {
  localStorage: Record<string, string>;
  baseURL?: string;
}

/**
 * Build a Playwright `test` instance with two fixtures:
 *  - `localPage` (page + localStorage injection + chain ready check)
 *  - `api` (typed PAPI client; lazy singleton across all tests in a worker)
 *
 * Each UI calls this to get a customised fixture.
 */
export function makeLocalPageFixture(opts: LocalPageFixtureOptions) {
  return base.extend<{ localPage: Page; api: ParachainApi }>({
    localPage: async ({ page }, use) => {
      await page.addInitScript((entries: Record<string, string>) => {
        for (const [k, v] of Object.entries(entries)) {
          localStorage.setItem(k, v);
        }
      }, opts.localStorage);
      await page.goto(opts.baseURL ?? "/");
      await waitForConnection(page);
      await waitForMinBlock(page);
      await use(page);
    },
    api: async ({}, use) => {
      await use(getApi());
    },
  });
}

/**
 * Tear down the cached PAPI client. Call from `globalTeardown` if a workflow
 * needs deterministic cleanup; otherwise the worker exit is fine.
 */
export function teardownChain(): void {
  disconnect();
}
