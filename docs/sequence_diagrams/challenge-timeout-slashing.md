# Challenge Timeout & Slashing

When a provider fails to respond before the deadline, slashing happens automatically in `on_finalize` — no extrinsic needed.

```mermaid
sequenceDiagram
    autonumber
    participant Client as Client (Challenger)
    participant Chain as Parachain (on_finalize)

    Note over Chain: Block N reaches challenge deadline
    Chain->>Chain: on_finalize(N): Challenges::take(N)
    Chain->>Chain: Provider failed to respond in time

    Chain->>Chain: Slash provider ENTIRE stake
    Chain->>Chain: Refund client full deposit (100 units)
    Chain->>Chain: Mint 10% of slashed stake to client as reward
    Chain->>Chain: Remaining 90% burned / treasury
    Chain->>Chain: provider.stake = 0
    Chain->>Chain: Increment provider.challenges_failed

    Chain-->>Client: Event: ChallengeSlashed{slashed_amount, challenger_reward}
```

## Key Details

- **Automatic**: Slashing is triggered by `on_finalize` at the deadline block. No one needs to call an extrinsic.
- **Full stake loss**: The provider's **entire stake** is slashed (not just a portion).
- **Challenger incentive**: Full deposit refund + 10% of the slashed stake as reward.
- **Remaining 90%**: Burned or sent to treasury via the runtime's slash handler.
- **Provider state**: `provider.stake` is set to 0 and `challenges_failed` is incremented.
- **Deadline**: `ChallengeTimeout` is configured at 48 hours (in blocks) in the runtime.
