// SPDX-License-Identifier: Apache-2.0
import React, { useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { LogtoProvider, useHandleSignInCallback, useLogto } from "@logto/react";
import {
  QueryClient,
  QueryClientProvider,
  useQueryClient,
} from "@tanstack/react-query";
import {
  TransportProvider,
  useMutation,
  useQuery,
  useInfiniteQuery,
} from "@connectrpc/connect-query";
import { Code, ConnectError, type Transport } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import {
  ComparisonStatus,
  DeleteKind,
  ExecutionState,
  GitTrackerService,
  type Comparison,
  type Execution,
  type Dataset,
} from "./gen/gittracker/v1/gittracker_pb";
import { DelcoreService } from "./gen/delcore/v1/delcore_pb";
import { points, readable, graphValue } from "./history.mjs";
import "./style.css";

const api =
  import.meta.env.PUBLIC_GIT_TRACKER_API_URL ||
  "https://git-tracker.api.delino.io";
const delcoreURL =
  import.meta.env.PUBLIC_DELCORE_API_URL || "https://delcore.api.delino.io";
const resource = import.meta.env.PUBLIC_DELCORE_RESOURCE || delcoreURL;
const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: 1, staleTime: 0, gcTime: 0, refetchOnWindowFocus: true },
  },
});
const method = GitTrackerService.method;
type Session = {
  authenticated: boolean;
  token: () => Promise<string | undefined>;
  signIn?: () => void;
  signOut?: () => void;
};
function LoggedIn() {
  const { isAuthenticated, getAccessToken, signIn, signOut } = useLogto();
  const token = React.useCallback(
    () => getAccessToken(resource),
    [getAccessToken],
  );
  return (
    <Shell
      authenticated={isAuthenticated}
      token={token}
      signIn={() => void signIn(`${location.origin}/callback`)}
      signOut={() => {
        queryClient.clear();
        void signOut(location.origin);
      }}
    />
  );
}
function Callback() {
  const { isLoading, error } = useHandleSignInCallback(() =>
    location.replace("/"),
  );
  return (
    <main>
      {isLoading
        ? "Signing in…"
        : error
          ? "Sign-in failed. Return to the home page and try again."
          : "Redirecting…"}
    </main>
  );
}
function Shell(session: Session) {
  const transports = useMemo(() => {
    const make = (baseUrl: string) =>
      createConnectTransport({
        baseUrl,
        interceptors: [
          (next) => async (request) => {
            if (session.authenticated) {
              const token = await session.token();
              if (token) request.header.set("Authorization", `Bearer ${token}`);
            }
            return next(request);
          },
        ],
      });
    return { tracker: make(api), delcore: make(delcoreURL) };
  }, [session.authenticated, session.token]);
  return (
    <QueryClientProvider client={queryClient}>
      <TransportProvider transport={transports.tracker}>
        <App session={session} delcore={transports.delcore} />
      </TransportProvider>
    </QueryClientProvider>
  );
}
function Failure({ error, retry }: { error: unknown; retry: () => void }) {
  const e = ConnectError.from(error);
  const denied =
    e.code === Code.PermissionDenied || e.code === Code.Unauthenticated;
  return (
    <div className="notice error" role="alert">
      <strong>
        {denied
          ? "You do not have access to this data."
          : "We could not load this data."}
      </strong>
      <p>
        {denied
          ? "Sign in with an account that has access to this space."
          : `Try again. Correlation ID: ${e.metadata.get("X-Correlation-Id") || "unavailable"}`}
      </p>
      <button onClick={retry}>Retry</button>
    </div>
  );
}
const status = (value: ComparisonStatus) =>
  ({
    [ComparisonStatus.UNSPECIFIED]: "Unavailable",
    [ComparisonStatus.REGRESSION]: "Regression",
    [ComparisonStatus.IMPROVEMENT]: "Improvement",
    [ComparisonStatus.UNCHANGED]: "Unchanged",
    [ComparisonStatus.CHANGED]: "Changed",
    [ComparisonStatus.NEW]: "New / not comparable",
    [ComparisonStatus.UNAVAILABLE]: "Comparison unavailable",
    [ComparisonStatus.INCOMPATIBLE]: "Incompatible units",
    [ComparisonStatus.BASELINE_DELETED]: "Baseline data deleted",
  })[value] || "Unavailable";
function ComparisonTable({ values }: { values: Comparison[] }) {
  return (
    <div className="table-scroll">
      <table>
        <caption>Exact values and comparison results</caption>
        <thead>
          <tr>
            <th>Item / column</th>
            <th>Value</th>
            <th>Baseline</th>
            <th>Change</th>
            <th>Result</th>
          </tr>
        </thead>
        <tbody>
          {values.map((v) => (
            <tr key={`${v.datasetId}/${v.rowId}/${v.columnId}`}>
              <th>
                {v.rowId}
                <small>{v.columnId}</small>
              </th>
              <td title={`${v.value} ${v.unit}`}>
                {v.value} {v.unit}
              </td>
              <td>
                {v.baselineValue || "—"} {v.baselineUnit}
                <small>
                  {v.baselineCommit.slice(0, 8)} {v.baselineExecutionId}
                </small>
              </td>
              <td>
                {v.delta || "—"}
                <small>
                  {v.percentage ? `${v.percentage}%` : "Percentage unavailable"}
                </small>
              </td>
              <td>
                <span
                  className={`badge ${v.status === ComparisonStatus.REGRESSION ? "bad" : v.status === ComparisonStatus.IMPROVEMENT ? "good" : ""}`}
                >
                  {status(v.status)}
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {!values.length && <p>No comparable measurements yet.</p>}
    </div>
  );
}
function App({ session, delcore }: { session: Session; delcore: Transport }) {
  const initial = useMemo(() => new URLSearchParams(location.search), []);
  const [space, setSpace] = useState("");
  const [repository, setRepository] = useState(initial.get("repository") || "");
  const [execution, setExecution] = useState(initial.get("execution") || "");
  const [branch, setBranch] = useState("main");
  const [dataset, setDataset] = useState("");
  const [column, setColumn] = useState("");
  const [selected, setSelected] = useState<string[] | null>(null);
  const [since, setSince] = useState("");
  const [until, setUntil] = useState("");
  const [fromCommit, setFromCommit] = useState("");
  const [toCommit, setToCommit] = useState("");
  const [baseline, setBaseline] = useState("");
  const [target, setTarget] = useState("");
  const [comparePair, setComparePair] = useState({
    baselineCommit: "",
    targetCommit: "",
  });
  const [manage, setManage] = useState(false);
  const user = useQuery(
    DelcoreService.method.getCurrentUser,
    {},
    { transport: delcore, enabled: session.authenticated },
  );
  const repositories = useQuery(method.listRepositories, { spaceId: space });
  const history = useInfiniteQuery(
    method.getHistory,
    {
      cursor: "",
      repositoryId: repository,
      branch,
      datasetId: "",
      since: since ? `${since}T00:00:00Z` : "",
      until: until ? `${until}T23:59:59Z` : "",
      fromCommit,
      toCommit,
    },
    {
      pageParamKey: "cursor",
      getNextPageParam: (page) => page.nextCursor || undefined,
      enabled: Boolean(repository),
      refetchInterval: 30000,
    },
  );
  const historyData = useMemo(
    () =>
      history.isError
        ? undefined
        : {
            executions:
              history.data?.pages.flatMap((page) => page.executions) || [],
            commits: history.data?.pages[0]?.commits || [],
          },
    [history.data, history.isError],
  );
  useEffect(() => {
    if (history.hasNextPage && !history.isFetching)
      void history.fetchNextPage();
  }, [history.hasNextPage, history.isFetching, history.fetchNextPage]);
  const details = useQuery(
    method.getExecution,
    { repositoryId: repository, executionId: execution },
    { enabled: Boolean(repository && execution), refetchInterval: 30000 },
  );
  const comparison = useQuery(
    method.compare,
    { repositoryId: repository, ...comparePair },
    {
      enabled: Boolean(
        repository && comparePair.targetCommit && comparePair.baselineCommit,
      ),
    },
  );
  const datasets = useMemo(() => {
    const latest = new Map<string, Dataset>();
    for (const execution of historyData?.executions || []) {
      for (const measurement of execution.measurements) {
        if (measurement.dataset && !latest.has(measurement.dataset.id))
          latest.set(measurement.dataset.id, measurement.dataset);
      }
    }
    return latest;
  }, [historyData]);
  const activeDataset = datasets.get(dataset) || datasets.values().next().value;
  const activeColumn =
    activeDataset?.columns.find((c) => c.id === column) ||
    activeDataset?.columns[0];
  const rows = useMemo(
    () => [
      ...new Set(
        (historyData?.executions || []).flatMap((e) =>
          e.measurements
            .filter((m) => m.dataset?.id === activeDataset?.id)
            .flatMap((m) => m.rows.map((r) => r.id)),
        ),
      ),
    ],
    [historyData, activeDataset],
  );
  const activeRows =
    selected !== null
      ? selected.filter((id) => rows.includes(id))
      : rows.slice(0, 3);
  const openExecution = (id: string) => {
    setExecution(id);
    const params = new URLSearchParams({ repository, execution: id });
    window.history.replaceState(null, "", `?${params}`);
    document
      .getElementById("execution-details")
      ?.scrollIntoView({ behavior: "smooth" });
  };
  useEffect(() => {
    if (!repository && repositories.data?.repositories[0])
      setRepository(repositories.data.repositories[0].id);
  }, [repository, repositories.data]);
  return (
    <>
      <header>
        <a className="brand" href="/">
          ↗ <span>Git Tracker</span>
        </a>
        <nav>
          <a href="https://docs.delino.io/services/git-tracker/">Setup guide</a>
          {session.authenticated ? (
            <button onClick={session.signOut}>Sign out</button>
          ) : (
            <button
              className="primary"
              onClick={session.signIn}
              disabled={!session.signIn}
            >
              Sign in with Delino
            </button>
          )}
        </nav>
      </header>
      <main>
        <div className="eyebrow">EVERY COMMIT TELLS A STORY</div>
        <div className="title-row">
          <div>
            <h1>See what changed.</h1>
            <p className="intro">
              Track size, speed, and the numbers that matter.
            </p>
          </div>
          {session.authenticated && (
            <button onClick={() => setManage(true)}>Manage repositories</button>
          )}
        </div>
        <section className="toolbar" aria-label="Repository filters">
          <label>
            Space
            <select
              value={space}
              onChange={(e) => {
                setSpace(e.target.value);
                setRepository("");
                setExecution("");
              }}
            >
              <option value="">Public repositories</option>
              {user.data?.tenants.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            Repository
            <select
              value={repository}
              onChange={(e) => {
                setRepository(e.target.value);
                setExecution("");
                setSelected(null);
              }}
            >
              <option value="">Select a repository</option>
              {!repositories.isError &&
                repositories.data?.repositories.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.fullName}
                    {r.public ? "" : " · Private"}
                    {r.connected ? "" : " · Disconnected"}
                  </option>
                ))}
            </select>
          </label>
          <label>
            Branch
            <input
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
              placeholder="main"
            />
          </label>
        </section>
        {session.authenticated && user.isError && (
          <Failure error={user.error} retry={() => void user.refetch()} />
        )}
        {repositories.isError && (
          <Failure
            error={repositories.error}
            retry={() => void repositories.refetch()}
          />
        )}
        {repositories.isLoading && <p role="status">Loading repositories…</p>}
        {!repository && !repositories.isLoading && !repositories.isError && (
          <section className="empty">
            <div className="empty-icon">↗</div>
            <h2>Your history starts with one upload.</h2>
            <p>
              Sign in, select a space, and connect a GitHub repository. Add the
              Git Tracker Action to upload your first measurements.
            </p>
            <a href="https://docs.delino.io/services/git-tracker/">
              Read the setup guide →
            </a>
          </section>
        )}
        {repository && (
          <>
            <section className="panel">
              <div className="section-heading">
                <div>
                  <div className="eyebrow">MEASUREMENT HISTORY</div>
                  <h2>Follow the trend</h2>
                </div>
                <span className="badge">Completed executions</span>
              </div>
              <div className="filters">
                <label>
                  Dataset
                  <select
                    value={activeDataset?.id || ""}
                    onChange={(e) => {
                      setDataset(e.target.value);
                      setSelected(null);
                    }}
                  >
                    <option value="">Select dataset</option>
                    {[...datasets.values()].map((d) => (
                      <option key={d.id} value={d.id}>
                        {d.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  Column
                  <select
                    value={activeColumn?.id || ""}
                    onChange={(e) => setColumn(e.target.value)}
                  >
                    {activeDataset?.columns.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name} · {c.unit}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  From date
                  <input
                    type="date"
                    value={since}
                    onChange={(e) => setSince(e.target.value)}
                  />
                </label>
                <label>
                  To date
                  <input
                    type="date"
                    value={until}
                    onChange={(e) => setUntil(e.target.value)}
                  />
                </label>
                <label>
                  First commit
                  <input
                    value={fromCommit}
                    onChange={(e) => setFromCommit(e.target.value)}
                    placeholder="Full SHA"
                  />
                </label>
                <label>
                  Last commit
                  <input
                    value={toCommit}
                    onChange={(e) => setToCommit(e.target.value)}
                    placeholder="Full SHA"
                  />
                </label>
              </div>
              <fieldset className="row-picker">
                <legend>Items</legend>
                {rows.map((id) => (
                  <label key={id}>
                    <input
                      type="checkbox"
                      checked={activeRows.includes(id)}
                      onChange={(e) =>
                        setSelected(
                          e.target.checked
                            ? [...activeRows, id]
                            : activeRows.filter((item) => item !== id),
                        )
                      }
                    />
                    {id}
                  </label>
                ))}
              </fieldset>
              {history.isLoading ? (
                <p role="status">Loading measurements…</p>
              ) : history.isError ? (
                <Failure
                  error={history.error}
                  retry={() => void history.refetch()}
                />
              ) : activeDataset && activeColumn ? (
                <HistoryGraph
                  commits={historyData?.commits || []}
                  executions={historyData?.executions || []}
                  dataset={activeDataset.id}
                  column={activeColumn.id}
                  unit={activeColumn.unit}
                  rows={activeRows}
                  open={openExecution}
                />
              ) : (
                <div className="empty">
                  <h3>No measurements on this branch yet.</h3>
                  <p>
                    Upload rows using the Action, then run a final completion
                    job. In-progress data appears below before it becomes a
                    graph point.
                  </p>
                </div>
              )}
            </section>
            <section className="panel">
              <h2>Execution history</h2>
              <p>
                Completion means measurement finished. It does not report CI
                success.
              </p>
              <div className="execution-list">
                {historyData?.executions.map((e) => (
                  <button
                    className="execution"
                    key={e.context?.executionId}
                    onClick={() => openExecution(e.context!.executionId)}
                  >
                    <code>{e.context?.commit.slice(0, 8)}</code>
                    <span>{e.context?.executionId}</span>
                    <span className="badge">
                      {e.state === ExecutionState.COMPLETED
                        ? "Complete"
                        : e.state === ExecutionState.INTERRUPTED
                          ? "Interrupted"
                          : "Measuring"}
                    </span>
                    <time>{new Date(e.registeredAt).toLocaleString()}</time>
                    <span aria-hidden="true">→</span>
                  </button>
                ))}
              </div>
              {history.hasNextPage && (
                <p role="status">Loading more history…</p>
              )}
            </section>
            {execution && (
              <section className="panel" id="execution-details">
                <h2>Execution details</h2>
                <p>
                  <code>{execution}</code>
                </p>
                {details.isLoading ? (
                  <p role="status">Loading execution…</p>
                ) : details.isError ? (
                  <Failure
                    error={details.error}
                    retry={() => void details.refetch()}
                  />
                ) : (
                  <>
                    <p>
                      {details.data?.execution?.state ===
                      ExecutionState.MEASURING
                        ? "Measuring: absent rows are uncollected."
                        : "Finished: absent rows are missing and are not regressions."}
                    </p>
                    <p>
                      Registered: {details.data?.execution?.registeredAt}.
                      Completed:{" "}
                      {details.data?.execution?.completedAt || "Not completed"}.
                    </p>
                    <ComparisonTable values={details.data?.comparisons || []} />
                  </>
                )}
              </section>
            )}
            <section className="panel">
              <h2>Compare two commits</h2>
              <p>
                Compare only these commits. Missing rows do not fall back to
                ancestors.
              </p>
              <form
                className="compare-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  setComparePair({
                    baselineCommit: baseline,
                    targetCommit: target,
                  });
                }}
              >
                <label>
                  Baseline SHA
                  <input
                    required
                    pattern="[a-f0-9]{40}"
                    value={baseline}
                    onChange={(e) => setBaseline(e.target.value)}
                  />
                </label>
                <label>
                  Target SHA
                  <input
                    required
                    pattern="[a-f0-9]{40}"
                    value={target}
                    onChange={(e) => setTarget(e.target.value)}
                  />
                </label>
                <button className="primary">Compare</button>
              </form>
              {comparison.isFetching && <p role="status">Comparing…</p>}
              {comparison.isError && (
                <Failure
                  error={comparison.error}
                  retry={() => void comparison.refetch()}
                />
              )}
              {!comparison.isError && comparison.data && (
                <ComparisonTable values={comparison.data.comparisons} />
              )}
            </section>
          </>
        )}
      </main>
      <footer>Git Tracker · Measurements without snapshots.</footer>
      {manage && (
        <Management
          space={space}
          repository={repository}
          close={() => setManage(false)}
        />
      )}
    </>
  );
}
function HistoryGraph({
  commits,
  executions,
  dataset,
  column,
  unit,
  rows,
  open,
}: {
  commits: string[];
  executions: Execution[];
  dataset: string;
  column: string;
  unit: string;
  rows: string[];
  open: (id: string) => void;
}) {
  const series = rows.map((row) => ({
    row,
    points: points(commits, executions, dataset, column, row).map((point) => ({
      ...point,
      plotted: graphValue(point.value, point.unit, unit),
    })),
  }));
  const values = series
    .flatMap((s) => s.points.map((p) => p.plotted))
    .filter(Number.isFinite);
  const low = Math.min(...values),
    high = Math.max(...values);
  const colors = ["#3668e8", "#128572", "#b5651c", "#934dc0"];
  const x = (index: number) =>
    60 + (index / Math.max(1, commits.length - 1)) * 820;
  const y = (value: number) => 235 - ((value - low) / (high - low || 1)) * 185;
  return (
    <>
      <div className="chart">
        <svg
          viewBox="0 0 940 280"
          role="img"
          aria-label={`${column} measurement history. The table below contains every point.`}
        >
          <line x1="60" x2="880" y1="235" y2="235" stroke="#dce2ec" />
          {values.length > 0 && (
            <>
              <text x="4" y="45">
                {readable(String(high), unit)}
              </text>
              <text x="4" y="235">
                {readable(String(low), unit)}
              </text>
            </>
          )}
          {series.map((s, index) => (
            <g key={s.row}>
              <polyline
                fill="none"
                stroke={colors[index % colors.length]}
                strokeWidth="2.5"
                points={s.points
                  .filter((p) => Number.isFinite(p.plotted))
                  .map((p) => `${x(p.index)},${y(p.plotted)}`)
                  .join(" ")}
              />
              {s.points
                .filter((p) => Number.isFinite(p.plotted))
                .map((p) => (
                  <circle
                    key={p.commit}
                    cx={x(p.index)}
                    cy={y(p.plotted)}
                    r="4.5"
                    fill={colors[index % colors.length]}
                  >
                    <title>
                      {s.row}: {p.value} {p.unit}, {p.commit}
                    </title>
                  </circle>
                ))}
            </g>
          ))}
          <text x="60" y="270">
            Older commits
          </text>
          <text x="785" y="270">
            Newer commits
          </text>
        </svg>
      </div>
      <p className="hint">
        Commit spacing is equal. Lines connect measured points across skipped
        commits; no values are filled in. Open a point using its table link.
      </p>
      <div className="table-scroll">
        <table>
          <caption>Graph data and execution links</caption>
          <thead>
            <tr>
              <th>Item</th>
              <th>Commit</th>
              <th>Measurement</th>
              <th>Exact value</th>
              <th>Execution</th>
            </tr>
          </thead>
          <tbody>
            {series.flatMap((s) =>
              s.points.map((p) => (
                <tr key={`${s.row}/${p.commit}`}>
                  <th>{s.row}</th>
                  <td>
                    <code>{p.commit.slice(0, 8)}</code>
                  </td>
                  <td>{readable(p.value, p.unit)}</td>
                  <td>
                    {p.value} {p.unit}
                  </td>
                  <td>
                    <button className="link" onClick={() => open(p.execution)}>
                      View execution →
                    </button>
                  </td>
                </tr>
              )),
            )}
          </tbody>
        </table>
        {!values.length && (
          <p>No completed measurements match these filters.</p>
        )}
      </div>
    </>
  );
}
function Management({
  space,
  repository,
  close,
}: {
  space: string;
  repository: string;
  close: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const cache = useQueryClient();
  const [name, setName] = useState("");
  const [githubToken, setGithubToken] = useState("");
  const [expiry, setExpiry] = useState("");
  const [tokenId, setTokenId] = useState("");
  const [deleteKind, setDeleteKind] = useState(DeleteKind.EXECUTION);
  const [deleteId, setDeleteId] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const connect = useMutation(method.connectRepository);
  const issue = useMutation(method.issueToken);
  const revoke = useMutation(method.revokeToken);
  const remove = useMutation(method.delete);
  const disconnect = useMutation(method.disconnect);
  const errors = [connect, issue, revoke, remove, disconnect].filter(
    (m) => m.error,
  );
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  const refresh = () => void cache.invalidateQueries();
  return (
    <dialog ref={dialog} onCancel={close} onClose={close}>
      <div className="dialog-heading">
        <h2>Repository management</h2>
        <button onClick={close} aria-label="Close management">
          ✕
        </button>
      </div>
      <p>
        Manage permission is required. Connecting also requires GitHub
        administrator access.
      </p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          connect.mutate(
            { spaceId: space, fullName: name, githubAccessToken: githubToken },
            {
              onSuccess: () => {
                setGithubToken("");
                refresh();
              },
            },
          );
        }}
      >
        <h3>Connect a repository</h3>
        <p>
          Install the Git Tracker GitHub App for the selected repository, then
          verify your GitHub account with a repository admin token. Select a
          Delino space first.
        </p>
        <label>
          Repository
          <input
            autoFocus
            required
            placeholder="owner/repository"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <label>
          GitHub access token
          <input
            type="password"
            required
            autoComplete="off"
            value={githubToken}
            onChange={(e) => setGithubToken(e.target.value)}
          />
        </label>
        <button disabled={!space || connect.isPending}>
          Connect repository
        </button>
        {connect.isSuccess && <p role="status">Repository connected.</p>}
      </form>
      {repository && (
        <>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              issue.mutate({
                repositoryId: repository,
                expiresAt: new Date(expiry).toISOString(),
              });
            }}
          >
            <h3>Issue an upload token</h3>
            <p>
              Connection ID for the Action’s repository-id:{" "}
              <code>{repository}</code>
            </p>
            <label>
              Expiration
              <input
                type="datetime-local"
                required
                value={expiry}
                onChange={(e) => setExpiry(e.target.value)}
              />
            </label>
            <button disabled={issue.isPending}>Create token</button>
            {issue.data && (
              <div className="notice">
                <p>Copy this token now. Keep it in a CI secret.</p>
                <code className="secret">{issue.data.token}</code>
                <p>
                  Token ID: <code>{issue.data.tokenId}</code>
                </p>
              </div>
            )}
          </form>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              revoke.mutate({ repositoryId: repository, tokenId });
            }}
          >
            <label>
              Token ID to revoke
              <input
                required
                value={tokenId}
                onChange={(e) => setTokenId(e.target.value)}
              />
            </label>
            <button disabled={revoke.isPending}>Revoke token</button>
            {revoke.isSuccess && <p role="status">Token revoked.</p>}
          </form>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              remove.mutate(
                {
                  repositoryId: repository,
                  kind: deleteKind,
                  id: deleteId,
                  confirmed,
                },
                { onSuccess: refresh },
              );
            }}
          >
            <h3>Delete measurements permanently</h3>
            <label>
              Delete
              <select
                value={deleteKind}
                onChange={(e) => setDeleteKind(Number(e.target.value))}
              >
                <option value={DeleteKind.EXECUTION}>One execution</option>
                <option value={DeleteKind.DATASET}>One dataset</option>
                <option value={DeleteKind.REPOSITORY}>
                  All repository measurements
                </option>
              </select>
            </label>
            {deleteKind !== DeleteKind.REPOSITORY && (
              <label>
                ID
                <input
                  required
                  value={deleteId}
                  onChange={(e) => setDeleteId(e.target.value)}
                />
              </label>
            )}
            <label className="check">
              <input
                type="checkbox"
                checked={confirmed}
                onChange={(e) => setConfirmed(e.target.checked)}
              />
              I understand this permanently deletes the selected data.
            </label>
            <button
              className="danger"
              disabled={!confirmed || remove.isPending}
            >
              Delete permanently
            </button>
            {remove.isSuccess && <p role="status">Measurements deleted.</p>}
          </form>
          <button
            onClick={() =>
              disconnect.mutate(
                { repositoryId: repository },
                { onSuccess: refresh },
              )
            }
          >
            Disconnect repository and make data private
          </button>
        </>
      )}
      {errors.map((m, i) => (
        <Failure key={i} error={m.error} retry={() => m.reset()} />
      ))}
    </dialog>
  );
}
const endpoint = import.meta.env.PUBLIC_LOGTO_ENDPOINT;
const appId = import.meta.env.PUBLIC_LOGTO_APP_ID;
const content =
  endpoint && appId ? (
    <LogtoProvider config={{ endpoint, appId, resources: [resource] }}>
      {location.pathname === "/callback" ? <Callback /> : <LoggedIn />}
    </LogtoProvider>
  ) : (
    <Shell authenticated={false} token={async () => undefined} />
  );
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>{content}</React.StrictMode>,
);
