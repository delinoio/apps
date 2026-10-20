import { pluginReact } from "@rsbuild/plugin-react";
import { defineConfig } from "@rstest/core";
export default defineConfig({
  plugins: [pluginReact()],
  include: ["src/**/*.test.{ts,tsx}"],
  testEnvironment: "jsdom",
  setupFiles: ["./src/test-setup.ts"],
});
