// SPDX-License-Identifier: Apache-2.0
import {
  OauthFlowKind,
  OauthFlowState,
  type ProviderOAuthFlow,
} from "@repo/protos/mcpa/v1/mcpa_pb";

/** Console steps for connecting a provider account through OAuth. */
export enum OAuthStep {
  Choose = "choose",
  Starting = "starting",
  Authorize = "authorize",
  Submitting = "submitting",
  Device = "device",
  Completed = "completed",
  Failed = "failed",
  Expired = "expired",
}

export enum OAuthEventType {
  Start = "start",
  FlowUpdated = "flow-updated",
  Submit = "submit",
  RequestFailed = "request-failed",
  Reset = "reset",
}

export type OAuthState = {
  step: OAuthStep;
  flow?: ProviderOAuthFlow;
  error?: unknown;
};

export type OAuthEvent =
  | { type: OAuthEventType.Start }
  | { type: OAuthEventType.FlowUpdated; flow: ProviderOAuthFlow }
  | { type: OAuthEventType.Submit }
  | { type: OAuthEventType.RequestFailed; error: unknown }
  | { type: OAuthEventType.Reset };

export const initialOAuthState: OAuthState = { step: OAuthStep.Choose };

export function stepForFlow(flow: ProviderOAuthFlow): OAuthStep {
  switch (flow.state) {
    case OauthFlowState.COMPLETED:
      return OAuthStep.Completed;
    case OauthFlowState.EXPIRED:
      return OAuthStep.Expired;
    case OauthFlowState.PENDING:
      if (flow.kind === OauthFlowKind.DEVICE_CODE) return OAuthStep.Device;
      if (flow.kind === OauthFlowKind.AUTHORIZATION_CODE)
        return OAuthStep.Authorize;
      return OAuthStep.Failed;
    default:
      return OAuthStep.Failed;
  }
}

const terminal = new Set([
  OAuthStep.Completed,
  OAuthStep.Failed,
  OAuthStep.Expired,
]);

export function oauthReducer(state: OAuthState, event: OAuthEvent): OAuthState {
  switch (event.type) {
    case OAuthEventType.Start:
      return { step: OAuthStep.Starting };
    case OAuthEventType.FlowUpdated:
      // A late poll response must not reopen a finished flow.
      if (terminal.has(state.step)) return state;
      return { step: stepForFlow(event.flow), flow: event.flow };
    case OAuthEventType.Submit:
      if (state.step !== OAuthStep.Authorize) return state;
      return { ...state, step: OAuthStep.Submitting, error: undefined };
    case OAuthEventType.RequestFailed:
      if (state.step === OAuthStep.Starting)
        return { step: OAuthStep.Choose, error: event.error };
      if (state.step === OAuthStep.Submitting)
        return { ...state, step: OAuthStep.Authorize, error: event.error };
      // Device polling keeps going after a transient failure.
      return { ...state, error: event.error };
    case OAuthEventType.Reset:
      return initialOAuthState;
  }
}

/** The pasted value must be the provider's final redirect URL. */
export function isCallbackUrl(value: string): boolean {
  try {
    const url = new URL(value.trim());
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
}
