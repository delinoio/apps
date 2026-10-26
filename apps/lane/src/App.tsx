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
import { getAdapter, listenHost, type OpenRoute } from "./runtime";
import { Button, Choices, Field, Notice, Dialog } from "./components";
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
import { TaskFilters } from "./TaskFilters";
import { usePlanning } from "./usePlanning";
import { StoreReset } from "./StoreReset";
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
import {
  AppIcon,
  AppRail,
  ComingSoon,
  MiniApp,
  MiniAppNavigation,
  SettingsSection,
} from "./AppNavigation";
import { RealQA, RealQADialogs } from "./RealQA";
import { CaptureComposer } from "./CaptureComposer";
import { useRealQA } from "./useRealQA";
import { savedCaptureRoute } from "./captures-model";
import { CommandPalette } from "./CommandPalette";
import {
  destinations,
  DestinationKind,
  LanePage,
  type Destination,
} from "./navigation";
import "./style.css";
export function App({ providedAdapter }: { providedAdapter?: LaneAdapter }) {
  const realqa = useRealQA();
  const [realqaDirty, setRealqaDirty] = useState(false);
  // Unsaved label, milestone or cycle input in the repository planning manager.
  const [planningDirty, setPlanningDirty] = useState(false);
  const [planningEpoch, setPlanningEpoch] = useState(0);
  const [miniApp, setMiniApp] = useState(MiniApp.Lane);
  const previousApp = useRef(MiniApp.Lane);
  const [pendingRoute, setPendingRoute] = useState<OpenRoute>();
  const [adapter, setAdapter] = useState<LaneAdapter>();
  const [resetAdapter, setResetAdapter] = useState<LaneAdapter>();
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [page, setPage] = useState("Tasks");
  const [settingsTarget, setSettingsTarget] = useState<{
    section: SettingsSection;
    request: number;
  }>();
  const [repository, setRepository] = useState("");
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
  const [progress, setProgress] = useState<Progress>(Progress.UNSPECIFIED);
  const [proposal, setProposal] = useState<Proposal>(Proposal.UNSPECIFIED);
  const [filter, setFilter] = useState<FilterState>(emptyFilter);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [sortKey, setSortKey] = useState<TaskSortKey>(TaskSortKey.UNSPECIFIED);
  const [sortDirection, setSortDirection] = useState<SortDirection>(
    SortDirection.ASCENDING,
  );
  const [listError, setListError] = useState("");
  const [draft, setDraft] = useState<Draft>();
  const [current, setCurrent] = useState<EntityResult>();
  const [spaceRevision, setSpaceRevision] = useState(0);
  const [revision, setRevision] = useState(0);
  const [mobile, setMobile] = useState(false);
  const [discard, setDiscard] = useState<() => void>();
  const [pendingCount, setPendingCount] = useState(0);
  const queryGeneration = useRef(0);
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
    adapter ? `${adapter.spaceId ?? ""}#${spaceRevision}` : "",
  );
  const currentDraft = useRef(draft);
  currentDraft.current = draft;
  const refresh = () => setRevision((v) => v + 1);
  useEffect(() => {
    "background only";
    if (previousApp.current === miniApp) return;
    previousApp.current = miniApp;
    if (adapter?.mode !== "web") {
      lynx
        .createSelectorQuery()
        .select("#lane-workspace")
        .invoke({ method: "setFocus", params: { focus: true } })
        .exec();
    }
  }, [miniApp, adapter]);
  useEffect(() => {
    "background only";
    let active = true;
    let owned: LaneAdapter | undefined;
    const stopHost = listenHost(setPendingRoute, setError, () => {
      setDraft(undefined);
      setCurrent(undefined);
      setTasks([]);
      setRepositories([]);
      setAdapter(undefined);
    });
    let unsubscribe: (() => void) | undefined;
    setLoading(true);
    setError("");
    void (async () => {
      try {
        owned = providedAdapter || getAdapter();
        unsubscribe = owned.subscribe(refresh);
        await owned.open();
        if (!active) {
          owned.close();
          return;
        }
        setAdapter(owned);
      } catch (e) {
        // An unsupported local format offers only the explicit, typed-confirmation reset.
        if (
          active &&
          owned?.resetUnopened &&
          /UNSUPPORTED_FORMAT|unsupported format/i.test(
            `${(e as { detail?: { reason?: string } })?.detail?.reason ?? ""} ${e instanceof Error ? e.message : ""}`,
          )
        )
          setResetAdapter(owned);
        if (active)
          setError(e instanceof Error ? e.message : "Lane could not open.");
      } finally {
        if (active) setLoading(false);
      }
    })();
    return () => {
      active = false;
      stopHost();
      unsubscribe?.();
      owned?.close();
    };
  }, [providedAdapter]);
  useEffect(() => {
    if (!adapter) return;
    let active = true;
    setRepositoryStatus("Loading repositories...");
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
          if (!repositoriesLoaded)
            setRepositoryStatus("Repositories could not be refreshed.");
          setError(e instanceof Error ? e.message : "Refresh failed.");
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
    if (!adapter || miniApp !== MiniApp.Lane) return;
    const generation = advanceQueryGeneration();
    setError("");
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
    miniApp,
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
  const navigate = (fn: () => void) => {
    const proceed = () => {
      if (planningDirty) {
        // Remount the manager so confirmed discards also apply when the page stays the same.
        setPlanningDirty(false);
        setPlanningEpoch((v) => v + 1);
      }
      fn();
    };
    if (draft?.dirty || realqaDirty || planningDirty) setDiscard(() => proceed);
    else fn();
  };
  const selectApp = (app: MiniApp) => {
    if (app === miniApp) return;
    navigate(() => {
      if (draft?.dirty) {
        setDraft(undefined);
        setCurrent(undefined);
      }
      setRealqaDirty(false);
      setMiniApp(app);
    });
  };
  const openSettings = (section: SettingsSection) => {
    navigate(() => {
      setDraft(undefined);
      setCurrent(undefined);
      setRealqaDirty(false);
      setMiniApp(MiniApp.Lane);
      setPage("Settings");
      setSettingsTarget((previous) => ({
        section,
        request: (previous?.request ?? 0) + 1,
      }));
    });
  };
  const selectDestination = (destination: Destination) => {
    if (destination.kind === DestinationKind.App) {
      selectApp(destination.target as MiniApp);
      return;
    }
    navigate(() => {
      setDraft(undefined);
      setCurrent(undefined);
      setRealqaDirty(false);
      setMiniApp(MiniApp.Lane);
      setSettingsTarget(undefined);
      if (destination.kind === DestinationKind.Repository) {
        setRepository(destination.target);
        setPage(LanePage.Tasks);
      } else {
        setPage(destination.target);
        // All tasks is the unfiltered repository destination; other filters remain.
        setRepository("");
      }
    });
  };
  const openTask = async (id: string) => {
    if (!adapter) return;
    try {
      const r = await adapter.call(LaneService.method.getTask, { id });
      if (r.result) {
        setCurrent(r.result);
        setDraft(draftFor(r.result));
        setPage("Tasks");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Task is unavailable.");
    }
  };
  useEffect(() => {
    if (!pendingRoute || !adapter) return;
    const route = pendingRoute;
    setPendingRoute(undefined);
    navigate(() => {
      setMiniApp(MiniApp.Lane);
      void (async () => {
        try {
          if (route.spaceId) {
            advanceQueryGeneration();
            setTasks([]);
            setRepositories([]);
            setDraft(undefined);
            setCurrent(undefined);
            await adapter.open(route.spaceId);
            setSpaceRevision((v) => v + 1);
            setRepository("");
            setPage("Tasks");
            refresh();
          }
          if (route.kind === "task" && route.id) await openTask(route.id);
          else if (route.kind === "repository" && route.id) {
            setRepository(route.id);
            setPage("Tasks");
            setDraft(undefined);
          }
        } catch (e) {
          setError(
            e instanceof Error
              ? e.message
              : "Could not open the requested data space.",
          );
        }
      })();
    });
  }, [pendingRoute, adapter]);
  const saved = (result?: EntityResult) => {
    if (result && taskOf(result.version)) {
      setCurrent(result);
      if (!conflicted(result)) setDraft(draftFor(result));
    }
    refresh();
  };
  if (realqa.state?.window === "composer")
    return (
      <CaptureComposer
        controller={realqa}
        adapter={adapter}
        repositories={repositories}
      />
    );
  return (
    <view
      id="lane-root"
      data-app={miniApp}
      className={mobile ? "app mobile" : "app desktop"}
      bindlayoutchange={(e) => setMobile(e.detail.width < 900)}
    >
      <view className="shell">
        <AppRail
          selected={miniApp}
          select={selectApp}
          openSettings={openSettings}
        />
        <view className="app-content">
          {miniApp === MiniApp.Lane ? (
            <view className="navigation">
              {!mobile && (
                <view className="lane-brand">
                  <AppIcon app={MiniApp.Lane} />
                  <text className="brand">Lane</text>
                </view>
              )}
              <view className="nav-items">
                {[
                  "Tasks",
                  "Proposals",
                  "Repositories",
                  "Events",
                  "Settings",
                ].map((p) => (
                  <Button
                    key={p}
                    primary={page === p}
                    onClick={() =>
                      navigate(() => {
                        setSettingsTarget(undefined);
                        setPage(p);
                        setDraft(undefined);
                        setCurrent(undefined);
                        if (p !== "Tasks") setRepository("");
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
                ))}
              </view>
              {!mobile && (
                <>
                  <text className="label">Repositories</text>
                  <scroll-view
                    className="repo-nav"
                    scroll-orientation="vertical"
                  >
                    {repositories.map((r) => (
                      <Button
                        key={r.version?.entityId}
                        primary={repository === r.version?.entityId}
                        onClick={() =>
                          navigate(() => {
                            setRepository(r.version!.entityId);
                            setPage("Tasks");
                            setDraft(undefined);
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
          ) : (
            <MiniAppNavigation app={miniApp} />
          )}
          <view
            id="lane-workspace"
            className="workspace"
            focusable
            accessibility-label={`${miniApp} workspace`}
          >
            {miniApp !== MiniApp.Lane ? (
              miniApp === MiniApp.RealQA ? (
                <RealQA
                  controller={realqa}
                  adapter={adapter}
                  repositories={repositories}
                  back={() => selectApp(MiniApp.Lane)}
                  dirty={setRealqaDirty}
                />
              ) : (
                <ComingSoon
                  app={miniApp}
                  back={() => selectApp(MiniApp.Lane)}
                />
              )
            ) : (
              <>
                {error && !resetAdapter && (
                  <Notice>
                    {error} <Button onClick={refresh}>Retry loading</Button>
                  </Notice>
                )}
                {resetAdapter?.resetUnopened && !adapter && (
                  <StoreReset
                    message={error}
                    reset={() => resetAdapter.resetUnopened!()}
                    ready={() => {
                      const owned = resetAdapter;
                      setResetAdapter(undefined);
                      setError("");
                      setLoading(true);
                      void owned.open().then(
                        () => {
                          setAdapter(owned);
                          setLoading(false);
                        },
                        (e) => {
                          setError(
                            e instanceof Error
                              ? e.message
                              : "Lane could not open.",
                          );
                          setLoading(false);
                        },
                      );
                    }}
                  />
                )}
                {loading && !adapter && <text>Opening Lane…</text>}
                {adapter && (page === "Tasks" || page === "Proposals") && (
                  <view className="task-workspace">
                    {(!mobile || !draft) && (
                      <view className="task-list">
                        <scroll-view
                          className="rows"
                          scroll-orientation="vertical"
                        >
                          <view className="task-list-content">
                            <view className="row between">
                              <view>
                                <text className="heading">
                                  {repository
                                    ? repositoryName(
                                        repositories.find(
                                          (r) =>
                                            r.version?.entityId === repository,
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
                                  navigate(() => {
                                    setCurrent(undefined);
                                    setDraft({
                                      id: newId(),
                                      task: blankTask(repository),
                                      dirty: false,
                                      external: false,
                                      deleted: false,
                                    });
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
                                onChange={setRepository}
                              />
                            )}
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
                                [
                                  Proposal.UNSPECIFIED,
                                  page === "Proposals"
                                    ? "Pending proposals"
                                    : "All",
                                ],
                                ...proposalOptions,
                              ]}
                              onChange={setProposal}
                            />
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
                            />
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
                              {filterInvalid ? (
                                <Notice>
                                  Fix the highlighted filter values to update
                                  the list. Nothing was sent.
                                </Notice>
                              ) : loading ? (
                                <text>Loading…</text>
                              ) : listError && tasks.length === 0 ? (
                                <view className="empty">
                                  <text className="heading">
                                    Tasks could not be loaded
                                  </text>
                                  <text>{listError}</text>
                                  <Button primary onClick={refresh}>
                                    Retry loading tasks
                                  </Button>
                                </view>
                              ) : tasks.length === 0 ? (
                                <view className="empty">
                                  <text className="heading">
                                    {activeFilters || search
                                      ? "No tasks match"
                                      : "No tasks here yet"}
                                  </text>
                                  <text>
                                    {activeFilters || search
                                      ? "No task matches the search and filters. Change them or clear the filters."
                                      : "Create a task with a title, or change your filters."}
                                  </text>
                                  {activeFilters > 0 && (
                                    <Button
                                      onClick={() => setFilter(emptyFilter())}
                                    >
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
                                        className={`task-row ${draft?.id === r.version?.entityId ? "selected" : ""}`}
                                        key={r.version?.entityId}
                                      >
                                        <Button
                                          onClick={() =>
                                            navigate(() => {
                                              void openTask(
                                                r.version!.entityId,
                                              );
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
                                          <text className="badge">
                                            Proposal
                                          </text>
                                        )}
                                        {conflicted(r) && (
                                          <text className="badge">
                                            Conflict
                                          </text>
                                        )}
                                        <TaskMetadataLine
                                          task={t}
                                          lookup={lookup}
                                          qualify={!repository}
                                        />
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
                                    if (
                                      continuation.current ||
                                      !cursorRef.current
                                    )
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
                                          if (owns())
                                            setListError(describeError(e));
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
                          navigate(() => {
                            void openTask(id);
                          })
                        }
                        onSaved={saved}
                        onClose={() =>
                          navigate(() => {
                            setDraft(undefined);
                            setCurrent(undefined);
                          })
                        }
                      />
                    )}
                  </view>
                )}
                {adapter && page === "Repositories" && (
                  <scroll-view
                    className="page-scroll"
                    scroll-orientation="vertical"
                  >
                    <Repositories
                      key={planningEpoch}
                      dirty={setPlanningDirty}
                      adapter={adapter}
                      items={repositories}
                      planning={planning}
                      refresh={refresh}
                      select={(id) => {
                        setRepository(id);
                        setPage("Tasks");
                      }}
                    />
                  </scroll-view>
                )}
                {adapter && page === "Events" && (
                  <scroll-view
                    className="page-scroll"
                    scroll-orientation="vertical"
                  >
                    <Events
                      adapter={adapter}
                      repositories={repositories}
                      openTask={(id) => {
                        void openTask(id);
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
                      key={spaceRevision}
                      adapter={adapter}
                      target={settingsTarget}
                      onReset={() => {
                        setDraft(undefined);
                        setTasks([]);
                        setRepositories([]);
                        void adapter.open().then(
                          () => {
                            setSpaceRevision((v) => v + 1);
                            refresh();
                          },
                          (e) => setError(e.message),
                        );
                      }}
                    />
                  </scroll-view>
                )}
              </>
            )}
          </view>
        </view>
      </view>
      <CommandPalette
        items={destinations(
          repositories.flatMap((result) =>
            result.version?.body.case === "repository"
              ? [
                  {
                    id: result.version.entityId,
                    name: repositoryName(result.version),
                    url: result.version.body.value.normalizedUrl,
                  },
                ]
              : [],
          ),
        )}
        repositoryStatus={repositoryStatus}
        onSelect={selectDestination}
      />
      <RealQADialogs
        controller={realqa}
        onOpenSaved={(saved) => setPendingRoute(savedCaptureRoute(saved))}
      />
      {discard && (
        <Dialog
          title="Discard unsaved changes?"
          onClose={() => setDiscard(undefined)}
        >
          <Notice>Your edits have not been saved.</Notice>
          <Button
            onClick={() => {
              const action = discard;
              setDiscard(undefined);
              action();
            }}
          >
            Discard changes
          </Button>
          <Button primary onClick={() => setDiscard(undefined)}>
            Keep editing
          </Button>
        </Dialog>
      )}
    </view>
  );
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
