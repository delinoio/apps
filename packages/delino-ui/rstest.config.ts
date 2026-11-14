// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@rstest/core";

export default defineConfig({
  // Components need a DOM for focus, keyboard and dialog behavior.
  testEnvironment: "happy-dom",
  // Match `"jsx": "react-jsx"` in tsconfig.json. Sources do not import React.
  tools: {
    swc: {
      jsc: {
        transform: {
          react: { runtime: "automatic" },
        },
      },
    },
  },
});
