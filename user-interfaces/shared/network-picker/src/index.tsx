// SPDX-License-Identifier: GPL-3.0-only

import { useEffect, useMemo, useRef, useState } from 'react'
import type {
  NetworkConfig,
  NetworkId,
} from '@web3-storage/network-config'
import './styles.css'

type Status = 'idle' | 'checking' | 'up' | 'down'

const PROBE_TIMEOUT_MS = 5000

function healthUrl(base: string): string {
  return base.endsWith('/') ? `${base}health` : `${base}/health`
}

/**
 * Probe both endpoints of a NetworkConfig (or custom inputs that override the
 * URLs). Returns the live { ws, http } statuses; re-runs whenever the resolved
 * URLs change.
 */
function useEndpointProbes(ws: string, http: string) {
  const [wsStatus, setWsStatus] = useState<Status>('idle')
  const [httpStatus, setHttpStatus] = useState<Status>('idle')
  const tokenRef = useRef(0)

  useEffect(() => {
    const token = ++tokenRef.current

    // WebSocket probe — no CORS issues, status is honest.
    setWsStatus(ws ? 'checking' : 'idle')
    let wsCloser: (() => void) | null = null
    if (ws) {
      let done = false
      let sock: WebSocket | null = null
      const finish = (state: Status) => {
        if (done || token !== tokenRef.current) return
        done = true
        setWsStatus(state)
        try { sock?.close() } catch { /* ignore */ }
      }
      try {
        sock = new WebSocket(ws)
        sock.onopen = () => finish('up')
        sock.onerror = () => finish('down')
        sock.onclose = (ev) => { if (!done) finish(ev.wasClean ? 'up' : 'down') }
        const t = setTimeout(() => finish('down'), PROBE_TIMEOUT_MS)
        wsCloser = () => { clearTimeout(t); finish('down') }
      } catch {
        finish('down')
      }
    }

    // HTTP probe — try CORS-mode first so res.ok catches nginx fall-through /
    // wrong route. On a CORS *rejection*, fall back to a no-cors fetch: opaque,
    // but resolves whenever the host answered. Guards against duplicate-ACAO
    // from a misconfigured proxy, which would otherwise show a healthy provider
    // as down.
    setHttpStatus(http ? 'checking' : 'idle')
    const ctrl = typeof AbortController !== 'undefined' ? new AbortController() : null
    let httpTimer: ReturnType<typeof setTimeout> | null = null
    if (http) {
      httpTimer = setTimeout(() => ctrl?.abort(), PROBE_TIMEOUT_MS)
      const target = healthUrl(http)
      const probe = async (): Promise<Status> => {
        try {
          const res = await fetch(target, { method: 'GET', mode: 'cors', cache: 'no-store', signal: ctrl?.signal })
          return res.ok ? 'up' : 'down'
        } catch {
          // CORS-mode failed without a readable answer — fall back to an opaque
          // liveness check. A still-aborted signal (timeout) rejects immediately.
          try {
            await fetch(target, { method: 'GET', mode: 'no-cors', cache: 'no-store', signal: ctrl?.signal })
            return 'up'
          } catch {
            return 'down'
          }
        }
      }
      probe()
        .then((state) => { if (token === tokenRef.current) setHttpStatus(state) })
        .finally(() => { if (httpTimer) clearTimeout(httpTimer) })
    }

    return () => {
      tokenRef.current++
      wsCloser?.()
      if (httpTimer) clearTimeout(httpTimer)
      ctrl?.abort()
    }
  }, [ws, http])

  return { wsStatus, httpStatus }
}

/**
 * Combined "worst of both" status for the compact button indicator.
 */
function combinedStatus(ws: Status, http: Status): Status {
  if (ws === 'down' || http === 'down') return 'down'
  if (ws === 'checking' || http === 'checking') return 'checking'
  if (ws === 'up' && http === 'up') return 'up'
  return 'idle'
}

/**
 * Read-only status panel — two rows (Parachain RPC, Provider HTTP) with the
 * configured URL and a live reachability dot. Same labels as NetworkPicker
 * ("(not running)" / "Not deployed yet").
 */
export function NetworkStatusPanel({
  network,
  className,
}: {
  network: NetworkConfig
  className?: string
}) {
  const { wsStatus, httpStatus } = useEndpointProbes(network.parachainWs, network.providerHttp)
  const placeholder = network.id === 'custom' ? '(not set)' : 'Not deployed yet'

  return (
    <dl className={`np-endpoints np-endpoints-standalone ${className ?? ''}`} data-testid="network-status-panel">
      <EndpointRow label="Parachain RPC" url={network.parachainWs} status={wsStatus} placeholder={placeholder} testidPrefix="network-status-ws" />
      <EndpointRow label="Provider HTTP" url={network.providerHttp} status={httpStatus} placeholder={placeholder} testidPrefix="network-status-http" />
    </dl>
  )
}

function EndpointRow({
  label,
  url,
  status,
  placeholder,
  testidPrefix,
}: {
  label: string
  url: string
  status: Status
  placeholder: string
  testidPrefix: string
}) {
  return (
    <>
      <dt>{label}</dt>
      <dd className={url ? '' : 'placeholder'}>
        <span className="np-status" data-state={status} data-testid={`${testidPrefix}-dot`} title={status} />
        <span data-testid={testidPrefix}>{url || placeholder}</span>
        {status === 'down' && <span className="np-status-text">(not running)</span>}
      </dd>
    </>
  )
}

export interface NetworkPickerProps {
  selectedNetwork: NetworkConfig
  networkList: NetworkConfig[]
  onSelect: (id: NetworkId) => void | Promise<void>
  onSelectCustom: (input: {
    parachainWs: string
    providerHttp: string
  }) => void | Promise<void>
  /** Render as a compact button that opens a popover. Default: inline panel. */
  compact?: boolean
  className?: string
  /** Color scheme. Default: "light". */
  theme?: "light" | "dark"
  // show provider status
  showProviderStatus?: boolean
}

export function NetworkPicker(props: NetworkPickerProps) {
  if (props.compact) return <CompactPicker {...props} />
  return <InlinePicker {...props} />
}

function InlinePicker({
  selectedNetwork,
  networkList,
  onSelect,
  onSelectCustom,
  className,
  theme = 'light',
  showProviderStatus = false,
}: NetworkPickerProps) {
  return (
    <div className={`np-root ${className ?? ''}`} data-theme={theme} data-testid="network-picker">
      <PickerBody
        selectedNetwork={selectedNetwork}
        networkList={networkList}
        onSelect={onSelect}
        onSelectCustom={onSelectCustom}
        showProviderStatus={showProviderStatus}
      />
    </div>
  )
}

function CompactPicker({
  selectedNetwork,
  networkList,
  onSelect,
  onSelectCustom,
  className,
  theme = 'light',
  showProviderStatus = false
}: NetworkPickerProps) {
  const [open, setOpen] = useState(false)
  // Probes run for the button indicator regardless of popover state.
  const { wsStatus, httpStatus } = useEndpointProbes(
    selectedNetwork.parachainWs,
    selectedNetwork.providerHttp,
  )
  const overall = combinedStatus(wsStatus, httpStatus)

  return (
    <div className={`np-compact-root ${className ?? ''}`} data-theme={theme} data-testid="network-picker-compact">
      <button
        type="button"
        className="np-compact-button"
        data-testid="network-picker-toggle"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="np-status np-status-button" data-state={overall} />
        <span className="np-compact-name">{selectedNetwork.name}</span>
        <span className="np-compact-chevron" aria-hidden>{open ? '▴' : '▾'}</span>
      </button>
      {open && (
        <>
          <div className="np-backdrop" onClick={() => setOpen(false)} />
          <div className="np-popover np-root" data-theme={theme} data-testid="network-picker-popover">
            <PickerBody
              selectedNetwork={selectedNetwork}
              networkList={networkList}
              onSelect={(id) => { void onSelect(id) }}
              onSelectCustom={(input) => { void onSelectCustom(input) }}
              showProviderStatus={showProviderStatus}
            />
          </div>
        </>
      )}
    </div>
  )
}

function PickerBody({
  selectedNetwork,
  networkList,
  onSelect,
  onSelectCustom,
  showProviderStatus,
}: Pick<NetworkPickerProps, 'selectedNetwork' | 'networkList' | 'onSelect' | 'onSelectCustom' | 'showProviderStatus'>) {
  const isCustom = selectedNetwork.id === 'custom'
  const [customWs, setCustomWs] = useState(isCustom ? selectedNetwork.parachainWs : '')
  const [customHttp, setCustomHttp] = useState(isCustom ? selectedNetwork.providerHttp : '')
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const displayedWs = isCustom ? customWs.trim() : selectedNetwork.parachainWs
  const displayedHttp = isCustom ? customHttp.trim() : selectedNetwork.providerHttp
  const placeholder = isCustom ? '(not set)' : 'Not deployed yet'

  const { wsStatus, httpStatus } = useEndpointProbes(displayedWs, displayedHttp)

  const hasCustomOption = useMemo(() => !networkList.some((n) => n.id === 'custom'), [networkList])

  function handleSelectChange(id: NetworkId) {
    if (id === 'custom') {
      if (customWs.trim() && customHttp.trim()) {
        void onSelectCustom({ parachainWs: customWs.trim(), providerHttp: customHttp.trim() })
      } else {
        void onSelect('custom')
      }
      return
    }
    void onSelect(id)
  }

  function scheduleCustomApply(ws: string, http: string) {
    if (debounceRef.current) clearTimeout(debounceRef.current)
    debounceRef.current = setTimeout(() => {
      if (ws && http) void onSelectCustom({ parachainWs: ws, providerHttp: http })
    }, 600)
  }

  return (
    <>
      <div className="np-row">
        <span className="np-label">Network</span>
        <select
          className="np-select"
          data-testid="network-picker-select"
          value={selectedNetwork.id}
          onChange={(e) => handleSelectChange(e.target.value as NetworkId)}
        >
          {networkList.map((n) => (
            <option key={n.id} value={n.id}>{n.name}</option>
          ))}
          {hasCustomOption && <option value="custom">Custom RPC…</option>}
        </select>
      </div>

      {isCustom && (
        <div className="np-custom">
          <input
            className="np-input"
            data-testid="network-picker-custom-ws"
            type="text"
            placeholder="ws://… parachain RPC"
            value={customWs}
            onChange={(e) => {
              setCustomWs(e.target.value)
              scheduleCustomApply(e.target.value.trim(), customHttp.trim())
            }}
          />
          <input
            className="np-input"
            data-testid="network-picker-custom-http"
            type="text"
            placeholder="https://… provider HTTP"
            value={customHttp}
            onChange={(e) => {
              setCustomHttp(e.target.value)
              scheduleCustomApply(customWs.trim(), e.target.value.trim())
            }}
          />
        </div>
      )}

      <dl className="np-endpoints">
        <EndpointRow label="Parachain RPC" url={displayedWs} status={wsStatus} placeholder={placeholder} testidPrefix="network-picker-endpoint-ws" />
        {
          showProviderStatus ? (
            <EndpointRow label="Provider HTTP" url={displayedHttp} status={httpStatus} placeholder={placeholder} testidPrefix="network-picker-endpoint-http" />
          ) : (<></>)
        }
      </dl>
    </>
  )
}
