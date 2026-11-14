// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";
import { resolveAccountsApiOrigin } from "./src/config.ts";

// Fail the build and the dev server early when the API origin is not valid.
// The Rsbuild CLI loads `.env*` files into `process.env` before this file.
resolveAccountsApiOrigin(process.env.PUBLIC_DELINO_ACCOUNTS_API_ORIGIN);

export default defineConfig({
  plugins: [pluginReact()],
  html: {
    title: "Delino Accounts",
    favicon: "./public/icon.svg",
  },
  server: {
    // Fixed loopback port; fail instead of picking another port on conflict.
    host: "127.0.0.1",
    port: 3100,
    strictPort: true,
  },
});
