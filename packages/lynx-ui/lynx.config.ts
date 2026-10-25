// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@lynx-js/rspeedy";
import { pluginReactLynx } from "@lynx-js/react-rsbuild-plugin";

export default defineConfig({
  source: { entry: { gallery: "./examples/gallery.tsx" } },
  plugins: [pluginReactLynx()],
  environments: { lynx: {}, web: {} },
});
