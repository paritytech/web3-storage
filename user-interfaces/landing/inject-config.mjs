#!/usr/bin/env node
// Substitute landing-page sentinels from the network-config TypeScript source.
//
//   __DEFAULT_NETWORK_ID__  ← networks.ts          DEFAULT_NETWORK_ID
//   __NETWORKS_JSON__       ← networks.ts          each *_NETWORK constant's
//                                                  id, parachainWs, providerHttp
//   __VALID_IDS_JSON__      ← types.ts             the NetworkId union
//
// Usage: node inject-config.mjs <input.html> <output.html>

import { readFileSync, writeFileSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const NETWORKS_TS = resolve(HERE, '../shared/network-config/src/networks.ts')
const TYPES_TS = resolve(HERE, '../shared/network-config/src/types.ts')

const [, , inHtml, outHtml] = process.argv
if (!inHtml || !outHtml) {
  console.error('usage: inject-config.mjs <input.html> <output.html>')
  process.exit(1)
}

function fail(msg) {
  console.error(`inject-config: ${msg}`)
  process.exit(2)
}

const networksSrc = readFileSync(NETWORKS_TS, 'utf8')
const typesSrc = readFileSync(TYPES_TS, 'utf8')

// DEFAULT_NETWORK_ID — may be a plain literal or a build-time ternary
// (`import.meta.env?.DEV ? 'local' : 'previewnet'`). This runs at production
// build time (DEV is false), so resolve to the else branch: the last quoted
// literal in the expression, or the sole literal for the plain form.
const defaultExpr = networksSrc.match(
  /DEFAULT_NETWORK_ID:\s*NetworkId\s*=\s*([\s\S]*?)(?=\n\s*\n|\nexport\b|\nfunction\b)/,
)?.[1]
if (!defaultExpr) fail('could not extract DEFAULT_NETWORK_ID from networks.ts')
const defaultLiterals = [...defaultExpr.matchAll(/'([a-z]+)'/g)].map((m) => m[1])
const defaultId = defaultLiterals.at(-1)
if (!defaultId) fail('could not extract DEFAULT_NETWORK_ID literal from networks.ts')

// NetworkId union → VALID_IDS (string[])
const unionMatch = typesSrc.match(/export\s+type\s+NetworkId\s*=\s*([^\n;]+)/)
if (!unionMatch) fail('could not extract NetworkId union from types.ts')
const validIds = [...unionMatch[1].matchAll(/'([a-z]+)'/g)].map((m) => m[1])
if (validIds.length === 0) fail('NetworkId union has no string literals')

// Each `export const X_NETWORK: NetworkConfig = { ... }` block, keyed by `id`,
// keeping only the two URLs the landing picker displays.
const networks = {}
const blockRe = /export\s+const\s+\w+_NETWORK\s*:\s*NetworkConfig\s*=\s*\{([\s\S]*?)\}/g
for (const m of networksSrc.matchAll(blockRe)) {
  const body = m[1]
  const id = body.match(/id:\s*'([a-z]+)'/)?.[1]
  if (!id) continue
  const parachainWs = body.match(/parachainWs:\s*'([^']*)'/)?.[1] ?? ''
  const providerHttp = body.match(/providerHttp:\s*'([^']*)'/)?.[1] ?? ''
  networks[id] = { parachainWs, providerHttp }
}
if (Object.keys(networks).length === 0) fail('no networks extracted from networks.ts')

const html = readFileSync(inHtml, 'utf8')
  .replaceAll('__DEFAULT_NETWORK_ID__', defaultId)
  .replaceAll('__NETWORKS_JSON__', JSON.stringify(networks))
  .replaceAll('__VALID_IDS_JSON__', JSON.stringify(validIds))

writeFileSync(outHtml, html)
console.error(
  `inject-config: DEFAULT=${defaultId}, networks=[${Object.keys(networks).join(',')}], validIds=[${validIds.join(',')}] → ${outHtml}`,
)
