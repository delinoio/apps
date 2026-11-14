// SPDX-License-Identifier: Apache-2.0
import type { E2EConfig } from "e2e";
import { web } from "@e2e-dev/web";
import { gateway } from "ai";

export default {
  projectId: "delino-accounts",
  tests: "e2e/**/*.e2e.ts",
  targets: [
    {
      engine: web(),
      app: {
        // Port 0 selects a free loopback port, so the suite does not conflict
        // with the fixed development port used by `pnpm dev`.
        url: "http://127.0.0.1:0",
        command: {
          executable: "rsbuild",
          args: ["dev", "--host", "127.0.0.1", "--port", "{port}"],
          // The runner passes only PATH, HOME and temporary-directory variables
          // to the app command. Pass a nondefault API origin explicitly so
          // `e2e/home.e2e.ts` can observe that the build-time variable reaches
          // the client bundle. Keep it equal to `E2E_API_ORIGIN` in that file.
          env: { PUBLIC_API_ORIGIN: "http://127.0.0.1:18100" },
          log: ".e2e/logs/app.log",
        },
      },
    },
  ],
  // Model for `agent.*` steps. `gateway()` reads AI_GATEWAY_API_KEY or a
  // Vercel OIDC token. Tests without agent steps do not call the model.
  agents: {
    default: {
      model: gateway("openai/gpt-6-luna-fast"),
    },
  },
} satisfies E2EConfig;
