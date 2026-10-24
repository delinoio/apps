// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import { useQueryClient } from "@tanstack/react-query";
import { NativeSelect } from "@delino/ui/components/native-select";
import {
  CancellationMode,
  ConnectionState,
  ExperimentService,
  EvidenceService,
  ExecutionState,
  PerformanceVerdict,
  CompletionState,
  Permission,
  ComparisonKind,
  CorrectnessState,
  PolicyException,
  ArtifactKind,
  type Connection,
  type Candidate,
  type Experiment,
} from "@delino/protos/perfine/v1/perfine_pb";
import {
  candidateDiffURL,
  completenessLabel,
  executionLabel,
  percent,
  useRequestId,
  uuid7,
  verdictLabel,
} from "./core";
import {
  Badge,
  Button,
  Card,
  Confirm,
  External,
  Failure,
  Notice,
  Pager,
} from "./ui";
import { Specification } from "./start";
import { DeleteResult } from "./repositories";
import { Review } from "./review";
export function Status({
  experiment,
}: {
  experiment: Pick<
    Experiment,
    "executionState" | "verdict" | "evidenceCompletion"
  >;
}) {
  return (
    <div className="status-row">
      <Badge variant="secondary">
        Execution: {executionLabel(experiment.executionState)}
      </Badge>
      <Badge variant="outline">
        Verdict:{" "}
        {experiment.evidenceCompletion === CompletionState.COMPLETE
          ? verdictLabel(experiment.verdict)
          : "Inconclusive · incomplete evidence"}
      </Badge>
      <Badge variant="outline">
        Evidence: {completenessLabel(experiment.evidenceCompletion)}
      </Badge>
    </div>
  );
}
export function History({
  spaceId,
  connectionId,
  open,
}: {
  spaceId: string;
  connectionId: string;
  open: (id: string) => void;
}) {
  const requestId = useRequestId(),
    [cursors, setCursors] = useState([""]),
    [execution, setExecution] = useState(0),
    [verdict, setVerdict] = useState(0);
  const list = useQuery(
    ExperimentService.method.listExperiments,
    {
      spaceId,
      connectionId,
      requestId,
      page: { pageSize: 20, cursor: cursors.at(-1) },
      executionState: execution || undefined,
      verdict: verdict || undefined,
    },
    { refetchInterval: 15_000 },
  );
  return (
    <div className="stack">
      <p className="lead">
        Every result has a provenance. Every verdict has evidence.
      </p>
      <div className="filters">
        <label className="field">
          Execution state
          <NativeSelect
            value={execution}
            onChange={(e) => {
              setExecution(Number(e.target.value));
              setCursors([""]);
            }}
          >
            <option value={0}>All execution states</option>
            {[1, 2, 3, 4, 5].map((v) => (
              <option value={v} key={v}>
                {executionLabel(v)}
              </option>
            ))}
          </NativeSelect>
        </label>
        <label className="field">
          Performance verdict
          <NativeSelect
            value={verdict}
            onChange={(e) => {
              setVerdict(Number(e.target.value));
              setCursors([""]);
            }}
          >
            <option value={0}>All verdicts</option>
            {[1, 2, 3, 4].map((v) => (
              <option value={v} key={v}>
                {verdictLabel(v)}
              </option>
            ))}
          </NativeSelect>
        </label>
      </div>
      {list.isPending ? (
        <Notice title="Loading experiments…" />
      ) : list.error ? (
        <Failure error={list.error} retry={() => void list.refetch()} />
      ) : !list.data?.experiments.length ? (
        <Notice title="No experiments yet">
          Start a cloud experiment when a tool is available, or upload local
          results with <code>perfine upload</code>. Try clearing filters if you
          expected history.
        </Notice>
      ) : (
        list.data.experiments.map((e) => (
          <Card className="experiment-row" key={e.id}>
            <div>
              <p className="eyebrow">
                {e.createdAt
                  ? new Date(Number(e.createdAt.seconds) * 1000).toLocaleString(
                      "en",
                    )
                  : "Date unavailable"}
              </p>
              <h2>
                <button onClick={() => open(e.id)}>
                  Experiment {e.id.slice(-8)} <span aria-hidden>→</span>
                </button>
              </h2>
              <p className="sha">
                Base {e.specification?.baseSha.slice(0, 12) || "Unavailable"}
              </p>
              <Status experiment={e} />
              <p>{e.statusReason}</p>
            </div>
            <Button variant="outline" onClick={() => open(e.id)}>
              Inspect result
            </Button>
          </Card>
        ))
      )}
      <Pager
        busy={list.isFetching}
        previous={
          cursors.length > 1
            ? () => setCursors((c) => c.slice(0, -1))
            : undefined
        }
        next={
          list.data?.page?.nextCursor
            ? () => setCursors((c) => [...c, list.data!.page!.nextCursor])
            : undefined
        }
      />
    </div>
  );
}
export function Detail({
  spaceId,
  connection,
  experimentId,
  permissions,
  back,
}: {
  spaceId: string;
  connection: Connection;
  experimentId: string;
  permissions: Permission[];
  back: () => void;
}) {
  const requestId = useRequestId(),
    cache = useQueryClient();
  const experiment = useQuery(
    ExperimentService.method.getExperiment,
    { spaceId, experimentId, requestId },
    { refetchInterval: 5000 },
  );
  const [cursors, setCursors] = useState([""]);
  const candidates = useQuery(
    ExperimentService.method.listCandidates,
    {
      spaceId,
      experimentId,
      requestId,
      page: { pageSize: 20, cursor: cursors.at(-1) },
    },
    {
      enabled: !!experiment.data && !experiment.error,
      refetchInterval: 10_000,
    },
  );
  const [candidateId, setCandidate] = useState(""),
    [review, setReview] = useState(false),
    [deleted, setDeleted] = useState(false);
  const candidate = useQuery(
    ExperimentService.method.getCandidate,
    { spaceId, experimentId, candidateId, requestId },
    {
      enabled: !!candidateId && !experiment.error && !deleted,
      refetchInterval: 10_000,
    },
  );
  const cancel = useMutation(ExperimentService.method.cancelExperiment),
    resume = useMutation(ExperimentService.method.resumeExperiment);
  const [fingerprint, setFingerprint] = useState("");
  const e = experiment.data?.experiment,
    c = candidate.error ? undefined : candidate.data?.candidate;
  const operator =
    permissions.includes(Permission.EXPERIMENT) &&
    connection.state === ConnectionState.CONNECTED;
  if (deleted)
    return (
      <>
        <Button variant="outline" onClick={back}>
          Back to history
        </Button>
        <Notice title="Deletion accepted">
          This experiment is no longer readable. A manager can inspect cleanup
          status in Settings using its ID.
        </Notice>
      </>
    );
  if (experiment.isPending) return <Notice title="Loading experiment…" />;
  if (experiment.error)
    return (
      <Failure
        error={experiment.error}
        retry={() => void experiment.refetch()}
      />
    );
  if (!e) return <Notice title="Experiment unavailable" />;
  if (review && c)
    return (
      <Review
        key={`${c.id}/${c.headSha}/${c.evidenceManifestSha256}`}
        spaceId={spaceId}
        experiment={e}
        candidate={c}
        connection={connection}
        allowed={permissions.includes(Permission.PUBLISH)}
        back={() => {
          setReview(false);
          requestAnimationFrame(() =>
            document.getElementById("review-button")?.focus(),
          );
        }}
      />
    );
  const running = [ExecutionState.RUNNING, ExecutionState.QUEUED].includes(
    e.executionState,
  );
  return (
    <div className="stack">
      <Button className="back" variant="ghost" onClick={back}>
        ← Experiment history
      </Button>
      <Card className="panel">
        <div className="section-heading">
          <h2>Experiment {e.id.slice(-8)}</h2>
          <span className="muted">Private result</span>
        </div>
        <Status experiment={e} />
        {e.statusReason && <p>{e.statusReason}</p>}
        {e.executionState === ExecutionState.CANCELLED && (
          <Notice title="Execution cancelled">
            Completed evidence and candidate branches are retained. Incomplete
            work cannot establish a performance pass.
          </Notice>
        )}
        {e.executionState === ExecutionState.FAILED && (
          <Notice title="Execution failed">
            Inspect the status reason and evidence. Correct the cause before
            resuming with matching inputs.
          </Notice>
        )}
        {(e.verdict === PerformanceVerdict.INCONCLUSIVE ||
          e.evidenceCompletion !== CompletionState.COMPLETE) && (
          <Notice title="Evidence does not establish a pass">
            Review missing evidence and measurement uncertainty. Preserve
            original evidence locally or on GitHub, and resend eligible
            artifacts.
          </Notice>
        )}
        {e.cancellation && (
          <Notice
            title={`Cancellation: ${e.cancellation.reason || "Requested"}`}
          >
            {e.cancellation.directCancelUrl && (
              <External github url={e.cancellation.directCancelUrl}>
                Cancel directly in GitHub if automatic cancellation failed
              </External>
            )}
          </Notice>
        )}
        <div className="actions">
          <Confirm
            label="Graceful stop"
            description="Stop starting new candidates. Active candidates may finish within their individual timeout; completed evidence is retained."
            disabled={!operator || !running}
            pending={cancel.isPending}
            onConfirm={() =>
              cancel.mutate(
                {
                  spaceId,
                  experimentId,
                  requestId: uuid7(),
                  mode: CancellationMode.GRACEFUL,
                },
                { onSuccess: () => void cache.invalidateQueries() },
              )
            }
          />
          <Confirm
            label="Cancel immediately"
            description="Stop new and active candidate work. Preserve completed evidence and branches; incomplete work has no performance conclusion."
            disabled={!operator || !running}
            pending={cancel.isPending}
            onConfirm={() =>
              cancel.mutate(
                {
                  spaceId,
                  experimentId,
                  requestId: uuid7(),
                  mode: CancellationMode.IMMEDIATE,
                },
                { onSuccess: () => void cache.invalidateQueries() },
              )
            }
          />
        </div>
        {cancel.error && <Failure error={cancel.error} />}{" "}
        {!running && operator && (
          <form
            className="stack"
            onSubmit={(ev) => {
              ev.preventDefault();
              resume.mutate(
                {
                  spaceId,
                  experimentId,
                  requestId: uuid7(),
                  expectedFingerprintSha256: fingerprint,
                },
                { onSuccess: () => void cache.invalidateQueries() },
              );
            }}
          >
            <label className="field">
              Current local specification fingerprint
              <input
                className="plain-input"
                required
                pattern="[a-fA-F0-9]{64}"
                value={fingerprint}
                onChange={(ev) => setFingerprint(ev.target.value)}
              />
            </label>
            <p>
              Resume reuses completed steps only when source, inputs,
              tools/environment and policy match. A mismatch requires a new
              experiment.
            </p>
            <Button
              disabled={
                resume.isPending ||
                fingerprint !== e.specification?.fingerprintSha256
              }
            >
              Resume matching experiment
            </Button>
            {resume.error && <Failure error={resume.error} />}
          </form>
        )}
      </Card>
      <section className="stack" aria-label="Candidates">
        <h2>Candidates</h2>
        {candidates.isPending ? (
          <Notice title="Loading candidates…" />
        ) : candidates.error ? (
          <Failure
            error={candidates.error}
            retry={() => void candidates.refetch()}
          />
        ) : !candidates.data?.candidates.length ? (
          <Notice title={running ? "No candidates yet" : "No candidate"}>
            {running
              ? "Profile-backed candidates will appear here."
              : "A completed experiment with no candidate is a valid outcome, not an error."}
          </Notice>
        ) : (
          <label className="field">
            Candidate
            <NativeSelect
              value={candidateId}
              onChange={(ev) => setCandidate(ev.target.value)}
            >
              <option value="">Select a candidate</option>
              {candidates.data.candidates.map((x) => (
                <option value={x.id} key={x.id}>
                  {x.headSha.slice(0, 12)} · {verdictLabel(x.verdict)}
                </option>
              ))}
            </NativeSelect>
          </label>
        )}
        <Pager
          previous={
            cursors.length > 1
              ? () => setCursors((x) => x.slice(0, -1))
              : undefined
          }
          next={
            candidates.data?.page?.nextCursor
              ? () =>
                  setCursors((x) => [...x, candidates.data!.page!.nextCursor])
              : undefined
          }
        />
        {candidateId && candidate.isPending && (
          <Notice title="Loading candidate…" />
        )}
        {candidate.error && (
          <Failure
            error={candidate.error}
            retry={() => void candidate.refetch()}
          />
        )}{" "}
        {c && (
          <>
            <CandidateEvidence
              candidate={c}
              complete={e.evidenceCompletion === CompletionState.COMPLETE}
            />
            <div className="actions">
              <External
                github
                url={candidateDiffURL(c, connection.repository) || ""}
              >
                Inspect exact GitHub diff
              </External>
              <Button
                id="review-button"
                disabled={
                  !permissions.includes(Permission.PUBLISH) ||
                  connection.state !== ConnectionState.CONNECTED ||
                  e.evidenceCompletion !== CompletionState.COMPLETE
                }
                onClick={() => setReview(true)}
              >
                Review draft publication
              </Button>
            </div>
          </>
        )}
      </section>
      <Evidence
        key={candidateId}
        spaceId={spaceId}
        experimentId={experimentId}
        candidateId={candidateId}
      />
      {e.specification && (
        <details className="panel">
          <summary>Immutable inputs, limits and policy</summary>
          <Specification spec={e.specification} />
        </details>
      )}
      {permissions.includes(Permission.MANAGE) && (
        <DeleteResult
          spaceId={spaceId}
          experimentId={experimentId}
          onDeleted={() => setDeleted(true)}
        />
      )}
    </div>
  );
}
export function CandidateEvidence({
  candidate: c,
  complete = true,
}: {
  candidate: Candidate;
  complete?: boolean;
}) {
  return (
    <div className="stack">
      <Card className="panel">
        <h2>Candidate {c.headSha.slice(0, 12)}</h2>
        <div className="status-row">
          <Badge variant="secondary">
            Execution: {executionLabel(c.executionState)}
          </Badge>
          <Badge variant="outline">
            Performance verdict:{" "}
            {complete
              ? verdictLabel(c.verdict)
              : "Inconclusive · incomplete evidence"}
          </Badge>
        </div>
        <dl className="facts">
          <div>
            <dt>Base SHA</dt>
            <dd>
              <code>{c.baseSha}</code>
            </dd>
          </div>
          <div>
            <dt>Head SHA</dt>
            <dd>
              <code>{c.headSha}</code>
            </dd>
          </div>
          <div>
            <dt>Evidence manifest</dt>
            <dd>
              <code>{c.evidenceManifestSha256 || "Missing"}</code>
            </dd>
          </div>
        </dl>
        <h3>Correctness</h3>
        {c.correctness.length ? (
          c.correctness.map((x) => (
            <p key={x.checkName}>
              {x.checkName}:{" "}
              <strong>
                {CorrectnessState[x.state]?.toLowerCase() || "Unknown"}
              </strong>{" "}
              · {x.artifactName}
            </p>
          ))
        ) : (
          <Notice title="Correctness evidence missing" />
        )}
        {c.humanJudgmentRequired && (
          <Notice title="Explicit human judgment required">
            Review protected-case or large regressions and justify publication.
          </Notice>
        )}
        {c.exceptions.map((x, i) => (
          <Notice
            key={i}
            title={`Policy exception: ${PolicyException[x]?.replaceAll("_", " ").toLowerCase()}`}
          />
        ))}
      </Card>
      {!c.comparisons.length && (
        <Notice title="Comparison evidence missing">
          No performance conclusion is available.
        </Notice>
      )}
      {c.comparisons.map((x, index) => (
        <Card className="panel" key={index}>
          <div className="section-heading">
            <h2>
              {ComparisonKind[x.kind]?.replaceAll("_", " ").toLowerCase()}{" "}
              comparison
            </h2>
            <Badge variant="outline">
              {complete ? verdictLabel(x.verdict) : "Inconclusive"}
            </Badge>
          </div>
          <p>
            {x.metric?.name === "Ir"
              ? "Ir · instruction count, not elapsed time"
              : `${x.metric?.name} · ${x.metric?.unit}`}
          </p>
          <p className="sha">
            {x.baseSha} → {x.headSha}
          </p>
          <dl className="metrics">
            <div>
              <dt>Portfolio improvement</dt>
              <dd>{percent(x.improvement)}</dd>
            </div>
            <div>
              <dt>Weighted score</dt>
              <dd>{x.portfolioScore.toPrecision(6)}</dd>
            </div>
            <div>
              <dt>Geometric mean · supplemental</dt>
              <dd>{x.geometricMeanRatio.toPrecision(6)}</dd>
            </div>
          </dl>
          <div className="comparison-cases">
            {x.cases.map((m) => (
              <article className="case-card" key={m.caseId}>
                <h3>{m.caseId}</h3>
                <dl className="case-values">
                  <div>
                    <dt>Base</dt>
                    <dd>
                      {m.baseValue.toLocaleString("en")} {x.metric?.unit}
                    </dd>
                  </div>
                  <div>
                    <dt>Head</dt>
                    <dd>
                      {m.headValue.toLocaleString("en")} {x.metric?.unit}
                    </dd>
                  </div>
                  <div>
                    <dt>Change (head − base)</dt>
                    <dd>
                      {m.baseValue === 0
                        ? "Unavailable (zero baseline)"
                        : percent((m.headValue - m.baseValue) / m.baseValue)}
                    </dd>
                  </div>
                  <div>
                    <dt>Weighted absolute change</dt>
                    <dd>{m.weightedAbsoluteChange.toLocaleString("en")}</dd>
                  </div>
                  <div>
                    <dt>Repetitions</dt>
                    <dd>{m.repetitions}</dd>
                  </div>
                </dl>
              </article>
            ))}
          </div>
          {x.humanJudgmentRequired && (
            <Notice title="Comparison requires human judgment" />
          )}
          {x.exceptions.map((ex, i) => (
            <p key={i}>
              Exception:{" "}
              {PolicyException[ex]?.replaceAll("_", " ").toLowerCase()}
            </p>
          ))}
          <p className="muted">
            Lossless engine evidence: {x.engineArtifactName || "Missing"}.
            Claims apply only to the measured environment. Summary numbers do
            not replace the engine evaluation.
          </p>
        </Card>
      ))}
    </div>
  );
}
function Evidence({
  spaceId,
  experimentId,
  candidateId,
}: {
  spaceId: string;
  experimentId: string;
  candidateId: string;
}) {
  const requestId = useRequestId(),
    [cursors, setCursors] = useState([""]);
  const list = useQuery(EvidenceService.method.listEvidence, {
    spaceId,
    experimentId,
    candidateId,
    requestId,
    page: { pageSize: 20, cursor: cursors.at(-1) },
  });
  const download = useMutation(EvidenceService.method.getEvidenceDownload);
  const [link, setLink] = useState<{
    url: string;
    name: string;
    expires: number;
  }>();
  return (
    <Card className="panel stack">
      <h2>Evidence</h2>
      <p>
        Download links are issued only after a fresh permission check and expire
        after 15 minutes. Existing links may survive permission revocation or
        deletion until expiry.
      </p>
      {list.isPending ? (
        <Notice title="Loading evidence…" />
      ) : list.error ? (
        <Failure error={list.error} retry={() => void list.refetch()} />
      ) : !list.data?.manifests.length ? (
        <Notice title="No hosted evidence">
          Original artifacts may remain locally or on GitHub. Incomplete hosted
          evidence cannot establish a pass.
        </Notice>
      ) : (
        list.data.manifests.map((m) => (
          <div className="evidence-group" key={m.manifestSha256}>
            <h3>{completenessLabel(m.completion)} manifest</h3>
            <p className="sha">{m.manifestSha256}</p>
            <p>
              Source: <code>{m.sourceBaseSha}</code> →{" "}
              <code>{m.sourceHeadSha}</code>
            </p>
            <p>
              Harness: <code>{m.harnessSha}</code>
            </p>
            {m.artifacts.map((a) => (
              <div className="artifact" key={a.name}>
                <div>
                  <strong>{a.name}</strong>
                  <p>
                    {ArtifactKind[a.kind]?.replaceAll("_", " ").toLowerCase()} ·{" "}
                    {a.sizeBytes.toString()} bytes ·{" "}
                    {completenessLabel(a.completion)}
                    {a.requiredForEvaluation
                      ? " · Required for evaluation"
                      : ""}
                  </p>
                  <code>{a.sha256}</code>
                </div>
                <Button
                  variant="outline"
                  disabled={
                    a.completion !== CompletionState.COMPLETE ||
                    download.isPending
                  }
                  onClick={() => {
                    setLink(undefined);
                    download.mutate(
                      {
                        spaceId,
                        experimentId,
                        requestId: uuid7(),
                        manifestSha256: m.manifestSha256,
                        artifactName: a.name,
                      },
                      {
                        onSuccess: (r) =>
                          setLink({
                            url: r.signedGetUrl,
                            name: a.name,
                            expires: Number(r.expiresAt?.seconds || 0) * 1000,
                          }),
                      },
                    );
                  }}
                >
                  Get download link
                </Button>
              </div>
            ))}
          </div>
        ))
      )}
      {download.error && <Failure error={download.error} />}{" "}
      {link && link.expires > Date.now() && (
        <Notice title={`Download ready: ${link.name}`}>
          <External url={link.url}>Download evidence</External>
          <p>
            Expires {new Date(link.expires).toLocaleTimeString("en")}. Request a
            new link after expiry.
          </p>
        </Notice>
      )}
      <Pager
        previous={
          cursors.length > 1
            ? () => setCursors((c) => c.slice(0, -1))
            : undefined
        }
        next={
          list.data?.page?.nextCursor
            ? () => setCursors((c) => [...c, list.data!.page!.nextCursor])
            : undefined
        }
      />
    </Card>
  );
}
