// SPDX-License-Identifier: Apache-2.0

/**
 * Layer 0 bucket reads shared by the layer-1 clients. A drive and an S3
 * bucket are both a plain Layer 0 bucket: the chain stores no name and no
 * type for it, so a bucket is identified by its id only.
 */

import type { ParachainApi, Visibility } from "@web3-storage/layer0";

import { resolveBucketProviders, type PrimaryProviderInfo } from "./provider-url.js";

export type MemberRole = "Admin" | "Writer" | "Reader";

export interface BucketMember {
  account: string;
  role: MemberRole;
}

export interface BucketInfo {
  bucketId: bigint;
  members: BucketMember[];
  visibility: Visibility;
  /** True once an admin froze the bucket (no further deletes). */
  frozen: boolean;
  /** Primary providers of the bucket. */
  providerInfo: PrimaryProviderInfo[];
  /** Largest `max_bytes` across the primary agreements; 0n with no agreement. */
  maxCapacity: bigint;
  /** Latest `expires_at` (anchor block) across the primary agreements; null with no agreement. */
  expiresAt: number | null;
}

function toRole(role: { type?: string } | string | undefined): MemberRole {
  const type = typeof role === "string" ? role : role?.type;
  return type === "Admin" || type === "Writer" ? type : "Reader";
}

/**
 * Read the given buckets in one batched query per storage map. Returns an
 * array aligned with `bucketIds`; an entry is null when the bucket does not
 * exist.
 */
export async function getBucketInfos(
  api: ParachainApi,
  bucketIds: bigint[],
  readOpts: { at: "best" | "finalized" } = { at: "finalized" },
): Promise<(BucketInfo | null)[]> {
  if (bucketIds.length === 0) return [];
  const buckets = await api.query.StorageProvider.Buckets.getValues(
    bucketIds.map((id) => [id] as const),
    readOpts,
  );

  const agreementKeys = buckets.flatMap((bucket, i) =>
    (bucket?.primary_providers ?? []).map(
      (provider): [bigint, typeof provider] => [bucketIds[i]!, provider],
    ),
  );
  const [agreements, providersByBucket] = await Promise.all([
    api.query.StorageProvider.StorageAgreements.getValues(agreementKeys, readOpts),
    resolveBucketProviders(api, buckets, readOpts),
  ]);
  const terms = new Map<bigint, { maxCapacity: bigint; expiresAt: number | null }>();
  agreementKeys.forEach(([bucketId], i) => {
    const agreement = agreements[i];
    if (!agreement) return;
    const current = terms.get(bucketId) ?? { maxCapacity: 0n, expiresAt: null };
    terms.set(bucketId, {
      maxCapacity:
        agreement.max_bytes > current.maxCapacity ? agreement.max_bytes : current.maxCapacity,
      expiresAt: Math.max(current.expiresAt ?? 0, agreement.expires_at),
    });
  });

  return buckets.map((bucket, i) => {
    if (!bucket) return null;
    const bucketId = bucketIds[i]!;
    const bucketTerms = terms.get(bucketId);
    return {
      bucketId,
      members: bucket.members.map((m) => ({ account: m.account, role: toRole(m.role) })),
      visibility: bucket.visibility.type as Visibility,
      frozen: bucket.frozen_start_seq !== undefined,
      providerInfo: providersByBucket[i] ?? [],
      maxCapacity: bucketTerms?.maxCapacity ?? 0n,
      expiresAt: bucketTerms?.expiresAt ?? null,
    };
  });
}

/** Every bucket `account` is a member of (any role), from `MemberBuckets`. */
export async function listMemberBuckets(
  api: ParachainApi,
  account: string,
  readOpts: { at: "best" | "finalized" } = { at: "finalized" },
): Promise<BucketInfo[]> {
  const ids = await api.query.StorageProvider.MemberBuckets.getValue(account, readOpts);
  if (!ids || ids.length === 0) return [];
  const infos = await getBucketInfos(api, ids, readOpts);
  return infos.filter((info): info is BucketInfo => info !== null);
}
