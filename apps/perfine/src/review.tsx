// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import { Code, ConnectError } from "@connectrpc/connect";
import { create, equals } from "@bufbuild/protobuf";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import { Textarea } from "@delino/ui/components/textarea";
import {
  ApprovalState,
  CompletionState,
  ConnectionState,
  CorrectnessState,
  DraftBindingSchema,
  GitHubRepositorySchema,
  PublicationService,
  PublicationState,
  type Candidate,
  type Connection,
  type Experiment,
  type DraftBinding,
} from "@delino/protos/perfine/v1/perfine_pb";
import { CandidateEvidence } from "./experiments";
import { Specification } from "./start";
import { Button, Card, External, Failure, Field, Notice } from "./ui";
import { candidateDiffURL, useRequestId, uuid7 } from "./core";
export function Review({
  spaceId,
  experiment,
  candidate,
  connection,
  allowed,
  back,
}: {
  spaceId: string;
  experiment: Experiment;
  candidate: Candidate;
  connection: Connection;
  allowed: boolean;
  back: () => void;
}) {
  const titleRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    titleRef.current?.focus();
  }, []);
  const [draft, setDraft] = useState(() =>
    create(DraftBindingSchema, {
      targetRepository: connection.repository,
      headRepository: connection.repository,
      baseSha: candidate.baseSha,
      headSha: candidate.headSha,
      candidateId: candidate.id,
      evidenceManifestSha256: candidate.evidenceManifestSha256,
    }),
  );
  const [checked, setChecked] = useState(false),
    [judgment, setJudgment] = useState(""),
    [approvalId, setApproval] = useState(""),
    [locked, setLocked] = useState<DraftBinding>(),
    [localError, setError] = useState<unknown>();
  const [ids, setIds] = useState(() => ({
    approve: uuid7(),
    publish: uuid7(),
  }));
  const requestId = useRequestId();
  const approve = useMutation(PublicationService.method.approveDraft),
    publish = useMutation(PublicationService.method.publishDraft);
  const publication = useQuery(
    PublicationService.method.getPublication,
    { spaceId, approvalId, requestId },
    {
      enabled: !!approvalId,
      refetchInterval: (q) =>
        q.state.data?.publication?.state === PublicationState.PUBLISHED
          ? false
          : 3000,
    },
  );
  const currentApproval = useQuery(
    PublicationService.method.getApproval,
    { spaceId, approvalId, requestId },
    { enabled: !!approvalId, refetchInterval: 10_000 },
  );
  const requiresJudgment =
    candidate.humanJudgmentRequired ||
    experiment.humanJudgmentRequired ||
    candidate.comparisons.some((c) => c.humanJudgmentRequired);
  const complete =
    experiment.evidenceCompletion === CompletionState.COMPLETE &&
    !!candidate.evidenceManifestSha256 &&
    candidate.correctness.length > 0 &&
    candidate.correctness.every((c) => c.state === CorrectnessState.PASSED) &&
    candidate.comparisons.length > 0;
  const diffURL = candidateDiffURL(candidate, connection.repository);
  const permitted =
    allowed &&
    connection.state === ConnectionState.CONNECTED &&
    complete &&
    !!diffURL;
  const busy = approve.isPending || publish.isPending;
  function update(next: DraftBinding) {
    setDraft(next);
    setChecked(false);
    setIds({ approve: uuid7(), publish: uuid7() });
    approve.reset();
    publish.reset();
    setError(undefined);
  }
  const result = publication.data?.publication || publish.data?.publication;
  const missingPublication =
    !result &&
    publication.error instanceof ConnectError &&
    publication.error.code === Code.NotFound;
  const validLockedApproval =
    currentApproval.data?.approval?.state === ApprovalState.VALID &&
    !!locked &&
    !!currentApproval.data.approval.draft &&
    equals(DraftBindingSchema, currentApproval.data.approval.draft, locked);
  const frozen = !!approvalId || busy;
  async function submit() {
    if (!checked || !permitted || frozen) return;
    setError(undefined);
    const snapshot = create(DraftBindingSchema, draft);
    try {
      const r = await approve.mutateAsync({
        spaceId,
        experimentId: experiment.id,
        requestId: ids.approve,
        draft: snapshot,
        humanJudgment: judgment,
      });
      if (
        !r.approval?.id ||
        r.approval.state !== ApprovalState.VALID ||
        !r.approval.draft ||
        !equals(DraftBindingSchema, r.approval.draft, snapshot)
      )
        throw new Error(
          "The approval binding changed. Review the current source and content before approving again.",
        );
      setLocked(snapshot);
      setApproval(r.approval.id);
      await publish.mutateAsync({
        spaceId,
        requestId: ids.publish,
        approvalId: r.approval.id,
        expectedDraft: snapshot,
      });
    } catch (e) {
      setError(e);
    }
  }
  return (
    <section className="review stack" aria-label="Draft publication review">
      <Button variant="ghost" className="back" onClick={back}>
        ← Back to experiment
      </Button>
      <div>
        <p className="eyebrow">HUMAN APPROVAL</p>
        <h2 ref={titleRef} tabIndex={-1}>
          Review the exact draft.
        </h2>
        <p className="lead">
          Check the evidence and GitHub diff before creating a draft pull
          request.
        </p>
      </div>
      <Notice title="Approval binds this exact content">
        Changing the target repository, base, head SHA, title or body requires
        new approval. Remeasure if production source changes, including rebase
        or conflict resolution; editing PR prose alone does not require
        remeasurement. Only measured production changes belong in this PR;
        exclude harnesses, fork infrastructure, unrelated commits and user work.
      </Notice>
      <CandidateEvidence
        candidate={candidate}
        complete={experiment.evidenceCompletion === CompletionState.COMPLETE}
      />
      {experiment.specification && (
        <details className="panel">
          <summary>Review frozen policy and protected workloads</summary>
          <Specification spec={experiment.specification} />
        </details>
      )}
      <External github url={diffURL || ""}>
        Inspect measured source on GitHub
      </External>
      <Card className="panel">
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <fieldset disabled={frozen}>
            <legend>Exact publication binding</legend>
            <div className="form-grid">
              <Field
                label="Target owner"
                value={draft.targetRepository?.owner || ""}
                required
                onChange={(e) =>
                  update(
                    create(DraftBindingSchema, {
                      ...draft,
                      targetRepository: create(GitHubRepositorySchema, {
                        ...create(
                          GitHubRepositorySchema,
                          draft.targetRepository,
                        ),
                        owner: e.target.value,
                      }),
                    }),
                  )
                }
              />
              <Field
                label="Target repository"
                value={draft.targetRepository?.name || ""}
                required
                onChange={(e) =>
                  update(
                    create(DraftBindingSchema, {
                      ...draft,
                      targetRepository: create(GitHubRepositorySchema, {
                        ...create(
                          GitHubRepositorySchema,
                          draft.targetRepository,
                        ),
                        name: e.target.value,
                      }),
                    }),
                  )
                }
              />
              <Field
                label="Target GitHub repository ID"
                value={
                  draft.targetRepository?.githubRepositoryId.toString() || ""
                }
                pattern="[1-9][0-9]*"
                required
                onChange={(e) => {
                  if (/^\d*$/.test(e.target.value))
                    update(
                      create(DraftBindingSchema, {
                        ...draft,
                        targetRepository: create(GitHubRepositorySchema, {
                          ...create(
                            GitHubRepositorySchema,
                            draft.targetRepository,
                          ),
                          githubRepositoryId: BigInt(e.target.value || "0"),
                        }),
                      }),
                    );
                }}
              />
              <Field
                label="Base branch"
                value={draft.baseRef}
                required
                onChange={(e) =>
                  update(
                    create(DraftBindingSchema, {
                      ...draft,
                      baseRef: e.target.value,
                    }),
                  )
                }
              />
              <Field
                label="Exact base SHA"
                value={draft.baseSha}
                pattern="[a-fA-F0-9]{40}"
                required
                onChange={(e) =>
                  update(
                    create(DraftBindingSchema, {
                      ...draft,
                      baseSha: e.target.value,
                    }),
                  )
                }
              />
              <Field
                label="Head owner"
                value={draft.headRepository?.owner || ""}
                required
                onChange={(e) =>
                  update(
                    create(DraftBindingSchema, {
                      ...draft,
                      headRepository: create(GitHubRepositorySchema, {
                        ...create(GitHubRepositorySchema, draft.headRepository),
                        owner: e.target.value,
                      }),
                    }),
                  )
                }
              />
              <Field
                label="Head repository"
                value={draft.headRepository?.name || ""}
                required
                onChange={(e) =>
                  update(
                    create(DraftBindingSchema, {
                      ...draft,
                      headRepository: create(GitHubRepositorySchema, {
                        ...create(GitHubRepositorySchema, draft.headRepository),
                        name: e.target.value,
                      }),
                    }),
                  )
                }
              />
              <Field
                label="Head GitHub repository ID"
                value={
                  draft.headRepository?.githubRepositoryId.toString() || ""
                }
                pattern="[1-9][0-9]*"
                required
                onChange={(e) => {
                  if (/^\d*$/.test(e.target.value))
                    update(
                      create(DraftBindingSchema, {
                        ...draft,
                        headRepository: create(GitHubRepositorySchema, {
                          ...create(
                            GitHubRepositorySchema,
                            draft.headRepository,
                          ),
                          githubRepositoryId: BigInt(e.target.value || "0"),
                        }),
                      }),
                    );
                }}
              />
              <Field
                label="Head branch"
                value={draft.headRef}
                required
                onChange={(e) =>
                  update(
                    create(DraftBindingSchema, {
                      ...draft,
                      headRef: e.target.value,
                    }),
                  )
                }
              />
              <Field
                label="Exact measured head SHA"
                value={draft.headSha}
                readOnly
              />
            </div>
            <Field
              label="Pull request title"
              value={draft.title}
              required
              onChange={(e) =>
                update(
                  create(DraftBindingSchema, {
                    ...draft,
                    title: e.target.value,
                  }),
                )
              }
            />
            <label className="field">
              Pull request body
              <Textarea
                value={draft.body}
                required
                rows={8}
                onChange={(e) =>
                  update(
                    create(DraftBindingSchema, {
                      ...draft,
                      body: e.target.value,
                    }),
                  )
                }
              />
            </label>
            {requiresJudgment && (
              <label className="field">
                Human judgment for disclosed regressions and exceptions
                <Textarea
                  required
                  value={judgment}
                  onChange={(e) => {
                    setJudgment(e.target.value);
                    setChecked(false);
                    setIds({ approve: uuid7(), publish: uuid7() });
                  }}
                />
              </label>
            )}
            <label className="check">
              <input
                type="checkbox"
                required
                checked={checked}
                onChange={(e) => setChecked(e.target.checked)}
              />
              I reviewed the exact target, base, measured head SHA, PR content,
              correctness, regressions and limitations above.
            </label>
          </fieldset>
          {!permitted && (
            <Notice title="Publication unavailable">
              Complete evidence, passing correctness, a reviewable GitHub diff,
              an active connection and publish permission are required. The
              server rechecks authority and the exact source binding.
            </Notice>
          )}
          <Button
            type="submit"
            disabled={
              !permitted ||
              !checked ||
              frozen ||
              (requiresJudgment && !judgment.trim())
            }
          >
            {busy
              ? "Submitting exact approval…"
              : "Approve and create draft PR"}
          </Button>
        </form>
        {!!localError && <Failure error={localError} />}{" "}
        {approvalId && (
          <div className="stack">
            <Notice
              title={
                result?.state === PublicationState.PUBLISHED
                  ? "Draft PR created"
                  : result?.state === PublicationState.FAILED
                    ? "Publication failed"
                    : "Reconciling publication"
              }
            >
              {result?.failureReason ||
                "Do not submit another approval while the outcome is uncertain. Check GitHub and the retained publication status."}
              {result?.githubPullRequestUrl && (
                <p>
                  <External github url={result.githubPullRequestUrl}>
                    Open draft pull request
                  </External>
                </p>
              )}
            </Notice>
            <Button
              variant="outline"
              onClick={() => {
                void publication.refetch();
                void currentApproval.refetch();
              }}
            >
              Check publication status
            </Button>
            {publication.error && <Failure error={publication.error} />}{" "}
            {currentApproval.error && <Failure error={currentApproval.error} />}{" "}
            {currentApproval.data?.approval?.state ===
              ApprovalState.INVALIDATED && (
              <Notice title="Approval invalidated">
                Return to the experiment, refresh evidence and review a new
                exact draft. Changed production content requires remeasurement.
              </Notice>
            )}
            {(result?.state === PublicationState.FAILED ||
              missingPublication) &&
              validLockedApproval &&
              locked && (
                <Button
                  disabled={
                    busy ||
                    publication.isFetching ||
                    currentApproval.isFetching ||
                    (!!publication.error && !missingPublication) ||
                    !!currentApproval.error
                  }
                  onClick={() =>
                    publish.mutate({
                      spaceId,
                      approvalId,
                      requestId: ids.publish,
                      expectedDraft: locked,
                    })
                  }
                >
                  Retry reconciled draft publication
                </Button>
              )}
          </div>
        )}
        <p className="muted">
          You may approve your own candidate. Perfine creates drafts only. There
          is no automatic merge.
        </p>
      </Card>
    </section>
  );
}
