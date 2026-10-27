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
  // Let web-core apply each pointer action across its renderer bridge.
  slowMo: 50,
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.setDefaultTimeout(15000);
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
page.on("console", (message) => {
  if (message.text().includes("lane_planning_discard"))
    console.log(message.text());
});

// Retained panes keep their controls mounted. Bind contextual actions to the
// selected pane, while allowing the global strip and root confirmation dialogs.
const surface = page.locator(
  '[data-active-tab="true"], .tab-bar, #lane-root > .overlay',
);
const button = (name) => surface.getByRole("button", { name, exact: true });
// A field's accessible name gains its validation message, so match by label prefix.
const box = (name) =>
  surface.getByRole("textbox", {
    name: new RegExp(`^${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(\\.|$)`),
  });
const saveButton = () =>
  surface.getByRole("button", { name: /^(Retry )?Save$/ });
const heading = (name) => surface.getByRole("heading", { name, exact: true });
const text = (value, exact = true) =>
  surface.getByText(value, { exact }).filter({ visible: true });
const waitPlanningName = (value) =>
  page.waitForFunction(
    (expected) =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector(
          '[data-active-tab="true"] x-input[accessibility-label="Name"]',
        )
        ?.shadowRoot?.querySelector("input")?.value === expected,
    value,
  );
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
      .filter(
        (e) =>
          !e.closest(
            '.app-tooltip,.tab-tooltip,.tab-scroll,[data-active-tab="false"]',
          ),
      )
      .filter((e) => {
        const r = e.getBoundingClientRect();
        return r.width > 0 && r.height > 0 && r.right > width + 1;
      })
      .map((e) => `${e.tagName}.${e.className}`);
    return { scroll: document.documentElement.scrollWidth, width, wide };
  });
const openTask = async (title) => {
  await button("All tasks").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Tasks",
  );
  await button(title).click();
  await heading("Properties").waitFor();
};

try {
  await page.goto(base);
  await button("Improve empty states").waitFor();

  console.log(
    JSON.stringify({ event: "lane_planning_phase", phase: "task-list" }),
  );
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
  // A data-space switch must not expose the previous space's planning names while the
  // new space loads or after loading fails; retry restores the options.
  const openSpace = async (spaceId) => {
    const generation = await page
      .locator("#lane-root")
      .getAttribute("data-tab-generation");
    await page
      .locator("lynx-view")
      .evaluate(
        (view, spaceId) =>
          view.sendGlobalEvent("lane.openRoute", [{ kind: "open", spaceId }]),
        spaceId,
      );
    await page.waitForFunction(
      (before) =>
        document
          .querySelector("lynx-view")
          .shadowRoot.querySelector("#lane-root")
          .getAttribute("data-tab-generation") !== before,
      generation,
    );
    await button("Filters, 0 active").click();
  };
  let releasePlanning;
  const held = new Promise((resolve) => (releasePlanning = resolve));
  const holdPlanning = async (route) => {
    await held;
    await route.continue();
  };
  await page.route(
    "**/lane.v1.PlanningService/ListPlanningItems",
    holdPlanning,
  );
  await openSpace("other-space");
  await text("Loading filter options…").waitFor();
  assert.equal(await button("Labels: apps / Backend").count(), 0);
  assert.equal(await button("Labels: docs / Backend").count(), 0);
  releasePlanning();
  await button("Labels: apps / Backend").waitFor();
  await page.unroute(
    "**/lane.v1.PlanningService/ListPlanningItems",
    holdPlanning,
  );
  const failPlanning = (route) => route.abort();
  await page.route(
    "**/lane.v1.PlanningService/ListPlanningItems",
    failPlanning,
  );
  await openSpace("third-space");
  await text("Could not load labels, milestones and cycles.", false).waitFor();
  assert.equal(await button("Labels: apps / Backend").count(), 0);
  await page.unroute(
    "**/lane.v1.PlanningService/ListPlanningItems",
    failPlanning,
  );
  await button("Retry loading options").click();
  await button("Labels: apps / Backend").waitFor();
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

  // A failed "Load more tasks" from an obsolete query must not replace the current results.
  let failMore;
  const moreFailed = new Promise((resolve) => (failMore = resolve));
  const paged = async (route) => {
    const input = route.request().postDataJSON();
    if (input?.proposal?.length) return route.continue();
    if (input?.page?.cursor === "obsolete-page") {
      await moreFailed;
      return route.abort();
    }
    const response = await route.fetch();
    const body = await response.json();
    if (!input?.search) body.nextCursor = "obsolete-page";
    await route.fulfill({ response, json: body });
  };
  await page.route("**/lane.v1.LaneService/ListTasks", paged);
  await box("Search title or description").fill("x");
  await box("Search title or description").fill("");
  await button("Load more tasks").click();
  await box("Search title or description").fill("guide");
  await text("Write onboarding guide").first().waitFor();
  failMore();
  await settle();
  assert.equal(
    await text("Tasks could not be loaded").count(),
    0,
    "an obsolete pagination failure does not replace the current results",
  );
  await text("Write onboarding guide").first().waitFor();
  await page.unroute("**/lane.v1.LaneService/ListTasks", paged);
  await box("Search title or description").fill("");

  // ---- Task editor: properties, relationships, validation, retained drafts ------
  await openTask("Improve empty states");
  await heading("Relationships").waitFor();
  assert.equal(await pressed("Priority: High"), true);
  assert.equal(await pressed("Labels: Backend"), true);
  const startBox = box("Start date (YYYY-MM-DD)");
  const dueBox = box("Due date (YYYY-MM-DD)");
  assert.equal(await dueBox.inputValue(), "2026-11-20");
  // The shared vertical tabs leave less editor width at 1440px. Verify two
  // columns when the editor has room, as required by the responsive contract.
  await page.setViewportSize({ width: 1800, height: 900 });
  await settle();
  const [startRect, dueRect] = await Promise.all([
    startBox.boundingBox(),
    dueBox.boundingBox(),
  ]);
  assert.ok(
    Math.abs(startRect.y - dueRect.y) < 2 && startRect.x !== dueRect.x,
    "two-column fields when the editor has sufficient width",
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

  // Selecting the current repository again keeps every draft change and relationship.
  await page
    .locator(".editor")
    .getByRole("button", { name: "apps", exact: true })
    .click();
  await settle();
  assert.equal(await pressed("Type: Bug"), true);
  await button("Change parent").waitFor();
  await button("Remove Plan the launch from Parent").waitFor();
  assert.equal(await box("Story points (whole number)").inputValue(), "0");

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

  // An open relationship picker is inert while a save is in flight: a pick would change
  // the draft after the request was built, and the saved result would discard it.
  await plan({ delayMs: 400 });
  await clearRequests();
  await button("Remove Write onboarding guide from Related").click();
  await button("Add related task").click();
  await box("Search tasks for Related").waitFor();
  await saveButton().click();
  await button("Saving…").waitFor();
  assert.equal(
    await button("Add Plan the launch to Related").getAttribute(
      "aria-disabled",
    ),
    "true",
    "picker results are disabled while saving",
  );
  await button("Add Plan the launch to Related").click({ force: true });
  await settle(700);
  assert.deepEqual((await last("SaveTask")).task.relatedTaskIds ?? [], []);
  assert.equal(
    await button("Remove Plan the launch from Related").count(),
    0,
    "a pick made while saving is not applied",
  );
  await plan({ delayMs: 0 });
  // The picker stays open after the save; restore the link for the following checks.
  await button("Add Write onboarding guide to Related").click();

  // A Related link removed before the repository tasks finish loading stays removed when
  // the other task's stored symmetric reference arrives.
  await saveButton().click();
  await settle(700);
  assert.deepEqual((await last("SaveTask")).task.relatedTaskIds, [ids.taskB]);
  let releaseTasks;
  const heldTasks = new Promise((resolve) => (releaseTasks = resolve));
  const storedReference = async (route) => {
    const input = route.request().postDataJSON();
    if (!input?.repositoryId || input.proposal?.length) return route.continue();
    await heldTasks;
    const response = await route.fetch();
    const body = await response.json();
    for (const item of body.items ?? [])
      if (item.version?.entityId === ids.taskB)
        item.version.task.relatedTaskIds = [ids.taskA];
    await route.fulfill({ response, json: body });
  };
  await page.route("**/lane.v1.LaneService/ListTasks", storedReference);
  // Closing the clean tab explicitly starts a fresh task-options lifetime;
  // selecting an existing tab intentionally keeps its already loaded options.
  await button("Close Improve empty states").click();
  await openTask("Improve empty states");
  await button("Remove (unavailable task) from Related").click();
  releaseTasks();
  await text("Loading tasks of this repository…").waitFor({
    state: "detached",
  });
  await settle();
  assert.equal(
    await button("Remove Write onboarding guide from Related").count(),
    0,
    "a removal before loading is not undone by the stored reference",
  );
  await page.unroute("**/lane.v1.LaneService/ListTasks", storedReference);
  await clearRequests();
  await saveButton().click();
  await settle(700);
  assert.deepEqual((await last("SaveTask")).task.relatedTaskIds ?? [], []);
  await button("Add related task").click();
  await button("Add Write onboarding guide to Related").click();

  // Related tasks open their own tabs without discarding the relationship draft.
  await button("Remove Plan the launch from Parent").click();
  await button("Open Write onboarding guide").first().click();
  await page
    .getByRole("tab", { name: "Write onboarding guide", exact: true })
    .waitFor();
  await settle();
  assert.equal(await text("Discard unsaved changes?").count(), 0);
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
  const reconciliation = page.waitForResponse(
    (response) =>
      response.url().endsWith("/auth/session") && response.status() === 200,
  );
  await page.context().setOffline(false);
  await reconciliation;
  await settle(500);
  const retriedSave = page.waitForResponse((response) =>
    response.url().endsWith("/lane.v1.LaneService/SaveTask"),
  );
  await button("Retry Save").click();
  await retriedSave;
  await settle();
  assert.equal((await last("SaveTask")).task.title, "Offline edit");

  // Title-only creation still works and leaves planning fields unassigned.
  await button("All tasks").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Tasks",
  );
  await button("+ New task").click();
  await text("New task").first().waitFor();
  await page.waitForFunction(() => {
    const title = document
      .querySelector("lynx-view")
      .shadowRoot.querySelector(
        '[data-active-tab="true"] x-input[accessibility-label="Title"]',
      );
    return title?.shadowRoot?.querySelector("input")?.value === "";
  });
  await box("Title").fill("Only a title");
  await page
    .getByRole("tab", { name: "Only a title, Unsaved changes", exact: true })
    .waitFor();
  await page
    .locator(".editor")
    .getByRole("button", { name: "apps", exact: true })
    .click();
  await clearRequests();
  const createdReply = page.waitForResponse((response) =>
    response.url().endsWith("/lane.v1.LaneService/CreateTask"),
  );
  await saveButton().click();
  await createdReply;
  await settle();
  const created = await last("CreateTask");
  assert.equal(created.title, "Only a title");
  assert.equal(created.priority, undefined);
  assert.equal(created.storyPoints, undefined);
  assert.equal(created.labelIds, undefined);

  // Conflict comparison lists each version's relationship targets, not compact counts.
  await plan({ conflictTask: ids.taskB });
  await button("All tasks").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Tasks",
  );
  await button("Offline edit").click();
  await button("Compare conflict").click();
  const compare = page.getByRole("dialog", {
    name: "Resolve whole-task conflict",
  });
  await compare.getByText("Parent: Plan the launch", { exact: true }).waitFor();
  await compare
    .getByText("Parent: Improve empty states", { exact: true })
    .waitFor();
  await compare
    .getByText(`Related: (unavailable task ${ids.unknown})`, { exact: true })
    .waitFor();
  assert.equal(await compare.getByText("Has parent").count(), 0);
  await page.keyboard.press("Escape");
  await plan({ conflictTask: "" });

  console.log(
    JSON.stringify({
      event: "lane_planning_phase",
      phase: "conflict-resolution",
    }),
  );
  // A conflict resolution stays bound to the head set that the user reviewed.
  {
    const head = (n, title) => ({
      versionId: `019a196d-0000-7000-8000-0000000091${n}`,
      title,
    });
    const [h1, h2, r, s1] = [
      head("01", "Head one"),
      head("02", "Head two"),
      head("03", "Head three"),
      head("04", "Head four"),
    ];
    const invalidate = () =>
      page
        .locator("lynx-view")
        .evaluate((view) => view.sendGlobalEvent("lane.onlineChanged", [{}]));
    const resolver = () =>
      page.getByRole("dialog", { name: "Resolve whole-task conflict" });
    const resolveField = (label) =>
      resolver().getByRole("textbox", {
        name: new RegExp(`^${label}(\\.|$)`),
      });
    const resolveButton = () =>
      resolver().getByRole("button", { name: "Save resolution", exact: true });
    const resolveRequests = async () => sent("ResolveConflict");
    const reopen = async () => {
      await page.keyboard.press("Escape");
      await resolver().waitFor({ state: "hidden" });
    };

    // Unchanged heads, including a reordered identical set, permit the edited resolution.
    await plan({ conflictTask: ids.taskB, heads: [h1, h2] });
    await invalidate();
    await settle(500);
    await button("All tasks").click();
    await page.waitForFunction(
      () =>
        document
          .querySelector("lynx-view")
          .shadowRoot.querySelector("#lane-root")
          .getAttribute("data-selected-tab") === "page-Tasks",
    );
    await page.locator(`[data-workspace-tab="task-${ids.taskB}"]`).click();
    await box("Title").waitFor();
    await button("Compare conflict").click();
    await resolver().waitFor();
    // The retained editor also keeps the earlier review session. Explicitly
    // review the new fixture heads instead of silently discarding that session.
    if (await button("Review current versions").count()) {
      await button("Review current versions").click();
      await button("Use this version").first().click();
    }
    await resolveField("Resolved title").fill("Reordered resolution");
    await plan({ heads: [h2, h1] });
    await invalidate();
    await settle(500);
    await resolveButton().waitFor();
    assert.equal(await resolveButton().getAttribute("aria-disabled"), null);
    await clearRequests();
    await resolveButton().click();
    await resolver().waitFor({ state: "hidden" });
    const accepted = await resolveRequests();
    assert.equal(accepted.length, 1);
    assert.deepEqual(
      accepted[0].expectedHeadIds,
      [h1, h2].map((h) => h.versionId).sort(),
    );
    assert.equal(accepted[0].resolved.task.title, "Reordered resolution");

    // Heads that change after selection block the retained body, even when current refreshes.
    await plan({ heads: [h1, h2] });
    await invalidate();
    await settle(500);
    await button("Compare conflict").click();
    await resolver().waitFor();
    await resolveField("Resolved title").fill("Local resolution");
    await resolveField("Resolved description").fill("Local description");
    await plan({ heads: [r] });
    await invalidate();
    // The single-head interval keeps the edits inspectable but never submittable.
    await text("The conflict was resolved elsewhere", false).waitFor();
    assert.equal(
      await resolveField("Resolved title").inputValue(),
      "Local resolution",
    );
    assert.equal(await resolveButton().getAttribute("aria-disabled"), "true");
    await plan({ heads: [r, s1] });
    await invalidate();
    await text(
      "The task changed after you started this resolution",
      false,
    ).waitFor();
    assert.equal(
      await resolveField("Resolved title").inputValue(),
      "Local resolution",
    );
    assert.equal(
      await resolveField("Resolved description").inputValue(),
      "Local description",
    );
    assert.equal(await resolveButton().getAttribute("aria-disabled"), "true");
    for (const use of await resolver()
      .getByRole("button", { name: "Use this version", exact: true })
      .all())
      assert.equal(await use.getAttribute("aria-disabled"), "true");
    await clearRequests();
    await resolveButton().click({ force: true });
    await settle();
    assert.equal((await resolveRequests()).length, 0);

    // Closing and reopening does not rebind the retained body to the newer heads.
    await reopen();
    await button("Compare conflict").click();
    await resolver().waitFor();
    assert.equal(
      await resolveField("Resolved title").inputValue(),
      "Local resolution",
    );
    assert.equal(await resolveButton().getAttribute("aria-disabled"), "true");

    // An explicit review starts a fresh session that can reuse the retained text.
    await resolver()
      .getByRole("button", { name: "Review current versions", exact: true })
      .click();
    assert.equal(await resolveButton().getAttribute("aria-disabled"), "true");
    await resolver()
      .getByRole("button", { name: "Use retained resolution", exact: true })
      .click();
    await resolveField("Resolved title").waitFor();
    await settle();
    assert.equal(
      await resolveField("Resolved title").inputValue(),
      "Local resolution",
    );
    await clearRequests();
    await resolveButton().click();
    await resolver().waitFor({ state: "hidden" });
    const fresh = await resolveRequests();
    assert.equal(fresh.length, 1);
    assert.deepEqual(
      fresh[0].expectedHeadIds,
      [r, s1].map((h) => h.versionId).sort(),
    );
    assert.equal(fresh[0].resolved.task.title, "Local resolution");

    // A server change after the last client read is rejected for the pinned old set and
    // the client keeps its input until the refresh marks the session stale.
    await plan({ heads: [h1, h2] });
    await invalidate();
    await settle(500);
    await button("Compare conflict").click();
    await resolver().waitFor();
    await resolveField("Resolved title").fill("Pinned resolution");
    await plan({ heads: [r, s1] });
    await clearRequests();
    await resolveButton().click();
    await resolver()
      .getByText("The conflict changed since it was read.", { exact: false })
      .waitFor();
    const pinned = await resolveRequests();
    assert.equal(pinned.length, 1);
    assert.deepEqual(
      pinned[0].expectedHeadIds,
      [h1, h2].map((h) => h.versionId).sort(),
    );
    assert.equal(
      await resolveField("Resolved title").inputValue(),
      "Pinned resolution",
    );
    await invalidate();
    await text(
      "The task changed after you started this resolution",
      false,
    ).waitFor();
    assert.equal(
      await resolveField("Resolved title").inputValue(),
      "Pinned resolution",
    );
    await page.keyboard.press("Escape");
    await plan({ conflictTask: "", heads: undefined });
  }

  // Task titles need not be unique: matches beyond the first page of results stay reachable.
  const twins = async (route) => {
    const input = route.request().postDataJSON();
    if (!input?.repositoryId || input.proposal?.length) return route.continue();
    const response = await route.fetch();
    const body = await response.json();
    const source = body.items?.[0];
    for (let n = 0; source && n < 10; n++)
      body.items.push({
        ...source,
        version: {
          ...source.version,
          entityId: `019a196d-0000-7000-8000-${String(900 + n).padStart(12, "0")}`,
          task: { ...source.version.task, title: "Twin task" },
        },
      });
    await route.fulfill({ response, json: body });
  };
  await page.route("**/lane.v1.LaneService/ListTasks", twins);
  await button("Close Only a title").click();
  await openTask("Only a title");
  await button("Add related task").click();
  await box("Search tasks for Related").fill("twin");
  const twin = button("Add Twin task to Related");
  await twin.first().waitFor();
  await settle();
  assert.equal(await twin.count(), 8, "the first page lists eight matches");
  await button("Show more matching tasks").waitFor();
  await text("2 more tasks match.").waitFor();
  await button("Show more matching tasks").focus();
  await page.keyboard.press("Enter");
  await settle();
  assert.equal(await twin.count(), 10, "Show more reveals every match");
  assert.equal(await button("Show more matching tasks").count(), 0);
  await page.unroute("**/lane.v1.LaneService/ListTasks", twins);

  console.log(
    JSON.stringify({
      event: "lane_planning_phase",
      phase: "repository-planning",
    }),
  );
  // ---- Repository planning management ------------------------------------------
  // Task editing scenarios are complete. Close their retained workspaces before
  // testing the planning manager; its own Settings round trip below verifies
  // preservation across page tabs without unrelated task editors in the fixture.
  const taskWorkspaces = page.locator(".workspace-tab").filter({
    has: page.locator('[data-workspace-tab^="task-"]'),
  });
  while (await taskWorkspaces.count()) {
    const workspace = taskWorkspaces.last();
    const selector = workspace.locator("[data-workspace-tab]");
    const id = await selector.getAttribute("data-workspace-tab");
    const dirty = (await selector.getAttribute("aria-label")).includes(
      "Unsaved changes",
    );
    await workspace.getByRole("button", { name: /^Close / }).click();
    if (dirty) {
      await page
        .getByRole("dialog", { name: "Discard unsaved changes?", exact: true })
        .waitFor();
      await page
        .getByRole("button", { name: "Discard changes", exact: true })
        .click();
    }
    await page
      .locator(`[data-workspace-tab="${id}"]`)
      .waitFor({ state: "detached" });
  }
  await button("Repositories").click();
  await button("Labels, milestones and cycles").first().click();
  await text("Labels (2)", false).waitFor();
  await text("Backend").waitFor();
  await text("#075AFF").waitFor();
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
  await page
    .locator('[data-active-tab="true"] x-input[accessibility-label="Name"]')
    .waitFor({ state: "attached" });
  await box("Color (#RRGGBB)").fill("#0a5aff");
  await page
    .locator(
      '[data-active-tab="true"] x-input[accessibility-label="Color (#RRGGBB)"]',
    )
    .waitFor({ state: "attached" });
  await clearRequests();
  const createdLabel = page.waitForResponse((response) =>
    response.url().endsWith("/lane.v1.PlanningService/CreatePlanningItem"),
  );
  await saveButton().click();
  await createdLabel;
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
  const savedLabel = page.waitForResponse((response) =>
    response.url().endsWith("/lane.v1.PlanningService/SavePlanningItem"),
  );
  await button("Retry Save").click();
  await savedLabel;
  await button("Save").waitFor();
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
  await text("v1 launch").waitFor();
  await text("Open · Target 2026-12-01").waitFor();
  await button("New milestone").click();
  await box("Target date (YYYY-MM-DD, optional)").fill("2026-02-30");
  await box("Name").fill("Beta");
  await saveButton().click();
  await text("Use a real calendar date in YYYY-MM-DD form.").waitFor();
  await box("Target date (YYYY-MM-DD, optional)").fill("");
  await page
    .locator(
      '[data-active-tab="true"] x-input[accessibility-label="Target date (YYYY-MM-DD, optional)"]',
    )
    .waitFor({ state: "attached" });
  await button("Completion: Completed").click();
  await clearRequests();
  const createdMilestone = page.waitForResponse((response) =>
    response.url().endsWith("/lane.v1.PlanningService/CreatePlanningItem"),
  );
  await saveButton().click();
  await createdMilestone;
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

  console.log(
    JSON.stringify({
      event: "lane_planning_phase",
      phase: "retained-planning-draft",
    }),
  );
  // Workspace tab switching preserves planning input; internal manager
  // navigation still uses its existing discard confirmation.
  await button("Settings").first().click();
  await page.getByRole("tab", { name: "Settings", exact: true }).waitFor();
  await settle();
  assert.equal(await text("Discard unsaved changes?").count(), 0);
  const repositoryWorkspace = page.getByRole("tab", {
    name: /Repositories, Unsaved changes/,
  });
  await repositoryWorkspace.scrollIntoViewIfNeeded();
  await settle();
  await repositoryWorkspace.click();
  console.log(
    JSON.stringify({
      event: "lane_planning_phase",
      phase: "return-to-repositories",
    }),
  );
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Repositories",
  );
  await box("Name").waitFor();
  assert.equal(await box("Name").inputValue(), "Sprint 2");
  await button("Labels tab").click();
  await text("Discard unsaved changes?").waitFor();
  await settle();
  await button("Keep editing").click();
  await text("Discard unsaved changes?").waitFor({ state: "detached" });
  assert.equal(await box("Name").inputValue(), "Sprint 2");
  await button("Back to repositories").click();
  await text("Discard unsaved changes?").waitFor();
  await settle();
  await button("Keep editing").click();
  await text("Discard unsaved changes?").waitFor({ state: "detached" });
  await button("Close").click();
  await text("Discard unsaved changes?").waitFor();
  await button("Discard changes").click();
  await text("Select an item to edit it, or create a new one.").waitFor();
  // A slow save response only updates the editor that submitted it: switching tabs
  // or selecting another item meanwhile keeps the new selection.
  await button("Labels tab").click();
  await button("Edit label Backend").click();
  await plan({ delayMs: 700 });
  await clearRequests();
  await saveButton().click();
  await button("Saving…").waitFor();
  await button("Milestones tab").click();
  await settle(1200);
  await text("Select an item to edit it, or create a new one.").waitFor();
  assert.equal(
    await text("Edit label").count(),
    0,
    "a late label save does not reopen the label under Milestones",
  );
  await button("Labels tab").click();
  await button("Edit label Backend").click();
  await saveButton().click();
  await button("Saving…").waitFor();
  await button("Edit label Frontend").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector(
          '[data-active-tab="true"] x-input[accessibility-label="Name"]',
        )
        ?.shadowRoot?.querySelector("input")?.value === "Frontend",
  );
  await settle(1200);
  assert.equal(
    await box("Name").inputValue(),
    "Frontend",
    "a late save does not replace the newly selected item",
  );
  await plan({ delayMs: 0 });
  await button("Close").click();
  await text("Select an item to edit it, or create a new one.").waitFor();
  // Clean editors never prompt, and workspace navigation keeps the draft.
  await button("Labels tab").click();
  await button("Edit label Backend").click();
  await waitPlanningName("Backend");
  await button("Back to repositories").click();
  assert.equal(await text("Discard unsaved changes?").count(), 0);
  await button("Labels, milestones and cycles").first().click();
  await button("Edit label Backend").click();
  await waitPlanningName("Backend");
  await box("Name").fill("Backend 2");
  await button("Settings").first().click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Settings",
  );
  await settle();
  assert.equal(await text("Discard unsaved changes?").count(), 0);

  // Development reset is native only.
  await button("Settings").first().click();
  await text("Online account", true).first().waitFor();
  assert.equal(await text("Development data").count(), 0);

  console.log(
    JSON.stringify({
      event: "lane_planning_phase",
      phase: "responsive-layout",
    }),
  );
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
} catch (error) {
  await page.screenshot({ path: join(evidence, "failure.png") });
  console.error(
    await page
      .locator("lynx-view")
      .evaluate((view) =>
        [...view.shadowRoot.querySelectorAll(".notice")]
          .filter((el) => el.getBoundingClientRect().width)
          .map((el) => el.textContent),
      ),
  );
  throw error;
} finally {
  await browser.close();
  server.kill();
}
assert.deepEqual(errors, []);
console.log(`Planning browser checks passed. Mock-only evidence: ${evidence}`);
