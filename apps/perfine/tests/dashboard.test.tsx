// SPDX-License-Identifier: Apache-2.0
import { toJsonString } from "@bufbuild/protobuf";
import { describe, expect, it } from "@rstest/core";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { TransportProvider } from "@connectrpc/connect-query";
import * as P from "@delino/protos/perfine/v1/perfine_pb";
import { App } from "../src/app";
import { CandidateEvidence, Detail, Status } from "../src/experiments";
import { Review } from "../src/review";
import { Operations } from "../src/operations";
import { Repositories, Settings } from "../src/repositories";
import {
  importConfiguration,
  emptyConfiguration,
  validateConfiguration,
} from "../src/configuration";
import { Start } from "../src/start";
import { Confirm } from "../src/ui";
import { candidateDiffURL, queryClient, safeURL, uuid7 } from "../src/core";
import {
  candidate,
  connection,
  experiment,
  fixture,
  spaceId,
  specification,
  type FixtureOptions,
} from "./fixture";
import type { ReactNode } from "react";
function mount(
  node: (f: ReturnType<typeof fixture>) => ReactNode,
  options: FixtureOptions = {},
) {
  const f = fixture(options),
    client = queryClient();
  client.setDefaultOptions({
    queries: { retry: false, gcTime: 0 },
    mutations: { retry: false },
  });
  render(
    <QueryClientProvider client={client}>
      <TransportProvider transport={f.transport}>{node(f)}</TransportProvider>
    </QueryClientProvider>,
  );
  return { ...f, client };
}
async function selectSpace() {
  await screen.findByRole("option", { name: "Example engineering · Personal" });
  fireEvent.change(screen.getByLabelText("Space"), {
    target: { value: spaceId },
  });
}
async function selectRepository() {
  await screen.findByRole("option", { name: "example/parser" });
  fireEvent.change(screen.getByLabelText("Repository"), {
    target: { value: connection.id },
  });
}
function fillDraft() {
  for (const [label, value] of [
    ["Base branch", "main"],
    ["Head branch", "perfine/candidate"],
    ["Pull request title", "perf(parser): reduce instruction count"],
    [
      "Pull request body",
      "Measured source only. Correctness passed; Ir is instruction count.",
    ],
  ])
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
}
function fillConfiguration() {
  for (const [label, value] of [
    ["Base SHA", "a".repeat(40)],
    ["Initial base SHA", "a".repeat(40)],
    ["Harness SHA", "b".repeat(40)],
    ["Workflow SHA", "b".repeat(40)],
    ["Language", String(P.Language.RUST)],
    ["Input 1 name", "small.json"],
    ["Input 1 SHA-256", "e".repeat(64)],
    ["Case 1 ID", "parse-small"],
    ["Case 1 family", "parser"],
    ["Correctness checks (one per line)", "cargo test"],
    [
      "Workload representativeness confirmation",
      "Parser inputs exercise parsing",
    ],
    ["Operating system", "Linux"],
    ["Architecture", "x86_64"],
    ["Runner", "GitHub Actions"],
    ["Toolchain and version", "rustc 1.90"],
    ["Tool 1 name", "Callgrind"],
    ["Tool 1 version", "3.25"],
    ["CPU / hardware identity", "fixture CPU"],
  ])
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
}
describe("mocked typed RPC dashboard", () => {
  it("sends the ephemeral token only on connection and clears it before completion", async () => {
    let finish!: () => void;
    const gate = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const f = mount((f) => <App delcore={f.transport} signOut={() => {}} />, {
      connectGate: gate,
    });
    await selectSpace();
    fireEvent.click(
      await screen.findByRole("button", { name: "Repositories" }),
    );
    await screen.findByLabelText("Repository owner");
    for (const [label, value] of [
      ["Repository owner", "example"],
      ["Repository name", "parser"],
      ["GitHub repository ID", "1"],
      ["GitHub App installation ID", "7"],
      ["GitHub access token", "fixture-secret-token"],
    ])
      fireEvent.change(screen.getByLabelText(label, { exact: false }), {
        target: { value },
      });
    const input = screen.getByLabelText(
      /^GitHub access token/,
    ) as HTMLInputElement;
    expect(input.type).toBe("password");
    fireEvent.click(screen.getByRole("button", { name: "Connect repository" }));
    expect(input.value).toBe("");
    await waitFor(() =>
      expect(
        f.calls.filter((x) => x.method === "connectRepository").length,
      ).toBe(1),
    );
    expect(
      f.calls.find((x) => x.method === "connectRepository")?.githubToken,
    ).toBe("fixture-secret-token");
    expect(
      (screen.getByRole("button", { name: "Connecting…" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    const variables = f.client
      .getMutationCache()
      .getAll()
      .map((m) => m.state.variables);
    expect(
      JSON.stringify(variables, (_, v) =>
        typeof v === "bigint" ? String(v) : v,
      ),
    ).not.toContain("fixture-secret-token");
    expect(JSON.stringify(localStorage)).not.toContain("fixture-secret-token");
    expect(JSON.stringify(sessionStorage)).not.toContain(
      "fixture-secret-token",
    );
    finish();
    await waitFor(() =>
      expect(
        (
          screen.getByRole("button", {
            name: "Connect repository",
          }) as HTMLButtonElement
        ).disabled,
      ).toBe(false),
    );
    expect(
      f.calls
        .filter((x) => x.method === "listConnections")
        .every((x) => !x.githubToken),
    ).toBe(true);
    expect(input.value).toBe("");
  });
  it("requires token re-entry after a failed connection and preserves the manage gate", async () => {
    const f = mount(
      () => (
        <Repositories
          spaceId={spaceId}
          manage
          connections={[]}
          onSelect={() => {}}
        />
      ),
      { connectError: true },
    );
    for (const [label, value] of [
      ["Repository owner", "example"],
      ["Repository name", "parser"],
      ["GitHub repository ID", "1"],
      ["GitHub App installation ID", "7"],
      ["GitHub access token", "rejected-token"],
    ])
      fireEvent.change(screen.getByLabelText(label, { exact: false }), {
        target: { value },
      });
    fireEvent.click(screen.getByRole("button", { name: "Connect repository" }));
    await screen.findByRole("alert");
    const input = screen.getByLabelText(
      /^GitHub access token/,
    ) as HTMLInputElement;
    expect(input.value).toBe("");
    fireEvent.submit(input.form!);
    expect(f.calls.filter((x) => x.method === "connectRepository").length).toBe(
      1,
    );
  });
  it("disables the token field without manage permission", () => {
    mount(() => (
      <Repositories
        spaceId={spaceId}
        manage={false}
        connections={[]}
        onSelect={() => {}}
      />
    ));
    expect(
      (screen.getByLabelText(/^GitHub access token/) as HTMLInputElement)
        .disabled,
    ).toBe(true);
    expect(
      (
        screen.getByRole("button", {
          name: "Connect repository",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
  });
  it("does not fetch private results before a space is selected", async () => {
    const f = mount((f) => <App delcore={f.transport} signOut={() => {}} />);
    await screen.findByText("Select a space to continue");
    expect(f.calls.length).toBe(0);
    await selectSpace();
    await selectRepository();
    await screen.findByText("Experiment 00000003");
    expect(f.calls.some((x) => x.method === "listExperiments")).toBe(true);
  });
  it("fails closed on denied authority", async () => {
    const f = mount((f) => <App delcore={f.transport} signOut={() => {}} />, {
      deny: true,
    });
    await selectSpace();
    await screen.findByText("Permission denied");
    expect(f.calls.length).toBe(0);
  });
  it("manage alone cannot read or start experiments", async () => {
    const f = mount((f) => <App delcore={f.transport} signOut={() => {}} />, {
      permissions: [P.Permission.MANAGE],
    });
    await selectSpace();
    await selectRepository();
    await screen.findByText("Read permission required");
    expect(f.calls.some((x) => x.method === "listExperiments")).toBe(false);
    expect(screen.queryByRole("button", { name: "New experiment" })).toBeNull();
  });
  it("shows execution and verdict separately and never displays a hosted pass for incomplete evidence", () => {
    render(
      <Status
        experiment={{
          executionState: P.ExecutionState.COMPLETED,
          verdict: P.PerformanceVerdict.PASS,
          evidenceCompletion: P.CompletionState.INCOMPLETE,
        }}
      />,
    );
    expect(screen.getByText("Execution: Completed")).toBeTruthy();
    expect(
      screen.getByText("Verdict: Inconclusive · incomplete evidence"),
    ).toBeTruthy();
    expect(screen.queryByText("Verdict: Pass")).toBeNull();
  });
  it.each([
    [P.MetricObjective.MAXIMIZE, 110, -0.1, "10%"],
    [P.MetricObjective.MINIMIZE, 90, -0.1, "-10%"],
  ])(
    "shows head-minus-base change for objective %s",
    (objective, head, regression, expected) => {
      const comparison = candidate.comparisons[0];
      const value = {
        ...candidate,
        comparisons: [
          {
            ...comparison,
            metric: { ...comparison.metric!, objective },
            cases: [
              {
                ...comparison.cases[0],
                baseValue: 100,
                headValue: head,
                relativeChange: regression,
              },
            ],
          },
        ],
      };
      render(<CandidateEvidence candidate={value} />);
      expect(
        screen.getByText("Change (head − base)").nextElementSibling
          ?.textContent,
      ).toBe(expected);
    },
  );
  it("shows valid no-candidate and cancellation states", async () => {
    mount(
      () => (
        <Detail
          spaceId={spaceId}
          connection={connection}
          experimentId={experiment.id}
          permissions={[P.Permission.READ]}
          back={() => {}}
        />
      ),
      { noCandidates: true, execution: P.ExecutionState.CANCELLED },
    );
    await screen.findByText("No candidate");
    expect(screen.getByText("Execution cancelled")).toBeTruthy();
  });
  it("disables unavailable cloud execution with a local alternative", async () => {
    mount(() => (
      <Start
        spaceId={spaceId}
        connection={connection}
        allowed
        onStarted={() => {}}
      />
    ));
    await screen.findByText(/Unavailable: Official unattended authentication/);
    expect(
      (
        screen.getByRole("button", {
          name: "Start cloud experiment",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    expect(screen.getByText("perfine optimize")).toBeTruthy();
  });
  it("revokes raw consent with the current revision", async () => {
    const f = mount(() => (
      <Settings spaceId={spaceId} connection={connection} manage />
    ));
    await screen.findByRole("button", { name: "Save consent" });
    fireEvent.click(screen.getByRole("button", { name: "Save consent" }));
    await waitFor(() =>
      expect(
        f.calls.find((x) => x.method === "setRawArtifactConsent"),
      ).toBeTruthy(),
    );
    const req = f.calls.find((x) => x.method === "setRawArtifactConsent")!
      .input as P.SetRawArtifactConsentRequest;
    expect(req.expectedRevision).toBe(2n);
    expect(req.allowedKinds).toEqual([]);
  });
  it("requires confirmation before disconnect and exposes cancellation failure recovery", async () => {
    const f = mount(() => (
      <Settings spaceId={spaceId} connection={connection} manage />
    ));
    fireEvent.click(
      screen.getByRole("button", { name: "Disconnect repository" }),
    );
    expect(f.calls.some((x) => x.method === "disconnectRepository")).toBe(
      false,
    );
    const buttons = screen.getAllByRole("button", {
      name: "Disconnect repository",
    });
    fireEvent.click(buttons.at(-1)!);
    await screen.findByText("Repository disconnected");
    expect(
      screen
        .getByRole("link", { name: /Cancel Actions directly/ })
        .getAttribute("href"),
    ).toContain("github.com");
  });
  it("supports Escape and restores confirmation trigger focus", async () => {
    let called = false;
    render(
      <Confirm
        label="Delete fixture"
        description="Delete this mocked result."
        onConfirm={() => {
          called = true;
        }}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Delete fixture" });
    trigger.focus();
    fireEvent.click(trigger);
    await screen.findByRole("dialog");
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(trigger));
    expect(called).toBe(false);
  });
  it("derives exact diff links from measured SHAs and rejects malformed repository paths", () => {
    const expected = `https://github.com/example/parser/compare/${candidate.baseSha}...${candidate.headSha}`;
    expect(candidate.githubDiffUrl).toBe("");
    expect(candidateDiffURL(candidate, connection.repository)).toBe(expected);
    expect(
      candidateDiffURL(candidate, {
        ...connection.repository!,
        name: "../other",
      }),
    ).toBeUndefined();
    expect(
      candidateDiffURL(
        { ...candidate, headSha: "main" },
        connection.repository,
      ),
    ).toBeUndefined();
    mount(() => (
      <Review
        spaceId={spaceId}
        experiment={experiment}
        candidate={candidate}
        connection={connection}
        allowed
        back={() => {}}
      />
    ));
    expect(
      screen
        .getByRole("link", { name: /Inspect measured source on GitHub/ })
        .getAttribute("href"),
    ).toBe(expected);
  });
  it("clears approval confirmation when content changes and publishes exactly the approved binding", async () => {
    const f = mount(() => (
      <Review
        spaceId={spaceId}
        experiment={experiment}
        candidate={candidate}
        connection={connection}
        allowed
        back={() => {}}
      />
    ));
    fillDraft();
    const check = screen.getByRole("checkbox");
    fireEvent.click(check);
    fireEvent.change(screen.getByLabelText("Pull request title"), {
      target: { value: "perf(parser): revise measured optimization" },
    });
    expect((check as HTMLInputElement).checked).toBe(false);
    expect(f.calls.some((x) => x.method === "approveDraft")).toBe(false);
    fireEvent.click(check);
    fireEvent.click(
      screen.getByRole("button", { name: "Approve and create draft PR" }),
    );
    await screen.findByText("Draft PR created");
    const approval = f.calls.find((x) => x.method === "approveDraft")!
      .input as P.ApproveDraftRequest;
    const publication = f.calls.find((x) => x.method === "publishDraft")!
      .input as P.PublishDraftRequest;
    expect(approval.draft?.title).toBe(
      "perf(parser): revise measured optimization",
    );
    expect(publication.expectedDraft).toEqual(approval.draft);
    expect(publication.expectedDraft?.headSha).toBe(candidate.headSha);
  });
  it("reconciles an uncertain publication without submitting another PR", async () => {
    const f = mount(
      () => (
        <Review
          spaceId={spaceId}
          experiment={experiment}
          candidate={candidate}
          connection={connection}
          allowed
          back={() => {}}
        />
      ),
      { publicationUncertain: true },
    );
    fillDraft();
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(
      screen.getByRole("button", { name: "Approve and create draft PR" }),
    );
    await screen.findByText("Reconciling publication");
    fireEvent.click(
      screen.getByRole("button", { name: "Check publication status" }),
    );
    await waitFor(() =>
      expect(f.calls.filter((x) => x.method === "publishDraft").length).toBe(1),
    );
    expect(
      (
        screen.getByRole("button", {
          name: "Approve and create draft PR",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
  });
  it("retries a confirmed absent publication with the original exact approval", async () => {
    const f = mount(
      () => (
        <Review
          spaceId={spaceId}
          experiment={experiment}
          candidate={candidate}
          connection={connection}
          allowed
          back={() => {}}
        />
      ),
      { publicationNotSubmitted: true },
    );
    fillDraft();
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(
      screen.getByRole("button", { name: "Approve and create draft PR" }),
    );
    const retry = await screen.findByRole("button", {
      name: "Retry reconciled draft publication",
    });
    await waitFor(() =>
      expect((retry as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(retry);
    await screen.findByText("Draft PR created");
    expect(f.calls.filter((x) => x.method === "approveDraft").length).toBe(1);
    const publishes = f.calls.filter((x) => x.method === "publishDraft");
    expect(publishes.length).toBe(2);
    expect(publishes[1].input).toEqual(publishes[0].input);
  });
  it.each(["getExperiment", "getCandidate"] as const)(
    "retains the exact publication session after a %s polling error",
    async (method) => {
      const options: FixtureOptions = { publicationNotSubmitted: true };
      const f = mount(
        () => (
          <Detail
            spaceId={spaceId}
            connection={connection}
            experimentId={experiment.id}
            permissions={[P.Permission.READ, P.Permission.PUBLISH]}
            back={() => {}}
          />
        ),
        options,
      );
      await screen.findByLabelText("Candidate");
      fireEvent.change(screen.getByLabelText("Candidate"), {
        target: { value: candidate.id },
      });
      fireEvent.click(
        await screen.findByRole("button", { name: "Review draft publication" }),
      );
      fillDraft();
      fireEvent.click(screen.getByRole("checkbox"));
      fireEvent.click(
        screen.getByRole("button", { name: "Approve and create draft PR" }),
      );
      await screen.findByRole("button", {
        name: "Retry reconciled draft publication",
      });
      options.pollFailure = method;
      await f.client.invalidateQueries();
      await screen.findByText(
        `Fixture: ${method === "getExperiment" ? "experiment" : "candidate"} poll failed`,
        { exact: false },
      );
      expect(
        screen.queryByRole("button", { name: "Approve and create draft PR" }),
      ).toBeNull();
      options.pollFailure = undefined;
      fireEvent.click(screen.getByRole("button", { name: "Retry" }));
      const retry = await screen.findByRole("button", {
        name: "Retry reconciled draft publication",
      });
      await waitFor(() =>
        expect((retry as HTMLButtonElement).disabled).toBe(false),
      );
      fireEvent.click(retry);
      await screen.findByText("Draft PR created");
      expect(f.calls.filter((x) => x.method === "approveDraft").length).toBe(1);
      const requests = f.calls.filter((x) => x.method === "publishDraft");
      expect(requests.length).toBe(2);
      expect(requests[1].input).toEqual(requests[0].input);
    },
  );
  it("downloads evidence only after explicitly requesting a fresh signed URL", async () => {
    const f = mount(() => (
      <Detail
        spaceId={spaceId}
        connection={connection}
        experimentId={experiment.id}
        permissions={[P.Permission.READ]}
        back={() => {}}
      />
    ));
    await screen.findByRole("button", { name: "Get download link" });
    expect(f.calls.some((x) => x.method === "getEvidenceDownload")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Get download link" }));
    await screen.findByRole("link", { name: /Download evidence/ });
    expect(
      f.calls.filter((x) => x.method === "getEvidenceDownload").length,
    ).toBe(1);
  });
});
describe("operation and lifecycle boundaries", () => {
  it("offers explicit-ID operations without fetching private results", async () => {
    const f = mount((f) => <App delcore={f.transport} signOut={() => {}} />, {
      permissions: [P.Permission.EXPERIMENT],
    });
    await selectSpace();
    await screen.findByText("Experiment operations without result access");
    expect(
      f.calls.some(
        (x) => x.method === "listExperiments" || x.method === "listConnections",
      ),
    ).toBe(false);
  });
  it("submits distinct cancellation modes and the supplied resume fingerprint", async () => {
    const f = mount(() => <Operations spaceId={spaceId} />);
    fireEvent.change(screen.getByLabelText("Experiment ID"), {
      target: { value: experiment.id },
    });
    for (const label of ["Graceful stop", "Cancel immediately"]) {
      fireEvent.click(screen.getByRole("button", { name: label }));
      fireEvent.click(screen.getAllByRole("button", { name: label }).at(-1)!);
      await waitFor(() =>
        expect(
          f.calls.filter((x) => x.method === "cancelExperiment").length,
        ).toBe(label === "Graceful stop" ? 1 : 2),
      );
    }
    fireEvent.change(
      screen.getByLabelText("Current local specification fingerprint"),
      { target: { value: specification.fingerprintSha256 } },
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Resume matching experiment" }),
    );
    await waitFor(() =>
      expect(f.calls.some((x) => x.method === "resumeExperiment")).toBe(true),
    );
    expect(
      f.calls
        .filter((x) => x.method === "cancelExperiment")
        .map((x) => (x.input as P.CancelExperimentRequest).mode),
    ).toEqual([P.CancellationMode.GRACEFUL, P.CancellationMode.IMMEDIATE]);
    expect(
      (
        f.calls.find((x) => x.method === "resumeExperiment")!
          .input as P.ResumeExperimentRequest
      ).expectedFingerprintSha256,
    ).toBe(specification.fingerprintSha256);
  });
  it("starts with all frozen limits and policy only after workload confirmation", async () => {
    const f = mount(
      () => (
        <Start
          spaceId={spaceId}
          connection={connection}
          allowed
          onStarted={() => {}}
        />
      ),
      { cloud: true },
    );
    await screen.findByRole("option", { name: "Test runner (mock only)" });
    fireEvent.change(screen.getByLabelText("Cloud tool"), {
      target: { value: "fixture-tool" },
    });
    fillConfiguration();
    fireEvent.click(
      screen.getByRole("button", { name: "Prepare experiment for review" }),
    );
    await screen.findByRole("region", { name: "Prepared experiment review" });
    expect(
      (
        screen.getByRole("button", {
          name: "Start cloud experiment",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    fireEvent.click(screen.getByLabelText(/I confirm these workloads/));
    fireEvent.click(
      screen.getByRole("button", { name: "Start cloud experiment" }),
    );
    await waitFor(() =>
      expect(f.calls.some((x) => x.method === "startExperiment")).toBe(true),
    );
    const req = f.calls.find((x) => x.method === "startExperiment")!
      .input as P.StartExperimentRequest;
    expect(req.specification?.fingerprintSha256).toBe(
      specification.fingerprintSha256,
    );
    expect(req.specification?.policy?.uncertainty?.engineSchemaVersion).toBe(1);
    expect(f.calls.find((x) => x.method === "prepareExperiment")).toBeTruthy();
    expect(req.experimentId).toMatch(/-7[a-f0-9]{3}-/);
  });
  it("preserves parent ID newlines during editing and normalizes the prepared request", async () => {
    const f = mount(() => (
      <Start
        spaceId={spaceId}
        connection={connection}
        allowed
        onStarted={() => {}}
      />
    ));
    fillConfiguration();
    const input = screen.getByLabelText(
      "Parent candidate IDs (optional, one per line)",
    ) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: candidate.id + "\n" } });
    expect(input.value).toBe(candidate.id + "\n");
    fireEvent.change(input, {
      target: { value: input.value + experiment.id + "\n\n" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Prepare experiment for review" }),
    );
    await screen.findByRole("region", { name: "Prepared experiment review" });
    const request = f.calls.find((x) => x.method === "prepareExperiment")!
      .input as P.PrepareExperimentRequest;
    expect(request.configuration?.parentCandidateIds).toEqual([
      candidate.id,
      experiment.id,
    ]);
  });
  it("prepares without cloud support, then invalidates review after editing a field", async () => {
    const f = mount(() => (
      <Start
        spaceId={spaceId}
        connection={connection}
        allowed
        onStarted={() => {}}
      />
    ));
    fillConfiguration();
    fireEvent.click(
      screen.getByRole("button", { name: "Prepare experiment for review" }),
    );
    await screen.findByRole("region", { name: "Prepared experiment review" });
    expect(
      (
        screen.getByRole("button", {
          name: "Start cloud experiment",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    const prepared = f.calls.find((x) => x.method === "prepareExperiment")!
      .input as P.PrepareExperimentRequest;
    expect(prepared.configuration!.policy!.policySha256).toBe("");
    expect(prepared.configuration!.environment!.environmentSha256).toBe("");
    fireEvent.click(screen.getByLabelText(/I confirm these workloads/));
    fireEvent.change(screen.getByLabelText("Candidate count"), {
      target: { value: "2" },
    });
    expect(
      screen.queryByRole("region", { name: "Prepared experiment review" }),
    ).toBeNull();
    expect(
      (screen.getByLabelText(/I confirm these workloads/) as HTMLInputElement)
        .checked,
    ).toBe(false);
    expect(f.calls.some((x) => x.method === "startExperiment")).toBe(false);
  });
  it("discards preparation that finishes after settings change", async () => {
    let release!: () => void;
    const prepareGate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const f = mount(
      () => (
        <Start
          spaceId={spaceId}
          connection={connection}
          allowed
          onStarted={() => {}}
        />
      ),
      { prepareGate },
    );
    fillConfiguration();
    fireEvent.click(
      screen.getByRole("button", { name: "Prepare experiment for review" }),
    );
    await waitFor(() =>
      expect(f.calls.some((x) => x.method === "prepareExperiment")).toBe(true),
    );
    fireEvent.change(screen.getByLabelText("Base SHA"), {
      target: { value: "c".repeat(40) },
    });
    release();
    await waitFor(() => expect(screen.queryByText("Preparing…")).toBeNull());
    expect(
      screen.queryByRole("region", { name: "Prepared experiment review" }),
    ).toBeNull();
  });
  it("reports preparation rejection with editable recovery", async () => {
    mount(
      () => (
        <Start
          spaceId={spaceId}
          connection={connection}
          allowed
          onStarted={() => {}}
        />
      ),
      { prepareError: true },
    );
    fillConfiguration();
    fireEvent.click(
      screen.getByRole("button", { name: "Prepare experiment for review" }),
    );
    await screen.findByText("Fixture: incomplete engine configuration");
    expect(screen.getByText(/Check the source revisions/)).toBeTruthy();
    expect(
      (screen.getByLabelText("Base SHA") as HTMLInputElement).disabled,
    ).toBe(false);
  });
  it("maps declared sample-range policy and clears irrelevant criteria in measurements-only mode", async () => {
    const f = mount(() => (
      <Start
        spaceId={spaceId}
        connection={connection}
        allowed
        onStarted={() => {}}
      />
    ));
    fillConfiguration();
    fireEvent.change(screen.getByLabelText("Policy"), {
      target: { value: String(P.PolicyKind.DECLARED) },
    });
    for (const [label, value] of [
      ["Metric kind", String(P.EngineMetricKind.TIME)],
      ["Metric name", "wall time"],
      ["Metric unit", "ms"],
      ["Maximum relative sample range", "0.03"],
      ["Decision margin", "0.002"],
      ["Repeat tolerance", "0.001"],
      ["Near-threshold margin", "0.001"],
    ])
      fireEvent.change(screen.getByLabelText(label, { exact: false }), {
        target: { value },
      });
    fireEvent.click(
      screen.getByRole("button", { name: "Prepare experiment for review" }),
    );
    await screen.findByRole("region", { name: "Prepared experiment review" });
    const req = f.calls.find((x) => x.method === "prepareExperiment")!
      .input as P.PrepareExperimentRequest;
    expect(req.configuration!.policy!.uncertainty).toMatchObject({
      engineSchemaVersion: 1,
      maximumRelativeRange: 0.03,
      decisionMargin: 0.002,
      repeatTolerance: 0.001,
      nearThreshold: 0.001,
      confidenceLevel: 0,
      maximumRelativeIntervalWidth: 0,
    });
    fireEvent.change(screen.getByLabelText("Policy"), {
      target: { value: String(P.PolicyKind.MEASUREMENTS_ONLY) },
    });
    expect(screen.queryByLabelText("Maximum relative sample range")).toBeNull();
    expect(
      screen.queryByRole("region", { name: "Prepared experiment review" }),
    ).toBeNull();
  });
  it("rejects missing fields and invalid limits without calling preparation", async () => {
    const f = mount(() => (
      <Start
        spaceId={spaceId}
        connection={connection}
        allowed
        onStarted={() => {}}
      />
    ));
    fireEvent.submit(
      screen
        .getByRole("button", { name: "Prepare experiment for review" })
        .closest("form")!,
    );
    await screen.findByText(/Enter exact source/);
    expect(f.calls.some((x) => x.method === "prepareExperiment")).toBe(false);
    fillConfiguration();
    fireEvent.change(screen.getByLabelText("Simultaneous candidates"), {
      target: { value: "6" },
    });
    fireEvent.submit(
      screen
        .getByRole("button", { name: "Prepare experiment for review" })
        .closest("form")!,
    );
    await screen.findByText(/Set positive candidate limits/);
    expect(f.calls.some((x) => x.method === "prepareExperiment")).toBe(false);
  });
  it("uses a new deletion request ID for each experiment", async () => {
    const f = mount(() => <Settings spaceId={spaceId} manage />);
    for (const id of [experiment.id, candidate.id]) {
      fireEvent.change(screen.getByLabelText("Experiment ID"), {
        target: { value: id },
      });
      fireEvent.click(
        screen.getByRole("button", { name: "Delete experiment" }),
      );
      fireEvent.click(
        screen.getAllByRole("button", { name: "Delete experiment" }).at(-1)!,
      );
      await screen.findByText("Deletion retrying");
    }
    const requests = f.calls
      .filter((x) => x.method === "deleteExperiment")
      .map((x) => x.input as P.DeleteExperimentRequest);
    expect(requests.length).toBe(2);
    expect(requests[0].requestId).not.toBe(requests[1].requestId);
    expect(requests.map((x) => x.experimentId)).toEqual([
      experiment.id,
      candidate.id,
    ]);
  });
  it("shows retained cleanup failures after accepted manager deletion", async () => {
    mount(() => <Settings spaceId={spaceId} manage />);
    fireEvent.change(screen.getByLabelText("Experiment ID"), {
      target: { value: experiment.id },
    });
    fireEvent.click(screen.getByRole("button", { name: "Delete experiment" }));
    fireEvent.click(
      screen.getAllByRole("button", { name: "Delete experiment" }).at(-1)!,
    );
    await screen.findByText("Deletion retrying");
    expect(
      screen.getByText(
        "Fixture: storage unavailable; cleanup retained for retry.",
      ),
    ).toBeTruthy();
  });
});
describe("input and URL boundaries", () => {
  it("does not fabricate evidence or source in defaults", () => {
    const c = emptyConfiguration();
    expect(c.baseSha).toBe("");
    expect(c.inputs[0].sha256).toBe("");
    expect(c.environment!.tools[0].version).toBe("");
    expect(() => validateConfiguration(c)).toThrow();
  });
  it("imports frozen metadata only as editable settings with no canonical hashes", () => {
    const c = importConfiguration(
      toJsonString(P.ExperimentSpecificationSchema, specification),
    );
    expect(c.baseSha).toBe(specification.baseSha);
    expect(c.policy!.policySha256).toBe("");
    expect(c.environment!.environmentSha256).toBe("");
    expect(c.limits!.startDuration!.seconds).toBe(3600n);
    expect(() => validateConfiguration(c)).not.toThrow();
  });
  it("produces UUID v7 resource identities and rejects unsafe external URLs", () => {
    expect(uuid7()).toMatch(
      /^[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/,
    );
    expect(safeURL("javascript:alert(1)")).toBeUndefined();
    expect(safeURL("https://github.com.evil.example/x", true)).toBeUndefined();
    expect(safeURL("https://github.com/example/parser", true)).toBeTruthy();
  });
});
