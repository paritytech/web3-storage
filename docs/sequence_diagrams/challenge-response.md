# Challenge Response (Successful Defense)

The provider detects a pending challenge, builds a two-layer proof from its storage backend, and submits it on-chain.

```mermaid
sequenceDiagram
    autonumber
    participant Chain as Parachain (Pallet)
    participant Provider as Provider Node
    participant Storage as Storage Backend

    Note over Provider: ChallengeResponder polls every ~6s
    Provider->>Chain: poll_challenges()
    Chain-->>Provider: DetectedChallenge{bucket_id, leaf_index, chunk_index, deadline}

    Provider->>Storage: get_mmr_proof(bucket_id, leaf_index)
    Storage-->>Provider: MmrProof{peaks, leaf{data_root, data_size}, leaf_proof}

    Provider->>Storage: get_chunk_at_index(data_root, chunk_index)
    Storage-->>Provider: chunk_data, chunk_proof

    Provider->>Chain: respond_to_challenge(challenge_id, Proof{chunk_data, mmr_proof, chunk_proof})

    Chain->>Chain: Verify caller == challenged provider
    Chain->>Chain: Verify current_block <= deadline
    Chain->>Chain: chunk_hash = blake2_256(chunk_data)
    Chain->>Chain: verify_merkle_proof(chunk_hash, chunk_index, chunk_proof, data_root)
    Chain->>Chain: verify_mmr_proof(mmr_proof, challenged mmr_root)
    Chain->>Chain: Remove challenge from storage

    Note over Chain: Cost split by response time
    Chain->>Chain: 1 blk: challenger 90% / provider 10%
    Chain->>Chain: 2-5: 80/20, 6-24: 70/30, 25-95: 60/40, 96+: 50/50
    Chain->>Chain: Partial deposit back to challenger, partial slash on provider
    Chain-->>Provider: Event: ChallengeDefended
```

## Two-Layer Proof Verification

1. **Chunk in leaf**: `verify_merkle_proof(blake2_256(chunk_data), chunk_index, chunk_proof, data_root)` proves the chunk belongs to the data blob.
2. **Leaf in MMR**: `verify_mmr_proof(mmr_proof, mmr_root)` proves the leaf (containing `data_root`) is in the MMR the provider committed to.

## Time-Based Cost Split

The faster the provider responds, the more the challenger pays (discouraging frivolous challenges):

| Response Time (blocks) | Challenger Pays | Provider Pays |
|---|---|---|
| 1 | 90% | 10% |
| 2-5 | 80% | 20% |
| 6-24 | 70% | 30% |
| 25-95 | 60% | 40% |
| 96+ | 50% | 50% |

## Alternative Defenses

Besides `Proof`, providers can also respond with:
- **`Deleted`**: Admin signed a newer `CommitmentPayload` with `start_seq` beyond the challenged leaf (data was legitimately deleted).
- **`Superseded`**: A newer on-chain snapshot already covers the challenged leaf index (challenge is moot).
