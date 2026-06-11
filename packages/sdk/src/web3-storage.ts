/**
 * Web3Storage — the #123 facade: one connected handle exposing the typed
 * chain API plus lazy layer-1 clients. Pure delegation over the flat
 * functions; nothing here that can't also be done with them directly.
 */

import type { PolkadotClient } from "polkadot-api";
import {
  connect,
  syncSs58Prefix,
  type ChainSigner,
  type ParachainApi,
  type TxStatusListener,
} from "@web3-storage/layer0";
import { FileSystemClient, S3Client } from "@web3-storage/layer1";

export interface Web3StorageOptions {
  signer?: ChainSigner | null;
  /**
   * Adopt the runtime's SS58 prefix for address rendering (default true).
   * Disable when connecting before the chain is producing blocks.
   */
  syncSs58?: boolean;
  /** Explicit provider URL for the layer-1 clients (dev/tests). */
  providerUrl?: string;
  /** Tx progress listener for the layer-1 clients. Default null (silent). */
  onStatus?: TxStatusListener | null;
}

export class Web3Storage {
  readonly papi: PolkadotClient;
  readonly api: ParachainApi;
  private currentSigner: ChainSigner | null;
  private readonly providerUrl?: string;
  private readonly onStatus: TxStatusListener | null;
  private fsClient?: FileSystemClient;
  private s3Client?: S3Client;

  private constructor(
    papi: PolkadotClient,
    api: ParachainApi,
    opts: Web3StorageOptions,
  ) {
    this.papi = papi;
    this.api = api;
    this.currentSigner = opts.signer ?? null;
    this.providerUrl = opts.providerUrl;
    this.onStatus = opts.onStatus ?? null;
  }

  static async connect(chainWs: string, opts: Web3StorageOptions = {}): Promise<Web3Storage> {
    const { papi, api } = connect(chainWs);
    if (opts.syncSs58 !== false) {
      await syncSs58Prefix(api);
    }
    return new Web3Storage(papi, api, opts);
  }

  get signer(): ChainSigner | null {
    return this.currentSigner;
  }

  setSigner(signer: ChainSigner | null): void {
    this.currentSigner = signer;
    this.fsClient?.setSigner(signer);
    this.s3Client?.setSigner(signer);
  }

  get fs(): FileSystemClient {
    this.fsClient ??= new FileSystemClient({
      api: this.api,
      signer: this.currentSigner,
      providerUrl: this.providerUrl,
      onStatus: this.onStatus,
    });
    return this.fsClient;
  }

  get s3(): S3Client {
    this.s3Client ??= new S3Client({
      api: this.api,
      signer: this.currentSigner,
      providerUrl: this.providerUrl,
      onStatus: this.onStatus,
    });
    return this.s3Client;
  }

  disconnect(): void {
    this.papi.destroy();
  }
}
