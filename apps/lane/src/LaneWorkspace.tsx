// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  LaneService,
  Progress,
  Proposal,
  SortDirection,
  TaskSortKey,
  type EntityResult as ProtoEntityResult,
} from "@repo/protos/lane/v1/lane_pb";
import { newId, type LaneAdapter } from "./adapter";

import { Button, Choices, Field, MenuChoices, Notice } from "./components";
import {
  blankTask,
  conflicted,
  draftFor,
  progressOptions,
  proposalOptions,
  receiveExternal,
  repositoryName,
  taskOf,
  type Draft,
} from "./model";
import { TaskEditor } from "./TaskEditor";
import { TaskFilters, TaskSortControls } from "./TaskFilters";
import { usePlanning } from "./usePlanning";

import {
  activeFilterCount,
  describeError,
  emptyFilter,
  filterErrors,
  hasErrors,
  lookupOf,
  pruneFilter,
  taskMetadata,
  toTaskFilter,
  type FilterState,
} from "./planning";
import { Repositories } from "./Repositories";
import { Settings } from "./Settings";
import { Events } from "./Events";
import { AppIcon, MiniApp, SettingsSection } from "./AppNavigation";
import { type TabTarget, TabKind, type WorkspaceReport } from "./tabs";
import {
  beginEditorCompletion,
  EditorLifetime,
  readTaskOwned,
} from "./editorLifetime";
import { LanePage } from "./navigation";
import "./style.css";
export function LaneWorkspace({
  adapter,
  target,
  tabId,
  active,
  mobile,
  onOpen,
  onReport,
  onReset,
  onRemoved,
  onCloseTab,
  revision,
  onChange,
  beforeDataChange,
  settingsTarget,
}: {
  adapter: LaneAdapter;
  target: TabTarget;
  tabId: string;
  active: boolean;
  mobile: boolean;
  onOpen: (target: TabTarget) => void;
  onReport: (report: WorkspaceReport) => void;
  onReset: () => void;
  onRemoved: (id: string) => void;
  onCloseTab: (id: string) => void;
  revision: number;
  onChange: () => void;
  beforeDataChange: () => Promise<boolean>;
  settingsTarget?: { section: SettingsSection; request: number };
}) {
  const [planningDirty, setPlanningDirty] = useState(false);
  const [taskError, setTaskError] = useState("");
  const [repositoryError, setRepositoryError] = useState("");
  const error = taskError || repositoryError;
  const [loading, setLoading] = useState(true);
  const page = target.page ?? LanePage.Tasks;
  const [repository, setRepository] = useState(target.repositoryId ?? "");
  const [repositoryStatus, setRepositoryStatus] = useState(
    "Loading repositories...",
  );
  const [repositories, setRepositories] = useState<EntityResult[]>([]);
  const [tasks, setTasks] = useState<EntityResult[]>([]);
  const [cursor, setCursorState] = useState("");
  // Mirrors `cursor` synchronously so a continuation never starts from a
  // cursor that an earlier page has already consumed.
  const cursorRef = useRef("");
  const setCursor = (next: string) => {
    cursorRef.current = next;
    setCursorState(next);
  };
  const [loadingMore, setLoadingMore] = useState(false);
  const [search, setSearch] = useState("");
  const searchFocusPending = useRef(page === "Proposals");
  useEffect(() => {
    if (active && page === "Proposals") searchFocusPending.current = false;
  }, [active, page]);
  const [progress, setProgress] = useState<Progress>(Progress.UNSPECIFIED);
  const [proposal, setProposal] = useState<Proposal>(Proposal.UNSPECIFIED);
  const [filter, setFilter] = useState<FilterState>(emptyFilter);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [sortKey, setSortKey] = useState<TaskSortKey>(TaskSortKey.UNSPECIFIED);
  const [sortDirection, setSortDirection] = useState<SortDirection>(
    SortDirection.ASCENDING,
  );
  const [listError, setListError] = useState("");
  // The web host resets a hidden scroll-view. Retain each Lane task pane's last
  // active offset until the host preserves hidden scroll offsets itself.
  const taskScrollTop = useRef(0);
  const [draft, setDraft] = useState<Draft>();
  const [current, setCurrent] = useState<EntityResult>();

  const [pageBusy, setPageBusy] = useState(false);
  const [editorBusy, setEditorBusy] = useState(false);
  const [settingsDirty, setSettingsDirty] = useState(false);
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [settingsExitBusy, setSettingsExitBusy] = useState(false);
  const [taskLoading, setTaskLoading] = useState(false);
  const [pendingCount, setPendingCount] = useState(0);
  const queryGeneration = useRef(0);
  // Task-detail reads publish only while they own the editor lifetime.
  const editorLifetime = useRef(new EditorLifetime()).current;
  // Owner of the single in-flight "Load more tasks" request. A continuation
  // may publish its page and release the lock only while it is still the
  // owner, so a settled, superseded or stale completion cannot append twice,
  // roll the cursor back or free a newer request's lock.
  const continuation = useRef<object | undefined>(undefined);
  // Starts a new query generation and drops continuation ownership with it.
  const advanceQueryGeneration = () => {
    continuation.current = undefined;
    setLoadingMore(false);
    return ++queryGeneration.current;
  };
  // Identifies the open data space; native route and reset switches change it.
  const planning = usePlanning(
    adapter,
    revision,
    adapter ? (adapter.spaceId ?? "") : "",
  );
  const currentDraft = useRef(draft);
  currentDraft.current = draft;
  const refresh = onChange;
  useEffect(() => {
    return () => {
      editorLifetime.supersede();
      continuation.current = undefined;
      ++queryGeneration.current;
    };
  }, [adapter]);
  useEffect(() => {
    if (!adapter) return;
    let active = true;
    setRepositoryStatus("Loading repositories...");
    setRepositoryError("");
    let repositoriesLoaded = false;
    void (async () => {
      try {
        const repos: EntityResult[] = [];
        let after = "";
        do {
          const r = await adapter.call(LaneService.method.listRepositories, {
            page: { size: 100, cursor: after },
          });
          repos.push(...r.items);
          after = r.nextCursor;
        } while (after);
        repositoriesLoaded = true;
        if (active) {
          setRepositories(repos);
          setRepositoryStatus("");
          setRepositoryError("");
        }
        let count = 0;
        after = "";
        do {
          const r = await adapter.call(LaneService.method.listTasks, {
            page: { size: 100, cursor: after },
            proposal: [Proposal.PENDING],
          });
          count += r.items.length;
          after = r.nextCursor;
        } while (after);
        if (active) setPendingCount(count);
        const editor = currentDraft.current;
        if (editor?.base) {
          const r = await adapter.call(LaneService.method.getTask, {
            id: editor.id,
          });
          if (active && currentDraft.current?.id === editor.id && r.result) {
            setCurrent(r.result);
            setDraft((d) => (d ? receiveExternal(d, r.result!) : d));
          }
        }
      } catch (e) {
        if (active) {
          if (!repositoriesLoaded) {
            setRepositoryStatus("Repositories could not be refreshed.");
            setRepositoryError(
              e instanceof Error ? e.message : "Refresh failed.",
            );
          } else {
            setTaskError(e instanceof Error ? e.message : "Refresh failed.");
          }
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [adapter, revision]);
  const filterProblems = filterErrors(filter);
  const filterInvalid = hasErrors(filterProblems);
  // One request builder serves the first page and "Load more", so the cursor
  // always belongs to the same query, filter and sort.
  const listRequest = (after = "") => ({
    page: { size: 100, cursor: after },
    repositoryId: repository || undefined,
    progress: progress ? [progress] : [],
    proposal:
      page === "Proposals"
        ? [proposal || Proposal.PENDING]
        : proposal
          ? [proposal]
          : [],
    search,
    filter: toTaskFilter(filter),
    sortKey,
    sortDirection:
      sortKey === TaskSortKey.UNSPECIFIED
        ? SortDirection.UNSPECIFIED
        : sortDirection,
  });
  useEffect(() => {
    if (!adapter) return;
    const generation = advanceQueryGeneration();
    setListError("");
    if (filterInvalid) {
      // Never send a malformed range; the Filters region explains the problem.
      setTasks([]);
      setCursor("");
      setLoading(false);
      return;
    }
    setLoading(true);
    void adapter
      .call(LaneService.method.listTasks, listRequest())
      .then(
        (r) => {
          if (generation === queryGeneration.current) {
            setTasks(r.items);
            setCursor(r.nextCursor);
          }
        },
        (e) => {
          if (generation === queryGeneration.current) {
            setTasks([]);
            setCursor("");
            setListError(describeError(e));
          }
        },
      )
      .finally(() => {
        if (generation === queryGeneration.current) setLoading(false);
      });
  }, [
    adapter,
    repository,
    progress,
    proposal,
    search,
    page,
    revision,
    JSON.stringify(filter),
    sortKey,
    sortDirection,
  ]);
  // Repository-owned selections only make sense inside their repository scope.
  useEffect(() => {
    if (planning.status !== "ready") return;
    const scoped = repository
      ? planning.items.filter((i) => i.repositoryId === repository)
      : planning.items;
    setFilter((f) => pruneFilter(f, scoped));
  }, [repository, planning.items, planning.status]);
  const repositoryNames = new Map(
    repositories.map((r) => [r.version!.entityId, repositoryName(r.version)]),
  );
  const lookup = lookupOf(planning.items, new Map(), repositoryNames);
  const activeFilters = activeFilterCount(filter);
  const openTask = async (id: string) => {
    if (!adapter) return;
    setTaskLoading(true);
    setTaskError("");
    await readTaskOwned(
      editorLifetime,
      async () => {
        const reply = await adapter.call(LaneService.method.getTask, { id });
        if (!reply.result)
          throw new Error(
            "Task is unavailable. It may have been deleted or access may have changed.",
          );
        return reply;
      },
      {
        opened: (result) => {
          setCurrent(result);
          setDraft(draftFor(result));
        },
        failed: (e) =>
          setTaskError(e instanceof Error ? e.message : "Task is unavailable."),
      },
    );
    setTaskLoading(false);
  };
  useEffect(() => {
    if (target.kind === TabKind.Task && target.taskId)
      void openTask(target.taskId);
    if (target.kind === TabKind.NewTask)
      setDraft({
        id: target.taskId!,
        task: blankTask(target.repositoryId ?? ""),
        dirty: false,
        external: false,
        deleted: false,
      });
  }, []);
  const dirty = !!draft?.dirty || planningDirty || settingsDirty;
  const busy = editorBusy || settingsBusy || pageBusy;
  const exitBusy = editorBusy || pageBusy || settingsExitBusy;
  const openedRepository =
    target.kind === TabKind.Repository
      ? repositories.find(
          (result) => result.version?.entityId === target.repositoryId,
        )?.version
      : undefined;
  const title =
    draft?.task.title ||
    (target.kind === TabKind.NewTask
      ? "New task"
      : openedRepository
        ? repositoryName(openedRepository)
        : target.label);
  useEffect(() => {
    onReport({
      id: tabId,
      title,
      dirty,
      busy,
      exitBusy,
      repositories,
      repositoryStatus,
    });
  }, [
    tabId,
    title,
    dirty,
    busy,
    exitBusy,
    repositories,
    repositoryStatus,
    onReport,
  ]);
  const saved = (result?: EntityResult) => {
    if (result && taskOf(result.version)) {
      setCurrent(result);
      if (!conflicted(result)) setDraft(draftFor(result));
    }
    refresh();
  };
  // A mutation belongs to this retained editor even while its tab is hidden.
  // Closing the pane ends its ownership; late results only refresh shared reads.
  const beginCompletion = () =>
    beginEditorCompletion<EntityResult>(editorLifetime, {
      saved,
      refresh,
      close: () => onRemoved(tabId),
    });
  return (
    <view className="app-content lane-tab-content" data-tab-id={tabId}>
      <view className="navigation">
        {!mobile && (
          <view className="lane-brand">
            <AppIcon app={MiniApp.Lane} />
            <text className="brand">Lane</text>
          </view>
        )}
        <view className="nav-items">
          {["Tasks", "Proposals", "Repositories", "Events", "Settings"].map(
            (p) => (
              <Button
                key={p}
                primary={page === p}
                onClick={() =>
                  onOpen({
                    kind: TabKind.Page,
                    page: p as LanePage,
                    label: p === "Tasks" ? "All tasks" : p,
                  })
                }
              >
                {mobile
                  ? p === "Repositories"
                    ? "Repos"
                    : p
                  : p === "Proposals"
                    ? `Proposals · ${pendingCount}`
                    : p === "Tasks"
                      ? "All tasks"
                      : p}
              </Button>
            ),
          )}
        </view>
        {!mobile && (
          <>
            <text className="label">Repositories</text>
            <scroll-view className="repo-nav" scroll-orientation="vertical">
              {repositories.map((r) => (
                <Button
                  key={r.version?.entityId}
                  primary={repository === r.version?.entityId}
                  onClick={() =>
                    onOpen({
                      kind: TabKind.Repository,
                      repositoryId: r.version!.entityId,
                      label: repositoryName(r.version),
                    })
                  }
                >
                  {repositoryName(r.version)}
                </Button>
              ))}
            </scroll-view>
            <text className="muted">
              {adapter?.mode === "web"
                ? "Online account required"
                : "Local data · optional synchronization"}
            </text>
          </>
        )}
      </view>
      <view
        id={active ? "lane-workspace" : `workspace-${tabId}`}
        className="workspace"
        focusable
        accessibility-label="Lane workspace"
      >
        {error && (
          <Notice>
            {error}
            <Button
              onClick={() =>
                taskError && target.taskId
                  ? void openTask(target.taskId)
                  : refresh()
              }
            >
              Retry
            </Button>
          </Notice>
        )}
        {taskLoading && <text>Loading…</text>}
        {adapter && (page === "Tasks" || page === "Proposals") && (
          <view className="task-workspace">
            {(!mobile || !draft) && (
              <view
                className={`task-list ${page === "Proposals" ? "proposal-list" : ""}`}
              >
                <scroll-view
                  className="rows"
                  scroll-orientation="vertical"
                  scroll-top={active ? taskScrollTop.current : undefined}
                  bindscroll={(event) => {
                    if (active) taskScrollTop.current = event.detail.scrollTop;
                  }}
                >
                  <view className="task-list-content">
                    <view className="row between">
                      <view>
                        <text className="heading">
                          {repository
                            ? repositoryName(
                                repositories.find(
                                  (r) => r.version?.entityId === repository,
                                )?.version,
                              )
                            : page === "Proposals"
                              ? "Proposals"
                              : "All tasks"}
                        </text>
                        <text className="muted">
                          Manage your tasks and prompts.
                        </text>
                      </view>
                      <Button
                        primary
                        onClick={() =>
                          onOpen({
                            kind: TabKind.NewTask,
                            taskId: newId(),
                            repositoryId: repository,
                            label: "New task",
                          })
                        }
                      >
                        + New task
                      </Button>
                    </view>
                    <Field
                      label="Search title or description"
                      value={search}
                      onChange={setSearch}
                      focus={active && searchFocusPending.current}
                    />
                    {mobile && (
                      <Choices
                        label="Repository"
                        value={repository}
                        options={[
                          ["", "All repositories"],
                          ...repositories.map(
                            (r) =>
                              [
                                r.version!.entityId,
                                repositoryName(r.version),
                              ] as const,
                          ),
                        ]}
                        onChange={(id) => {
                          if (target.kind !== TabKind.Repository) {
                            setRepository(id);
                            return;
                          }
                          if (!id) {
                            onOpen({
                              kind: TabKind.Page,
                              page: LanePage.Tasks,
                              label: "All tasks",
                            });
                            return;
                          }
                          const destination = repositories.find(
                            (r) => r.version?.entityId === id,
                          )?.version;
                          onOpen({
                            kind: TabKind.Repository,
                            repositoryId: id,
                            label: destination
                              ? repositoryName(destination)
                              : "Repository",
                          });
                        }}
                      />
                    )}
                    <view
                      className={
                        page === "Proposals" ? "proposal-controls" : undefined
                      }
                    >
                      {page === "Proposals" ? (
                        <>
                          <MenuChoices
                            label="Proposal disposition"
                            value={proposal}
                            options={[
                              [Proposal.UNSPECIFIED, "Pending proposals"],
                              ...proposalOptions,
                            ]}
                            onChange={setProposal}
                          />
                          <MenuChoices
                            label="Progress"
                            value={progress}
                            options={[
                              [Progress.UNSPECIFIED, "All"],
                              ...progressOptions,
                            ]}
                            onChange={setProgress}
                          />
                        </>
                      ) : (
                        <>
                          <Choices
                            label="Progress filter"
                            value={progress}
                            options={[
                              [Progress.UNSPECIFIED, "All"],
                              ...progressOptions,
                            ]}
                            onChange={setProgress}
                          />
                          <Choices
                            label="Proposal filter"
                            value={proposal}
                            options={[
                              [Proposal.UNSPECIFIED, "All"],
                              ...proposalOptions,
                            ]}
                            onChange={setProposal}
                          />
                        </>
                      )}
                    </view>
                    <TaskFilters
                      open={filtersOpen}
                      setOpen={setFiltersOpen}
                      filter={filter}
                      setFilter={setFilter}
                      errors={filterProblems}
                      planning={planning}
                      repositoryId={repository}
                      repositoryNames={repositoryNames}
                      sortKey={sortKey}
                      setSortKey={setSortKey}
                      sortDirection={sortDirection}
                      setSortDirection={setSortDirection}
                      separateSort={page === "Proposals"}
                    />
                    {page === "Proposals" && (
                      <TaskSortControls
                        compact
                        sortKey={sortKey}
                        setSortKey={setSortKey}
                        sortDirection={sortDirection}
                        setSortDirection={setSortDirection}
                      />
                    )}
                    <view
                      className="task-results"
                      key={
                        loading
                          ? "loading"
                          : tasks.length === 0
                            ? "empty"
                            : "populated"
                      }
                    >
                      {page === "Proposals" && (
                        <text
                          className="result-announcement"
                          accessibility-live-region="polite"
                        >
                          {filterInvalid
                            ? "Fix filter values to update proposals."
                            : loading
                              ? "Loading proposals."
                              : listError
                                ? "Proposal list error."
                                : "Proposals updated."}
                        </text>
                      )}
                      {filterInvalid ? (
                        <Notice>
                          Fix the highlighted filter values to update the list.
                          Nothing was sent.
                        </Notice>
                      ) : loading ? (
                        <text>Loading…</text>
                      ) : listError && tasks.length === 0 ? (
                        <view className="empty">
                          <text className="heading">
                            {page === "Proposals"
                              ? "Proposals could not be loaded"
                              : "Tasks could not be loaded"}
                          </text>
                          <text>{listError}</text>
                          <Button primary onClick={refresh}>
                            Retry loading tasks
                          </Button>
                        </view>
                      ) : tasks.length === 0 ? (
                        <view className="empty">
                          <text className="heading">
                            {page === "Proposals"
                              ? activeFilters || search || progress
                                ? "No proposals match"
                                : "No proposals here yet"
                              : activeFilters || search
                                ? "No tasks match"
                                : "No tasks here yet"}
                          </text>
                          <text>
                            {page === "Proposals"
                              ? activeFilters || search || progress
                                ? "Change the search or filters to find proposals."
                                : "There are no proposals for this disposition. Change the filters to see other tasks."
                              : activeFilters || search
                                ? "No task matches the search and filters. Change them or clear the filters."
                                : "Create a task with a title, or change your filters."}
                          </text>
                          {activeFilters > 0 && (
                            <Button onClick={() => setFilter(emptyFilter())}>
                              Clear filters
                            </Button>
                          )}
                        </view>
                      ) : (
                        tasks.map((r) => {
                          const t = taskOf(r.version);
                          return (
                            t && (
                              <view
                                className={`task-row ${page === "Proposals" ? "proposal-row" : ""} ${draft?.id === r.version?.entityId ? "selected" : ""}`}
                                key={r.version?.entityId}
                              >
                                {page === "Proposals" ? (
                                  <>
                                    <view className="proposal-row-main">
                                      <Button
                                        onClick={() =>
                                          onOpen({
                                            kind: TabKind.Task,
                                            taskId: r.version!.entityId,
                                            repositoryId: repository,
                                            label: t.title,
                                          })
                                        }
                                      >
                                        {t.title}
                                      </Button>
                                      {conflicted(r) && (
                                        <text className="badge">Conflict</text>
                                      )}
                                      <TaskMetadataLine
                                        task={t}
                                        lookup={lookup}
                                        qualify={!repository}
                                      />
                                    </view>
                                    <view className="proposal-row-states">
                                      <text className="proposal-disposition">
                                        {proposalOptions.find(
                                          ([value]) => value === t.proposal,
                                        )?.[1] ?? "Unknown disposition"}
                                      </text>
                                      <text className="proposal-progress">
                                        <text accessibility-elements-hidden>
                                          {progressSymbol(t.progress)}
                                        </text>{" "}
                                        {progressOptions.find(
                                          ([value]) => value === t.progress,
                                        )?.[1] ?? "Unknown progress"}
                                      </text>
                                    </view>
                                  </>
                                ) : (
                                  <>
                                    <Button
                                      onClick={() =>
                                        onOpen({
                                          kind: TabKind.Task,
                                          taskId: r.version!.entityId,
                                          repositoryId: repository,
                                          label: t.title,
                                        })
                                      }
                                    >
                                      {t.title}
                                    </Button>
                                    <text className="badge">
                                      {
                                        progressOptions.find(
                                          ([v]) => v === t.progress,
                                        )?.[1]
                                      }
                                    </text>
                                    {t.proposal === Proposal.PENDING && (
                                      <text className="badge">Proposal</text>
                                    )}
                                    {conflicted(r) && (
                                      <text className="badge">Conflict</text>
                                    )}
                                    <TaskMetadataLine
                                      task={t}
                                      lookup={lookup}
                                      qualify={!repository}
                                    />
                                  </>
                                )}
                              </view>
                            )
                          );
                        })
                      )}
                      {/* A failed continuation keeps the loaded rows and shows its error here; the first page failing still uses the full error view above. */}
                      {listError && tasks.length > 0 && (
                        <Notice>{listError}</Notice>
                      )}
                      {!loading && tasks.length > 0 && cursor && (
                        <Button
                          disabled={loadingMore}
                          onClick={() => {
                            // Reserve synchronously: repeated taps or
                            // Enter/Space presses can arrive before
                            // React re-renders the disabled state.
                            if (continuation.current || !cursorRef.current)
                              return;
                            const owner = {};
                            const generation = queryGeneration.current;
                            continuation.current = owner;
                            setLoadingMore(true);
                            setListError("");
                            const owns = () =>
                              continuation.current === owner &&
                              generation === queryGeneration.current;
                            void adapter
                              .call(
                                LaneService.method.listTasks,
                                listRequest(cursorRef.current),
                              )
                              .then(
                                (r) => {
                                  if (owns()) {
                                    setTasks((t) => [...t, ...r.items]);
                                    setCursor(r.nextCursor);
                                  }
                                },
                                (e) => {
                                  // An obsolete query's failure must not replace current results.
                                  if (owns()) setListError(describeError(e));
                                },
                              )
                              .finally(() => {
                                // Only the owner releases the lock, on success and failure.
                                if (continuation.current === owner) {
                                  continuation.current = undefined;
                                  setLoadingMore(false);
                                }
                              });
                          }}
                        >
                          Load more tasks
                        </Button>
                      )}
                    </view>
                  </view>
                </scroll-view>
              </view>
            )}
            {draft && (
              <TaskEditor
                key={draft.id}
                adapter={adapter}
                draft={draft}
                setDraft={setDraft}
                current={current}
                repositories={repositories}
                planning={planning}
                revision={revision}
                openTask={(id) =>
                  onOpen({
                    kind: TabKind.Task,
                    taskId: id,
                    repositoryId: repository,
                    label: "Task",
                  })
                }
                beginCompletion={beginCompletion}
                busyChanged={setEditorBusy}
                onClose={() => onCloseTab(tabId)}
                onBack={() =>
                  onOpen({
                    kind: repository ? TabKind.Repository : TabKind.Page,
                    page: LanePage.Tasks,
                    repositoryId: repository || undefined,
                    label: repository
                      ? repositoryName(
                          repositories.find(
                            (r) => r.version?.entityId === repository,
                          )?.version,
                        )
                      : "All tasks",
                  })
                }
              />
            )}
          </view>
        )}
        {adapter && page === "Repositories" && (
          <scroll-view className="page-scroll" scroll-orientation="vertical">
            <Repositories
              dirty={setPlanningDirty}
              busyChanged={setPageBusy}
              adapter={adapter}
              items={repositories}
              status={repositoryStatus}
              loadError={repositoryError}
              planning={planning}
              refresh={refresh}
              select={(id) => {
                onOpen({
                  kind: TabKind.Repository,
                  repositoryId: id,
                  label: repositoryName(
                    repositories.find((r) => r.version?.entityId === id)
                      ?.version,
                  ),
                });
              }}
            />
          </scroll-view>
        )}
        {adapter && page === "Events" && (
          <scroll-view className="page-scroll" scroll-orientation="vertical">
            <Events
              adapter={adapter}
              repositories={repositories}
              revision={revision}
              active={active}
              stateChanged={(dirty, busy) => {
                setPlanningDirty(dirty);
                setPageBusy(busy);
              }}
              openTask={(id) => {
                onOpen({ kind: TabKind.Task, taskId: id, label: "Task" });
              }}
            />
          </scroll-view>
        )}
        {adapter && page === "Settings" && (
          <scroll-view
            id="settings-scroll"
            className="page-scroll"
            scroll-orientation="vertical"
          >
            <Settings
              adapter={adapter}
              target={settingsTarget}
              stateChanged={(dirty, busy, exitBusy) => {
                setSettingsDirty(dirty);
                setSettingsBusy(busy);
                setSettingsExitBusy(exitBusy);
              }}
              active={active}
              beforeDataChange={beforeDataChange}
              onReset={onReset}
            />
          </scroll-view>
        )}
      </view>
    </view>
  );
}

function progressSymbol(progress: Progress): string {
  switch (progress) {
    case Progress.TODO:
      return "○";
    case Progress.IN_PROGRESS:
      return "◐";
    case Progress.BLOCKED:
      return "!";
    case Progress.DONE:
      return "✓";
    case Progress.CANCELED:
      return "×";
    default:
      return "?";
  }
}

// Compact planning metadata under a row; names are always text, color never alone.
function TaskMetadataLine({
  task,
  lookup,
  qualify,
}: {
  task: NonNullable<ReturnType<typeof taskOf>>;
  lookup: ReturnType<typeof lookupOf>;
  qualify: boolean;
}) {
  const meta = taskMetadata(task, lookup, qualify);
  const parts = [...meta.summary, ...meta.relationships];
  if (!parts.length && !meta.labels.length) return null;
  return (
    <view className="task-meta">
      {parts.length > 0 && <text className="muted">{parts.join(" · ")}</text>}
      {meta.labels.length > 0 && (
        <view className="row wrap">
          {meta.labels.map((name, index) => (
            <text className="badge" key={`${name}-${index}`}>
              {name}
            </text>
          ))}
        </view>
      )}
    </view>
  );
}

type EntityResult = UI<ProtoEntityResult>;
