// SPDX-License-Identifier: Apache-2.0
import { useRef, useState } from "react";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import {
  CloudService,
  CloudEligibilityState,
  ConnectionState,
  ExperimentService,
  PolicyKind,
  MetricObjective,
  Language,
  FrequencySource,
  PolicyException,
  type Connection,
  type ExperimentSpecification,
} from "@delino/protos/perfine/v1/perfine_pb";
import {
  ConfigurationForm,
  emptyConfiguration,
  validateConfiguration,
  importConfiguration,
} from "./configuration";
import { NativeSelect } from "@delino/ui/components/native-select";
import { Button, Card, Failure, Notice, Pager } from "./ui";
import { percent, useRequestId, uuid7 } from "./core";
export function Specification({ spec }: { spec: ExperimentSpecification }) {
  const p = spec.policy,
    l = spec.limits;
  return (
    <div className="stack">
      <dl className="facts">
        <div>
          <dt>Base SHA</dt>
          <dd>
            <code>{spec.baseSha}</code>
          </dd>
        </div>
        <div>
          <dt>Initial base SHA</dt>
          <dd>
            <code>{spec.initialBaseSha}</code>
          </dd>
        </div>
        <div>
          <dt>Harness SHA</dt>
          <dd>
            <code>{spec.harnessSha}</code>
          </dd>
        </div>
        <div>
          <dt>Language</dt>
          <dd>{Language[spec.language]}</dd>
        </div>
        <div>
          <dt>Candidate limit / simultaneous</dt>
          <dd>
            {l?.candidateCount} / {l?.simultaneousCandidates}
          </dd>
        </div>
        <div>
          <dt>Deadline for new starts</dt>
          <dd>
            {l?.startDeadline
              ? new Date(Number(l.startDeadline.seconds) * 1000).toLocaleString(
                  "en",
                )
              : "Missing"}
          </dd>
        </div>
        <div>
          <dt>Per-candidate timeout</dt>
          <dd>{l?.candidateTimeout?.seconds.toString()} seconds</dd>
        </div>
        <div>
          <dt>Metric / objective</dt>
          <dd>
            {p?.metric?.name === "Ir"
              ? "Ir · instruction count (not elapsed time)"
              : `${p?.metric?.name} · ${p?.metric?.unit}`}{" "}
            · {p?.metric && MetricObjective[p.metric.objective]?.toLowerCase()}
          </dd>
        </div>
        <div>
          <dt>Policy</dt>
          <dd>{p && PolicyKind[p.kind]?.replaceAll("_", " ")}</dd>
        </div>
        <div>
          <dt>Minimum portfolio improvement</dt>
          <dd>{p ? percent(p.minimumImprovement) : "Missing"}</dd>
        </div>
        <div>
          <dt>Maximum case regression</dt>
          <dd>{p ? percent(p.maximumCaseRegression) : "Missing"}</dd>
        </div>
        <div>
          <dt>Repetitions</dt>
          <dd>{p?.repetitions}</dd>
        </div>
        <div>
          <dt>Maximum relative sample range / decision margin</dt>
          <dd>
            {p?.uncertainty
              ? `${p.uncertainty.maximumRelativeRange === undefined ? "Not applicable" : percent(p.uncertainty.maximumRelativeRange)} / ${p.uncertainty.decisionMargin === undefined ? "Not applicable" : percent(p.uncertainty.decisionMargin)}`
              : "Not declared"}
          </dd>
        </div>
        <div>
          <dt>Near-threshold margin</dt>
          <dd>
            {p?.uncertainty?.nearThreshold !== undefined
              ? percent(p.uncertainty.nearThreshold)
              : "Not declared"}
          </dd>
        </div>
        <div>
          <dt>Repeat tolerance</dt>
          <dd>
            {p?.uncertainty?.repeatTolerance === undefined
              ? "Not declared"
              : percent(p.uncertainty.repeatTolerance)}
          </dd>
        </div>
        <div>
          <dt>Workflow SHA</dt>
          <dd>
            <code>{spec.workflowSha}</code>
          </dd>
        </div>
        <div>
          <dt>Correctness checks</dt>
          <dd>{spec.correctnessChecks.join(", ") || "Missing"}</dd>
        </div>
        <div>
          <dt>Workload confirmation</dt>
          <dd>{spec.workloadConfirmation || "Missing"}</dd>
        </div>
        <div>
          <dt>Input hashes</dt>
          <dd>
            {spec.inputs.map((i) => (
              <p key={i.name}>
                {i.name}: <code>{i.sha256}</code>
              </p>
            ))}
          </dd>
        </div>
        <div>
          <dt>Hardware</dt>
          <dd>
            {Object.entries(spec.environment?.hardware || {})
              .map(([k, v]) => `${k}: ${v}`)
              .join(", ") || "None declared"}
          </dd>
        </div>
        <div>
          <dt>Allowed policy exceptions</dt>
          <dd>
            {p?.allowedExceptions
              .map((x) =>
                PolicyException[x]?.replaceAll("_", " ").toLowerCase(),
              )
              .join(", ") || "None"}
          </dd>
        </div>
        <div>
          <dt>Environment</dt>
          <dd>
            {spec.environment?.operatingSystem} /{" "}
            {spec.environment?.architecture} · {spec.environment?.runner} ·{" "}
            {spec.environment?.runtime} {spec.environment?.runtimeVersion}
          </dd>
        </div>
        <div>
          <dt>Build options / runtime flags</dt>
          <dd>
            {spec.environment?.buildOptions.join(" ") || "None"} /{" "}
            {spec.environment?.runtimeFlags.join(" ") || "None"}
          </dd>
        </div>
        <div>
          <dt>Tools</dt>
          <dd>
            {spec.environment?.tools
              .map((t) => `${t.name} ${t.version}`)
              .join(", ")}
          </dd>
        </div>
        <div>
          <dt>Fingerprint</dt>
          <dd>
            <code>{spec.fingerprintSha256}</code>
          </dd>
        </div>
        <div>
          <dt>Policy hash</dt>
          <dd>
            <code>{p?.policySha256}</code>
          </dd>
        </div>
      </dl>
      <h3>Fixed workload cases</h3>
      {spec.cases.map((c) => (
        <div className="case-card" key={c.id}>
          <strong>{c.id}</strong>
          <p>
            Family: {c.family} · Frequency: {c.frequency}
            {c.frequencySource === FrequencySource.ASSUMED_ONE
              ? " (unknown; assumed 1)"
              : ""}{" "}
            · {c.protectedCase ? "Protected" : "Unprotected"}
          </p>
          {c.inputs.map((i) => (
            <p key={i.name}>
              {i.name}: <code>{i.sha256}</code>
            </p>
          ))}
        </div>
      ))}
      {spec.parentCandidateIds.length > 0 && (
        <Notice title="Dependent experiment">
          Incremental and cumulative comparisons are required. Parents:{" "}
          {spec.parentCandidateIds.join(", ")}
        </Notice>
      )}
    </div>
  );
}
export function Start({
  spaceId,
  connection,
  connectionId,
  allowed,
  onStarted,
}: {
  spaceId: string;
  connection?: Connection;
  connectionId?: string;
  allowed: boolean;
  onStarted: (id: string) => void;
}) {
  const id = connection?.id || connectionId || "";
  const requestId = useRequestId(),
    [cursors, setCursors] = useState([""]);
  const tools = useQuery(
    CloudService.method.listCloudTools,
    {
      spaceId,
      connectionId: id,
      requestId,
      page: { pageSize: 100, cursor: cursors.at(-1) },
    },
    { enabled: allowed && !!id },
  );
  const preparation = useMutation(ExperimentService.method.prepareExperiment);
  const [configuration, setConfiguration] = useState(emptyConfiguration);
  const [engineConfig, setEngineConfig] = useState("");
  const revision = useRef(0);
  const start = useMutation(ExperimentService.method.startExperiment);
  const [spec, setSpec] = useState<ExperimentSpecification>(),
    [error, setError] = useState<unknown>(),
    [tool, setTool] = useState(""),
    [confirmed, setConfirmed] = useState(false),
    [identity, setIdentity] = useState(() => ({
      requestId: uuid7(),
      experimentId: uuid7(),
    }));
  const eligible =
    tools.data?.tools.filter(
      (t) => t.state === CloudEligibilityState.ELIGIBLE,
    ) || [];
  const enabled =
    allowed &&
    (!connection || connection.state === ConnectionState.CONNECTED) &&
    !tools.error &&
    eligible.some((t) => t.toolId === tool);
  return (
    <div className="stack">
      <p className="lead">Freeze the inputs. Set the limits. Then measure.</p>
      <Notice title="Cloud execution eligibility">
        {!allowed
          ? "Experiment permission is required."
          : tools.isPending
            ? "Checking supported tools…"
            : eligible.length
              ? "Choose a verified tool below. Authentication stays inside your repository’s GitHub Actions."
              : "Cloud execution unavailable. No eligible tool is currently established. Continue locally with perfine optimize."}
      </Notice>
      {tools.error && (
        <Failure error={tools.error} retry={() => void tools.refetch()} />
      )}{" "}
      {tools.data?.tools.map((t) => (
        <Card className="panel" key={t.toolId}>
          <h2>{t.displayName}</h2>
          <p>
            {t.state === CloudEligibilityState.ELIGIBLE
              ? "Available"
              : `Unavailable: ${t.reason || "Official unattended support is not verified."}`}
          </p>
          <p>
            Local alternative:{" "}
            <code>{t.localAlternative || "perfine optimize"}</code>
          </p>
        </Card>
      ))}
      {tools.data?.page?.nextCursor || cursors.length > 1 ? (
        <Pager
          previous={
            cursors.length > 1
              ? () => setCursors((c) => c.slice(0, -1))
              : undefined
          }
          next={
            tools.data?.page?.nextCursor
              ? () => setCursors((c) => [...c, tools.data!.page!.nextCursor])
              : undefined
          }
        />
      ) : null}
      <Card className="panel">
        <h2>Execution limits and policy</h2>
        <p>
          Enter reproducible settings and confirm the workload before
          preparation. The server computes fingerprints. Started experiments
          remain immutable; edits here prepare a new experiment.
        </p>
        <label className="field">
          Import configuration or specification JSON (optional)
          <input
            type="file"
            accept=".json,application/json"
            disabled={!allowed || start.isPending || start.isSuccess}
            onChange={async (e) => {
              const file = e.target.files?.[0];
              if (!file) return;
              const current = ++revision.current;
              setSpec(undefined);
              setEngineConfig("");
              setConfirmed(false);
              setError(undefined);
              try {
                if (file.size > 2_000_000)
                  throw new Error(
                    "Configuration exceeds 2 MB. Import metadata only.",
                  );
                const next = importConfiguration(await file.text());
                if (revision.current === current) setConfiguration(next);
              } catch (error) {
                if (revision.current === current) setError(error);
              }
            }}
          />
        </label>
        <ConfigurationForm
          value={configuration}
          disabled={!allowed || start.isPending || start.isSuccess}
          onChange={(next) => {
            revision.current++;
            setConfiguration(next);
            setSpec(undefined);
            setEngineConfig("");
            setConfirmed(false);
            setError(undefined);
            setIdentity({ requestId: uuid7(), experimentId: uuid7() });
          }}
          onPrepare={() => {
            try {
              validateConfiguration(configuration);
            } catch (e) {
              setError(e);
              return;
            }
            const current = ++revision.current;
            setSpec(undefined);
            setEngineConfig("");
            setConfirmed(false);
            setError(undefined);
            preparation.mutate(
              { requestId: uuid7(), spaceId, connectionId: id, configuration },
              {
                onSuccess: (r) => {
                  // A response for earlier settings must never restore an obsolete review.
                  if (revision.current !== current) return;
                  if (!r.specification || !r.engineConfigJson) {
                    setError(
                      new Error(
                        "Preparation returned incomplete data. Prepare again.",
                      ),
                    );
                    return;
                  }
                  setSpec(r.specification);
                  setEngineConfig(r.engineConfigJson);
                  setIdentity({ requestId: uuid7(), experimentId: uuid7() });
                },
                onError: (e) => {
                  if (revision.current === current) setError(e);
                },
              },
            );
          }}
          pending={preparation.isPending}
        />
        {!!error && (
          <>
            <Failure error={error} />
            <p>
              Check the source revisions, workload hashes, tools, limits and
              policy above, then prepare again.
            </p>
          </>
        )}
        {spec && (
          <section aria-label="Prepared experiment review" className="stack">
            <h2>Review prepared experiment</h2>
            <Specification spec={spec} />
            <a
              download="perfine-config.json"
              href={`data:application/json;charset=utf-8,${encodeURIComponent(engineConfig)}`}
            >
              Download prepared engine configuration
            </a>
            <p>
              Review this configuration for local use with perfine optimize.
              Preparation does not start execution.
            </p>
          </section>
        )}
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            if (!enabled || !spec || !confirmed || start.isSuccess) return;
            try {
              if (
                Number(spec.limits!.startDeadline!.seconds) * 1000 <=
                Date.now()
              )
                throw new Error(
                  "The start deadline has passed. Prepare a new specification.",
                );
              start.mutate(
                {
                  spaceId,
                  connectionId: id,
                  ...identity,
                  specification: spec,
                  cloudToolId: tool,
                },
                {
                  onSuccess: (r) => r.experiment && onStarted(r.experiment.id),
                },
              );
            } catch (e) {
              setError(e);
            }
          }}
        >
          <label className="field">
            Cloud tool
            <NativeSelect
              value={tool}
              disabled={!allowed || start.isPending || start.isSuccess}
              onChange={(e) => {
                revision.current++;
                setSpec(undefined);
                setEngineConfig("");
                setConfirmed(false);
                setTool(e.target.value);
                setIdentity({ requestId: uuid7(), experimentId: uuid7() });
              }}
            >
              <option value="">Select an eligible tool</option>
              {eligible.map((t) => (
                <option key={t.toolId} value={t.toolId}>
                  {t.displayName}
                </option>
              ))}
            </NativeSelect>
          </label>
          <label className="check">
            <input
              type="checkbox"
              disabled={!spec || start.isPending || start.isSuccess}
              checked={confirmed}
              onChange={(e) => setConfirmed(e.target.checked)}
            />
            I confirm these workloads represent the intended use and approve
            these immutable limits and policies.
          </label>
          <Button
            disabled={
              !enabled ||
              !spec ||
              !confirmed ||
              start.isPending ||
              start.isSuccess
            }
            type="submit"
          >
            {start.isPending ? "Starting…" : "Start cloud experiment"}
          </Button>
        </form>
        {start.error && <Failure error={start.error} />}
        <p className="muted">
          The overall deadline stops new candidates; active candidates may
          finish within their timeout. Graceful stop preserves completed work.
          Immediate cancel also stops active work. Request deduplication lasts
          30 days from creation; after that, reconcile state before
          resubmitting.
        </p>
      </Card>
    </div>
  );
}
