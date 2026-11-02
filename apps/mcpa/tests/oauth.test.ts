// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "@rstest/core";
import { create } from "@bufbuild/protobuf";
import {
  OauthFlowKind,
  OauthFlowState,
  ProviderOAuthFlowSchema,
} from "@repo/protos/mcpa/v1/mcpa_pb";
import {
  initialOAuthState,
  isCallbackUrl,
  OAuthEventType,
  oauthReducer,
  OAuthStep,
  type OAuthEvent,
  type OAuthState,
} from "../src/lib/oauth";

const flow = (kind: OauthFlowKind, state: OauthFlowState) =>
  create(ProviderOAuthFlowSchema, { id: "flow-1", kind, state });
const run = (events: OAuthEvent[], from: OAuthState = initialOAuthState) =>
  events.reduce(oauthReducer, from);

describe("OAuth connect flow", () => {
  it("moves through the authorization-code steps", () => {
    let state = run([{ type: OAuthEventType.Start }]);
    expect(state.step).toBe(OAuthStep.Starting);
    state = oauthReducer(state, {
      type: OAuthEventType.FlowUpdated,
      flow: flow(OauthFlowKind.AUTHORIZATION_CODE, OauthFlowState.PENDING),
    });
    expect(state.step).toBe(OAuthStep.Authorize);
    state = oauthReducer(state, { type: OAuthEventType.Submit });
    expect(state.step).toBe(OAuthStep.Submitting);
    state = oauthReducer(state, {
      type: OAuthEventType.FlowUpdated,
      flow: flow(OauthFlowKind.AUTHORIZATION_CODE, OauthFlowState.COMPLETED),
    });
    expect(state.step).toBe(OAuthStep.Completed);
  });

  it("keeps the flow and shows the error when submitting fails", () => {
    const error = new Error("bad callback");
    const state = run([
      { type: OAuthEventType.Start },
      {
        type: OAuthEventType.FlowUpdated,
        flow: flow(OauthFlowKind.AUTHORIZATION_CODE, OauthFlowState.PENDING),
      },
      { type: OAuthEventType.Submit },
      { type: OAuthEventType.RequestFailed, error },
    ]);
    expect(state.step).toBe(OAuthStep.Authorize);
    expect(state.flow?.id).toBe("flow-1");
    expect(state.error).toBe(error);
  });

  it("returns to the provider choice when starting fails", () => {
    const state = run([
      { type: OAuthEventType.Start },
      { type: OAuthEventType.RequestFailed, error: "denied" },
    ]);
    expect(state).toEqual({ step: OAuthStep.Choose, error: "denied" });
  });

  it("polls device-code flows until a terminal state", () => {
    let state = run([
      { type: OAuthEventType.Start },
      {
        type: OAuthEventType.FlowUpdated,
        flow: flow(OauthFlowKind.DEVICE_CODE, OauthFlowState.PENDING),
      },
    ]);
    expect(state.step).toBe(OAuthStep.Device);
    state = oauthReducer(state, {
      type: OAuthEventType.RequestFailed,
      error: "network",
    });
    expect(state.step).toBe(OAuthStep.Device);
    state = oauthReducer(state, {
      type: OAuthEventType.FlowUpdated,
      flow: flow(OauthFlowKind.DEVICE_CODE, OauthFlowState.EXPIRED),
    });
    expect(state.step).toBe(OAuthStep.Expired);
    // A late poll response does not reopen a finished flow.
    state = oauthReducer(state, {
      type: OAuthEventType.FlowUpdated,
      flow: flow(OauthFlowKind.DEVICE_CODE, OauthFlowState.PENDING),
    });
    expect(state.step).toBe(OAuthStep.Expired);
  });

  it("maps failed flows and resets", () => {
    const failed = run([
      { type: OAuthEventType.Start },
      {
        type: OAuthEventType.FlowUpdated,
        flow: flow(OauthFlowKind.DEVICE_CODE, OauthFlowState.FAILED),
      },
    ]);
    expect(failed.step).toBe(OAuthStep.Failed);
    expect(oauthReducer(failed, { type: OAuthEventType.Reset })).toBe(
      initialOAuthState,
    );
  });

  it("accepts only absolute http(s) callback URLs", () => {
    expect(isCallbackUrl("http://localhost:1455/auth/callback?code=x")).toBe(
      true,
    );
    expect(isCallbackUrl("code=x")).toBe(false);
    expect(isCallbackUrl("javascript:alert(1)")).toBe(false);
  });
});
