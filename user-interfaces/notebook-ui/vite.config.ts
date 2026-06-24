// SPDX-License-Identifier: GPL-3.0-only

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { resolve } from "path";

const ghBase = process.env.GITHUB_PAGES ? "/web3-storage/notebook/" : "/";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: ghBase,
  resolve: {
    alias: {
      "@": resolve(__dirname, "./src"),
    },
  },
  server: {
    host: "127.0.0.1",
    port: 5178,
  },
});
