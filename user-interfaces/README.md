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
cd user-interfaces/drive-ui && pnpm dev      # http://localhost:5174
cd user-interfaces/console-ui && pnpm dev    # http://localhost:5173
cd user-interfaces/provider && pnpm dev      # http://localhost:5175
```

The `user-interfaces/` workspace uses **pnpm** (`pnpm-workspace.yaml`); inter-workspace deps use the `workspace:*` protocol. Stick with pnpm — switching to npm breaks workspace resolution.

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

Add `data-testid="{area}-{element}"` to interactive elements. `area` is the UI region (`connect`, `account`, `drive-list`, `file-browser`, `manage-access`, `checkpoint`, `commit-strategy`, `s3`, `bucket`, `accounts`, `nav`, `provider`, `registration`, `settings`, …). `element` is the role (`button`, `submit`, `dialog`, `input`, `row-{id}`).

Examples in use across the three UIs:

**drive-ui** (post PR 1):
- `connect-button`, `connect-dialog`, `connect-endpoint-input`, `connect-submit`
- `account-button`, `account-dialog`, `account-dialog-alice`, `signer-address`, `balance-display`
- `block-number` (chain-connection indicator — present in all three UIs)
- `drive-list`, `drive-list-item-{id}`, `drive-list-delete-{id}`
- `new-drive-dialog`, `new-drive-name`, `new-drive-submit`
- `file-browser`, `breadcrumbs`, `breadcrumb-{i}`, `entry-row-{type}-{name}`, `entries-grid`, `entries-table`, `upload-input`
- `checkpoint-panel`, `checkpoint-trigger`, `checkpoint-refresh`
- `manage-access-dialog`, `members-table`, `member-row-{address}`, `add-member-address`, `add-member-role`, `add-member-submit`, `add-member-error`
- `upload-button`, `upload-cancel`, `view-mode-toggle`, `refresh-button`, `new-folder-button`

**console-ui**:
- `nav-{name}` (sidebar nav links: dashboard / storage / explorer / accounts), `layout-disconnect`, `signer-name`, `signer-address`, `balance-display`
- `connect-button`, `connect-dialog`, `connect-ws-input`, `connect-submit`, `connect-cancel`
- `s3-bucket-selector`, `s3-new-bucket`, `s3-delete-bucket`, `s3-delete-confirm`, `s3-delete-cancel`
- `s3-create-bucket-form`, `s3-bucket-name-input`, `s3-bucket-capacity-input`, `s3-bucket-duration-input`, `s3-bucket-maxpayment-input`, `s3-create-submit`, `s3-create-cancel`, `s3-create-error`
- `s3-upload-button`, `s3-upload-form`, `s3-upload-key-input`, `s3-upload-file-input`, `s3-upload-choose-file`, `s3-upload-submit`, `s3-upload-cancel`
- `s3-encryption-toggle`, `s3-encryption-form`, `s3-encryption-key-input`, `s3-encryption-generate`, `s3-encryption-enable`, `s3-encryption-cancel`, `s3-encryption-copy`
- `s3-refresh-objects`, `s3-objects-table`, `s3-folder-row-{name}`, `s3-object-row-{key}`, `s3-download-{key}`, `s3-delete-object-{key}`, `s3-user-role`
- `bucket-info-panel`, `bucket-info-toggle`, `bucket-members-table`, `bucket-member-row-{address}`, `bucket-member-remove-{address}`, `bucket-add-member-{address,role,submit}`
- `accounts-custom-form`, `accounts-custom-name-input`, `accounts-custom-seed-input`, `accounts-custom-submit`
- `accounts-list`, `accounts-list-row-{name}`, `accounts-active-badge-{name}`, `accounts-set-active-{name}`, `accounts-copy-{name}`, `accounts-delete-{name}`

**provider**:
- `nav-{label}` (overview / registration / agreements / buckets / checkpoints / challenges / earnings)
- `provider-account-button`, `provider-account-name`, `provider-account-select-{name}`, `provider-disconnect`
- `provider-connect-button`, `provider-connect-dev`, `provider-connect-wallet`
- `registration-stake-input`, `registration-stake-continue`, `registration-multiaddr-input`, `registration-priceperbyte-input`, `registration-maxcapacity-input`, `registration-minduration-input`, `registration-maxduration-input`, `registration-settings-continue`, `registration-submit`, `registration-complete`
- `settings-multiaddr-input`, `settings-multiaddr-update`, `settings-priceperbyte-input`, `settings-maxcapacity-input`, `settings-update`
- `provider-info`, `stat-card-{slug}`, `stat-value-{slug}`
- `buckets-table`, `buckets-row-{id}`, `agreements-table`, `agreements-row-{id}`

### Running the feature-level e2e suite (PR 3)

PR 3 adds a feature-level Playwright suite covering bucket / drive / member / file / registration flows. Specs live in `e2e/integration/` per UI. Tests need a live local chain + provider.

```bash
just start-chain         # terminal 1
just start-provider      # terminal 2
just test-ui-drive       # terminal 3 — drive-ui specs (~21 feature + 5 smoke)
just test-ui-console     # terminal 3 — console-ui specs (~11 feature + 3 smoke)
just test-ui-provider    # terminal 3 — provider specs (~8 feature + 3 smoke)
```

Tests are idempotent: chain-state collisions (already-registered provider, leftover drives/buckets) are detected and either reused or auto-skipped with a clear message. Re-running without restarting the chain should stay green.

### CI gates

- **Tier 1 — `ui-checks.yml`**: typecheck + Vite build + Vitest. Runs on every PR touching `user-interfaces/**`. ~2-3 min.
- **Tier 2 — `integration-tests.yml` (`ui-integration-tests` job)**: full Playwright e2e against a standalone paseo dev chain (omni-node, 2s blocks, no relay chain) + provider, reusing the shared `build` artifact instead of rebuilding. Runs on every PR (like the rest of the integration suite) and is gated by the required **Integration Tests** check. **Required from day 1** — robustness comes from idempotent test setup, generous CI timeouts, and detect-and-skip for state collisions, not from softening the gate.

## Workspace gotchas

- Each UI has its own `.papi/descriptors/` (a `file:` dependency). `pnpm install` at the workspace root resolves them correctly because the per-UI `vite.config.ts` (and `vitest.config.ts` for unit tests) include an explicit `@polkadot-api/descriptors` alias. Inter-workspace deps (`@web3-storage/network-config`, `@web3-storage/test-helpers`) use `workspace:*` and only resolve under pnpm.
- `provider/` historically defaulted to port `5173`, which collides with `console-ui/`. Provider now uses `5175`. Adjust your bookmarks if you had it open.

## License

[GPL-3.0-only](../LICENSE-GPL3)
