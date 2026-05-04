import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

const base = process.env.GITHUB_PAGES ? "/web3-storage/console/" : "/";

export default defineConfig({
  base,
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      "@polkadot-api/descriptors": path.resolve(__dirname, "./.papi/descriptors"),
    },
  },
});
