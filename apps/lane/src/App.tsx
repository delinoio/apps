// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useRef, useState } from "@lynx-js/react";
import type { LaneAdapter } from "./adapter";
import { CompletedSelection } from "@repo/protos/lane/v1/lane_pb";
import { getAdapter, listenHost, type OpenRoute } from "./runtime";
import {
  NewTabPage,
  ComingSoon,
  MiniApp,
  MiniAppNavigation,
  SettingsSection,
} from "./AppNavigation";
import { Button, Dialog, Notice, hasOpenDialog } from "./components";
import { StoreReset } from "./StoreReset";
import { RealQA, RealQADialogs } from "./RealQA";
import { CaptureComposer } from "./CaptureComposer";
import { useRealQA } from "./useRealQA";
import { savedCaptureRoute } from "./captures-model";
import { CommandPalette, hasOpenCommandPalette } from "./CommandPalette";
import {
  destinations,
  DestinationKind,
  LanePage,
  type Destination,
} from "./navigation";
import { repositoryName } from "./model";
import { LaneWorkspace } from "./LaneWorkspace";
import { TaskLayout } from "./completedDisplay";
import { TabBar } from "./TabBar";
import {
  adjacentTab,
  allTasks,
  closedTab,
  insertRestoredTab,
  moveTab,
  recordClosedTab,
  restoredTarget,
  tabAtIndex,
  tabAtOffset,
  tabIdentity,
  TabCommand,
  TabKind,
  type ClosedTab,
  type TabCommandRequest,
  type TabTarget,
  type WorkspaceReport,
  type WorkspaceTab,
} from "./tabs";
import {
  reportWorkspaceState,
  reportWorkspaceRoute,
  listenWorkspaceExit,
  listenWorkspaceTabCommands,
} from "./workspace-host";
import { targetPath } from "./routes";
import { newId } from "./adapter";
import "./style.css";

const initialTab = (): WorkspaceTab => ({
  id: tabIdentity(allTasks()),
  target: allTasks(),
});
const initialTaskDisplay = {
  layout: TaskLayout.List,
  completed: CompletedSelection.NONE,
};
export function App({ providedAdapter }: { providedAdapter?: LaneAdapter }) {
  const realqa = useRealQA();
  const [realqaDirty, setRealqaDirty] = useState(false);
  const [adapter, setAdapter] = useState<LaneAdapter>();
  const [resetAdapter, setResetAdapter] = useState<LaneAdapter>();
  const [error, setError] = useState("");
  const [tabs, setTabs] = useState<WorkspaceTab[]>(() => [initialTab()]);
  const [taskDisplay, setTaskDisplay] = useState(initialTaskDisplay);
  const closedTabs = useRef<ClosedTab[]>([]);
  const [selected, setSelected] = useState(() => initialTab().id);
  const [reports, setReports] = useState<Record<string, WorkspaceReport>>({});
  const [epoch, setEpoch] = useState(0);
  const [mobile, setMobile] = useState(false);
  const cancelDiscard = useRef<(() => void) | undefined>(undefined);
  const [discard, setDiscard] = useState<() => void>();
  const pendingDiscard = useRef(false);
  const guardedRouteAction = useRef<(() => void) | undefined>(undefined);
  const [focusedRow, setFocusedRow] = useState<string>();
  const onRowFocus = (id: string, focused: boolean) =>
    setFocusedRow((current) =>
      focused ? id : current === id ? undefined : current,
    );
  const [openTabs, setOpenTabs] = useState(false);
  const lastLaneTab = useRef(initialTab().id);
  const activeTab = tabs.find((tab) => tab.id === selected);
  const miniApp =
    activeTab?.target.kind === TabKind.NewTab
      ? undefined
      : (activeTab?.target.app ?? MiniApp.Lane);
  const settingsRequest = useRef(0);
  const [settingsTarget, setSettingsTarget] = useState<{
    section: SettingsSection;
    request: number;
  }>();
  const [pendingRoute, setPendingRoute] = useState<OpenRoute>();
  const [routeRequest, setRouteRequest] = useState<number>();
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(() => setRevision((v) => v + 1), []);
  const [switching, setSwitching] = useState(false);
  const state = useRef({
    reports,
    realqaDirty,
    realqaBusy: realqa.busy,
    tabs,
    selected,
    miniApp,
    discard,
  });
  state.current = {
    reports,
    realqaDirty,
    realqaBusy: realqa.busy,
    tabs,
    selected,
    miniApp,
    discard,
  };
  const onReport = useCallback((report: WorkspaceReport) => {
    setReports((previous) => ({ ...previous, [report.id]: report }));
    if (report.savedTaskId)
      setTabs((previous) =>
        previous.map((tab) =>
          tab.id === report.id && tab.target.kind === TabKind.NewTask
            ? { ...tab, target: { ...tab.target, kind: TabKind.Task } }
            : tab,
        ),
      );
  }, []);
  const resetTabs = (preserveClosed = false) => {
    const first = initialTab();
    setTaskDisplay(initialTaskDisplay);
    guardedRouteAction.current = undefined;
    if (!preserveClosed) closedTabs.current = [];
    setTabs([first]);
    setSelected(first.id);
    lastLaneTab.current = first.id;
    setReports({});
    setEpoch((v) => v + 1);
    setOpenTabs(false);
    setSettingsTarget(undefined);
  };
  const guard = (
    action: () => void,
    all = false,
    id = selected,
    busyField: "busy" | "exitBusy" = "busy",
  ) => {
    const entries = all
      ? Object.values(state.current.reports)
      : [state.current.reports[id]];
    if (entries.some((entry) => entry?.[busyField])) {
      setError(
        "Wait for the current operation to finish before closing or changing data spaces.",
      );
      return;
    }
    if (
      entries.some((entry) => entry?.dirty) ||
      (all && state.current.realqaDirty)
    ) {
      if (pendingDiscard.current) return;
      pendingDiscard.current = true;
      setDiscard(() => action);
    } else action();
  };
  const beforeDataChange = () =>
    new Promise<boolean>((resolve) => {
      const entries = Object.values(state.current.reports);
      if (entries.some((entry) => entry.busy)) {
        setError(
          "Wait for the current operation to finish before changing data spaces.",
        );
        resolve(false);
        return;
      }
      if (!entries.some((entry) => entry.dirty) && !state.current.realqaDirty) {
        resolve(true);
        return;
      }
      cancelDiscard.current = () => resolve(false);
      pendingDiscard.current = true;
      setDiscard(() => () => {
        cancelDiscard.current = undefined;
        resolve(true);
      });
    });
  const dismissDiscard = () => {
    pendingDiscard.current = false;
    guardedRouteAction.current = undefined;
    setDiscard(undefined);
    cancelDiscard.current?.();
    cancelDiscard.current = undefined;
  };
  const leaveRealQA = (action: () => void) => {
    if (state.current.miniApp === MiniApp.RealQA && state.current.realqaDirty) {
      if (pendingDiscard.current) return;
      pendingDiscard.current = true;
      setDiscard(() => action);
    } else action();
  };
  const open = (
    target: TabTarget,
    section?: SettingsSection,
    requestId?: number,
  ) => {
    "background only";
    const id = tabIdentity(target);
    const replacing = state.current.selected;
    const existing = state.current.tabs.some((tab) => tab.id === id);
    const current = state.current.tabs.find((tab) => tab.id === replacing);
    // Explicit creation and mini-app switching retain their own panes. Ordinary
    // Lane navigation replaces the current pane only after its exit guard.
    const replaceCurrent =
      !existing &&
      target.kind !== TabKind.NewTab &&
      target.kind !== TabKind.NewTask &&
      !target.app &&
      !!current &&
      !current.target.app;
    const proceed = () => {
      if (requestId !== undefined) setRouteRequest(requestId);
      setTabs((previous) => {
        const withoutPicker = previous.filter(
          (tab) =>
            tab.id !== replacing ||
            (!replaceCurrent && tab.target.kind !== TabKind.NewTab),
        );
        return withoutPicker.some((tab) => tab.id === id)
          ? withoutPicker
          : [...withoutPicker, { id, target }];
      });
      if (replaceCurrent)
        setReports((previous) => {
          const copy = { ...previous };
          delete copy[replacing];
          return copy;
        });
      setSelected(id);
      if (target.page === LanePage.Settings)
        setSettingsTarget(
          section ? { section, request: ++settingsRequest.current } : undefined,
        );
      setRealqaDirty(false);
      if (!target.app && target.kind !== TabKind.NewTab)
        lastLaneTab.current = id;
      setOpenTabs(false);
    };
    const confirmNavigation = (action: () => void) =>
      leaveRealQA(() => {
        // Leaving Settings cancels its updater on unmount. Only business
        // mutations must block navigation, as they already block window exit.
        if (replaceCurrent) guard(action, false, replacing, "exitBusy");
        else action();
      });
    if (requestId === undefined) confirmNavigation(proceed);
    else {
      if (pendingDiscard.current && !guardedRouteAction.current) {
        // A different operation owns this confirmation. Reject navigation now
        // so it cannot acknowledge that operation's eventual destination.
        reportWorkspaceRoute(
          targetPath(current?.target ?? allTasks()),
          requestId,
        );
        return;
      }
      // History can move again while either exit guard is visible. Keep the
      // pending action paired with the latest target and acknowledgement ID.
      guardedRouteAction.current = proceed;
      if (pendingDiscard.current) return;
      confirmNavigation(() => {
        const action = guardedRouteAction.current;
        guardedRouteAction.current = undefined;
        action?.();
      });
      // A busy pane rejects navigation without showing a discard dialog.
      if (!pendingDiscard.current) guardedRouteAction.current = undefined;
    }
  };
  const selectTab = (id: string) => {
    if (id === state.current.selected) {
      setOpenTabs(false);
      return;
    }
    leaveRealQA(() => {
      const target = state.current.tabs.find((tab) => tab.id === id)?.target;
      if (target && !target.app && target.kind !== TabKind.NewTab)
        lastLaneTab.current = id;
      setRealqaDirty(false);
      setSelected(id);
      setOpenTabs(false);
    });
  };
  const selectApp = (app: MiniApp) => {
    if (app === state.current.miniApp && app !== MiniApp.Lane) return;
    if (app === MiniApp.Lane) {
      const previous = state.current.tabs.find(
        (tab) => tab.id === lastLaneTab.current,
      );
      open(previous?.target ?? allTasks());
    } else open({ kind: TabKind.App, app, label: app });
  };
  const createTab = () => open({ kind: TabKind.NewTab, label: "New tab" });
  const removeTab = (id: string, record = false) => {
    const currentTabs = state.current.tabs;
    const position = currentTabs.findIndex((tab) => tab.id === id);
    if (position < 0) return;
    if (id === tabIdentity(allTasks())) setTaskDisplay(initialTaskDisplay);
    if (record)
      closedTabs.current = recordClosedTab(
        closedTabs.current,
        closedTab(
          currentTabs[position],
          position,
          state.current.reports[id]?.savedTaskId,
        ),
      );
    const currentSelected = state.current.selected;
    const next = currentTabs.filter((tab) => tab.id !== id);
    const selectedNext =
      currentSelected === id ? adjacentTab(currentTabs, id) : currentSelected;
    if (!next.length) resetTabs(true);
    else {
      setTabs(next);
      setSelected(selectedNext!);
      const target = next.find((tab) => tab.id === selectedNext)?.target;
      if (target && !target.app && target.kind !== TabKind.NewTab)
        lastLaneTab.current = selectedNext!;
      setReports((previous) => {
        const copy = { ...previous };
        delete copy[id];
        return copy;
      });
    }
    setOpenTabs(false);
  };
  const closeTab = (id: string) => {
    if (id === "app-RealQA" && state.current.realqaBusy) return;
    const action = () => guard(() => removeTab(id, true), false, id);
    if (id === state.current.selected) leaveRealQA(action);
    else action();
  };
  const reopenTab = () => {
    const entry = closedTabs.current.at(-1);
    if (!entry) return;
    leaveRealQA(() => {
      const target = restoredTarget(
        entry,
        entry.target.kind === TabKind.NewTask ? newId() : "",
      );
      const currentTabs = state.current.tabs.filter(
        (tab) =>
          target.kind === TabKind.NewTab ||
          tab.id !== state.current.selected ||
          tab.target.kind !== TabKind.NewTab,
      );
      const restored = insertRestoredTab(currentTabs, entry, target);
      closedTabs.current = closedTabs.current.slice(0, -1);
      setTabs(restored.tabs);
      setSelected(restored.selected);
      setRealqaDirty(false);
      if (!target.app && target.kind !== TabKind.NewTab)
        lastLaneTab.current = restored.selected;
      setOpenTabs(false);
    });
  };
  const runTabCommand = ({ command, index, tabId }: TabCommandRequest) => {
    if (
      !adapter ||
      switching ||
      hasOpenDialog() ||
      hasOpenCommandPalette() ||
      state.current.discard ||
      pendingDiscard.current
    )
      return;
    const current = state.current;
    const selectedPosition = current.tabs.findIndex(
      (tab) => tab.id === current.selected,
    );
    if (command === TabCommand.New) createTab();
    else if (command === TabCommand.Close) closeTab(current.selected);
    else if (command === TabCommand.Reopen) reopenTab();
    else if (command === TabCommand.Next || command === TabCommand.Previous) {
      const id = tabAtOffset(
        current.tabs,
        current.selected,
        command === TabCommand.Next ? 1 : -1,
      );
      if (id) selectTab(id);
    } else if (command === TabCommand.SelectIndex) {
      const id = tabAtIndex(current.tabs, index ?? -1);
      if (id) selectTab(id);
    } else if (command === TabCommand.SelectLast) {
      const id = current.tabs.at(-1)?.id;
      if (id) selectTab(id);
    } else if (
      command === TabCommand.MoveEarlier ||
      command === TabCommand.MoveLater
    ) {
      if (selectedPosition < 0) return;
      setTabs(
        moveTab(
          current.tabs,
          current.selected,
          selectedPosition + (command === TabCommand.MoveEarlier ? -1 : 1),
        ),
      );
    } else if (
      (command === TabCommand.MoveFirst || command === TabCommand.MoveLast) &&
      (tabId ?? focusedRow) &&
      current.tabs.some((tab) => tab.id === (tabId ?? focusedRow))
    ) {
      setTabs(
        moveTab(
          current.tabs,
          (tabId ?? focusedRow)!,
          command === TabCommand.MoveFirst ? 0 : current.tabs.length - 1,
        ),
      );
    }
  };
  const tabReports = {
    ...reports,
    "app-RealQA": {
      id: "app-RealQA",
      title: "RealQA",
      dirty: realqaDirty,
      busy: realqa.busy,
      exitBusy: false,
      repositories: [],
      repositoryStatus: "",
    },
  };
  const latest = reports[selected] ?? Object.values(reports)[0];
  const repositories = latest?.repositories ?? [];
  const repositoryItems = repositories.flatMap((result) =>
    result.version?.body.case === "repository"
      ? [
          {
            id: result.version.entityId,
            name: repositoryName(result.version),
            url: result.version.body.value.normalizedUrl,
          },
        ]
      : [],
  );
  const selectDestination = (destination: Destination) => {
    if (destination.kind === DestinationKind.App)
      selectApp(destination.target as MiniApp);
    else if (destination.kind === DestinationKind.Repository)
      open({
        kind: TabKind.Repository,
        repositoryId: destination.target,
        label: destination.label,
      });
    else
      open({
        kind: TabKind.Page,
        page: destination.target as LanePage,
        label: destination.label,
      });
  };
  const openSettings = (section: SettingsSection) => {
    open(
      { kind: TabKind.Page, page: LanePage.Settings, label: "Settings" },
      section,
    );
  };
  useEffect(() => {
    "background only";
    let active = true;
    let owned: LaneAdapter;
    try {
      owned = providedAdapter ?? getAdapter();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Lane storage is unavailable.");
      return;
    }
    const stopInvalidation = owned.subscribe(refresh);
    const stopHost = listenHost(setPendingRoute, setError, () => {
      dismissDiscard();
      resetTabs();
      setAdapter(undefined);
    });
    void owned.open().then(
      () => {
        if (active) setAdapter(owned);
      },
      (e) => {
        if (!active) return;
        if (
          owned.resetUnopened &&
          /UNSUPPORTED_FORMAT|unsupported format/i.test(
            `${e?.detail?.reason ?? ""} ${e?.message ?? ""}`,
          )
        )
          setResetAdapter(owned);
        setError(e instanceof Error ? e.message : "Lane could not open.");
      },
    );
    return () => {
      active = false;
      stopHost();
      stopInvalidation();
      owned.close();
    };
  }, [providedAdapter]);
  useEffect(() => {
    if (!pendingRoute || !adapter) return;
    const route = pendingRoute;
    setPendingRoute(undefined);
    if (route.requestId !== undefined) setRouteRequest(route.requestId);
    const follow = () => {
      if (route.kind === "workspace" && route.target)
        open(route.target, undefined, route.requestId);
      else if (route.kind === "task" && route.id)
        open({ kind: TabKind.Task, taskId: route.id, label: "Task" });
      else if (route.kind === "repository" && route.id)
        open({
          kind: TabKind.Repository,
          repositoryId: route.id,
          label: "Repository",
        });
      else selectApp(MiniApp.Lane);
    };
    if (route.spaceId && route.spaceId !== adapter.spaceId)
      guard(() => {
        setSwitching(true);
        setRealqaDirty(false);
        state.current.miniApp = MiniApp.Lane;
        state.current.realqaDirty = false;
        // Unmount every old pane before changing the shared adapter's data space.
        resetTabs();
        setAdapter(undefined);
        void adapter
          .open(route.spaceId)
          .then(
            () => {
              setAdapter(adapter);
              follow();
            },
            (e) => setError(e.message),
          )
          .finally(() => setSwitching(false));
      }, true);
    else follow();
  }, [pendingRoute, adapter]);
  useEffect(() => {
    "background only";
    if (adapter && activeTab && !pendingRoute && !discard)
      reportWorkspaceRoute(targetPath(activeTab.target), routeRequest);
  }, [
    adapter,
    activeTab?.target,
    selected,
    pendingRoute,
    discard,
    routeRequest,
  ]);
  useEffect(() => {
    "background only";
    if (adapter && adapter.mode !== "web")
      lynx
        .createSelectorQuery()
        .select("#lane-workspace")
        .invoke({ method: "setFocus", params: { focus: true } })
        .exec();
  }, [selected, adapter]);
  const dirty = Object.values(reports).some((entry) => entry.dirty);
  // Updates are cancelled on normal exit. Only in-flight business mutations
  // block shutdown; otherwise an updater would block its own safe restart.
  const busy = Object.values(reports).some((entry) => entry.exitBusy);
  useEffect(() => {
    "background only";
    reportWorkspaceState(dirty || realqaDirty, busy, false, {
      count: tabs.length,
      history: closedTabs.current.length,
      selectedIndex: tabs.findIndex((tab) => tab.id === selected),
      focusedIndex: tabs.findIndex((tab) => tab.id === focusedRow),
    });
  }, [dirty, busy, realqaDirty, tabs, selected, focusedRow]);
  useEffect(() => {
    "background only";
    return listenWorkspaceExit(() =>
      guard(() => reportWorkspaceState(false, false, true), true),
    );
  }, [reports, realqaDirty]);
  useEffect(() => {
    "background only";
    if (NativeModules.LaneOnline) return;
    return listenWorkspaceTabCommands(runTabCommand);
  }, [
    adapter,
    switching,
    tabs,
    selected,
    reports,
    realqaDirty,
    discard,
    focusedRow,
  ]);
  const reset = () => {
    if (!adapter) return;
    resetTabs();
    setAdapter(undefined);
    setSwitching(true);
    void adapter
      .open()
      .then(
        () => setAdapter(adapter),
        (e) => setError(e.message),
      )
      .finally(() => setSwitching(false));
  };
  if (realqa.state?.window === "composer")
    return <CaptureComposer controller={realqa} adapter={adapter} />;
  return (
    <view
      id="lane-root"
      data-app={miniApp ?? "new-tab"}
      data-selected-tab={selected}
      data-tab-generation={String(epoch)}
      className={mobile ? "app mobile" : "app desktop"}
      bindlayoutchange={(e) => setMobile(e.detail.width < 900)}
    >
      <view className="shell">
        {!mobile && (
          <TabBar
            tabs={tabs}
            selected={selected}
            reports={tabReports}
            select={selectTab}
            close={closeTab}
            create={createTab}
            openSettings={openSettings}
            onRowFocus={onRowFocus}
          />
        )}
        <view className="tab-shell">
          {mobile && (
            <view className="mobile-tab-toggle">
              <Button
                label={`Tabs (${tabs.length})`}
                onClick={() => setOpenTabs(true)}
              >
                Tabs
              </Button>
              <Button label="New tab" onClick={createTab}>
                +
              </Button>
            </view>
          )}
          {error && (
            <Notice>
              {error}
              <Button onClick={() => setError("")}>Dismiss</Button>
            </Notice>
          )}
          {miniApp === MiniApp.Lane && !adapter && !resetAdapter && (
            <text>{switching ? "Opening data space…" : "Opening Lane…"}</text>
          )}
          {resetAdapter && (
            <StoreReset
              message={error}
              reset={() => resetAdapter.resetUnopened!()}
              ready={() => {
                const owned = resetAdapter;
                setResetAdapter(undefined);
                setError("");
                void owned.open().then(
                  () => setAdapter(owned),
                  (e) => setError(e.message),
                );
              }}
            />
          )}
          {adapter &&
            tabs
              .filter(
                (tab) => !tab.target.app && tab.target.kind !== TabKind.NewTab,
              )
              .map((tab) => (
                <view
                  key={`${epoch}-${tab.id}`}
                  className="tab-panel"
                  style={{
                    display:
                      miniApp === MiniApp.Lane && selected === tab.id
                        ? "flex"
                        : "none",
                    // Lynx can still paint text inside display:none panes.
                    // Clip inactive panes until the native display bug is fixed;
                    // active panes must allow menus and focus rings to overflow.
                    overflow:
                      miniApp === MiniApp.Lane && selected === tab.id
                        ? "visible"
                        : "hidden",
                  }}
                  data-active-tab={
                    miniApp === MiniApp.Lane && selected === tab.id
                      ? "true"
                      : "false"
                  }
                  accessibility-elements-hidden={
                    miniApp !== MiniApp.Lane || selected !== tab.id
                  }
                >
                  <LaneWorkspace
                    adapter={adapter}
                    target={tab.target}
                    tabId={tab.id}
                    mobile={mobile}
                    active={miniApp === MiniApp.Lane && selected === tab.id}
                    onOpen={open}
                    onReport={onReport}
                    onRemoved={removeTab}
                    onCloseTab={closeTab}
                    revision={revision}
                    onChange={refresh}
                    beforeDataChange={beforeDataChange}
                    onReset={reset}
                    settingsTarget={
                      tab.target.page === LanePage.Settings
                        ? settingsTarget
                        : undefined
                    }
                    displayOptions={
                      tab.id === tabIdentity(allTasks())
                        ? taskDisplay
                        : undefined
                    }
                    onDisplayOptionsChange={
                      tab.id === tabIdentity(allTasks())
                        ? setTaskDisplay
                        : undefined
                    }
                  />
                </view>
              ))}
          {activeTab?.target.kind === TabKind.NewTab && (
            <view
              className="tab-panel"
              data-tab-id={selected}
              data-active-tab="true"
            >
              <NewTabPage open={selectApp} />
            </view>
          )}
          {(miniApp === MiniApp.Pace || miniApp === MiniApp.RealQA) && (
            <view
              className="tab-panel app-content"
              data-tab-id={selected}
              data-active-tab="true"
            >
              <MiniAppNavigation app={miniApp} />
              <view id="lane-workspace" className="workspace" focusable>
                {miniApp === MiniApp.RealQA ? (
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
                )}
              </view>
            </view>
          )}
        </view>
      </view>
      <CommandPalette
        items={destinations(repositoryItems)}
        repositoryStatus={latest?.repositoryStatus ?? "Loading repositories..."}
        onSelect={selectDestination}
      />
      <RealQADialogs
        controller={realqa}
        onOpenSaved={(saved) => setPendingRoute(savedCaptureRoute(saved))}
      />
      {openTabs && (
        <Dialog title="Open tabs" onClose={() => setOpenTabs(false)}>
          <TabBar
            fullTitles
            tabs={tabs}
            selected={selected}
            reports={tabReports}
            select={selectTab}
            close={closeTab}
            create={() => {
              setOpenTabs(false);
              createTab();
            }}
            openSettings={openSettings}
            onRowFocus={onRowFocus}
          />
        </Dialog>
      )}
      {discard && (
        <Dialog title="Discard unsaved changes?" onClose={dismissDiscard}>
          <Notice>Your edits have not been saved.</Notice>
          <Button
            onClick={() => {
              const action = discard;
              pendingDiscard.current = false;
              setDiscard(undefined);
              action();
            }}
          >
            Discard changes
          </Button>
          <Button primary focusOnShow onClick={dismissDiscard}>
            Keep editing
          </Button>
        </Dialog>
      )}
    </view>
  );
}
