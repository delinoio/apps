// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import { useMutation } from "@connectrpc/connect-query";
import {
  CancellationMode,
  CancellationState,
  ExperimentService,
} from "@delino/protos/perfine/v1/perfine_pb";
import { Button, Card, Confirm, External, Failure, Field, Notice } from "./ui";
import { Start } from "./start";
import { executionLabel, uuid7 } from "./core";
// Experiment grants do not imply READ. Operate explicit IDs without querying
// connection metadata, history, candidates or evidence under another grant.
export function Operations({ spaceId }: { spaceId: string }) {
  const [connectionId, setConnection] = useState(""),
    [experimentId, setExperiment] = useState(""),
    [fingerprint, setFingerprint] = useState(""),
    [setup, setSetup] = useState(false);
  const [ids, setIds] = useState(() => ({
    graceful: uuid7(),
    immediate: uuid7(),
    resume: uuid7(),
  }));
  const cancel = useMutation(ExperimentService.method.cancelExperiment, {
      onSuccess: (response, request) => {
        if (
          response.experiment?.cancellation?.state !== CancellationState.FAILED
        )
          return;
        const mode =
          request.mode === CancellationMode.IMMEDIATE
            ? "immediate"
            : "graceful";
        setIds((current) =>
          current[mode] === request.requestId
            ? { ...current, [mode]: uuid7() }
            : current,
        );
      },
    }),
    resume = useMutation(ExperimentService.method.resumeExperiment);
  const result = cancel.data?.experiment || resume.data?.experiment;
  if (setup)
    return (
      <>
        <Button variant="ghost" onClick={() => setSetup(false)}>
          ← Operations
        </Button>
        <Start
          spaceId={spaceId}
          connectionId={connectionId}
          allowed
          onStarted={(id) => {
            setExperiment(id);
            setSetup(false);
          }}
        />
      </>
    );
  return (
    <div className="stack">
      <Notice title="Experiment operations without result access">
        Your experiment permission permits start, stop and resume. It does not
        permit browsing private history or evidence. Obtain resource IDs from
        your space manager or local upload record.
      </Notice>
      <Card className="panel">
        <h2>Start cloud setup</h2>
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            setSetup(true);
          }}
        >
          <Field
            label="Connection ID"
            autoFocus
            required
            value={connectionId}
            onChange={(e) => setConnection(e.target.value)}
          />
          <Button>Review execution setup</Button>
        </form>
      </Card>
      <Card className="panel">
        <h2>Control an existing experiment</h2>
        <Field
          label="Experiment ID"
          value={experimentId}
          onChange={(e) => {
            setExperiment(e.target.value);
            setIds({ graceful: uuid7(), immediate: uuid7(), resume: uuid7() });
            cancel.reset();
            resume.reset();
          }}
        />
        <div className="actions">
          <Confirm
            label="Graceful stop"
            disabled={!experimentId}
            pending={cancel.isPending}
            description="Stop new starts and let active candidates finish within their timeout. Completed evidence is retained."
            onConfirm={() =>
              cancel.mutate({
                spaceId,
                experimentId,
                requestId: ids.graceful,
                mode: CancellationMode.GRACEFUL,
              })
            }
          />
          <Confirm
            label="Cancel immediately"
            disabled={!experimentId}
            pending={cancel.isPending}
            description="Stop new and active candidates. Completed evidence and branches are retained."
            onConfirm={() =>
              cancel.mutate({
                spaceId,
                experimentId,
                requestId: ids.immediate,
                mode: CancellationMode.IMMEDIATE,
              })
            }
          />
        </div>
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            resume.mutate({
              spaceId,
              experimentId,
              requestId: ids.resume,
              expectedFingerprintSha256: fingerprint,
            });
          }}
        >
          <Field
            label="Current local specification fingerprint"
            value={fingerprint}
            pattern="[a-fA-F0-9]{64}"
            required
            onChange={(e) => {
              setFingerprint(e.target.value);
              setIds((x) => ({ ...x, resume: uuid7() }));
            }}
          />
          <p>
            Resume only when source, inputs, tools/environment and policy match.
            A mismatch requires a new experiment.
          </p>
          <Button disabled={!experimentId || resume.isPending}>
            Resume matching experiment
          </Button>
        </form>
        {cancel.error && <Failure error={cancel.error} />}{" "}
        {resume.error && <Failure error={resume.error} />}{" "}
        {result && (
          <Notice title={`Execution: ${executionLabel(result.executionState)}`}>
            {result.statusReason}
            {result.cancellation?.reason && <p>{result.cancellation.reason}</p>}
            {result.cancellation?.directCancelUrl && (
              <External github url={result.cancellation.directCancelUrl}>
                Cancel directly in GitHub
              </External>
            )}
          </Notice>
        )}
      </Card>
    </div>
  );
}
