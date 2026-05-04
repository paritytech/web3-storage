# Web3 Storage UIs

Three React + Vite + Polkadot-API single-page apps:

- **`drive-ui/`** (port `5174`) — Layer 1 file system browser. Drives, files, members, checkpoints. Talks to the parachain (`ws://127.0.0.1:2222`) and to whichever provider node a drive's bucket points at (resolved via on-chain multiaddr).
- **`console-ui/`** (port `5173`) — Layer 0 storage console. Buckets, S3-style objects, low-level explorer.
- **`provider/`** (port `5175`) — Provider operator dashboard. Registration, agreements, checkpoints, challenges, earnings.

State management:
- `provider/` and `drive-ui/` use RxJS `BehaviorSubject` + `@react-rxjs/core` `bind()`. State files live in `src/state/*.state.ts`.
- `console-ui/` is a lighter Context+useState surface; on the to-do list to migrate.

All three share `@web3-storage/network-config` (in `shared/network-config`) for endpoint selection and persistence.

## Local development

```bash
# In separate terminals:
just start-chain         # zombienet relay + parachain (ports 9900, 2222)
just start-provider      # provider HTTP node (port 3333)

# Then any of:
cd user-interfaces/drive-ui && npm run dev      # http://localhost:5174
cd user-interfaces/console-ui && npm run dev    # http://localhost:5173
cd user-interfaces/provider && npm run dev      # http://localhost:5175
```

## Tests

The harness is shared across the three UIs. Layout:

- **Vitest** for pure-function unit tests (drive-ui's `src/lib/__tests__/multiaddr.test.ts`, provider's existing `src/lib/chain-client.test.ts` + `src/utils/format.test.ts`).
- **Playwright** for E2E smoke + feature tests against a real local chain + provider. Each UI has its own `playwright.config.ts` and `e2e/` directory.
- Shared helpers in `shared/test-helpers/` (`makeLocalPageFixture`, `waitForConnection`, `waitForMinBlock`, `probeProviderHealth`).

### Running

```bash
# Unit tests only (no chain required)
just test-ui-unit

# E2E for one UI (requires chain + provider running)
just test-ui-drive
just test-ui-console
just test-ui-provider

# Everything: unit + e2e × 3 UIs (waits for chain + provider /health, then serial)
just test-ui
```

### Adding a new test

1. **Unit (drive-ui or provider):** add `src/**/*.test.{ts,tsx}` next to the code. Uses Vitest config in each package.
2. **E2E (any UI):** add `e2e/integration/<feature>.spec.ts`. Use the local fixture: `import { test, expect } from "../fixtures";`. Reach for `localPage` to get a hydrated, connected page.

### Test-id naming convention

Add `data-testid="{area}-{element}"` to interactive elements. `area` is the UI region (`connect`, `account`, `drive-list`, `file-browser`, `manage-access`, `checkpoint`, `commit-strategy`, …). `element` is the role (`button`, `submit`, `dialog`, `input`, `row-{id}`).

Examples already in use:
- `connect-button`, `connect-dialog`, `connect-endpoint-input`, `connect-submit`
- `account-button`, `account-dialog`, `account-dialog-alice`, `signer-address`, `balance-display`
- `block-number` (chain-connection indicator — present in all three UIs)
- `drive-list`, `drive-list-item-{id}`, `drive-list-rename-{id}`, `drive-list-clear-{id}`, `drive-list-delete-{id}`
- `new-drive-dialog`, `new-drive-name`, `commit-strategy-{kind}`, `new-drive-submit`
- `file-browser`, `breadcrumbs`, `breadcrumb-{i}`, `entry-row-{type}-{name}`, `entries-grid`, `entries-table`
- `pending-changes-banner`, `commit-now-button`
- `checkpoint-panel`, `checkpoint-trigger`, `checkpoint-refresh`, `root-cid`, `last-committed-at`, `commit-strategy-display`
- `manage-access-dialog`, `members-table`, `member-row-{address}`, `add-member-address`, `add-member-role`, `add-member-submit`, `add-member-error`
- `upload-button`, `upload-cancel`, `view-mode-toggle`, `refresh-button`, `new-folder-button`

### CI gate (planned)

`pnpm papi:check` per UI — runs `papi generate` and fails if `.papi/descriptors` drifts from the runtime's metadata. Stops PRs from landing with stale codecs.

## Workspace gotchas

- Each UI has its own `.papi/descriptors/` (a `file:` dependency). `npm install` at the root only hoists *one* of them to `node_modules/@polkadot-api/descriptors`. Vite resolves correctly via an explicit `@polkadot-api/descriptors` alias in each `vite.config.ts` (and `vitest.config.ts` for unit tests).
- `provider/` historically defaulted to port `5173`, which collides with `console-ui/`. Provider now uses `5175`. Adjust your bookmarks if you had it open.
