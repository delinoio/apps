// SPDX-License-Identifier: Apache-2.0
import { pluginReact } from "@rsbuild/plugin-react";
import { defineConfig } from "@rstest/core";

export default defineConfig({
  plugins: [pluginReact()],
  testEnvironment: "jsdom",
  include: ["tests/**/*.test.{ts,tsx}"],
  setupFiles: ["./tests/setup.ts"],
});
