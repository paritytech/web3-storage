/**
 * Console-UI network state.
 *
 * console-ui's chain client lives in a React Context (`useChain`), so we can't
 * call connect/disconnect from a plain module function. We expose a tiny hook
 * that produces ready-to-use callbacks bound to that context.
 */

import { useState, useCallback } from 'react'
import {
  type NetworkConfig,
  type NetworkId,
  NETWORK_LIST,
  NETWORKS,
  loadSelectedNetwork,
  saveSelectedNetwork,
  createCustomNetwork,
} from '@web3-storage/network-config'
import { useChain } from '@/hooks/useChain'

export function useNetworkSelection() {
  const { connect, disconnect } = useChain()
  const [selectedNetwork, setSelectedNetwork] = useState<NetworkConfig>(
    () => loadSelectedNetwork().config,
  )

  const selectNetwork = useCallback(
    async (id: NetworkId) => {
      const config = NETWORKS[id]
      if (!config) return
      setSelectedNetwork(config)
      saveSelectedNetwork(id)
      if (config.parachainWs) {
        disconnect()
        await connect(config.parachainWs)
      }
    },
    [connect, disconnect],
  )

  const selectCustomNetwork = useCallback(
    async (input: { parachainWs: string; providerHttp: string }) => {
      const config = createCustomNetwork(input.parachainWs, input.providerHttp)
      setSelectedNetwork(config)
      saveSelectedNetwork('custom', config)
      if (config.parachainWs) {
        disconnect()
        await connect(config.parachainWs)
      }
    },
    [connect, disconnect],
  )

  return {
    selectedNetwork,
    networkList: NETWORK_LIST,
    selectNetwork,
    selectCustomNetwork,
  }
}
