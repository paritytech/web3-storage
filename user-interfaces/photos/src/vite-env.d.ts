// SPDX-License-Identifier: GPL-3.0-only
/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Optional dev override for the deployed Photos contract H160 address. */
  readonly VITE_PHOTOS_CONTRACT?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
