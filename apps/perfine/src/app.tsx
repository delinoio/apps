// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import type { Transport } from "@connectrpc/connect";
import { useInfiniteQuery, useQuery } from "@connectrpc/connect-query";
import { useQueryClient } from "@tanstack/react-query";
import { NativeSelect } from "@delino/ui/components/native-select";
import { DelcoreService } from "@delino/protos/delcore/v1/delcore_pb";
import {
  AccessService,
  ConnectionService,
  Permission,
  type Connection,
} from "@delino/protos/perfine/v1/perfine_pb";
import { useRequestId } from "./core";
import { Button, Badge, Failure, Notice, Pager } from "./ui";
import { Repositories, Settings } from "./repositories";
import { History, Detail } from "./experiments";
import { Start } from "./start";
import { Operations } from "./operations";
enum View {
  Repositories = "repositories",
  Experiments = "experiments",
  Settings = "settings",
  Start = "start",
}
export function App({
  delcore,
  signOut,
}: {
  delcore: Transport;
  signOut: () => void;
}) {
  const [space, setSpace] = useState("");
  const cache = useQueryClient();
  const user = useInfiniteQuery(
    DelcoreService.method.getCurrentUser,
    { page: { size: 100, afterId: "" } },
    {
      transport: delcore,
      pageParamKey: "page",
      getNextPageParam: (p) =>
        p.nextAfterId ? { size: 100, afterId: p.nextAfterId } : undefined,
    },
  );
  const tenants = user.data?.pages.flatMap((p) => p.tenants) || [];
  const current = tenants.find((t) => t.id === space);
  return (
    <>
      <a href="#main" className="skip-link">
        Skip to content
      </a>
      <header className="topbar">
        <a className="brand" href="/">
          ◈ Perfine
        </a>
        <Badge variant="secondary">Private workspace</Badge>
        <Button variant="ghost" onClick={signOut}>
          Sign out
        </Button>
      </header>
      <div className="workspace">
        <aside>
          <label className="field">
            Space
            <NativeSelect
              value={space}
              onChange={(e) => {
                void cache.cancelQueries();
                cache.removeQueries({
                  predicate: (q) =>
                    JSON.stringify(q.queryKey).includes("perfine.v1."),
                });
                setSpace(e.target.value);
              }}
            >
              <option value="">Select a space</option>
              {tenants.map((t) => (
                <option
                  key={t.id}
                  value={t.id}
                  disabled={t.blocked || t.deleting}
                >
                  {t.name}
                  {t.personal ? " · Personal" : " · Organization"}
                  {t.blocked || t.deleting ? " · Unavailable" : ""}
                </option>
              ))}
            </NativeSelect>
          </label>
          {user.isPending && <Notice title="Loading spaces…" />}
          {user.error && (
            <Failure error={user.error} retry={() => void user.refetch()} />
          )}
          {user.hasNextPage && (
            <Button variant="outline" onClick={() => void user.fetchNextPage()}>
              More spaces
            </Button>
          )}
          <p className="muted">
            Results stay private to authorized readers in this space, including
            public repositories.
          </p>
          <div className="local-note">
            <strong>Keep working locally</strong>
            <p>No hosted account required.</p>
            <code>perfine optimize</code>
          </div>
        </aside>
        <main id="main" tabIndex={-1}>
          {user.error ? (
            <Notice title="Space authority unavailable">
              Resolve the account error before accessing hosted results.
            </Notice>
          ) : !current ? (
            <>
              <p className="eyebrow">YOUR PERFORMANCE WORKSPACE</p>
              <h1>Evidence before conclusions.</h1>
              <Notice
                title={
                  tenants.length
                    ? "Select a space to continue"
                    : user.isPending
                      ? "Loading your spaces…"
                      : "No spaces available"
                }
              >
                Use your existing Delino personal or organization space.{" "}
                <a href="https://delino.io" target="_blank" rel="noreferrer">
                  Manage Delino spaces ↗
                </a>
              </Notice>
            </>
          ) : current.blocked || current.deleting ? (
            <Notice title="Space unavailable">
              New work is blocked. Contact your space manager.
            </Notice>
          ) : (
            <Space key={space} spaceId={space} name={current.name} />
          )}
        </main>
      </div>
    </>
  );
}
function Space({ spaceId, name }: { spaceId: string; name: string }) {
  const requestId = useRequestId();
  const access = useQuery(
    AccessService.method.getAuthorization,
    { spaceId, requestId },
    { refetchInterval: 30_000 },
  );
  const [view, setView] = useState(View.Experiments),
    [connectionId, setConnection] = useState(""),
    [experimentId, setExperiment] = useState("");
  const [cursors, setCursors] = useState([""]);
  const permissions = access.error
    ? []
    : access.data?.authorization?.permissions || [];
  const may = (p: Permission) => permissions.includes(p);
  const connections = useQuery(
    ConnectionService.method.listConnections,
    { spaceId, requestId, page: { pageSize: 50, cursor: cursors.at(-1) } },
    { enabled: may(Permission.READ) || may(Permission.MANAGE) },
  );
  const selected = useQuery(
    ConnectionService.method.getConnection,
    { spaceId, requestId, connectionId },
    {
      enabled:
        !!connectionId && (may(Permission.READ) || may(Permission.MANAGE)),
      refetchInterval: 30_000,
    },
  );
  const connection = selected.error ? undefined : selected.data?.connection;
  function navigate(v: View) {
    setView(v);
    setExperiment("");
  }
  if (access.isPending) return <Notice title="Checking space permissions…" />;
  if (access.error)
    return <Failure error={access.error} retry={() => void access.refetch()} />;
  return (
    <>
      <nav className="tabs" aria-label="Workspace">
        <Button
          variant={view === View.Repositories ? "secondary" : "ghost"}
          onClick={() => navigate(View.Repositories)}
        >
          Repositories
        </Button>
        <Button
          variant={view === View.Experiments ? "secondary" : "ghost"}
          onClick={() => navigate(View.Experiments)}
        >
          Experiments
        </Button>
        <Button
          variant={view === View.Settings ? "secondary" : "ghost"}
          onClick={() => navigate(View.Settings)}
        >
          Settings
        </Button>
      </nav>
      <div className="page-heading">
        <div>
          <p className="eyebrow">{name}</p>
          <h1>
            {view === View.Repositories
              ? "Repositories"
              : view === View.Settings
                ? "Repository settings"
                : view === View.Start
                  ? "Start an experiment"
                  : experimentId
                    ? "Experiment detail"
                    : "Experiments"}
          </h1>
        </div>
        {may(Permission.EXPERIMENT) &&
          connection &&
          view === View.Experiments && (
            <Button onClick={() => navigate(View.Start)}>New experiment</Button>
          )}
      </div>
      <div className="repository-picker">
        <label className="field">
          Repository
          <NativeSelect
            value={connectionId}
            onChange={(e) => {
              setConnection(e.target.value);
              setExperiment("");
            }}
          >
            <option value="">Select a repository</option>
            {connection &&
              !connections.data?.connections.some(
                (c) => c.id === connection.id,
              ) && (
                <option value={connection.id}>
                  {connection.repository?.owner}/{connection.repository?.name}
                </option>
              )}
            {connections.data?.connections.map((c) => (
              <option key={c.id} value={c.id}>
                {c.repository?.owner}/{c.repository?.name}
              </option>
            ))}
          </NativeSelect>
        </label>
        {connections.data?.page?.nextCursor || cursors.length > 1 ? (
          <Pager
            previous={
              cursors.length > 1
                ? () => setCursors((c) => c.slice(0, -1))
                : undefined
            }
            next={
              connections.data?.page?.nextCursor
                ? () =>
                    setCursors((c) => [
                      ...c,
                      connections.data!.page!.nextCursor,
                    ])
                : undefined
            }
            busy={connections.isFetching}
          />
        ) : null}
      </div>
      {connections.isLoading && <Notice title="Loading repositories…" />}
      {connections.error && (
        <Failure
          error={connections.error}
          retry={() => void connections.refetch()}
        />
      )}
      {selected.error && (
        <Failure error={selected.error} retry={() => void selected.refetch()} />
      )}
      {view === View.Repositories ? (
        <Repositories
          spaceId={spaceId}
          manage={may(Permission.MANAGE)}
          connections={connections.data?.connections || []}
          onSelect={(c) => {
            setConnection(c.id);
            navigate(View.Experiments);
          }}
        />
      ) : view === View.Settings ? (
        <Settings
          key={connectionId}
          spaceId={spaceId}
          connection={connection}
          manage={may(Permission.MANAGE)}
        />
      ) : !may(Permission.READ) && may(Permission.EXPERIMENT) ? (
        <Operations spaceId={spaceId} />
      ) : !connection ? (
        <Notice title="Choose a repository">
          Select a connected repository, or open Repositories to connect one.
        </Notice>
      ) : view === View.Start ? (
        <Start
          key={connectionId}
          spaceId={spaceId}
          connection={connection}
          allowed={may(Permission.EXPERIMENT)}
          onStarted={(id) => {
            setExperiment(id);
            setView(View.Experiments);
          }}
        />
      ) : !may(Permission.READ) ? (
        <Notice title="Read permission required">
          Management, experiment and publication permissions do not grant result
          access.
        </Notice>
      ) : experimentId ? (
        <Detail
          key={experimentId}
          spaceId={spaceId}
          connection={connection}
          experimentId={experimentId}
          permissions={permissions}
          back={() => setExperiment("")}
        />
      ) : (
        <History
          key={connectionId}
          spaceId={spaceId}
          connectionId={connectionId}
          open={setExperiment}
        />
      )}
    </>
  );
}
