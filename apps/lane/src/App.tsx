// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useRef, useState } from "@lynx-js/react";
import type { LaneAdapter } from "./adapter";
import { getAdapter, listenHost, type OpenRoute } from "./runtime";
import {
  AppRail,
  ComingSoon,
  MiniApp,
  MiniAppNavigation,
  SettingsSection,
} from "./AppNavigation";
import { Button, Dialog, Notice } from "./components";
import { StoreReset } from "./StoreReset";
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
import { repositoryName } from "./model";
import { LaneWorkspace } from "./LaneWorkspace";
import { TabBar } from "./TabBar";
import {
  adjacentTab,
  allTasks,
  tabIdentity,
  TabKind,
  type TabTarget,
  type WorkspaceReport,
  type WorkspaceTab,
} from "./tabs";
import { reportWorkspaceState, listenWorkspaceExit } from "./workspace-host";
import "./style.css";

const initialTab = (): WorkspaceTab => ({
  id: tabIdentity(allTasks()),
  target: allTasks(),
});
export function App({ providedAdapter }: { providedAdapter?: LaneAdapter }) {
  const realqa = useRealQA();
  const [miniApp, setMiniApp] = useState(MiniApp.Lane);
  const [realqaDirty, setRealqaDirty] = useState(false);
  const [adapter, setAdapter] = useState<LaneAdapter>();
  const [resetAdapter, setResetAdapter] = useState<LaneAdapter>();
  const [error, setError] = useState("");
  const [tabs, setTabs] = useState<WorkspaceTab[]>(() => [initialTab()]);
  const [selected, setSelected] = useState(() => initialTab().id);
  const [reports, setReports] = useState<Record<string, WorkspaceReport>>({});
  const [epoch, setEpoch] = useState(0);
  const [mobile, setMobile] = useState(false);
  const cancelDiscard = useRef<(() => void) | undefined>(undefined);
  const [discard, setDiscard] = useState<() => void>();
  const [openTabs, setOpenTabs] = useState(false);
  const [paletteRequest, setPaletteRequest] = useState(0);
  const settingsRequest = useRef(0);
  const [settingsTarget, setSettingsTarget] = useState<{
    section: SettingsSection;
    request: number;
  }>();
  const [pendingRoute, setPendingRoute] = useState<OpenRoute>();
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(() => setRevision((v) => v + 1), []);
  const [switching, setSwitching] = useState(false);
  const state = useRef({ reports, realqaDirty, tabs, selected, miniApp });
  state.current = { reports, realqaDirty, tabs, selected, miniApp };
  const onReport = useCallback((report: WorkspaceReport) => {
    setReports((previous) => ({ ...previous, [report.id]: report }));
  }, []);
  const resetTabs = () => {
    const first = initialTab();
    setTabs([first]);
    setSelected(first.id);
    setReports({});
    setEpoch((v) => v + 1);
    setOpenTabs(false);
    setSettingsTarget(undefined);
  };
  const guard = (action: () => void, all = false, id = selected) => {
    const entries = all
      ? Object.values(state.current.reports)
      : [state.current.reports[id]];
    if (entries.some((entry) => entry?.busy)) {
      setError(
        "Wait for the current operation to finish before closing or changing data spaces.",
      );
      return;
    }
    if (
      entries.some((entry) => entry?.dirty) ||
      (all && state.current.realqaDirty)
    )
      setDiscard(() => action);
    else action();
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
      setDiscard(() => () => {
        cancelDiscard.current = undefined;
        resolve(true);
      });
    });
  const dismissDiscard = () => {
    setDiscard(undefined);
    cancelDiscard.current?.();
    cancelDiscard.current = undefined;
  };
  const open = (target: TabTarget, section?: SettingsSection) => {
    const proceed = () => {
      const id = tabIdentity(target);
      setTabs((previous) =>
        previous.some((tab) => tab.id === id)
          ? previous
          : [...previous, { id, target }],
      );
      setSelected(id);
      if (target.page === LanePage.Settings)
        setSettingsTarget(
          section ? { section, request: ++settingsRequest.current } : undefined,
        );
      setRealqaDirty(false);
      setMiniApp(MiniApp.Lane);
      setOpenTabs(false);
    };
    if (state.current.miniApp === MiniApp.RealQA && state.current.realqaDirty)
      setDiscard(() => proceed);
    else proceed();
  };
  const selectApp = (app: MiniApp) => {
    if (app === miniApp) return;
    const proceed = () => {
      setRealqaDirty(false);
      setMiniApp(app);
    };
    if (realqaDirty) setDiscard(() => proceed);
    else proceed();
  };
  const removeTab = (id: string) => {
    const currentTabs = state.current.tabs;
    const currentSelected = state.current.selected;
    const next = currentTabs.filter((tab) => tab.id !== id);
    const selectedNext =
      currentSelected === id ? adjacentTab(currentTabs, id) : currentSelected;
    if (!next.length) resetTabs();
    else {
      setTabs(next);
      setSelected(selectedNext!);
      setReports((previous) => {
        const copy = { ...previous };
        delete copy[id];
        return copy;
      });
    }
    setOpenTabs(false);
  };
  const closeTab = (id: string) => guard(() => removeTab(id), false, id);
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
    const follow = () => {
      if (route.kind === "task" && route.id)
        open({ kind: TabKind.Task, taskId: route.id, label: "Task" });
      else if (route.kind === "repository" && route.id)
        open({
          kind: TabKind.Repository,
          repositoryId: route.id,
          label: "Repository",
        });
      else setMiniApp(MiniApp.Lane);
    };
    if (route.spaceId && route.spaceId !== adapter.spaceId)
      guard(() => {
        setSwitching(true);
        setMiniApp(MiniApp.Lane);
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
    if (adapter && adapter.mode !== "web")
      lynx
        .createSelectorQuery()
        .select("#lane-workspace")
        .invoke({ method: "setFocus", params: { focus: true } })
        .exec();
  }, [miniApp, adapter]);
  const dirty = Object.values(reports).some((entry) => entry.dirty);
  // Updates are cancelled on normal exit. Only in-flight business mutations
  // block shutdown; otherwise an updater would block its own safe restart.
  const busy = Object.values(reports).some((entry) => entry.exitBusy);
  useEffect(() => {
    "background only";
    reportWorkspaceState(dirty || realqaDirty, busy);
  }, [dirty, busy, realqaDirty]);
  useEffect(() => {
    "background only";
    return listenWorkspaceExit(() =>
      guard(() => reportWorkspaceState(false, false, true), true),
    );
  }, [reports, realqaDirty]);
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
      data-app={miniApp}
      data-selected-tab={selected}
      data-tab-generation={String(epoch)}
      className={mobile ? "app mobile" : "app desktop"}
      bindlayoutchange={(e) => setMobile(e.detail.width < 900)}
    >
      <view className="shell">
        <AppRail
          selected={miniApp}
          select={selectApp}
          openSettings={openSettings}
        />
        <view className="tab-shell">
          {miniApp === MiniApp.Lane && (
            <TabBar
              tabs={tabs}
              selected={selected}
              reports={reports}
              select={setSelected}
              close={closeTab}
              create={() => setPaletteRequest((v) => v + 1)}
              showAll={() => setOpenTabs(true)}
            />
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
            tabs.map((tab) => (
              <view
                key={`${epoch}-${tab.id}`}
                className="tab-panel"
                style={{
                  display:
                    miniApp === MiniApp.Lane && selected === tab.id
                      ? "flex"
                      : "none",
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
                />
              </view>
            ))}
          {miniApp !== MiniApp.Lane && (
            <view className="app-content">
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
        openRequest={paletteRequest}
      />
      <RealQADialogs
        controller={realqa}
        onOpenSaved={(saved) => setPendingRoute(savedCaptureRoute(saved))}
      />
      {openTabs && (
        <Dialog title="Open tabs" onClose={() => setOpenTabs(false)}>
          {tabs.map((tab) => (
            <view className="row open-tab-row" key={tab.id}>
              <view className="open-tab-choice">
                <Button
                  pressed={tab.id === selected}
                  label={`${reports[tab.id]?.title ?? tab.target.label}${reports[tab.id]?.dirty ? ", Unsaved changes" : ""}`}
                  onClick={() => {
                    setSelected(tab.id);
                    setOpenTabs(false);
                  }}
                >
                  {`${reports[tab.id]?.title ?? tab.target.label}${reports[tab.id]?.dirty ? " · Unsaved changes" : ""}`}
                </Button>
              </view>
              <view className="open-tab-close">
                <Button
                  label={`Close ${reports[tab.id]?.title ?? tab.target.label}`}
                  disabled={reports[tab.id]?.busy}
                  onClick={() => closeTab(tab.id)}
                >
                  ×
                </Button>
              </view>
            </view>
          ))}
        </Dialog>
      )}
      {discard && (
        <Dialog title="Discard unsaved changes?" onClose={dismissDiscard}>
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
          <Button primary focusOnShow onClick={dismissDiscard}>
            Keep editing
          </Button>
        </Dialog>
      )}
    </view>
  );
}
