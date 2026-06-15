# Sequence Diagrams

Visual sequence diagrams for the core protocol flows in Scalable Web3 Storage.

## S3 Bucket Creation

| Diagram | Description |
|---|---|
| [S3 Bucket Creation](./s3-bucket-creation.md) | End-to-end flow: provider discovery, off-chain negotiation with term signing, single atomic on-chain extrinsic (Layer 0 bucket + agreement + S3 registry), and UI completion. |

## Checkpoint Flows

How on-chain `BucketSnapshot` records are created, making providers liable for stored data.

| Diagram | Description |
|---|---|
| [Client-Initiated Checkpoint](./checkpoint-client-initiated.md) | Client collects provider signatures via HTTP, verifies consensus, submits on-chain. |
| [Provider-Initiated Checkpoint](./checkpoint-provider-initiated.md) | Autonomous leader election, peer co-signing, grace period fallback, and missed-window penalties. |

## Challenge Flows

The dispute mechanism: clients challenge providers to prove they still hold data.

| Diagram | Description |
|---|---|
| [Challenge Creation](./challenge-creation.md) | How clients create challenges (both `challenge_checkpoint` and `challenge_offchain`). Includes how the client obtains `chunk_index` and `provider_sig`. |
| [Challenge Response](./challenge-response.md) | Provider detects challenge, builds two-layer MMR + Merkle proof, submits on-chain. Includes time-based cost split. |
| [Challenge Timeout & Slashing](./challenge-timeout-slashing.md) | Automatic slashing via `on_finalize` when a provider fails to respond. |
