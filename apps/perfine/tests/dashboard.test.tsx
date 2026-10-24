// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "@rstest/core";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { TransportProvider } from "@connectrpc/connect-query";
import { create, toJsonString } from "@bufbuild/protobuf";
import * as P from "@delino/protos/perfine/v1/perfine_pb";
import { App } from "../src/app";
import { Detail, Status } from "../src/experiments";
import { Review } from "../src/review";
import { Operations } from "../src/operations";
import { Settings } from "../src/repositories";
import { Start } from "../src/start";
import { Confirm } from "../src/ui";
import { parseSpecification, queryClient, safeURL, uuid7 } from "../src/core";
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
  return f;
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
describe("mocked typed RPC dashboard", () => {
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
    const file = {
      size: 1000,
      text: async () =>
        toJsonString(P.ExperimentSpecificationSchema, specification),
    };
    fireEvent.change(screen.getByLabelText("Prepared specification JSON"), {
      target: { files: [file] },
    });
    await screen.findByText("Candidate limit / simultaneous");
    fireEvent.change(screen.getByLabelText("Cloud tool"), {
      target: { value: "fixture-tool" },
    });
    expect(
      (
        screen.getByRole("button", {
          name: "Start cloud experiment",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(
      screen.getByRole("button", { name: "Start cloud experiment" }),
    );
    await waitFor(() =>
      expect(f.calls.some((x) => x.method === "startExperiment")).toBe(true),
    );
    const req = f.calls.find((x) => x.method === "startExperiment")!
      .input as P.StartExperimentRequest;
    expect(req.specification).toEqual(specification);
    expect(req.experimentId).toMatch(/-7[a-f0-9]{3}-/);
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
  it("accepts complete metadata and rejects missing limits, policy uncertainty and unconfirmed workloads", () => {
    expect(
      parseSpecification(
        toJsonString(P.ExperimentSpecificationSchema, specification),
      ).baseSha,
    ).toBe(specification.baseSha);
    for (const bad of [
      create(P.ExperimentSpecificationSchema, {
        ...specification,
        limits: { ...specification.limits!, simultaneousCandidates: 6 },
      }),
      create(P.ExperimentSpecificationSchema, {
        ...specification,
        policy: { ...specification.policy!, uncertainty: undefined },
      }),
      create(P.ExperimentSpecificationSchema, {
        ...specification,
        workloadRepresentativenessConfirmed: false,
      }),
    ])
      expect(() =>
        parseSpecification(toJsonString(P.ExperimentSpecificationSchema, bad)),
      ).toThrow();
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
