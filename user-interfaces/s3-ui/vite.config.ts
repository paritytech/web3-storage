// SPDX-License-Identifier: Apache-2.0

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { resolve } from "path";

const ghBase = process.env.GITHUB_PAGES ? "/web3-storage/s3/" : "/";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: ghBase,
  resolve: {
    alias: {
      "@": resolve(__dirname, "./src"),
    },
  },
  server: {
    port: 5177,
  },
});
