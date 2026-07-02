// SPDX-License-Identifier: Apache-2.0

import { defineConfig } from "@playwright/test";
export default defineConfig({ testDir: ".", testMatch: /verify\.spec\.ts/, workers: 1, retries: 0, reporter: [["list"]] });
