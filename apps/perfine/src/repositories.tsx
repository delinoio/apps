// SPDX-License-Identifier: Apache-2.0
import { useRef, useState } from "react";
import { createClient } from "@connectrpc/connect";
import type { MessageInitShape } from "@bufbuild/protobuf";
import { useMutation, useQuery, useTransport } from "@connectrpc/connect-query";
import {
  useQueryClient,
  useMutation as useRequestMutation,
} from "@tanstack/react-query";
import {
  ArtifactKind,
  ConnectionService,
  ConnectRepositoryRequestSchema,
  ConnectionState,
  ManagementService,
  DeletionState,
  type Connection,
} from "@delino/protos/perfine/v1/perfine_pb";
import { Button, Card, Field, Failure, Notice, Confirm, External } from "./ui";
import { uuid7, useRequestId } from "./core";
export function Repositories({
  spaceId,
  manage,
  connections,
  onSelect,
}: {
  spaceId: string;
  manage: boolean;
  connections: Connection[];
  onSelect: (c: Connection) => void;
}) {
  const client = createClient(ConnectionService, useTransport());
  const token = useRef("");
  const connect = useRequestMutation({
    retry: false,
    mutationFn: (
      request: MessageInitShape<typeof ConnectRepositoryRequestSchema>,
    ) => {
      // Keep credentials out of retained mutation variables and shared transport state.
      const githubToken = token.current;
      token.current = "";
      return client.connectRepository(request, {
        headers: { "x-github-token": githubToken },
      });
    },
  });
  const cache = useQueryClient();
  const [requestId, setRequestId] = useState(uuid7);
  return (
    <div className="stack">
      <p className="lead">
        Connect once. Keep a history of reproducible experiments.
      </p>
      {connections.length ? (
        connections.map((c) => (
          <Card key={c.id} className="repository-card">
            <div>
              <h2>
                {c.repository?.owner}/{c.repository?.name}
              </h2>
              <p>
                {ConnectionState[c.state]?.toLowerCase().replaceAll("_", " ") ||
                  "Unknown connection"}
              </p>
            </div>
            <Button variant="outline" onClick={() => onSelect(c)}>
              View experiments
            </Button>
            {c.executionCancellations.map((x, i) => (
              <Notice
                key={i}
                title={x.reason || "Actions cancellation requested"}
              >
                {x.directCancelUrl && (
                  <External url={x.directCancelUrl} github>
                    Cancel directly in GitHub
                  </External>
                )}
              </Notice>
            ))}
          </Card>
        ))
      ) : (
        <Notice title="No connected repositories">
          A manager can connect a repository. Local experiments do not require a
          connection.
        </Notice>
      )}
      <Card className="panel">
        <h2>Connect a GitHub repository</h2>
        <p>
          You need Perfine manage permission and GitHub repository administrator
          authority. A repository can belong to only one space at a time.
        </p>
        {import.meta.env.PUBLIC_PERFINE_GITHUB_APP_URL && (
          <External url={import.meta.env.PUBLIC_PERFINE_GITHUB_APP_URL} github>
            Install or configure the GitHub App
          </External>
        )}
        <form
          className="form-grid"
          onChange={() => setRequestId(uuid7())}
          onSubmit={(e) => {
            e.preventDefault();
            if (!manage || connect.isPending) return;
            const input = e.currentTarget.elements.namedItem(
              "githubToken",
            ) as HTMLInputElement;
            if (!input.value.trim()) return;
            const f = new FormData(e.currentTarget);
            token.current = input.value.trim();
            input.value = "";
            f.delete("githubToken");
            connect.mutate(
              {
                spaceId,
                requestId,
                repository: {
                  owner: String(f.get("owner")),
                  name: String(f.get("name")),
                  githubRepositoryId: BigInt(String(f.get("repositoryId"))),
                },
                githubInstallationId: BigInt(String(f.get("installationId"))),
              },
              {
                onSuccess: () => {
                  setRequestId(uuid7());
                  void cache.invalidateQueries();
                },
              },
            );
          }}
        >
          <Field
            label="Repository owner"
            name="owner"
            required
            disabled={!manage}
          />
          <Field
            label="Repository name"
            name="name"
            required
            disabled={!manage}
          />
          <Field
            label="GitHub repository ID"
            name="repositoryId"
            pattern="[1-9][0-9]*"
            inputMode="numeric"
            required
            disabled={!manage}
          />
          <Field
            label="GitHub App installation ID"
            name="installationId"
            pattern="[1-9][0-9]*"
            inputMode="numeric"
            required
            disabled={!manage}
          />
          <div className="connection-token">
            <Field
              label="GitHub access token"
              name="githubToken"
              type="password"
              autoComplete="off"
              spellCheck={false}
              required
              disabled={!manage || connect.isPending}
              hint="Used only to verify repository administrator access. Not saved."
            />
          </div>
          <Button type="submit" disabled={!manage || connect.isPending}>
            {connect.isPending ? "Connecting…" : "Connect repository"}
          </Button>
        </form>
        {!manage && <Notice title="Manage permission required" />}
        {connect.error && <Failure error={connect.error} />}
        <p className="muted">
          Reconnecting here reuses this space’s history. Connecting in another
          space starts new history; retained private results do not move.
        </p>
      </Card>
      <Card className="panel">
        <h2>Set up local or GitHub execution</h2>
        <p>
          Use your own repository checkout. Confirm workloads before
          measurement.
        </p>
        <pre>
          perfine init{"\n"}perfine doctor{"\n"}perfine measure{"\n"}perfine
          optimize{"\n"}perfine upload
        </pre>
        <p>
          For a manual GitHub Actions workflow, run{" "}
          <code>perfine init --github</code>. Review configuration and the
          pinned Action revision before committing. AI authentication stays in
          your local CLI or GitHub Secrets; never send it to Perfine.
        </p>
        <p>
          Local uploads use a Delcore service API key with this space and
          Perfine grants. Actions uploads use repository-scoped OIDC. An upload
          failure preserves local results for manual resend.
        </p>
      </Card>
    </div>
  );
}
export function Settings({
  spaceId,
  connection,
  manage,
}: {
  spaceId: string;
  connection?: Connection;
  manage: boolean;
}) {
  const requestId = useRequestId(),
    cache = useQueryClient();
  const consent = useQuery(
    ConnectionService.method.getRawArtifactConsent,
    { spaceId, requestId, connectionId: connection?.id || "" },
    { enabled: manage && !!connection },
  );
  const save = useMutation(ConnectionService.method.setRawArtifactConsent),
    disconnect = useMutation(ConnectionService.method.disconnectRepository);
  const [selection, setSelection] = useState<ArtifactKind[] | null>(null);
  const allowed = selection ?? consent.data?.consent?.allowedKinds ?? [];
  const [result, setResult] = useState<Connection>();
  return (
    <div className="stack">
      <Notice title="Retention and privacy">
        Results remain until a manager deletes them. Space usage limits do not
        change retention. Existing download URLs can remain usable for up to 15
        minutes after revocation or deletion; previously downloaded data cannot
        be recalled.
      </Notice>
      {!manage ? (
        <Notice title="Manage permission required">
          Management does not grant experiment or publication permission.
        </Notice>
      ) : (
        <>
          {connection ? (
            <>
              <Card className="panel">
                <h2>Raw evidence consent</h2>
                <p>
                  Raw profiles and test logs may contain source snippets.
                  Approval applies only to subsequent runs for this repository.
                  Revoking consent blocks new raw uploads. Credentials and
                  secrets must always be removed or rejected.
                </p>
                {consent.isPending ? (
                  <Notice title="Loading consent…" />
                ) : consent.error ? (
                  <Failure
                    error={consent.error}
                    retry={() => void consent.refetch()}
                  />
                ) : (
                  <form
                    onSubmit={(e) => {
                      e.preventDefault();
                      save.mutate(
                        {
                          spaceId,
                          requestId: uuid7(),
                          connectionId: connection.id,
                          expectedRevision: consent.data?.consent?.revision,
                          allowedKinds: allowed,
                        },
                        {
                          onSuccess: () => {
                            setSelection(null);
                            void cache.invalidateQueries();
                          },
                        },
                      );
                    }}
                  >
                    {[ArtifactKind.RAW_PROFILE, ArtifactKind.RAW_TEST_LOG].map(
                      (kind) => (
                        <label className="check" key={kind}>
                          <input
                            type="checkbox"
                            checked={allowed.includes(kind)}
                            onChange={(e) =>
                              setSelection(
                                e.target.checked
                                  ? [...allowed, kind]
                                  : allowed.filter((x) => x !== kind),
                              )
                            }
                          />
                          {kind === ArtifactKind.RAW_PROFILE
                            ? "Allow raw profiles"
                            : "Allow raw test logs"}
                        </label>
                      ),
                    )}
                    <Button disabled={save.isPending} type="submit">
                      Save consent
                    </Button>
                    {save.isSuccess && <Notice title="Consent updated" />}
                  </form>
                )}
                {save.error && <Failure error={save.error} />}
              </Card>
              <Card className="panel">
                <h2>Disconnect repository</h2>
                <p>
                  Stops new jobs, uploads and publication. Retained evidence
                  stays private in this space. Perfine requests cancellation of
                  running Actions.
                </p>
                <Confirm
                  label="Disconnect repository"
                  description="Retained results will not be deleted. If Actions cancellation fails, cancel the run directly in GitHub."
                  pending={disconnect.isPending}
                  disabled={connection.state !== ConnectionState.CONNECTED}
                  onConfirm={() =>
                    disconnect.mutate(
                      {
                        spaceId,
                        requestId: uuid7(),
                        connectionId: connection.id,
                      },
                      {
                        onSuccess: (r) => {
                          setResult(r.connection);
                          void cache.invalidateQueries();
                        },
                      },
                    )
                  }
                />
                {disconnect.error && <Failure error={disconnect.error} />}{" "}
                {result && (
                  <Notice title="Repository disconnected">
                    {result.executionCancellations.map((c, i) => (
                      <p key={i}>
                        {c.reason}{" "}
                        {c.directCancelUrl && (
                          <External github url={c.directCancelUrl}>
                            Cancel Actions directly
                          </External>
                        )}
                      </p>
                    ))}
                  </Notice>
                )}
              </Card>
            </>
          ) : (
            <Notice title="Select a repository to manage consent or disconnect" />
          )}
          <DeleteResult spaceId={spaceId} />
        </>
      )}
    </div>
  );
}
export function DeleteResult({
  spaceId,
  experimentId,
  onDeleted,
}: {
  spaceId: string;
  experimentId?: string;
  onDeleted?: () => void;
}) {
  const [id, setId] = useState(experimentId || ""),
    [accepted, setAccepted] = useState(false);
  const [requestId, setRequestId] = useState(uuid7);
  const remove = useMutation(ManagementService.method.deleteExperiment),
    cache = useQueryClient();
  const deletion = useQuery(
    ManagementService.method.getDeletion,
    { spaceId, experimentId: id, requestId },
    {
      enabled: accepted,
      refetchInterval: (q) =>
        q.state.data?.deletion?.state === DeletionState.COMPLETE ? false : 5000,
    },
  );
  return (
    <Card className="panel">
      <h2>Delete an experiment</h2>
      <p>
        Accepted deletion immediately blocks new reads and download links.
        Cleanup continues asynchronously. Deleted IDs cannot be resumed or
        uploaded again.
      </p>
      {!experimentId && (
        <Field
          label="Experiment ID"
          autoFocus
          value={id}
          disabled={remove.isPending}
          onChange={(e) => {
            if (e.target.value !== id) setRequestId(uuid7());
            setId(e.target.value);
            setAccepted(false);
          }}
        />
      )}
      <Confirm
        label="Delete experiment"
        description="Permanently delete this experiment and its hosted evidence. Existing signed URLs may remain valid for up to 15 minutes."
        disabled={!id || accepted}
        pending={remove.isPending}
        onConfirm={() =>
          remove.mutate(
            { spaceId, experimentId: id, requestId },
            {
              onSuccess: () => {
                setAccepted(true);
                cache.removeQueries({
                  predicate: (q) => JSON.stringify(q.queryKey).includes(id),
                });
                void cache.invalidateQueries();
                onDeleted?.();
              },
            },
          )
        }
      />
      {remove.error && <Failure error={remove.error} />}{" "}
      {accepted && (
        <Notice
          title={`Deletion ${deletion.data?.deletion ? DeletionState[deletion.data.deletion.state].toLowerCase() : "accepted"}`}
        >
          {deletion.data?.deletion?.failureReason ||
            "New reads are blocked. Cleanup failures remain queued for retry."}
        </Notice>
      )}
      {deletion.error && (
        <Failure error={deletion.error} retry={() => void deletion.refetch()} />
      )}
    </Card>
  );
}
