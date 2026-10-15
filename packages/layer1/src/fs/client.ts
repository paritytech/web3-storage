// SPDX-License-Identifier: Apache-2.0

/**
 * FileSystemClient — drives (plain Layer 0 buckets) + the provider node's
 * /fs HTTP surface. The chain stores no drive name or drive record; a drive
 * is identified by its bucket id. Chain ops delegate to the layer-0 pallet wrappers (silent, no
 * auto-retry, finalized submission + finalized reads by default — UI-grade,
 * reorg-safe; tests/examples opt into in-block/best via readOpts/submitMode);
 * HTTP ops go through core's retrying fetch and are signed with the signer's
 * raw keypair, which the provider always requires.
 *
 * Verification: `downloadByCid` is verified (single chunk — its hash IS the
 * CID; the layer-0 downloadChunk throws CidMismatchError). Path-based
 * `downloadFile` is NOT verified: the provider's /fs file route returns no
 * data_root, and multi-chunk verification needs a Merkle DAG walk
 * (Rust-client parity) — tracked separately.
 */

import { httpFetch } from "@web3-storage/core";
import {
  createBucketWithPrimary as createBucketWithPrimaryTx,
  downloadChunk,
  removeMember as removeMemberTx,
  setMember as setMemberTx,
  type WaitOpts,
} from "@web3-storage/layer0";

import { Layer1Client, type Layer1ClientOptions } from "../base-client.js";
import { getBucketInfos, listMemberBuckets } from "../bucket-info.js";
import { resolveCreationTerms } from "../provider-url.js";
import type {
  BucketMember,
  CreateDriveOptions,
  DriveInfo,
  FileWithType,
  FsEntry,
  IndexRoot,
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

  // ── FS HTTP ops ─────────────────────────────────────────────────────────

  async listDirectory(
    bucketId: bigint,
    path: string,
    opts: { recursive?: boolean; signal?: AbortSignal } = {},
  ): Promise<FsEntry[]> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const params = new URLSearchParams({ path });
    if (opts.recursive) params.set("recursive", "true");
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/ls?${params.toString()}`,
      { signal: opts.signal, headers: await this.authHeaders("GET", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) throw new Error(`List directory failed: ${response.status}`);
    const result = await response.json();
    type WireEntry = { name: string; path: string; entry_type: string; size?: number; mtime?: number };
    return ((result.entries ?? []) as WireEntry[]).map((e) => ({
      name: e.name,
      path: e.path,
      entryType: e.entry_type as "file" | "directory",
      size: e.size ?? 0,
      mtime: (e.mtime ?? 0) * 1000,
    }));
  }

  async uploadFile(
    bucketId: bigint,
    path: string,
    data: Uint8Array,
    options: UploadOptions = {},
  ): Promise<UploadResult> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`,
      {
        method: "PUT",
        headers: {
          "Content-Type": options.contentType || "application/octet-stream",
          ...(await this.authHeaders("PUT", bucketId)),
        },
        body: data as BodyInit,
        signal: options.signal,
      },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Upload failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
    const body = await response.json().catch(() => ({}));
    return { dataRoot: body.data_root, size: data.length };
  }

  /** GET the /fs file route, shared by the path-based download methods. */
  private async fetchFileResponse(
    bucketId: bigint,
    path: string,
    opts: { signal?: AbortSignal },
  ): Promise<Response> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`,
      { signal: opts.signal, headers: await this.authHeaders("GET", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) throw new Error(`Download failed: ${response.status}`);
    return response;
  }

  /**
   * Download a file by path. UNVERIFIED — the /fs file route returns no
   * data_root to check against; see the module docs.
   */
  async downloadFile(
    bucketId: bigint,
    path: string,
    opts: { signal?: AbortSignal } = {},
  ): Promise<Uint8Array> {
    const response = await this.fetchFileResponse(bucketId, path, opts);
    return new Uint8Array(await response.arrayBuffer());
  }

  /**
   * Download a file by path along with its stored MIME type (from the
   * provider's `Content-Type` header) — e.g. to re-`uploadFile` it unchanged
   * when moving/renaming a path. UNVERIFIED, like {@link downloadFile}.
   */
  async downloadFileWithType(
    bucketId: bigint,
    path: string,
    opts: { signal?: AbortSignal } = {},
  ): Promise<FileWithType> {
    const response = await this.fetchFileResponse(bucketId, path, opts);
    const contentType = response.headers.get("content-type") || "application/octet-stream";
    return { bytes: new Uint8Array(await response.arrayBuffer()), contentType };
  }

  /** Download a single chunk by CID — VERIFIED (throws CidMismatchError). */
  async downloadByCid(providerUrl: string, cid: string): Promise<Uint8Array> {
    return downloadChunk(providerUrl, cid);
  }

  async deleteFile(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/file?path=${encodeURIComponent(path)}`,
      { method: "DELETE", headers: await this.authHeaders("DELETE", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Delete failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  async createDirectory(bucketId: bigint, path: string): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/mkdir?path=${encodeURIComponent(path)}`,
      { method: "POST", headers: await this.authHeaders("POST", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Create directory failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }

  /** The provider's own view of the drive's metadata Merkle root + counts. */
  async getIndexRoot(bucketId: bigint): Promise<IndexRoot> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/fs/${bucketId}/index_root`,
      { headers: await this.authHeaders("GET", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) throw new Error(`index_root failed: ${response.status}`);
    const body = await response.json();
    if (typeof body.metadata_merkle_root !== "string") {
      throw new Error("index_root response is missing metadata_merkle_root");
    }
    return {
      indexRoot: body.metadata_merkle_root,
      fileCount: Number(body.file_count ?? 0),
      dirCount: Number(body.dir_count ?? 0),
      totalSize: BigInt(body.total_size ?? 0),
    };
  }
}
