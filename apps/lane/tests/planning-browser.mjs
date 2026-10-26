// SPDX-License-Identifier: Apache-2.0
// Task planning UI checks against the test-only planning fixture (web, online-only).
// Fixture evidence verifies the shared ReactLynx screens, not a real account or device.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";
import { ids } from "./planning-mock.mjs";

const evidence =
  process.env.LANE_BROWSER_EVIDENCE || join(tmpdir(), "lane-planning-evidence");
await mkdir(evidence, { recursive: true });
const port = 46416;
const base = `http://127.0.0.1:${port}`;
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: {
    ...process.env,
    LANE_PREVIEW_PORT: String(port),
    LANE_PREVIEW_SCENARIO: "planning",
  },
  stdio: ["ignore", "pipe", "inherit"],
});
await new Promise((resolve, reject) => {
  server.stdout.once("data", resolve);
  server.once("exit", (code) => reject(new Error(`Preview exited ${code}`)));
});
const browser = await chromium.launch({
  channel: process.env.LANE_BROWSER_CHANNEL || "chrome",
  headless: true,
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));

const button = (name) => page.getByRole("button", { name, exact: true });
// A field's accessible name gains its validation message, so match by label prefix.
const box = (name) =>
  page.getByRole("textbox", {
    name: new RegExp(`^${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(\\.|$)`),
  });
const saveButton = () => page.getByRole("button", { name: /^(Retry )?Save$/ });
const heading = (name) => page.getByRole("heading", { name, exact: true });
const text = (value, exact = true) => page.getByText(value, { exact });
const requests = async () => (await fetch(`${base}/test/requests`)).json();
const clearRequests = () => fetch(`${base}/test/reset-requests`);
const plan = (body) =>
  fetch(`${base}/test/plan?body=${encodeURIComponent(JSON.stringify(body))}`);
const sent = async (method) =>
  (await requests()).filter((r) => r.method === method).map((r) => r.input);
const last = async (method) => (await sent(method)).at(-1);
// The proposal counter issues its own pending-only ListTasks; list queries have no proposal filter here.
const lists = async () =>
  (await sent("ListTasks")).filter((r) => !r.proposal?.length);
const lastList = async () => (await lists()).at(-1);
// Polls because list queries are issued asynchronously after each control change.
const waitList = async (accept) => {
  for (let i = 0; i < 50; i++) {
    const request = await lastList();
    if (request && accept(request)) return request;
    await page.waitForTimeout(100);
  }
  throw new Error(
    `No matching list query: ${JSON.stringify(await lastList())}`,
  );
};
const settle = (ms = 250) => page.waitForTimeout(ms);
const pressed = async (name) =>
  (await button(name).getAttribute("aria-pressed")) === "true";
// Every rendered element must stay inside the viewport width (no horizontal scrolling).
const overflow = () =>
  page.locator("lynx-view").evaluate((view) => {
    const width = document.documentElement.clientWidth;
    const wide = [...view.shadowRoot.querySelectorAll("*")]
      .filter((e) => !e.closest(".app-tooltip"))
      .filter((e) => {
        const r = e.getBoundingClientRect();
        return r.width > 0 && r.height > 0 && r.right > width + 1;
      })
      .map((e) => `${e.tagName}.${e.className}`);
    return { scroll: document.documentElement.scrollWidth, width, wide };
  });
const openTask = async (title) => {
  await button("All tasks").click();
  await button(title).click();
  await heading("Properties").waitFor();
};

try {
  await page.goto(base);
  await button("Improve empty states").waitFor();

  // ---- Task list: metadata, qualification, filters and sort --------------------
  await text(
    "Priority High · Due 2026-11-20 · 3 points · Milestone apps / v1 launch",
    false,
  ).waitFor();
  await text("apps / Backend", true).first().waitFor();
  // In the all-repository list repository-owned values name their repository.
  await text("docs / Backend", true).waitFor();
  await page.screenshot({ path: join(evidence, "desktop-task-list.png") });

  // The web runtime drops aria-*="false"; an expanded region is exposed as "true" only.
  assert.notEqual(
    await button("Filters, 0 active").getAttribute("aria-expanded"),
    "true",
  );
  await button("Filters, 0 active").click();
  await settle();
  assert.equal(
    await button("Filters, 0 active").getAttribute("aria-expanded"),
    "true",
  );
  await text("Loading filter options…")
    .waitFor({ state: "detached" })
    .catch(() => {});
  // Repository-qualified selections in the all-repository list: same-name labels stay distinct.
  assert.equal(await button("Labels: apps / Backend").count(), 1);
  assert.equal(await button("Labels: docs / Backend").count(), 1);
  for (const field of [
    "Priority",
    "Type",
    "Labels",
    "Start date",
    "Due date",
    "Story points",
    "Milestone",
    "Cycle",
    "Relationships",
  ])
    assert.equal(
      await button(`${field}: Unassigned`).count(),
      1,
      `${field} has its own Unassigned choice`,
    );
  // Keyboard operation: a focused chip toggles with Space and exposes its state.
  await button("Type: Bug").focus();
  await page.keyboard.press(" ");
  await settle();
  assert.equal(await pressed("Type: Bug"), true);
  await page.keyboard.press("Enter");
  await settle();
  assert.equal(await pressed("Type: Bug"), false);
  await clearRequests();
  await button("Priority: High").click();
  await button("Priority: Unassigned").click();
  assert.equal(await pressed("Priority: High"), true);
  let list = await waitList((r) => r.filter?.includeUnassignedPriority);
  assert.deepEqual(list.filter.priorities, ["TASK_PRIORITY_HIGH"]);
  assert.equal(list.filter.includeUnassignedPriority, true);
  await button("Labels: docs / Backend").click();
  await button("Relationships: Parent").click();
  await button("Relationships: Unassigned").click();
  await box("Due date from (YYYY-MM-DD)").fill("2026-11-01");
  await box("Story points minimum").fill("0");
  list = await waitList(
    (r) =>
      r.filter?.storyPoints && r.filter?.dueDate?.from && r.filter?.labelIds,
  );
  assert.deepEqual(list.filter.labelIds, [ids.labelDocsBackend]);
  assert.deepEqual(list.filter.relationships, ["RELATIONSHIP_KIND_PARENT"]);
  assert.equal(list.filter.includeNoRelationships, true);
  assert.equal(list.filter.dueDate.from, "2026-11-01");
  assert.equal(list.filter.storyPoints.min, 0);
  assert.equal(await button("Filters, 5 active").count(), 1);

  // Separate sort key and direction; both reach the query.
  await button("Sort by: Due date").click();
  await button("Sort direction: Descending").click();
  await text(
    "Tasks without a value always sort last; ties sort by task ID.",
  ).waitFor();
  list = await waitList((r) => r.sortDirection === "SORT_DIRECTION_DESCENDING");
  assert.equal(list.sortKey, "TASK_SORT_KEY_DUE_DATE");
  assert.equal(list.sortDirection, "SORT_DIRECTION_DESCENDING");

  // A malformed range is explained in text and never sent.
  const before = (await lists()).length;
  await box("Due date from (YYYY-MM-DD)").fill("2026-02-30");
  await text("Use a real calendar date in YYYY-MM-DD form.").waitFor();
  await text(
    "Fix the highlighted filter values to update the list. Nothing was sent.",
    false,
  ).waitFor();
  await settle();
  assert.equal((await lists()).length, before);
  await box("Due date from (YYYY-MM-DD)").fill("2026-11-01");

  // Clear filters keeps search and sort.
  await box("Search title or description").fill("guide");
  await page
    .locator(".filters")
    .getByRole("button", { name: "Clear filters", exact: true })
    .click();
  list = await waitList((r) => !r.filter);
  assert.equal(list.filter, undefined);
  assert.equal(list.search, "guide");
  assert.equal(list.sortKey, "TASK_SORT_KEY_DUE_DATE");
  assert.equal(await button("Filters, 0 active").count(), 1);
  await box("Search title or description").fill("");
  await button("Sort by: Default order").click();
  // Collapse the region: the editor reuses the same chip names.
  await button("Filters, 0 active").click();
  await settle();
  assert.equal(await text("Fields combine with AND", false).count(), 0);

  // ---- Task editor: properties, relationships, validation, retained drafts ------
  await openTask("Improve empty states");
  await heading("Relationships").waitFor();
  assert.equal(await pressed("Priority: High"), true);
  assert.equal(await pressed("Labels: Backend"), true);
  const startBox = box("Start date (YYYY-MM-DD)");
  const dueBox = box("Due date (YYYY-MM-DD)");
  assert.equal(await dueBox.inputValue(), "2026-11-20");
  // Two columns at desktop width: start and due date share a row.
  const [startRect, dueRect] = await Promise.all([
    startBox.boundingBox(),
    dueBox.boundingBox(),
  ]);
  assert.ok(
    Math.abs(startRect.y - dueRect.y) < 2 && startRect.x !== dueRect.x,
    "two-column fields at 1440px",
  );
  await heading("Properties").scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(evidence, "desktop-task-editor.png") });
  await heading("Relationships").scrollIntoViewIfNeeded();
  await page.screenshot({
    path: join(evidence, "desktop-task-relationships.png"),
  });

  await button("Type: Bug").click();
  await button("Labels: Frontend").click();
  await button("Cycle: Sprint 1 (2026-11-02 to 2026-11-15)").click();
  await startBox.fill("2026-12-01");
  await box("Story points (whole number)").fill("1.5");
  await saveButton().click();
  await text("The start date must be on or before the due date.").waitFor();
  await text(
    "Story points must be a whole number from 0 to 2,147,483,647.",
  ).waitFor();
  await text("Fix the fields marked above, then Save again.", false).waitFor();
  assert.equal(
    (await sent("SaveTask")).length,
    0,
    "invalid drafts are not sent",
  );
  assert.equal(
    await pressed("Type: Bug"),
    true,
    "draft retained after validation failure",
  );
  await startBox.fill("2026-11-02");
  await box("Story points (whole number)").fill("0");

  // Relationships: search-select same-repository tasks by title.
  await button("Add parent").click();
  await box("Search tasks for Parent").fill("launch");
  await button("Add Plan the launch to Parent").click();
  await text("Plan the launch").first().waitFor();
  await button("Change parent").waitFor();
  await button("Add subtask").click();
  await box("Search tasks for Subtasks").fill("onboarding");
  assert.equal(
    await button("Add Fix docs typo to Subtasks").count(),
    0,
    "other repositories are not offered",
  );
  await button("Add Write onboarding guide to Subtasks").click();
  await button("Add dependency").click();
  await button("Add Plan the launch to Depends on").click();
  await button("Add related task").click();
  await button("Add Write onboarding guide to Related").click();
  await button("Add original task").click();
  await button("Add Plan the launch to Duplicate of").click();

  // Constraint violation: explained in text, draft and request retained.
  await clearRequests();
  await plan({
    failNext: {
      method: "SaveTask",
      code: "failed_precondition",
      message: "loop",
      violation: "PARENT_CYCLE",
    },
  });
  await saveButton().click();
  await text("This parent would create a loop", false).waitFor();
  await text("Your changes are not saved.", false).waitFor();
  assert.equal(await box("Title").inputValue(), "Improve empty states");
  assert.equal(await pressed("Type: Bug"), true);
  assert.equal(await button("Retry Save").count(), 1);
  const failed = await last("SaveTask");

  // Retry keeps the request ID (idempotent), applies once and disables duplicate clicks.
  await plan({ delayMs: 400 });
  await clearRequests();
  await button("Retry Save").click();
  await button("Saving…").waitFor();
  assert.equal(await button("Saving…").getAttribute("aria-disabled"), "true");
  await button("Saving…")
    .click({ force: true })
    .catch(() => {});
  await settle(700);
  const saves = await sent("SaveTask");
  assert.equal(saves.length, 1, "duplicate submit is blocked while saving");
  assert.equal(
    saves[0].meta.requestId,
    failed.meta.requestId,
    "unchanged retry reuses the request ID",
  );
  const saved = saves[0];
  assert.equal(saved.task.priority, "TASK_PRIORITY_HIGH");
  assert.equal(saved.task.type, "TASK_TYPE_BUG");
  assert.deepEqual(
    saved.task.labelIds,
    [ids.labelBackend, ids.labelFrontend].sort(),
  );
  assert.equal(saved.task.startDate, "2026-11-02");
  assert.equal(saved.task.dueDate, "2026-11-20");
  assert.equal(
    saved.task.storyPoints,
    0,
    "zero story points are sent, not dropped",
  );
  assert.equal(saved.task.milestoneId, ids.milestone);
  assert.equal(saved.task.cycleId, ids.cycle);
  assert.equal(saved.task.parentTaskId, ids.taskC);
  assert.deepEqual(saved.task.dependsOnTaskIds, [ids.taskC]);
  assert.deepEqual(saved.task.relatedTaskIds, [ids.taskB]);
  assert.equal(saved.task.duplicateOfTaskId, ids.taskC);
  assert.deepEqual(saved.relationshipEdits, { addSubtaskIds: [ids.taskB] });
  await plan({ delayMs: 0 });

  // Removing a link and Open: unsaved changes ask before leaving.
  await button("Remove Plan the launch from Parent").click();
  await button("Open Write onboarding guide").first().click();
  await text("Discard unsaved changes?").waitFor();
  await page.keyboard.press("Escape");
  await text("Discard unsaved changes?").waitFor({ state: "detached" });
  assert.equal(await box("Title").inputValue(), "Improve empty states");
  await button("Open Write onboarding guide").first().click();
  await button("Discard changes").click();
  await settle(500);
  assert.equal(await box("Title").inputValue(), "Write onboarding guide");

  // Web saves are online-only: offline fails, retains the draft and needs an explicit retry.
  await box("Title").fill("Offline edit");
  await clearRequests();
  await page.context().setOffline(true);
  await saveButton().click();
  await text("You are offline.", false).first().waitFor();
  await button("Retry Save").waitFor();
  assert.equal(await box("Title").inputValue(), "Offline edit");
  assert.equal((await sent("SaveTask")).length, 0);
  await page.context().setOffline(false);
  await settle(500);
  await button("Retry Save").click();
  await settle();
  assert.equal((await last("SaveTask")).task.title, "Offline edit");

  // Title-only creation still works and leaves planning fields unassigned.
  await button("All tasks").click();
  await button("+ New task").click();
  await box("Title").fill("Only a title");
  await page
    .locator(".editor")
    .getByRole("button", { name: "apps", exact: true })
    .click();
  await clearRequests();
  await saveButton().click();
  await settle();
  const created = await last("CreateTask");
  assert.equal(created.title, "Only a title");
  assert.equal(created.priority, undefined);
  assert.equal(created.storyPoints, undefined);
  assert.equal(created.labelIds, undefined);

  // ---- Repository planning management ------------------------------------------
  await button("Repositories").click();
  await button("Labels, milestones and cycles").first().click();
  await text("Labels (2)", false).waitFor();
  await text("Backend · #075AFF", false).waitFor();
  await page.screenshot({
    path: join(evidence, "desktop-planning-labels.png"),
  });
  await button("New label").click();
  await box("Name").fill("backend");
  await box("Color (#RRGGBB)").fill("red");
  await saveButton().click();
  await text(
    "Another item of this kind in this repository already uses this name.",
    false,
  ).waitFor();
  await text("Use a color in #RRGGBB form, for example #075AFF.").waitFor();
  assert.equal((await sent("CreatePlanningItem")).length, 0);
  await box("Name").fill("Design");
  await box("Color (#RRGGBB)").fill("#0a5aff");
  await clearRequests();
  await saveButton().click();
  await settle();
  let create = await last("CreatePlanningItem");
  assert.equal(create.item.label.color, "#0A5AFF");
  assert.equal(create.item.label.name, "Design");
  assert.equal(create.item.label.repositoryId, ids.repoApps);
  await text("Edit label").waitFor();

  // Server-side duplicate name is explained and the input retained.
  await box("Name").fill("Interface");
  await plan({
    failNext: {
      method: "SavePlanningItem",
      code: "failed_precondition",
      message: "duplicate",
      violation: "DUPLICATE_NAME",
    },
  });
  await saveButton()
    .click()
    .catch(() => {});
  await text("already uses this name", false).first().waitFor();
  assert.equal(await box("Name").inputValue(), "Interface");
  assert.equal((await sent("SavePlanningItem")).length, 1);

  // History: before/after, time and origin, no Restore.
  await box("Name").fill("Design system");
  await button("Retry Save")
    .click()
    .catch(() => {});
  await settle();
  await button("History").click();
  await text("Item history").waitFor();
  await text("Before: Design").waitFor();
  await text("After: Design system").waitFor();
  assert.equal(await page.getByText(/Restore/).count(), 0);
  await page.keyboard.press("Escape");
  await text("Item history").waitFor({ state: "detached" });

  // Deletion explains detachment; Escape cancels and keeps the item.
  await button("Delete").click();
  await text("detaches it from every task that uses it", false).waitFor();
  await text("The tasks remain.", false).waitFor();
  await page.keyboard.press("Escape");
  await text("Delete label?").waitFor({ state: "detached" });
  assert.equal((await sent("DeletePlanningItem")).length, 0);
  await button("Delete").click();
  await button("Delete permanently").click();
  await settle();
  assert.equal((await sent("DeletePlanningItem")).length, 1);

  // Milestones and cycles tabs, plus empty-state creation.
  await button("Milestones tab").click();
  await button("Edit milestone v1 launch").waitFor();
  await text("v1 launch · Open · Target 2026-12-01").waitFor();
  await button("New milestone").click();
  await box("Target date (YYYY-MM-DD, optional)").fill("2026-02-30");
  await box("Name").fill("Beta");
  await saveButton().click();
  await text("Use a real calendar date in YYYY-MM-DD form.").waitFor();
  await box("Target date (YYYY-MM-DD, optional)").fill("");
  await button("Completion: Completed").click();
  await clearRequests();
  await saveButton().click();
  await settle();
  create = await last("CreatePlanningItem");
  assert.equal(create.item.milestone.completed, true);
  assert.equal(create.item.milestone.targetDate, undefined);
  await button("Cycles tab").click();
  await button("New cycle").click();
  await box("Name").fill("Sprint 2");
  await box("Start date (YYYY-MM-DD)").fill("2026-11-16");
  await box("End date (YYYY-MM-DD)").fill("2026-11-01");
  await saveButton().click();
  await text("The end date must be on or after the start date.").waitFor();

  // Development reset is native only.
  await button("Settings").first().click();
  await text("Online account", true).first().waitFor();
  assert.equal(await text("Development data").count(), 0);

  // ---- Responsive layout and keyboard operation --------------------------------
  for (const width of [900, 899, 360]) {
    await page.setViewportSize({ width, height: 800 });
    await settle(400);
    for (const [label, action] of [
      ["planning manager", async () => {}],
      [
        "task list",
        async () => {
          await button(width >= 900 ? "All tasks" : "Tasks").click();
          await button("Filters, 0 active")
            .click()
            .catch(() => {});
        },
      ],
      [
        "task editor",
        async () => {
          await button("Improve empty states").click();
          await heading("Properties").waitFor();
        },
      ],
    ]) {
      await action();
      await settle(300);
      const result = await overflow();
      assert.deepEqual(
        result.wide,
        [],
        `${label} fits ${width}px without horizontal scrolling`,
      );
      assert.ok(
        result.scroll <= result.width,
        `${label} has no horizontal scroll at ${width}px`,
      );
    }
    if (width < 900) {
      const [a, b] = await Promise.all([
        box("Start date (YYYY-MM-DD)").boundingBox(),
        box("Due date (YYYY-MM-DD)").boundingBox(),
      ]);
      assert.ok(b.y > a.y + 10, `fields stack in one column at ${width}px`);
    }
    await heading("Properties").scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(evidence, `editor-${width}.png`) });
  }
} finally {
  await browser.close();
  server.kill();
}
assert.deepEqual(errors, []);
console.log(`Planning browser checks passed. Mock-only evidence: ${evidence}`);
