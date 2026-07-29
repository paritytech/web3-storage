// SPDX-License-Identifier: GPL-3.0-only

// Dev-server config for the landing page. Reproduces what
// `user-interfaces/landing/inject-config.mjs` does at build time, but as an
// in-memory Vite `transformIndexHtml` plugin so the dev server can serve a
// working index.html (without the `__…__` placeholders that would otherwise
// break the inline JS) and full-reload when the network-config TS sources
// change. Also rewrites the card links to absolute Vite dev-server URLs so
// the landing's "Provider / Drive / S3" cards open the locally-running
// apps instead of falling through to the landing's own port.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
// `defineConfig` is import-from-'vite', which doesn't resolve when the config
// lives outside a workspace's node_modules — and it's only there for types,
// so we just `export default { ... }`.

const HERE = dirname(fileURLToPath(import.meta.url))
// .claude/skills/run-local-uis/ → repo root → user-interfaces/
const REPO = resolve(HERE, '../../..')
const UI = resolve(REPO, 'user-interfaces')
const LANDING = resolve(UI, 'landing')
const NETWORKS_TS = resolve(UI, 'shared/network-config/src/networks.ts')
const TYPES_TS = resolve(UI, 'shared/network-config/src/types.ts')

function readConfig() {
  const networksSrc = readFileSync(NETWORKS_TS, 'utf8')
  const typesSrc = readFileSync(TYPES_TS, 'utf8')

  // `DEFAULT_NETWORK_ID` may be a plain literal or a build-time ternary
  // (`import.meta.env?.DEV ? 'local' : 'previewnet'`). This is a dev server, so
  // mirror `import.meta.env.DEV === true` and pick the truthy branch (the
  // literal right after `?`); fall back to the sole literal otherwise.
  const defaultExpr = networksSrc.match(
    /DEFAULT_NETWORK_ID:\s*NetworkId\s*=\s*([\s\S]*?)(?=\n\s*\n|\nexport\b|\nfunction\b)/,
  )?.[1]
  if (!defaultExpr) throw new Error('could not extract DEFAULT_NETWORK_ID')
  const defaultId = defaultExpr.includes('?')
    ? defaultExpr.match(/\?\s*'([a-z]+)'/)?.[1]
    : defaultExpr.match(/'([a-z]+)'/)?.[1]
  if (!defaultId) throw new Error('could not extract DEFAULT_NETWORK_ID literal')

  const unionBody = typesSrc.match(/export\s+type\s+NetworkId\s*=\s*([^\n;]+)/)?.[1]
  if (!unionBody) throw new Error('could not extract NetworkId union')
  const validIds = [...unionBody.matchAll(/'([a-z]+)'/g)].map((m) => m[1])

  const networks = {}
  const blockRe = /export\s+const\s+\w+_NETWORK\s*:\s*NetworkConfig\s*=\s*\{([\s\S]*?)\}/g
  for (const m of networksSrc.matchAll(blockRe)) {
    const body = m[1]
    const id = body.match(/id:\s*'([a-z]+)'/)?.[1]
    if (!id) continue
    networks[id] = {
      parachainWs: body.match(/parachainWs:\s*'([^']*)'/)?.[1] ?? '',
      providerHttp: body.match(/providerHttp:\s*'([^']*)'/)?.[1] ?? '',
    }
  }
  return { defaultId, validIds, networks }
}

// The landing page's card links and BASES map are relative (`./console/`,
// `./provider/`, `./drive/`) because in production all four are deployed under
// the same origin. Locally each app is on its own Vite port, so we rewrite the
// three known paths to absolute dev URLs — both the single-quoted BASES values
// and the double-quoted `href` attributes (the click handler uses BASES, but
// middle-click / "open in new tab" relies on the `href`).
const DEV_BASES = {
  './provider/': 'http://127.0.0.1:5175/',
  './drive/': 'http://127.0.0.1:5174/',
  './s3/': 'http://127.0.0.1:5177/',
  './photos/': 'http://127.0.0.1:5178/',
}

function injectConfigPlugin() {
  return {
    name: 'landing-inject-config',
    transformIndexHtml(html) {
      const { defaultId, validIds, networks } = readConfig()
      let out = html
        .replaceAll('__DEFAULT_NETWORK_ID__', defaultId)
        .replaceAll('__NETWORKS_JSON__', JSON.stringify(networks))
        .replaceAll('__VALID_IDS_JSON__', JSON.stringify(validIds))
      for (const [rel, abs] of Object.entries(DEV_BASES)) {
        out = out.replaceAll(`'${rel}'`, `'${abs}'`).replaceAll(`"${rel}"`, `"${abs}"`)
      }
      return out
    },
    configureServer(server) {
      // Full-reload when the upstream config sources change so the substituted
      // HTML reflects fresh network-config edits.
      server.watcher.add([NETWORKS_TS, TYPES_TS])
      const reload = (file) => {
        if (file === NETWORKS_TS || file === TYPES_TS) {
          server.ws.send({ type: 'full-reload', path: '*' })
        }
      }
      server.watcher.on('change', reload)
      server.watcher.on('add', reload)
    },
  }
}

export default ({
  root: LANDING,
  // Keep dev artifacts out of the landing tree.
  cacheDir: resolve(HERE, '.vite-cache'),
  server: {
    host: '127.0.0.1',
    port: 5176,
    strictPort: true,
  },
  plugins: [injectConfigPlugin()],
})
