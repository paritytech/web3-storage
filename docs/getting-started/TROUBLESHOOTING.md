# Troubleshooting

Common errors when running the local network and demos, and how to resolve
them. Parameter values quoted below are the local-runtime defaults — the
authoritative values live in `runtimes/web3-storage-local/src/storage.rs`.

## "InsufficientStake" error

- Minimum required provider stake: 1000 tokens = `1000000000000000`
  (12 decimals, like Polkadot).
- Check the account's balance in the Accounts tab of
  [polkadot.js Apps](https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222).

## "PaymentExceedsMax" error

- Calculate the payment: `price_per_byte × max_bytes × duration`.
- Set `maxPayment` with a 10–20% buffer to account for price changes.
- See the [Payment Calculator](../reference/PAYMENT_CALCULATOR.md) for worked
  examples.

## Upload fails

- Complete the on-chain setup first: register the provider, create a bucket,
  establish an agreement.
- With the chain and provider already running, `just demo` performs that
  setup end to end.
- For chain health, run `bash scripts/check-chain.sh` (relay + parachain
  probe); `just health` checks the provider.

## Provider not accepting agreements

- Call `updateProviderSettings` after registration.
- Set `acceptingPrimary: true`.

## "CapacityExceeded" or "InsufficientStakeForCapacity" error

- The provider's `max_capacity` is too low for the agreement, or the
  provider's stake doesn't cover its declared capacity.
- Required: `stake >= max_capacity * MinStakePerByte`.
- Use `DiscoveryClient::find_providers()` to find providers with sufficient
  capacity.
