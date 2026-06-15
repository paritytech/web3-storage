# Challenge Creation

The client (challenger) initiates a challenge when a provider fails to serve data. Two entry points converge into the same on-chain challenge.

```mermaid
sequenceDiagram
    autonumber
    participant Client as Client (Challenger)
    participant Provider as Provider Node (HTTP)
    participant Chain as Parachain (Pallet)

    Note over Client,Provider: Earlier: Upload & Commit (client saves metadata)
    Client->>Provider: POST /upload (chunked data)
    Provider-->>Client: data_root

    Client->>Provider: POST /commit {bucket_id, data_roots}
    Provider->>Provider: Append leaves to MMR, sign CommitmentPayload (leaf_count=0)
    Provider-->>Client: {mmr_root, start_seq, leaf_indices, provider_signature}
    Client->>Client: Save leaf_indices, provider_sig, chunk count per leaf

    Note over Client,Chain: Later: Client spot-checks by downloading a chunk
    Client->>Provider: GET /download?bucket_id=X&leaf=3&chunk=7
    Provider-->>Client: (no response / wrong data / timeout)
    Client->>Client: Verification failed — initiate challenge

    Note over Client,Chain: Option A: challenge_checkpoint (on-chain snapshot)
    Client->>Chain: challenge_checkpoint(bucket_id, provider, leaf_index, chunk_index)
    Chain->>Chain: Verify provider bit set in snapshot.primary_signers
    Chain->>Chain: Reserve deposit (100 units) from client
    Chain->>Chain: Store Challenge at deadline = now + 48h
    Chain-->>Client: Event: ChallengeCreated{challenge_id, respond_by}

    Note over Client,Chain: Option B: challenge_offchain (saved provider_sig)
    Client->>Chain: challenge_offchain(bucket_id, provider, mmr_root, start_seq, leaf_index, chunk_index, provider_sig)
    Chain->>Chain: Verify provider_sig over CommitmentPayload{..., leaf_count=0}
    Chain->>Chain: Reserve deposit, store Challenge at deadline
    Chain-->>Client: Event: ChallengeCreated{challenge_id, respond_by}
```

## How the Client Knows `chunk_index` and `provider_sig`

| Parameter | Source |
|---|---|
| `chunk_index` | The client chunked the data itself (256 KiB chunks). Any index from `0` to `ceil(data_size / 256KiB) - 1` is valid. |
| `leaf_index` | Returned by `POST /commit` in the `leaf_indices` array. |
| `provider_sig` | Returned by `POST /commit` or `GET /commitment` (signed with `leaf_count=0`). |
| `mmr_root` | Returned by `POST /commit` or `GET /commitment`. |
| `start_seq` | Returned by `POST /commit` or `GET /commitment`. |

## Two Challenge Modes

- **`challenge_checkpoint`**: Uses the on-chain snapshot. No off-chain signature needed. Simpler but requires a checkpoint to exist.
- **`challenge_offchain`**: Uses the provider's off-chain commitment signature (with `leaf_count=0`). Works even without an on-chain checkpoint. Preferred for "hot" buckets where checkpoints change frequently.
