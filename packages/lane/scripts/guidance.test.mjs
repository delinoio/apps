// SPDX-License-Identifier: Apache-2.0
// Checks that the common agent skill, guides and README describe the task planning
// capability, its limits and the explicit development data reset. The CLI help text
// is owned by cmds/lane; the CLI cross-check below skips until that source lists the
// planning commands, so this test does not depend on integration order.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (relative) => readFileSync(resolve(packageRoot, relative), "utf8");

const skill = read("SKILL.md");
const readme = read("README.md");
const guides = {
  installation: read("guides/installation.md"),
  mcp: read("guides/mcp.md"),
  troubleshooting: read("guides/troubleshooting.md"),
  userGuide: read("guides/user-guide.md"),
};

const planningTaskFlags = [
  "--priority",
  "--type",
  "--label-id",
  "--clear-labels",
  "--start-date",
  "--due-date",
  "--story-points",
  "--milestone-id",
  "--cycle-id",
  "--parent-id",
  "--depends-on",
  "--clear-depends-on",
  "--related",
  "--clear-related",
  "--duplicate-of",
  "--add-subtask",
  "--remove-subtask",
  "--add-blocks",
  "--remove-blocks",
];
const planningListFlags = [
  "--priority-unassigned",
  "--type-unassigned",
  "--unlabeled",
  "--start-from",
  "--start-to",
  "--start-unassigned",
  "--due-from",
  "--due-to",
  "--due-unassigned",
  "--points-min",
  "--points-max",
  "--points-unassigned",
  "--no-milestone",
  "--no-cycle",
  "--relationship",
  "--no-relationships",
  "--sort",
  "--direction",
];
const planningTools = [
  "lane_get_planning_item",
  "lane_list_planning_items",
  "lane_create_planning_item",
  "lane_save_planning_item",
  "lane_delete_planning_item",
];
const resetCommand = "data reset --confirm-development-reset";

test("skill has valid front matter and mentions planning", () => {
  const match = /^---\nname: lane\ndescription: (.+)\n---\n/.exec(skill);
  assert.ok(match, "SKILL.md needs name and description front matter");
  assert.match(match[1], /planning/i);
});

test("skill describes planning fields, items, relationships, filters and limits", () => {
  for (const term of [
    "priority",
    "type",
    "labels",
    "start date",
    "due date",
    "story points",
    "0 is different from unassigned",
    "milestone",
    "cycle",
    "same repository",
    "duplicate of",
    "never change progress",
    "Moving a task",
    "detaches",
    "AND",
    "OR",
    "unassigned",
    "at most one sort key",
    "sort last in both directions",
    "task ID",
    "CONSTRAINT_VIOLATION",
    "--retry-blocked",
    "online-only",
    "Assignees",
    "bulk edits",
    "saved views",
    "metadata restore",
    "fractional points",
    "notifications",
  ]) {
    assert.ok(
      skill.toLowerCase().includes(term.toLowerCase()),
      `SKILL.md must mention "${term}"`,
    );
  }
  for (const flag of [...planningTaskFlags, "--sort", "--direction"]) {
    assert.ok(skill.includes(flag), `SKILL.md must mention ${flag}`);
  }
  for (const tool of planningTools) {
    assert.ok(skill.includes(tool), `SKILL.md must mention ${tool}`);
  }
});

test("skill forbids agents from running the development data reset", () => {
  assert.ok(skill.includes(resetCommand));
  assert.match(skill, /Agents must never run this command/);
  assert.match(skill, /explicitly asks/);
  assert.match(skill, /UNSUPPORTED_FORMAT/);
  assert.doesNotMatch(
    skill,
    /(?:you|agents?) (?:should|can|may) (?:run|use) [^.]*data reset/i,
    "the skill must not encourage running the reset",
  );
});

test("user guide documents every planning command option and the reset", () => {
  for (const flag of [...planningTaskFlags, ...planningListFlags]) {
    assert.ok(
      guides.userGuide.includes(flag),
      `user-guide.md must mention ${flag}`,
    );
  }
  for (const command of ["label create", "cycle create", "task list"]) {
    assert.ok(guides.userGuide.includes(command), `missing example ${command}`);
  }
  assert.ok(guides.userGuide.includes(resetCommand));
  assert.ok(guides.userGuide.includes("STORAGE_BUSY"));
  assert.ok(guides.userGuide.includes("sync run --retry-blocked"));
  assert.ok(guides.userGuide.includes("CONSTRAINT_VIOLATION"));
  assert.match(guides.userGuide, /online connection/);
});

test("MCP guide lists the typed planning tools and reset boundary", () => {
  for (const tool of planningTools) {
    assert.ok(guides.mcp.includes(tool), `mcp.md must mention ${tool}`);
  }
  assert.match(guides.mcp, /No MCP tool resets development data/);
  assert.ok(guides.mcp.includes("CONSTRAINT_VIOLATION"));
});

test("guidance limits omission-preserving edits to the CLI and warns MCP callers", () => {
  // lane_save_task replaces the whole task snapshot; omitted fields are cleared.
  assert.match(
    skill,
    /`lane task edit`[^.]*omitted option leaves that field unchanged/,
  );
  assert.match(skill, /`lane_save_task` replaces the whole task snapshot/);
  assert.doesNotMatch(skill, /Edits that omit a field leave it unchanged/);
  for (const text of [skill, guides.mcp]) {
    assert.ok(
      text.includes("lane_get_task"),
      "must tell MCP callers to read the task first",
    );
    assert.match(text, /omit/);
    assert.match(text, /clear/);
  }
  assert.match(
    guides.mcp,
    /`lane_save_task` replaces the entire task snapshot/,
  );
  assert.match(
    guides.mcp,
    /Only `lane task edit` in the CLI preserves omitted fields/,
  );
  assert.match(guides.userGuide, /applies to the CLI only/);
  assert.ok(guides.userGuide.includes("lane_save_task"));
});

test("troubleshooting and installation guides explain rejection and reset", () => {
  assert.ok(guides.troubleshooting.includes("--retry-blocked"));
  assert.ok(guides.troubleshooting.includes("CONSTRAINT_VIOLATION"));
  assert.ok(guides.troubleshooting.includes("UNSUPPORTED_FORMAT"));
  assert.match(guides.troubleshooting, /agents must not run it/);
  assert.ok(guides.installation.includes("UNSUPPORTED_FORMAT"));
  assert.ok(readme.includes(resetCommand));
  assert.match(readme, /must not be run by an agent/);
});

test("guidance no longer refers to the removed migration command", () => {
  for (const text of [skill, readme, ...Object.values(guides)]) {
    assert.doesNotMatch(text, /data migrate/);
  }
});

test("guidance does not expose repository-internal paths", () => {
  for (const text of [skill, readme, ...Object.values(guides)]) {
    assert.doesNotMatch(
      text,
      /\b(?:cmds|servers|protos)\/|\bdocs\/|schema\.sql|commands\.json/,
    );
  }
});

test("relative links resolve to files and headings", () => {
  const files = {
    "SKILL.md": skill,
    "README.md": readme,
    "guides/installation.md": guides.installation,
    "guides/mcp.md": guides.mcp,
    "guides/troubleshooting.md": guides.troubleshooting,
    "guides/user-guide.md": guides.userGuide,
  };
  const slug = (heading) =>
    heading
      .toLowerCase()
      .replace(/[`*]/g, "")
      .replace(/[^\p{L}\p{N}\s-]/gu, "")
      .trim()
      .replace(/\s/g, "-");
  const anchors = (text) =>
    new Set(
      [...text.matchAll(/^#{1,6} (.+)$/gm)].map((match) => slug(match[1])),
    );
  for (const [name, text] of Object.entries(files)) {
    for (const [, target] of text.matchAll(/\]\(([^)\s]+)\)/g)) {
      if (/^[a-z][a-z0-9+.-]*:/i.test(target)) continue;
      const [path, anchor] = target.split("#");
      const targetName = path
        ? resolve(packageRoot, dirname(name), path)
        : resolve(packageRoot, name);
      assert.ok(existsSync(targetName), `${name} links to missing ${target}`);
      if (anchor) {
        const relative = targetName
          .slice(packageRoot.length + 1)
          .split(sep)
          .join("/");
        assert.ok(
          anchors(files[relative]).has(anchor),
          `${name} links to missing heading ${target}`,
        );
      }
    }
  }
});

// Until the CLI help source lists the planning commands (owned by cmds/lane), this
// cross-check is skipped. Once present, the skill and user guide must cover each
// planning command group so help text and guidance cannot drift apart.
test("skill and user guide cover planning commands present in the CLI help source", (t) => {
  const commandsPath = resolve(
    packageRoot,
    "../../cmds/lane/src/commands.json",
  );
  if (!existsSync(commandsPath)) {
    t.skip("CLI help source is not part of this checkout");
    return;
  }
  const commands = Object.keys(JSON.parse(readFileSync(commandsPath, "utf8")));
  const planning = commands.filter((name) =>
    /^(label|milestone|cycle) /.test(name),
  );
  if (planning.length === 0) {
    t.skip("CLI help source does not list planning commands yet");
    return;
  }
  for (const name of planning) {
    const [group, action] = name.split(" ");
    assert.ok(
      skill.includes(`\`lane ${group}`) || skill.includes(group),
      `SKILL.md must cover ${group}`,
    );
    assert.ok(
      guides.userGuide.includes(`\`${action}\``),
      `user-guide.md must mention ${action} for ${group}`,
    );
  }
  if (commands.includes("data reset")) {
    assert.ok(skill.includes("data reset"));
  }
});
