# Virtual Provider Extension for scalable web3 storage

| Field | Value |
| --- | --- |
| **Authors** | eskimor |
| **Status** | Draft |
| **Version** | 0.1 |
| **Related** | TODO: Fill out|

## Version History

| Version | Changes |
|---------|---------|
| 0.1 | Initial version |
---


## Motivation

We slash providers if they can't provide stored data. This might be a problem though and might limit availability of high-quality (high-stake) providers, because users might upload data that is seen as illegal in some jurisdictions and providers might be forced by a court order to take down some content. Now it does not seem fair to expose a provider to a slashing risk, just because it adhered to the law (enforcement). A provider should not have to chose between losing money and going to prison, because of the actions of a random user.

A provider should be able to adhere to the law and not get punished for it.

The benefits go farther of course. Being part of a virtual provider also reduces risk for other reasons. E.g. [key theft](https://github.com/paritytech/web3-storage/issues/300), with proper hot-cold key separation the only risk remaining is being griefed into a slash. This is prevented if commitments require signatures from more than one provider - a single key theft is no longer enough to cause any damage.


## Solution Overview

We are introducing the notion of a virtual provider. A virtual provider is backed by multiple real/physical providers.

The virtual provider has virtual stake which consists of physical stake provided by the underlying physical providers. They all together vouch for the content. If successfully challenged, their entire stake is gone, but any one of them can defend. Thus if one provider can no longer serve, the others can still provide the content and respond to challenges.

A virtual provider is essentially an ensurance mechanism for providers. Instead of bearing the risk all alone, they share it with others.

Providers choose themselves with what other providers they want to provide a virtual provider with and it is in their self-interest that these other providers are truly independent and in different jurisdictions for the ensurance to be meaningful. This is perfect incentive alignment as a provider is also the only one who can know whether it is independent from another provider or not - without giving up privacy, that is.

Users should pick a high-staked virtual provider for the best decentralization guarantees.

## Role of Multiple Primaries

Multiple primaries as in the original design lose in importance with the introduction of virtual providers and a reasonable expectation would be that most buckets will only be served by a single primary. Users who don't care about decentralization for their bucket, backed by a physical ones, users who do care, pick a virtual one.

Multiple primaries can still be useful, because here the user/client is in control. A virtual provider likely has the better decentralization properties than hand-picked physical ones, but the user might still have a legitmate interest to have multiple primaries. One very basic one is, that a user might want to change the virtual provider. For 0-downtime this implies that it needs to concurrently run two for the transition period. Other reasons, the user might just know providers and have a trust relationship with them, they might even run one themselves they want to use.

TL;DR: Multple primaries still seem useful, even with virtual providers.

## Adjustments to original design
What needs to change in the original design, such that we can support virtual providers seemlessly later (no breaking changes). TODO: This section should likely not exist and instead the necessary changes should be introduced directly in the original design - if they should be done in preparation. We can of course always also migrate storage to new semantics, in that case we should at least double-check though, that we have proper APIs for everything, so we will not break existing infrastructure built on top.


## Detailed Design

```rust
enum ProviderType {
  Physical(PhysicalProvider),
  Virtual(VirtualProvider),
}

struct PhysicalProvider {
	pub multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
	
}

struct VirtualProvider {
	/// Providers being added must have at least that much stake. Needed signature threshold : virtual_provider.stake <= per_provider_stake * num_sigs -> To actually get the necessary backing.
	/// Note: Extrinsics for adding and removing providers should likely also take a new (optional) per_provider_stake and an (optional) new virtual provider total stake value, such that no undesired intermediate states occur (too much or too little voting power per physical provider).
	per_provider_stake: Balance,	
	/// Free-form "link" to some coordination channel, where physical providers can
	/// coordinate to agree on updating settings for example.
	/// TODO: Replace String with some proper (bounded) type.
	/// Ideally we would get some integration into the Polkadot APP chat, where we get an automated group chat for all providers of a virtual provider automatically: With the physical provider accounts becoming the only members.
	coordination_channel: String,
	physical_provider_list: BoundedVec<AccountId, T::MaxPhysicalProviders>,
}

pub struct ProviderInfo<T: Config> {
    /// Multiaddr for connecting to this provider
    pub multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
    // remaining generic provider fields
    }
```

### Payment distribution

Payment is equally distributed between all providers (they also all contribute the same amount of stake). 

A provider can be kicked out of virtual provider by others, if their combined stake is enough to provide the virtual provider's stake. So providers can kick out redundant providers. 

This protects honest servicing providers as they can always kick out a freeloader (e.g. a provider that never gives commitments), they can also avoid the freeloader getting paid this way (if they kick it out before a payment is paid).

Of course the opposite is also true, a dishonest majority can kick out a single honest participant before payment, to avoid paying. This seems acceptable though as that provider was redundant to begin with, so adding the provider does not bring value, other than redundancy. The only obvious reason to kick the provider out is that it did not provide that redundancy, otherwise why having it added in the first place? And how would such a dishonest majority look like, the only point of adding this additional provider would then have been if one of the dishonest majority wanted to freeload at the cost of the honest minority: A weird scenario that seems unlikely to happen as it would require quite some coordination, a simple counter/statistic on the virtual provider how often someone was kicked out should be enough deterrent. 

If this ever becomes a problem, more elaborate rules can be devised. In the end it is just some operational business risk for a provider, which can be mitigated by tracking reputation signals - e.g. that kick out counter.

### Challenges

There is always one provider responsible for answering challenges towards a virtual provider. We rotate the responsible provider on two occasions:

1. Dispute window amount of blocks has passed
2. An actual challenge happened

So we keep track of the number of challenges the virtual provider defended (already present) and define the responsible provider via:

index into physical provider list := ((block_num + total defended challenges count) / dispute_window_len) % physical_provider_list.len()

The idea is that a virtual provider also gives some protection against being targetted by a challenger and to ensure fairness between providers - sharing the cost of responding to challenges.

The responsible provider is supposed to respond to a challenge before the split (for authorized challengers) hits the 50:50 split (we might extend that time if that seems too tough), once the 50:50 split is hit, all other providers might also respond to the challenge (they are blocked before).

We keep track of this on chain. A provider missing its duty, will get a missed_challenge counter incremented. A provider covering gets an cover_miss counter incremented. For now we don't don nothing with these stats on-chain, they just serve for the physical providers (and their client software) to make decisions. E.g. they might want to get rid of a provider missing a challenge. A provider who has a cover_miss counter being higher than that of other providers, might wait longer before covering next time, increasingly so, the larger the gap.

Note that we on purpose rotate the responsible provider through all physical providers, even ones which might have never signed a commitment for the challenged data. This is on purpose, for simplicity and also because all providers are expected to sign, not signing is bad service and a non zero missed_challenge counter is just an on-chain indicator of that fact.

### Data Upload

Client software selects one physical provider at random and uploads data. The provider is then responsible to distribute the data to the other providers and to collect enough signaturs to match the virtual provider's stake. Once collected, the provider responds with all collected signatures. Only enough signatures (reaching the virtual provider's stake) are a valid commitment for a virtual provider.

### Challenges

### Getting out

A provider might want to leave a physical provider. It can always do so as long as per_provider_stake * remaining_providers >= virtual provider stake. Therefore a virtual provider also provides flexibility to physical providers in that sense, you can leave binding agreements as long as others cover for you.

If the above condition does not hold you are required, not only for commitments but also for any decisions, e.g. accepting agreements. You can therefore (in the worst case), force your way out, just as a physical provider would: Stop accepting agreements and one none remains, you can tear down your service. If the other providers want to maintain service, they should be supportive in finding a replacement (providing the necessary signatures).
## Othern non-solutions

Enforce encryption:

1. Not meaningfully possible.
2. Does not solve the problem solved here even: You can also be forced to take down encrypted content.
3. Enforcing encryption on the protocol level could be considered as willful blindness by courts and actually trigger liability.
