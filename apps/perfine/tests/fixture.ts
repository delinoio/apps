// SPDX-License-Identifier: Apache-2.0
// Mock RPCs only: never imported by the production entry point.
import { create } from "@bufbuild/protobuf";
import { Code, ConnectError, createRouterTransport } from "@connectrpc/connect";
import { DelcoreService } from "@delino/protos/delcore/v1/delcore_pb";
import * as P from "@delino/protos/perfine/v1/perfine_pb";
export const spaceId = "019a10ab-0100-7000-8000-000000000001";
export const connection = create(P.ConnectionSchema, {
  id: "019a10ab-0100-7000-8000-000000000002",
  spaceId,
  state: P.ConnectionState.CONNECTED,
  repository: { owner: "example", name: "parser", githubRepositoryId: 42n },
  githubInstallationId: 7n,
  consent: { revision: 2n, allowedKinds: [] },
});
export const specification = create(P.ExperimentSpecificationSchema, {
  schemaVersion: 1,
  baseSha: "a".repeat(40),
  initialBaseSha: "a".repeat(40),
  harnessSha: "b".repeat(40),
  workflowSha: "b".repeat(40),
  inputs: [{ name: "small.json", sha256: "e".repeat(64) }],
  correctnessChecks: ["cargo test"],
  workloadConfirmation: "Fixture parser inputs exercise parsing",
  startDuration: { seconds: 3600n },
  language: P.Language.RUST,
  environment: {
    operatingSystem: "Linux",
    architecture: "x86_64",
    runner: "GitHub Actions",
    runtime: "native",
    hardware: { cpu: "fixture CPU" },
    environmentSha256: "c".repeat(64),
    tools: [
      {
        name: "Callgrind",
        version: "3.25",
      },
    ],
  },
  cases: [
    {
      id: "parse-small",
      family: "parser",
      frequency: 1,
      frequencySource: P.FrequencySource.ASSUMED_ONE,
      protectedCase: true,
      inputs: [{ name: "small.json", sha256: "e".repeat(64) }],
    },
  ],
  policy: {
    schemaVersion: 1,
    kind: P.PolicyKind.CALLGRIND_IR,
    metric: {
      engineKind: P.EngineMetricKind.IR,
      name: "Ir",
      unit: "instructions",
      objective: P.MetricObjective.MINIMIZE,
    },
    minimumImprovement: 0.01,
    maximumCaseRegression: 0.01,
    repetitions: 1,
    uncertainty: {
      engineSchemaVersion: 1,
      nearThreshold: 0.001,
      repeatTolerance: 0.001,
    },
    policySha256: "f".repeat(64),
  },
  limits: {
    candidateCount: 3,
    simultaneousCandidates: 2,
    startDeadline: { seconds: BigInt(Math.floor(Date.now() / 1000) + 86400) },
    candidateTimeout: { seconds: 600n },
  },
  fingerprintSha256: "1".repeat(64),
  workloadRepresentativenessConfirmed: true,
});
export const experiment = create(P.ExperimentSchema, {
  id: "019a10ab-0100-7000-8000-000000000003",
  spaceId,
  connectionId: connection.id,
  specification,
  executionState: P.ExecutionState.COMPLETED,
  verdict: P.PerformanceVerdict.PASS,
  evidenceCompletion: P.CompletionState.COMPLETE,
  statusReason: "All declared checks completed.",
  createdAt: { seconds: 1792796400n },
});
export const candidate = create(P.CandidateSchema, {
  id: "019a10ab-0100-7000-8000-000000000004",
  experimentId: experiment.id,
  baseSha: specification.baseSha,
  headSha: "2".repeat(40),
  executionState: P.ExecutionState.COMPLETED,
  verdict: P.PerformanceVerdict.PASS,
  githubDiffUrl: `https://github.com/example/parser/compare/${specification.baseSha}...${"2".repeat(40)}`,
  evidenceManifestSha256: "3".repeat(64),
  correctness: [
    {
      checkName: "Unit and workload assertions",
      state: P.CorrectnessState.PASSED,
      artifactName: "correctness.json",
    },
  ],
  comparisons: [
    {
      kind: P.ComparisonKind.INCREMENTAL,
      baseSha: specification.baseSha,
      headSha: "2".repeat(40),
      metric: specification.policy!.metric,
      cases: [
        {
          caseId: "parse-small",
          baseValue: 1040000,
          headValue: 990000,
          relativeChange: -0.0480769,
          weightedAbsoluteChange: -50000,
          repetitions: 2,
        },
      ],
      portfolioScore: 0.951923,
      improvement: 0.0480769,
      geometricMeanRatio: 0.951923,
      verdict: P.PerformanceVerdict.PASS,
      engineArtifactName: "engine.json",
    },
  ],
});
export type FixtureOptions = {
  permissions?: P.Permission[];
  empty?: boolean;
  deny?: boolean;
  cloud?: boolean;
  execution?: P.ExecutionState;
  verdict?: P.PerformanceVerdict;
  incomplete?: boolean;
  publicationUncertain?: boolean;
  noCandidates?: boolean;
  prepareGate?: Promise<void>;
  prepareError?: boolean;
};
export function fixture(options: FixtureOptions = {}) {
  const calls: { method: string; input: unknown }[] = [];
  let approval: P.Approval | undefined, publication: P.Publication | undefined;
  const e = create(P.ExperimentSchema, {
    ...experiment,
    executionState: options.execution ?? experiment.executionState,
    verdict: options.verdict ?? experiment.verdict,
    evidenceCompletion: options.incomplete
      ? P.CompletionState.INCOMPLETE
      : P.CompletionState.COMPLETE,
  });
  const permissions = options.permissions ?? [
    P.Permission.READ,
    P.Permission.EXPERIMENT,
    P.Permission.PUBLISH,
    P.Permission.MANAGE,
  ];
  const transport = createRouterTransport((router) => {
    router.service(DelcoreService, {
      getCurrentUser: () => ({
        userId: "fixture-user",
        tenants: [{ id: spaceId, name: "Example engineering", personal: true }],
      }),
    });
    router.service(P.AccessService, {
      getAuthorization: () => {
        if (options.deny)
          throw new ConnectError(
            "Fixture: authority denied",
            Code.PermissionDenied,
          );
        return { authorization: { spaceId, permissions } };
      },
    });
    router.service(P.ConnectionService, {
      listConnections: (req) => {
        calls.push({ method: "listConnections", input: req });
        return { connections: options.empty ? [] : [connection] };
      },
      getConnection: () => ({ connection }),
      connectRepository: (req) => {
        calls.push({ method: "connectRepository", input: req });
        return { connection };
      },
      getRawArtifactConsent: () => ({ consent: connection.consent }),
      setRawArtifactConsent: (req) => {
        calls.push({ method: "setRawArtifactConsent", input: req });
        return { consent: { revision: 3n, allowedKinds: req.allowedKinds } };
      },
      disconnectRepository: (req) => {
        calls.push({ method: "disconnectRepository", input: req });
        return {
          connection: {
            ...connection,
            state: P.ConnectionState.DISCONNECTED,
            executionCancellations: [
              create(P.CancellationSchema, {
                state: P.CancellationState.FAILED,
                reason: "Fixture: Actions cancellation permission was revoked.",
                directCancelUrl:
                  "https://github.com/example/parser/actions/runs/1",
              }),
            ],
          },
        };
      },
    });
    router.service(P.ExperimentService, {
      prepareExperiment: async (req) => {
        calls.push({ method: "prepareExperiment", input: req });
        await options.prepareGate;
        if (options.prepareError)
          throw new ConnectError(
            "Fixture: incomplete engine configuration",
            Code.InvalidArgument,
          );
        const c = req.configuration!;
        return {
          specification: create(P.ExperimentSpecificationSchema, {
            ...c,
            $typeName: undefined,
            limits: {
              candidateCount: c.limits!.candidateCount,
              simultaneousCandidates: c.limits!.simultaneousCandidates,
              candidateTimeout: c.limits!.candidateTimeout,
              startDeadline: specification.limits!.startDeadline,
            },
            startDuration: c.limits!.startDuration,
            fingerprintSha256: specification.fingerprintSha256,
            policy: {
              ...c.policy!,
              policySha256: specification.policy!.policySha256,
            },
            environment: {
              ...c.environment!,
              environmentSha256: specification.environment!.environmentSha256,
            },
            workloadRepresentativenessConfirmed: true,
          }),
          engineConfigJson: '{"mockOnly":true}',
        };
      },
      listExperiments: (req) => {
        calls.push({ method: "listExperiments", input: req });
        return { experiments: options.empty ? [] : [e] };
      },
      getExperiment: () => ({ experiment: e }),
      listCandidates: () => ({
        candidates: options.noCandidates ? [] : [candidate],
      }),
      getCandidate: () => ({ candidate }),
      startExperiment: (req) => {
        calls.push({ method: "startExperiment", input: req });
        return {
          experiment: {
            ...e,
            id: req.experimentId,
            executionState: P.ExecutionState.QUEUED,
          },
        };
      },
      cancelExperiment: (req) => {
        calls.push({ method: "cancelExperiment", input: req });
        return { experiment: e };
      },
      resumeExperiment: (req) => {
        calls.push({ method: "resumeExperiment", input: req });
        return { experiment: e };
      },
    });
    router.service(P.CloudService, {
      listCloudTools: () => ({
        tools: [
          {
            toolId: "fixture-tool",
            displayName: "Test runner (mock only)",
            state: options.cloud
              ? P.CloudEligibilityState.ELIGIBLE
              : P.CloudEligibilityState.UNAVAILABLE,
            reason: "Official unattended authentication has not been verified.",
            localAlternative: "perfine optimize",
          },
        ],
      }),
    });
    router.service(P.EvidenceService, {
      listEvidence: () => ({
        manifests: [
          {
            schemaVersion: 1,
            experimentId: e.id,
            candidateId: candidate.id,
            manifestSha256: candidate.evidenceManifestSha256,
            sourceBaseSha: candidate.baseSha,
            sourceHeadSha: candidate.headSha,
            harnessSha: specification.harnessSha,
            completion: P.CompletionState.COMPLETE,
            artifacts: [
              {
                name: "engine.json",
                kind: P.ArtifactKind.ENGINE_EVIDENCE,
                sha256: "4".repeat(64),
                sizeBytes: 1200n,
                completion: P.CompletionState.COMPLETE,
                requiredForEvaluation: true,
              },
            ],
          },
        ],
      }),
      getEvidenceDownload: (req) => {
        calls.push({ method: "getEvidenceDownload", input: req });
        return {
          signedGetUrl: "https://example.invalid/fixture-evidence",
          expiresAt: { seconds: BigInt(Math.floor(Date.now() / 1000) + 900) },
        };
      },
    });
    router.service(P.ManagementService, {
      deleteExperiment: (req) => {
        calls.push({ method: "deleteExperiment", input: req });
        return {
          deletion: {
            experimentId: req.experimentId,
            state: P.DeletionState.PENDING,
          },
        };
      },
      getDeletion: () => ({
        deletion: {
          experimentId: e.id,
          state: P.DeletionState.RETRYING,
          failureReason:
            "Fixture: storage unavailable; cleanup retained for retry.",
        },
      }),
    });
    router.service(P.PublicationService, {
      approveDraft: (req) => {
        calls.push({ method: "approveDraft", input: req });
        approval = create(P.ApprovalSchema, {
          id: "019a10ab-0100-7000-8000-000000000005",
          experimentId: e.id,
          draft: req.draft,
          state: P.ApprovalState.VALID,
        });
        return { approval };
      },
      getApproval: () => ({ approval }),
      publishDraft: (req) => {
        calls.push({ method: "publishDraft", input: req });
        publication = create(P.PublicationSchema, {
          approvalId: req.approvalId,
          candidateId: candidate.id,
          state: options.publicationUncertain
            ? P.PublicationState.RECONCILING
            : P.PublicationState.PUBLISHED,
          draft: req.expectedDraft,
          githubPullRequestUrl: options.publicationUncertain
            ? ""
            : "https://github.com/example/parser/pull/1",
        });
        if (options.publicationUncertain)
          throw new ConnectError(
            "Fixture: publication outcome uncertain",
            Code.Unavailable,
          );
        return { publication };
      },
      getPublication: () => ({ publication }),
    });
  });
  return { transport, calls };
}
