// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  LaneService,
  Progress,
  Proposal,
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
import { StoreMigration } from "./StoreMigration";
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
  const [migrationAdapter, setMigrationAdapter] = useState<LaneAdapter>();
  const [miniApp, setMiniApp] = useState(MiniApp.Lane);
  const previousApp = useRef(MiniApp.Lane);
  const [pendingRoute, setPendingRoute] = useState<OpenRoute>();
  const [adapter, setAdapter] = useState<LaneAdapter>();
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
  const [cursor, setCursor] = useState("");
  const [search, setSearch] = useState("");
  const [progress, setProgress] = useState<Progress>(Progress.UNSPECIFIED);
  const [proposal, setProposal] = useState<Proposal>(Proposal.UNSPECIFIED);
  const [draft, setDraft] = useState<Draft>();
  const [current, setCurrent] = useState<EntityResult>();
  const [spaceRevision, setSpaceRevision] = useState(0);
  const [revision, setRevision] = useState(0);
  const [mobile, setMobile] = useState(false);
  const [discard, setDiscard] = useState<() => void>();
  const [pendingCount, setPendingCount] = useState(0);
  const queryGeneration = useRef(0);
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
        if (
          active &&
          owned?.migrate &&
          /format|schema|migration|version/i.test(
            e instanceof Error ? e.message : "",
          )
        )
          setMigrationAdapter(owned);
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
  useEffect(() => {
    if (!adapter || miniApp !== MiniApp.Lane) return;
    const generation = ++queryGeneration.current;
    setLoading(true);
    setError("");
    void adapter
      .call(LaneService.method.listTasks, {
        page: { size: 100 },
        repositoryId: repository || undefined,
        progress: progress ? [progress] : [],
        proposal:
          page === "Proposals"
            ? [proposal || Proposal.PENDING]
            : proposal
              ? [proposal]
              : [],
        search,
      })
      .then(
        (r) => {
          if (generation === queryGeneration.current) {
            setTasks(r.items);
            setCursor(r.nextCursor);
          }
        },
        (e) => {
          if (generation === queryGeneration.current)
            setError(e instanceof Error ? e.message : "Could not load tasks.");
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
  ]);
  const navigate = (fn: () => void) => {
    if (draft?.dirty || realqaDirty) setDiscard(() => fn);
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
            ++queryGeneration.current;
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
                {error && (
                  <Notice>
                    {error} <Button onClick={refresh}>Retry loading</Button>
                  </Notice>
                )}
                {migrationAdapter && (
                  <StoreMigration
                    adapter={migrationAdapter}
                    ready={() => {
                      setAdapter(migrationAdapter);
                      setMigrationAdapter(undefined);
                      setError("");
                      refresh();
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
                              {loading ? (
                                <text>Loading…</text>
                              ) : tasks.length === 0 ? (
                                <view className="empty">
                                  <text className="heading">
                                    No tasks here yet
                                  </text>
                                  <text>
                                    Create a task with a title, or change your
                                    filters.
                                  </text>
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
                                      </view>
                                    )
                                  );
                                })
                              )}
                              {!loading && tasks.length > 0 && cursor && (
                                <Button
                                  onClick={() => {
                                    const generation = queryGeneration.current;
                                    void adapter
                                      .call(LaneService.method.listTasks, {
                                        page: { size: 100, cursor },
                                        repositoryId: repository || undefined,
                                        progress: progress ? [progress] : [],
                                        proposal:
                                          page === "Proposals"
                                            ? [proposal || Proposal.PENDING]
                                            : proposal
                                              ? [proposal]
                                              : [],
                                        search,
                                      })
                                      .then(
                                        (r) => {
                                          if (
                                            generation ===
                                            queryGeneration.current
                                          ) {
                                            setTasks((t) => [...t, ...r.items]);
                                            setCursor(r.nextCursor);
                                          }
                                        },
                                        (e) => setError(String(e)),
                                      );
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
                      adapter={adapter}
                      items={repositories}
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

type EntityResult = UI<ProtoEntityResult>;
