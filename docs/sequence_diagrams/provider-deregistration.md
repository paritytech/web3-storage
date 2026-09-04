# Provider Deregistration (Two-Step Exit)

Providers cannot exit instantly — a cooldown period (>= `ChallengeTimeout`, default 48h) ensures any pending challenges can still be resolved. The provider remains on-chain and slashable during the wait.

## Deregistration Flow

```mermaid
sequenceDiagram
    autonumber
    participant Provider as Provider (Wallet)
    participant Chain as Parachain (StorageProvider Pallet)

    Note over Provider: Step 1: Announce intent to deregister

    Provider->>Chain: deregister_provider()

    Chain->>Chain: Ensure provider registered
    Chain->>Chain: Ensure committed_bytes == 0<br/>(no active agreements)
    Chain->>Chain: Set deregister_at = now + DeregisterAnnouncementPeriod
    Chain->>Chain: Force accepting_primary = false
    Chain->>Chain: Force accepting_extensions = false

    Chain-->>Provider: Event: DeregisterAnnounced{<br/>provider, complete_after}

    Note over Provider,Chain: Cooldown period (>= ChallengeTimeout)<br/>Provider remains on-chain and slashable.<br/>Cannot accept new agreements.<br/>Existing challenges can still be resolved.

    rect rgb(255, 245, 235)
    Note over Provider,Chain: Wait for DeregisterAnnouncementPeriod to elapse
    end

    Note over Provider: Step 2: Complete deregistration

    Provider->>Chain: complete_deregister()

    Chain->>Chain: Ensure current_block >= deregister_at
    Chain->>Chain: Ensure committed_bytes == 0 (still no agreements)
    Chain->>Chain: Drain pending CheckpointRewards into free balance
    Chain->>Chain: Currency::unreserve(provider, stake)<br/>Tokens moved: reserved -> free balance
    Chain->>Chain: Remove Providers storage entry
    Chain->>Chain: Remove ProviderReplayStates entry

    Chain-->>Provider: Event: ProviderDeregistered{<br/>provider, stake_returned}

    Note over Provider: Provider fully removed from chain.<br/>Stake returned to free balance.
```

## Cancellation Flow

A provider can cancel the deregistration at any time before completing it.

```mermaid
sequenceDiagram
    autonumber
    participant Provider as Provider (Wallet)
    participant Chain as Parachain

    Provider->>Chain: cancel_deregister()

    Chain->>Chain: Ensure deregister_at is set
    Chain->>Chain: Clear deregister_at = None
    Chain->>Chain: Restore accepting_primary = true
    Chain->>Chain: Restore accepting_extensions = true

    Chain-->>Provider: Event: DeregisterCancelled{provider}

    Note over Provider: Provider back to normal operation
```

## Why Two Steps?

The cooldown period exists for security:

1. **Challenge resolution**: Any client who has challenged the provider needs time for the challenge to be resolved. If providers could exit instantly, they could dodge slashing by deregistering before the challenge deadline.
2. **DeregisterAnnouncementPeriod >= ChallengeTimeout**: This guarantee ensures that any challenge created before the announcement will have its deadline expire before the provider can withdraw stake.
3. **Provider remains slashable**: During the cooldown, the provider's stake is still reserved and can be slashed by `on_finalize` if a challenge expires without response.

## Preconditions

| Check | Error |
|-------|-------|
| Provider must be registered | `ProviderNotFound` |
| `committed_bytes == 0` (no active agreements) | `ProviderHasActiveAgreements` |
| `deregister_at` not already set (for announce) | `DeregisterAnnounced` |
| `deregister_at` is set (for complete/cancel) | `DeregisterNotAnnounced` |
| `current_block >= deregister_at` (for complete) | `DeregisterPeriodNotElapsed` |

## Events

| Event | When |
|-------|------|
| `DeregisterAnnounced { provider, complete_after }` | Cooldown begins |
| `DeregisterCancelled { provider }` | Provider cancels exit |
| `ProviderDeregistered { provider, stake_returned }` | Stake returned, provider removed |
