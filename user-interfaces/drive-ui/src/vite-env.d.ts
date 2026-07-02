// SPDX-License-Identifier: Apache-2.0

/// <reference types="vite/client" />

declare module "*.css" {
  const content: string;
  export default content;
}
