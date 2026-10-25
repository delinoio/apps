// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@rstest/core";
import { withLynxConfig } from "@lynx-js/react/testing-library/rstest-config";
import { createRequire } from "node:module";
const lynxRequire = createRequire(import.meta.resolve("@lynx-js/react"));

export default defineConfig({
  extends: withLynxConfig(),
  include: ["tests/**/*.test.tsx"],
  resolve: {
    // Rstest otherwise resolves the fixture's preact/hooks from this workspace.
    // Reuse ReactLynx's exact fork to avoid a second hook dispatcher. Remove when
    // the upstream Rstest adapter resolves these testing-library imports itself.
    alias: {
      "preact/hooks": lynxRequire.resolve("preact/hooks"),
      preact: lynxRequire.resolve("preact"),
    },
  },
});
