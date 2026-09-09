# Client-Initiated Checkpoint

The client orchestrates: collects provider signatures via HTTP, verifies consensus, then submits a single on-chain extrinsic.

```mermaid
sequenceDiagram
    autonumber
    participant Client as Client SDK
    participant P1 as Provider 1
    participant P2 as Provider 2
    participant Chain as Parachain

    Note over Client: submit_checkpoint(bucket_id)

    par Collect signatures
        Client->>P1: GET /checkpoint-signature?bucket_id=X
        P1->>P1: Sign CommitmentPayload (sr25519)
        P1-->>Client: mmr_root, start_seq, leaf_count, sig
    and
        Client->>P2: GET /checkpoint-signature?bucket_id=X
        P2->>P2: Sign CommitmentPayload (sr25519)
        P2-->>Client: mmr_root, start_seq, leaf_count, sig
    end

    Client->>Client: Group by mmr_root, check consensus >= 51%

    alt Consensus reached
        Client->>Chain: checkpoint(bucket_id, mmr_root, start_seq, leaf_count, sigs[])
        Chain->>Chain: Verify caller is writer/admin
        Chain->>Chain: Verify each signature against registered pubkeys
        Chain->>Chain: Check signing_count >= min_providers
        Chain->>Chain: Create BucketSnapshot
        Chain-->>Client: Event BucketCheckpointed
    else No consensus
        Client-->>Client: CheckpointResult::NoConsensus
    end
```

## Key Details

- **CommitmentPayload** is signed with the **real** `leaf_count` (via `/checkpoint-signature`).
- The client verifies majority consensus (same `mmr_root` from >= 51% of providers) before submitting.
- On-chain, each signature is verified against the provider's registered public key, and the provider's bit is set in the `primary_signers` bitfield.
- The resulting `BucketSnapshot` makes providers liable for the data.
