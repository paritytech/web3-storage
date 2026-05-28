---
name: run-local-uis
description: Start all four UIs (landing + console-ui + drive-ui + provider-dashboard) locally with hot reload and print the ports table. Use when the user says "run locally", "run the UIs", "start the UIs", "spin up the UIs", or similar — covers any time the goal is to drive the user-interfaces apps in a browser, including verifying changes to the shared network-picker, the landing page, or any individual UI.
---

Spin up every `user-interfaces/` app on its dedicated port with Vite HMR, then
print the ports table so the user can click straight into a browser. The
landing page needs special handling — it's a static HTML file with sentinel
placeholders that production replaces at build time — so we serve it through
a small Vite config kept alongside this skill that does the substitution
in-memory and rewrites the card links to point at the local dev ports.

## Ports

| App | URL | Workspace package |
|---|---|---|
| Landing | http://127.0.0.1:5176/ | (static; served via this skill's vite config) |
| Console UI | http://127.0.0.1:5173/ | `@web3-storage/console-ui` |
| Drive UI | http://127.0.0.1:5174/ | `@web3-storage/drive-ui` |
| Provider Dashboard | http://127.0.0.1:5175/ | `provider-dashboard` |

These ports are the canonical ones — `user-interfaces/README.md` and each
app's `vite.config.ts` are aligned to them. 5176 is reserved for the landing
page by this skill.

## Steps

1. **Check what's already up.** Be idempotent — don't restart servers the user
   already has running:

   ```bash
   for p in 5173 5174 5175 5176; do
     ss -ltn "sport = :$p" 2>/dev/null | grep -q LISTEN && echo "$p: up" || echo "$p: free"
   done
   ```

2. **Free ports → start that app, leave the rest alone.** Launch each as a
   detached background process so it survives the turn. Log to
   `/tmp/run-local-uis/<app>.log`:

   ```bash
   mkdir -p /tmp/run-local-uis
   cd /home/bparity/parity/web3-storage/user-interfaces

   # Console UI — vite default 5173
   nohup pnpm --filter @web3-storage/console-ui run dev -- --host 127.0.0.1 \
     > /tmp/run-local-uis/console-ui.log 2>&1 &
   disown

   # Drive UI — vite.config.ts pins 5174
   nohup pnpm --filter @web3-storage/drive-ui run dev -- --host 127.0.0.1 \
     > /tmp/run-local-uis/drive-ui.log 2>&1 &
   disown

   # Provider dashboard — vite.config.ts pins 5175
   nohup pnpm --filter provider-dashboard run dev -- --host 127.0.0.1 \
     > /tmp/run-local-uis/provider.log 2>&1 &
   disown

   # Landing — no package.json. Borrow any per-app vite binary (they're all
   # the same version) and point it at this skill's config, which substitutes
   # __NETWORKS_JSON__ etc. in-memory and rewrites card links to the dev ports.
   nohup ./console-ui/node_modules/.bin/vite \
     --config /home/bparity/parity/web3-storage/.claude/skills/run-local-uis/landing-vite.config.mjs \
     > /tmp/run-local-uis/landing.log 2>&1 &
   disown
   ```

   Skip any app whose port is already listening.

3. **Wait briefly and verify each port is up.** Vite usually binds within 2-5s
   after `pnpm install` is already warm:

   ```bash
   sleep 5
   ss -ltn 2>/dev/null | grep -E ':51(73|74|75|76)\b'
   ```

   If a port is still missing, `tail -30 /tmp/run-local-uis/<app>.log` to find
   the failure (most common: `pnpm install` not yet run for the workspace, or
   another process holding the port).

4. **Confirm the landing page's substitution worked.** The raw HTML contains
   `__NETWORKS_JSON__`, `__DEFAULT_NETWORK_ID__`, `__VALID_IDS_JSON__` and
   relative `./console/` style links — if any of those survive into the served
   HTML, the vite config didn't run:

   ```bash
   body=$(curl -s --max-time 5 http://127.0.0.1:5176/)
   echo "stale placeholders: $(grep -cE '__(NETWORKS_JSON|DEFAULT_NETWORK_ID|VALID_IDS_JSON)__' <<<"$body")"
   echo "stale relative cards: $(grep -cE \"['\\\"]\\\\./(console|provider|drive)/['\\\"]\" <<<"$body")"
   ```

   Both counts should be `0`.

5. **Print the table.** Show the user the four URLs above as a compact markdown
   table so they can click into any of them. Mention that hot reload is wired
   for every app (Vite HMR for the React apps; the landing's HMR also covers
   `user-interfaces/shared/network-config/src/{networks,types}.ts` via the
   plugin's watcher).

6. **Stopping.** When the user wants to stop, kill by port (don't use
   `pkill -f vite.config.mjs` — the pattern matches the kill command itself
   and silently kills your shell):

   ```bash
   for p in 5173 5174 5175 5176; do
     pid=$(ss -ltnp 2>/dev/null | sed -n "s/.*:$p .*pid=\([0-9]*\).*/\1/p" | head -1)
     [ -n "$pid" ] && kill "$pid"
   done
   ```

## Notes

- **Hot reload coverage**: edits anywhere under each app's `src/` HMR-update
  the app; edits in `user-interfaces/shared/network-picker/src/` HMR-update
  all three React apps that import it as source. The landing page's plugin
  watches `shared/network-config/src/{networks,types}.ts` and triggers full
  reload on change.
- **Why a custom vite config for landing?** `landing/index.html` has sentinel
  placeholders (`__NETWORKS_JSON__`, etc.) that
  `user-interfaces/landing/inject-config.mjs` rewrites at build/deploy time.
  Serving the raw file would surface JS syntax errors. The skill's config
  reproduces the substitution as an in-memory `transformIndexHtml` plugin and
  additionally rewrites the relative card links (`./console/`, `./provider/`,
  `./drive/`) to absolute dev-server URLs so the landing's cards open the
  local apps instead of falling through to the landing's own port.
- **First-time setup** (only if `pnpm install` hasn't been run in this clone
  yet): `cd user-interfaces && pnpm install`.
