// SPDX-License-Identifier: GPL-3.0-only

/// <reference types="vite/client" />

declare module "*.css" {
  const content: string;
  export default content;
}
