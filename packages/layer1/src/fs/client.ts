// SPDX-License-Identifier: Apache-2.0

/**
 * FileSystemClient: drives on plain Layer 0 buckets. The chain stores no
 * drive record; a drive is identified by its bucket id. The directory tree
 * is data in the bucket itself (format and limits in ../tree.ts), written
 * and read with the provider's Layer 0 routes only.
 *
 * Chain ops delegate to the layer-0 pallet wrappers (silent, no auto-retry,
 * finalized submission and reads by default; tests and examples opt into
 * best-block via readOpts/submitMode). Writes are signed with the signer,
 * which needs a Writer or Admin role. Reads are unauthenticated: anyone who
 * knows a CID can read the blob, so confidential files need client-side
 * encryption. Every read is checked against the CIDs in the tree.
 *
 * One writer at a time: two clients that write the same drive concurrently
 * can lose one of the changes (the last commit wins).
 */

import {
  createBucketWithPrimary as createBucketWithPrimaryTx,
  readBlob,
  removeMember as removeMemberTx,
  setMember as setMemberTx,
  type WaitOpts,
} from "@web3-storage/layer0";

import { Layer1Client, type Layer1ClientOptions } from "../base-client.js";
import { getBucketInfos, listMemberBuckets } from "../bucket-info.js";
import { withHttpContext } from "../http-context.js";
import { resolveCreationTerms } from "../provider-url.js";
import type {
  BucketMember,
  CreateDriveOptions,
  DriveInfo,
  FileWithType,
  FsEntry,
  MemberRole,
  UploadOptions,
  UploadResult,
} from "./types.js";

export type FileSystemClientOptions = Layer1ClientOptions;

export class FileSystemClient extends Layer1Client {
  // ── Drive chain ops ─────────────────────────────────────────────────────

  /**
   * Create a drive: a Layer 0 bucket with one primary agreement. Picks a
   * provider (explicit `options.provider` or auto-discovered), POSTs
   * /negotiate for signed terms (unless `options.signedTerms` is supplied),
   * then redeems them in `create_bucket_with_primary`.
   */
  async createDrive(options: CreateDriveOptions): Promise<{
    bucketId: bigint;
    provider: string;
  }> {
    const signer = this.requireSigner();
    const { provider, signedTerms } = await resolveCreationTerms(this.api, {
      owner: signer.address,
      maxBytes: options.maxCapacity,
      duration: options.storagePeriod,
      provider: options.provider,
      signedTerms: options.signedTerms,
      urlOverride: this.creationUrlOverride,
      readOpts: this.readOpts,
      fetchOpts: this.fetchOpts,
    });
    const { bucketId } = await createBucketWithPrimaryTx(this.api, signer, provider, signedTerms, {
      ...this.submitOpts(),
      visibility: options.visibility,
    });
    return { bucketId, provider: provider.address };
  }

  async getDrive(bucketId: bigint): Promise<DriveInfo | null> {
    const [info] = await getBucketInfos(this.api, [bucketId], this.readOpts);
    return info ?? null;
  }

  /**
   * Every bucket `account` (default: the signer) is a member of, owned or
   * shared. The chain does not record which buckets hold a file system, so
   * this lists all of them.
   */
  async listDrives(account?: string): Promise<DriveInfo[]> {
    return listMemberBuckets(this.api, account ?? this.requireSigner().address, this.readOpts);
  }

  async addMember(bucketId: bigint, account: string, role: MemberRole): Promise<void> {
    await setMemberTx(
      this.api,
      this.requireSigner(),
      bucketId,
      { address: account },
      role,
      this.submitOpts(),
    );
  }

  async removeMember(bucketId: bigint, account: string): Promise<void> {
    await removeMemberTx(
      this.api,
      this.requireSigner(),
      bucketId,
      { address: account },
      this.submitOpts(),
    );
  }

  async getBucketMembers(bucketId: bigint): Promise<BucketMember[]> {
    const info = await this.getDrive(bucketId);
    if (!info) throw new Error(`Bucket ${bucketId} not found`);
    return info.members;
  }

  // ── Provider resolution ─────────────────────────────────────────────────

  getProviderUrl(bucketId: bigint): Promise<string> {
    return this.providers.get(bucketId);
  }

  invalidateProviderUrl(bucketId?: bigint): void {
    this.providers.invalidate(bucketId);
  }

  waitForProvider(bucketId: bigint, opts?: WaitOpts): Promise<string> {
    return this.providers.waitForProvider(bucketId, opts);
  }

  // ── File-system ops (the tree in the bucket; see ../tree.ts) ───────────

  /**
   * List a directory. With `recursive`, every descendant is included,
   * depth-first. `mtime` is in milliseconds.
   */
  async listDirectory(
    bucketId: bigint,
    path: string,
    opts: { recursive?: boolean; signal?: AbortSignal } = {},
  ): Promise<FsEntry[]> {
    const tree = await this.openTree(bucketId, { signal: opts.signal });
    const entries = await withHttpContext("List directory failed", () => tree.list(path, { recursive: opts.recursive }));
    return entries.map((e) => ({
      name: e.name,
      path: e.path,
      entryType: e.entryType,
      size: Number(e.size),
      mtime: Number(e.mtime) * 1000,
      cid: e.cid,
    }));
  }

  /**
   * Store a file at `path`, replacing an existing file and creating missing
   * parent directories. Fails when `path` is a directory. Requires a Writer
   * or Admin role on the bucket.
   */
  async uploadFile(
    bucketId: bigint,
    path: string,
    data: Uint8Array,
    options: UploadOptions = {},
  ): Promise<UploadResult> {
    const tree = await this.openTree(bucketId, { write: true, signal: options.signal });
    const r = await withHttpContext("Upload failed", () =>
      tree.putFile(path, data, { contentType: options.contentType }),
    );
    return { dataRoot: r.contentRoot, manifestCid: r.manifestCid, rootCid: r.rootCid, size: r.size };
  }

  /** Download a file by path. The content is checked against the drive's tree. */
  async downloadFile(
    bucketId: bigint,
    path: string,
    opts: { signal?: AbortSignal } = {},
  ): Promise<Uint8Array> {
    return (await this.downloadFileWithType(bucketId, path, opts)).bytes;
  }

  /**
   * Download a file by path with its stored content type, e.g. to upload it
   * again unchanged when moving a path. Checked like {@link downloadFile}.
   */
  async downloadFileWithType(
    bucketId: bigint,
    path: string,
    opts: { signal?: AbortSignal } = {},
  ): Promise<FileWithType> {
    const tree = await this.openTree(bucketId, { signal: opts.signal });
    const file = await withHttpContext("Download failed", () => tree.getFile(path));
    return { bytes: file.bytes, contentType: file.contentType };
  }

  /**
   * Download a blob by CID from `providerUrl`. With `size`, every chunk is
   * checked against the CID. Without it, a 64-byte result is not verified
   * (see `ReadBlobOpts.size` in @web3-storage/layer0).
   */
  async downloadByCid(providerUrl: string, cid: string, size?: bigint | number): Promise<Uint8Array> {
    return readBlob(providerUrl, cid, { fetch: this.fetchOpts.fetchImpl, size });
  }

  /** Remove a file or an empty directory. Requires a Writer or Admin role. */
  async deleteFile(bucketId: bigint, path: string): Promise<void> {
    const tree = await this.openTree(bucketId, { write: true });
    await withHttpContext("Delete failed", () => tree.delete(path));
  }

  /**
   * Create a directory and any missing parents. Fails when `path` exists.
   * Requires a Writer or Admin role.
   */
  async createDirectory(bucketId: bigint, path: string): Promise<void> {
    const tree = await this.openTree(bucketId, { write: true });
    await withHttpContext("Create directory failed", () => tree.mkdir(path));
  }

  /**
   * CID of the drive's root directory (the bucket's last MMR leaf), 0x-hex,
   * or `null` for a drive with no commits.
   */
  async getRootCid(bucketId: bigint): Promise<string | null> {
    const tree = await this.openTree(bucketId);
    return withHttpContext("Root lookup failed", () => tree.rootCid());
  }
}
