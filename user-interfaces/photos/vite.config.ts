// SPDX-License-Identifier: GPL-3.0-only

import { defineConfig, searchForWorkspaceRoot } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'path'

const base = process.env.GITHUB_PAGES ? '/web3-storage/photos/' : '/'

export default defineConfig({
  base,
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 5178,
    fs: {
      allow: [
        searchForWorkspaceRoot(process.cwd()),
        '../../../trinity-user-agents/js/packages/truapi'
      ],
    },
  },
})
