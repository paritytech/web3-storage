/** Thin typed wrappers around the S3Registry pallet (Layer 1 — S3-style objects). */

import { asHex, type ParachainApi } from "../address.js";
import type { ChainSigner } from "../signers.js";
import { requireOneEvent, submitTx } from "../tx.js";

const utf8 = (s: string) => new TextEncoder().encode(s);

export async function createS3BucketWithStorage(
  api: ParachainApi,
  client: ChainSigner,
  name: string,
  params: any,
) {
  const result = await submitTx(
    api.tx.S3Registry.create_s3_bucket_with_storage({
      name: utf8(name),
      ...params,
    }),
    client.signer,
    "create_s3_bucket_with_storage",
  );
  const event = requireOneEvent(
    result.events,
    api.event.S3Registry.S3BucketCreated,
    "S3BucketCreated",
  );
  const requested = api.event.StorageProvider.AgreementRequested.filter(
    result.events as never,
  );
  return {
    s3BucketId: event.s3_bucket_id,
    layer0BucketId: event.layer0_bucket_id,
    matchedProvider: requested[0]?.payload.provider,
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
    `put_object_metadata(${key})`,
  );
}

export async function copyObjectMetadata(
  api: ParachainApi,
  client: ChainSigner,
  srcBucketId: bigint,
  srcKey: string,
  dstBucketId: bigint,
  dstKey: string,
) {
  return submitTx(
    api.tx.S3Registry.copy_object_metadata({
      src_bucket_id: srcBucketId,
      src_key: utf8(srcKey),
      dst_bucket_id: dstBucketId,
      dst_key: utf8(dstKey),
    }),
    client.signer,
    `copy_object_metadata(${srcKey} -> ${dstKey})`,
  );
}

export async function deleteObjectMetadata(
  api: ParachainApi,
  client: ChainSigner,
  s3BucketId: bigint,
  key: string,
) {
  return submitTx(
    api.tx.S3Registry.delete_object_metadata({
      s3_bucket_id: s3BucketId,
      key: utf8(key),
    }),
    client.signer,
    `delete_object_metadata(${key})`,
  );
}

export async function deleteS3Bucket(
  api: ParachainApi,
  client: ChainSigner,
  s3BucketId: bigint,
) {
  const result = await submitTx(
    api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: s3BucketId }),
    client.signer,
    "delete_s3_bucket",
  );
  return requireOneEvent(
    result.events,
    api.event.S3Registry.S3BucketDeleted,
    "S3BucketDeleted",
  );
}
