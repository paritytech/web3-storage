// SPDX-License-Identifier: GPL-3.0-only
/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Genesis hash of the parachain, for reaching it through a TrUAPI host. */
  readonly VITE_PHOTOS_PARACHAIN_GENESIS?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
