// SPDX-License-Identifier: GPL-3.0-only

/**
 * Apply chain-derived configuration after the chain connects. For Photos the
 * only chain-derived setting we need is the SS58 prefix used to encode
 * addresses; the rest of the returned identity is published into chain state.
 * Display formatters live in the shared `@web3-storage/format` package.
 */
export async function configureFromChain(props: {
  ss58Prefix: number
  specName: string
  specVersion: number
  genesisHash: string
}): Promise<{ name: string; version: string; genesisHash: string }> {
  // Dynamically import to avoid a circular dependency (wallet → chain-client → chain)
  const { updateSs58Prefix } = await import('@/state/wallet.state')
  await updateSs58Prefix(props.ss58Prefix)

  return {
    name: props.specName,
    version: String(props.specVersion),
    genesisHash: props.genesisHash,
  }
}
