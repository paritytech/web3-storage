# Web3 Storage UIs

Three React + Vite + Polkadot-API single-page apps:

- **`drive-ui/`** (port `5174`) — Layer 1 file system browser. Drives, files, members, checkpoints. Talks to the parachain (`ws://127.0.0.1:2222`) and to whichever provider node a drive's bucket points at (resolved via on-chain multiaddr).
- **`s3-ui/`** (port `5177`) — Layer 0 S3-style object store. Buckets, objects, client-side encryption, checkpoints.
- **`provider/`** (port `5175`) — Provider operator dashboard. Registration, agreements, checkpoints, challenges, earnings.

State management:
- `provider/`, `drive-ui/`, and `s3-ui/` use RxJS `BehaviorSubject` + `@react-rxjs/core` `bind()`. State files live in `src/state/*.state.ts`.

All three share `@web3-storage/network-config` (in `shared/network-config`) for endpoint selection and persistence.

## Local development

```bash
# In separate terminals:
just start-chain         # zombienet relay + parachain (ports 9900, 2222)
just start-provider      # provider HTTP node (port 3333)

# Then any of:
cd user-interfaces/drive-ui && pnpm dev      # http://localhost:5174
cd user-interfaces/s3-ui && pnpm dev         # http://localhost:5177
cd user-interfaces/provider && pnpm dev      # http://localhost:5175
```

The UIs are members of the repo-root **pnpm** workspace (`/pnpm-workspace.yaml`, alongside `packages/*` and `examples/papi`); inter-workspace deps use the `workspace:*` protocol. Run `pnpm install` at the repo root. Stick with pnpm — switching to npm breaks workspace resolution.

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
just test-ui-provider

# Everything: unit + e2e (waits for chain + provider /health, then serial)
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

**s3-ui** (S3 object store): test-ids follow the same `{area}-{element}` convention (`s3`, `bucket`, `object`, `connect`, `account`, …). Playwright coverage is not yet wired — it's a tracked follow-up.

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
just test-ui-provider    # terminal 3 — provider specs (~8 feature + 3 smoke)
```

Tests are idempotent: chain-state collisions (already-registered provider, leftover drives/buckets) are detected and either reused or auto-skipped with a clear message. Re-running without restarting the chain should stay green.

### CI gates

- **Tier 1 — `ui-checks.yml`**: typecheck + Vite build + Vitest. Runs on every PR touching `user-interfaces/**`. ~2-3 min.
- **Tier 2 — `integration-tests.yml` (`ui-integration-tests` job)**: full Playwright e2e against a standalone paseo dev chain (omni-node, 2s blocks, no relay chain) + provider, reusing the shared `build` artifact instead of rebuilding. Runs on every PR (like the rest of the integration suite) and is gated by the required **Integration Tests** check. **Required from day 1** — robustness comes from idempotent test setup, generous CI timeouts, and detect-and-skip for state collisions, not from softening the gate.

## Workspace gotchas

- The parachain descriptors have a single owner: `packages/papi` tracks the only metadata snapshot, and its nested `@polkadot-api/descriptors` package is a workspace member every consumer (UIs, `packages/sdk`, `examples/papi`) depends on via `workspace:*`. `pnpm install` at the repo root regenerates descriptors from the tracked metadata; `pnpm run papi:generate` (chain running) refreshes the snapshot itself. Inter-workspace deps only resolve under pnpm.
- Canonical dev ports: drive-ui `5174`, provider `5175`, s3-ui `5177`.

## License

[GPL-3.0-only](../LICENSE-GPL3)
