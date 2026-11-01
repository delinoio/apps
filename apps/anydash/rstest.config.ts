// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@rstest/core";
import { pluginReactLynx } from "@lynx-js/react-rsbuild-plugin";
import { withDefaultConfig } from "@lynx-js/react/testing-library/rstest-config";
import { createRequire } from "node:module";
const lynxRequire = createRequire(import.meta.resolve("@lynx-js/react"));

export default defineConfig({
  // withLynxConfig() imports @lynx-js/rspeedy from ReactLynx's own package
  // directory, which pnpm's local global virtual store does not link. Apply
  // the ReactLynx plugin directly instead; remove this when the adapter
  // resolves the build tool from the project.
  extends: withDefaultConfig(),
  plugins: [pluginReactLynx()],
  include: ["tests/**/*.test.ts", "tests/**/*.test.tsx"],
  setupFiles: ["./tests/setup.ts"],
  source: { define: { __ANYDASH_VERSION__: JSON.stringify("0.1.0-test") } },
  resolve: {
    // Reuse ReactLynx's exact preact fork to avoid a second hook dispatcher,
    // and resolve React Query's "react" imports to ReactLynx as the Rspeedy
    // build does. Remove when the upstream Rstest adapter applies these aliases.
    alias: {
      "preact/hooks": lynxRequire.resolve("preact/hooks"),
      preact: lynxRequire.resolve("preact"),
      react$: lynxRequire.resolve("@lynx-js/react"),
      "react/jsx-runtime": lynxRequire.resolve("@lynx-js/react/jsx-runtime"),
      "react/jsx-dev-runtime": lynxRequire.resolve(
        "@lynx-js/react/jsx-dev-runtime",
      ),
    },
  },
});
