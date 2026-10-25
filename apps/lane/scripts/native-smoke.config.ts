// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@lynx-js/rspeedy";
import { pluginReactLynx } from "@lynx-js/react-rsbuild-plugin";
import { isAbsolute } from "node:path";
const output = process.env.LANE_NATIVE_SMOKE_OUTPUT;
if (!output || !isAbsolute(output))
  throw new Error(
    "Set LANE_NATIVE_SMOKE_OUTPUT to an absolute scratch directory",
  );
export default defineConfig({
  source: { entry: { main: "./scripts/native-smoke.tsx" } },
  output: { distPath: { root: output } },
  plugins: [pluginReactLynx()],
});
