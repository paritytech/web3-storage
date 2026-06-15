# Provider-Initiated Checkpoint

Autonomous: a deterministic leader coordinates signature collection from peers and submits on-chain. Includes grace period fallback and missed-window slashing.

```mermaid
sequenceDiagram
    autonumber
    participant Leader as Leader Provider
    participant Peer as Peer Provider
    participant Chain as Parachain
    participant Reporter as Reporter

    Note over Chain: Leader = blake2_256(bucket_id || window) % N

    Leader->>Chain: get_active_checkpoint_duties()
    Chain-->>Leader: Duty: bucket_id, window, is_leader=true

    Leader->>Leader: Build CheckpointProposal with window
    Leader->>Leader: Self-sign proposal (sr25519)

    Leader->>Peer: POST /checkpoint/sign
    Peer->>Peer: Compare local state to proposal

    alt State matches
        Peer-->>Leader: agreed=true, signature
    else State disagrees
        Peer-->>Leader: agreed=false
    end

    Leader->>Leader: Collected sigs >= min_providers

    Leader->>Chain: provider_checkpoint(bucket_id, mmr_root, window, sigs[])
    Chain->>Chain: Verify window == current_window
    Chain->>Chain: Verify window > LastCheckpointWindow

    alt Within grace period
        Chain->>Chain: Only designated leader allowed
    else After grace period
        Chain->>Chain: Any primary provider allowed
    end

    Chain->>Chain: Verify signatures, update BucketSnapshot
    Chain->>Chain: Pay reward from CheckpointPool
    Chain-->>Leader: Event ProviderCheckpointSubmitted

    Note over Reporter, Chain: Missed Checkpoint Penalty

    Reporter->>Chain: report_missed_checkpoint(bucket_id, window)
    Chain->>Chain: Verify window passed without checkpoint
    Chain->>Chain: Slash leader 0.5 token, reward reporter 10%
    Chain-->>Reporter: Event CheckpointMissPenalized
```

## Key Details

- **Window**: `current_block / interval`. Default interval is 100 blocks.
- **Leader election**: Deterministic via `blake2_256(bucket_id || window) % num_providers`.
- **Grace period** (default 20 blocks): Only the leader can submit during grace. After grace, any primary provider can submit as fallback.
- **CheckpointProposal** includes the `window` field (unlike `CommitmentPayload`) to prevent cross-window replay.
- **Rewards**: Submitter receives `CheckpointReward` (1 token) from the `CheckpointPool` funded by clients.
- **Penalties**: If no checkpoint is submitted for a past window, anyone can call `report_missed_checkpoint`. The leader is slashed `CheckpointMissPenalty` (0.5 token), reporter gets 10%.
