// SPDX-License-Identifier: Apache-2.0
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { defineNode, z } from "@midscene/test";
import {
  defineFileSetup,
  defineProjectSetup,
  defineTestProject,
} from "@midscene/test/config";
import { createMidsceneNodes } from "@midscene/test/midscene";
import { PlaywrightAgent } from "@midscene/web/playwright/agent";
import {
  chromium,
  type Browser,
  type BrowserContext,
  type Page,
} from "playwright";

const appRoot = fileURLToPath(new URL("../../", import.meta.url));
const port = 46440;
const baseURL = `http://127.0.0.1:${port}`;
interface ProjectContext {
  browser: Browser;
}
interface Context extends ProjectContext {
  session?: BrowserContext;
  page?: Page;
  agent?: PlaywrightAgent;
}
const setup = defineProjectSetup<ProjectContext>({
  name: "browser",
  async setup({ onTeardown }) {
    const browser = await chromium.launch({
      channel: process.env.LANE_BROWSER_CHANNEL || "chrome",
      headless: process.env.HEADLESS !== "false",
    });
    onTeardown(() => browser.close());
    return { browser };
  },
});
const fileSetup = defineFileSetup<Context, ProjectContext>({
  name: "isolated-preview",
  async setup({ projectContext, onTeardown }) {
    if (!projectContext) throw new Error("Browser setup did not complete");
    const context: Context = { browser: projectContext.browser };
    // Preview state lives in the server process, so a new page alone cannot
    // isolate saved tasks. Each file gets a fresh process and runs serially.
    const server = spawn(process.execPath, ["tests/preview.mjs"], {
      cwd: appRoot,
      env: {
        ...process.env,
        LANE_PREVIEW_PORT: String(port),
        LANE_PREVIEW_SCENARIO: "",
        LANE_PREVIEW_UPDATES: "",
        LANE_PREVIEW_ROOT: `${appRoot}dist`,
      },
      stdio: ["ignore", "pipe", "inherit"],
    });
    const stopped = new Promise<void>((resolve) =>
      server.once("close", () => resolve()),
    );
    onTeardown(async () => {
      try {
        await context.agent?.destroy();
      } finally {
        try {
          await context.session?.close();
        } finally {
          if (server.exitCode === null && server.signalCode === null) {
            server.kill("SIGTERM");
            const timer = setTimeout(() => server.kill("SIGKILL"), 5000);
            try {
              await stopped;
            } finally {
              clearTimeout(timer);
            }
          }
        }
      }
    });
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(
        () => finish(new Error("Lane preview startup timed out")),
        15000,
      );
      let output = "";
      const onData = (chunk: Buffer) => {
        output += chunk.toString();
        if (output.includes(`Lane test-only preview ${baseURL}`)) finish();
      };
      const onExit = (code: number | null) =>
        finish(new Error(`Lane preview exited: ${code}`));
      const finish = (error?: Error) => {
        clearTimeout(timer);
        server.stdout.off("data", onData);
        server.off("exit", onExit);
        server.off("error", finish);
        error ? reject(error) : resolve();
      };
      server.stdout.on("data", onData);
      server.once("exit", onExit);
      server.once("error", finish);
    });
    return context;
  },
});
const openInput = z.strictObject({
  viewport: z.enum(["desktop", "mobile"]).default("desktop"),
});
const open = defineNode<typeof openInput, void, Context>({
  name: "app.open",
  description:
    "Open the local fixture in a fresh browser context, preserving this file's server state.",
  inputSchema: openInput,
  async execute({ context, input }) {
    await context.agent?.destroy();
    context.agent = undefined;
    await context.session?.close();
    context.session = await context.browser.newContext({
      viewport:
        input.viewport === "mobile"
          ? { width: 390, height: 844 }
          : { width: 1440, height: 900 },
      locale: "en-US",
    });
    // Only the fixture may receive browser requests; model calls run in Node.
    await context.session.route("**/*", (route) =>
      new URL(route.request().url()).origin === baseURL
        ? route.continue()
        : route.abort("blockedbyclient"),
    );
    context.page = await context.session.newPage();
    await context.page.goto(baseURL);
    await context.page
      .getByRole("textbox", {
        name: "Search title or description",
        exact: true,
      })
      .waitFor();
  },
});
export default defineTestProject<Context, ProjectContext>({
  test: {
    maxConcurrentProjects: 1,
    maxConcurrentFiles: 1,
    testTimeout: 10 * 60_000,
  },
  projects: [
    {
      name: "web",
      setup,
      fileSetup,
      maxConcurrentFiles: 1,
      retry: 0,
      files: { include: ["cases/*.yaml"] },
    },
  ],
  nodes: [
    open,
    ...createMidsceneNodes<Context>({
      agentClass: PlaywrightAgent,
      getAgent({ context }) {
        if (!context.page) throw new Error("Call app.open first");
        // The current Lynx web controlled input can lose characters during
        // rapid keyboard.type calls. Bulk input matches paste/fill semantics
        // for these workflows; it does not validate per-keystroke stability.
        // Remove this workaround when the host retains rapid typed input.
        return (context.agent ??= new PlaywrightAgent(context.page, {
          inputStrategy: "bulk",
        }));
      },
    }),
  ],
});
