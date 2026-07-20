// SPDX-License-Identifier: Apache-2.0

/** Thin typed wrappers around the S3Registry pallet (Layer 1 — S3-style objects). */

import type { SignedTerms } from "@web3-storage/core";

import { asHex, type ParachainApi } from "../address.js";
import type { ChainSigner } from "../signers.js";
import { requireOneEvent, submitTx, type SubmitOpts } from "../tx.js";
import { buildSignedTermsArgs } from "./storage-provider.js";

const utf8 = (s: string) => new TextEncoder().encode(s);

/**
 * Create an S3 bucket by redeeming provider-signed terms (#105). Layer 0's
 * establish_storage_agreement_internal opens the underlying Layer 0 bucket +
 * primary agreement atomically inside create_s3_bucket, so `provider`/`signed`
 * come from a prior {@link negotiateTerms} against that provider.
 */
export async function createS3Bucket(
  api: ParachainApi,
  client: ChainSigner,
  name: string,
  provider: ChainSigner | { address: string },
  signed: SignedTerms,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.S3Registry.create_s3_bucket({
      name: utf8(name),
      ...buildSignedTermsArgs(provider, signed),
    }),
    client.signer,
    { label: "create_s3_bucket", ...opts },
  );
  const event = requireOneEvent(
    result.events,
    api.event.S3Registry.S3BucketCreated,
    "S3BucketCreated",
  );
  return {
    s3BucketId: event.s3_bucket_id,
    layer0BucketId: event.layer0_bucket_id,
    provider: provider.address,
  };
}

export async function putObjectMetadata(
  api: ParachainApi,
  client: ChainSigner,
  s3BucketId: bigint,
  key: string,
  obj: { cid: Uint8Array; size: bigint },
  contentType: string,
  userMetadata: Array<[string, string]> = [],
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.S3Registry.put_object_metadata({
      s3_bucket_id: s3BucketId,
      key: utf8(key),
      cid: asHex(obj.cid),
      size: obj.size,
      content_type: utf8(contentType),
      user_metadata: userMetadata.map(([k, v]) => [utf8(k), utf8(v)] as [Uint8Array, Uint8Array]),
    }),
    client.signer,
    { label: `put_object_metadata(${key})`, ...opts },
  );
}

export async function copyObjectMetadata(
  api: ParachainApi,
  client: ChainSigner,
  srcBucketId: bigint,
  srcKey: string,
  dstBucketId: bigint,
  dstKey: string,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.S3Registry.copy_object_metadata({
      src_bucket_id: srcBucketId,
      src_key: utf8(srcKey),
      dst_bucket_id: dstBucketId,
      dst_key: utf8(dstKey),
    }),
    client.signer,
    { label: `copy_object_metadata(${srcKey} -> ${dstKey})`, ...opts },
  );
}

export async function deleteObjectMetadata(
  api: ParachainApi,
  client: ChainSigner,
  s3BucketId: bigint,
  key: string,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.S3Registry.delete_object_metadata({
      s3_bucket_id: s3BucketId,
      key: utf8(key),
    }),
    client.signer,
    { label: `delete_object_metadata(${key})`, ...opts },
  );
}

export async function deleteS3Bucket(
  api: ParachainApi,
  client: ChainSigner,
  s3BucketId: bigint,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: s3BucketId }),
    client.signer,
    { label: "delete_s3_bucket", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.S3Registry.S3BucketDeleted,
    "S3BucketDeleted",
  );
}
