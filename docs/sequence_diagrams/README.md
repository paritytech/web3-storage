# Sequence Diagrams

Visual sequence diagrams for the core protocol flows in Scalable Web3 Storage.

## Provider Lifecycle

| Diagram | Description |
|---|---|
| [Provider Registration](./provider-registration.md) | On-chain registration (stake locking, settings), provider node startup sequence, and adding stake. |
| [Provider Deregistration](./provider-deregistration.md) | Two-step exit: announce (cooldown >= ChallengeTimeout), then complete (stake returned). Includes cancellation flow. |

## S3 Bucket Creation

| Diagram | Description |
|---|---|
| [S3 Bucket Creation](./s3-bucket-creation.md) | End-to-end flow: provider discovery, off-chain negotiation with term signing, single atomic on-chain extrinsic (Layer 0 bucket + agreement + S3 registry), and UI completion. |

## S3 Object Read & Write

| Diagram | Description |
|---|---|
| [S3 Write Flow (putObject)](./s3-write-flow.md) | Client upload to primary provider: auth, chunking (256 KiB), Merkle tree, MMR commit, S3 index update. Includes replica sync (background pull from primaries). |
| [S3 Read Flow (getObject)](./s3-read-flow.md) | Client download: provider URL resolution, auth, DFS tree traversal, chunk reassembly. Includes Rust SDK verified download and multi-provider topology. |

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
