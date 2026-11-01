// SPDX-License-Identifier: Apache-2.0
import {
  ActorKind,
  AuditAction,
  CallStatus,
  CredentialKind,
  EndpointFormat,
  PrincipalKind,
  Provider,
  ResourceRole,
  StatusClass,
} from "@repo/protos/mcpa/v1/mcpa_pb";

export const providerLabels: Record<Provider, string> = {
  [Provider.UNSPECIFIED]: "Unknown provider",
  [Provider.GEMINI]: "Gemini API",
  [Provider.GEMINI_CLI]: "Gemini CLI",
  [Provider.VERTEX]: "Vertex AI",
  [Provider.CLAUDE]: "Claude",
  [Provider.CODEX]: "OpenAI / Codex",
  [Provider.ANTIGRAVITY]: "Antigravity",
  [Provider.KIMI]: "Kimi",
  [Provider.OPENAI_COMPATIBLE]: "OpenAI-compatible",
};

export const providerLabel = (provider: Provider) =>
  providerLabels[provider] ?? providerLabels[Provider.UNSPECIFIED];

/** Providers that sign in with an OAuth account, with sign-in wording. */
export const oauthProviders: { provider: Provider; label: string }[] = [
  { provider: Provider.CODEX, label: "Codex (ChatGPT account)" },
  { provider: Provider.CLAUDE, label: "Claude Code (Claude account)" },
  { provider: Provider.GEMINI_CLI, label: "Gemini CLI (Google account)" },
  { provider: Provider.ANTIGRAVITY, label: "Antigravity (Google account)" },
  { provider: Provider.KIMI, label: "Kimi" },
];

/** Providers that accept an API key, with key wording. */
export const apiKeyProviders: { provider: Provider; label: string }[] = [
  { provider: Provider.CODEX, label: "OpenAI API" },
  { provider: Provider.CLAUDE, label: "Anthropic API (Claude)" },
  { provider: Provider.GEMINI, label: "Gemini API" },
  { provider: Provider.VERTEX, label: "Vertex AI" },
  { provider: Provider.KIMI, label: "Kimi" },
  { provider: Provider.OPENAI_COMPATIBLE, label: "OpenAI-compatible service" },
];

export const credentialKindLabel = (kind: CredentialKind) =>
  kind === CredentialKind.OAUTH
    ? "Account sign-in"
    : kind === CredentialKind.API_KEY
      ? "API key"
      : "Unknown";

export const roleLabel = (role: ResourceRole) =>
  role === ResourceRole.MANAGE
    ? "Manage"
    : role === ResourceRole.INVOKE
      ? "Invoke"
      : "No access";

export const roleDescription = (role: ResourceRole) =>
  role === ResourceRole.MANAGE
    ? "Change settings, credentials, access, budgets and all call keys, and view usage, errors and audit."
    : "Issue and manage their own call keys and view usage.";

export const principalKindLabel = (kind: PrincipalKind) =>
  kind === PrincipalKind.GROUP ? "Group" : "User";

export const endpointFormatLabels: Record<EndpointFormat, string> = {
  [EndpointFormat.UNSPECIFIED]: "Other",
  [EndpointFormat.OPENAI]: "OpenAI-compatible",
  [EndpointFormat.CLAUDE]: "Claude-compatible",
  [EndpointFormat.GEMINI]: "Gemini-compatible",
};

export enum Tone {
  Neutral = "neutral",
  Good = "good",
  Warning = "warning",
  Bad = "bad",
}

export const callStatusInfo: Record<CallStatus, { label: string; tone: Tone }> =
  {
    [CallStatus.UNSPECIFIED]: { label: "Unknown", tone: Tone.Neutral },
    [CallStatus.SUCCEEDED]: { label: "Succeeded", tone: Tone.Good },
    [CallStatus.PROVIDER_ERROR]: { label: "Provider error", tone: Tone.Bad },
    [CallStatus.PROVIDER_AUTH_ERROR]: {
      label: "Provider sign-in failed",
      tone: Tone.Bad,
    },
    [CallStatus.CLIENT_CANCELLED]: {
      label: "Cancelled by caller",
      tone: Tone.Warning,
    },
    [CallStatus.STREAM_INTERRUPTED]: {
      label: "Stream interrupted",
      tone: Tone.Warning,
    },
    [CallStatus.REJECTED_MODEL_NOT_ALLOWED]: {
      label: "Model not allowed",
      tone: Tone.Bad,
    },
    [CallStatus.REJECTED_BUDGET]: { label: "Budget reached", tone: Tone.Bad },
    [CallStatus.REJECTED_TENANT]: { label: "Tenant inactive", tone: Tone.Bad },
    [CallStatus.REJECTED_DEPENDENCY]: {
      label: "Service unavailable",
      tone: Tone.Bad,
    },
    [CallStatus.NO_CREDENTIAL]: { label: "No credential", tone: Tone.Bad },
    [CallStatus.INTERNAL_ERROR]: { label: "Internal error", tone: Tone.Bad },
  };

export const callStatusOptions = (
  Object.keys(callStatusInfo).map(Number) as CallStatus[]
).filter((status) => status !== CallStatus.UNSPECIFIED);

export const statusClassLabels: Record<StatusClass, string> = {
  [StatusClass.STATUS_CLASS_UNSPECIFIED]: "—",
  [StatusClass.STATUS_CLASS_2XX]: "2xx",
  [StatusClass.STATUS_CLASS_3XX]: "3xx",
  [StatusClass.STATUS_CLASS_4XX]: "4xx",
  [StatusClass.STATUS_CLASS_429]: "429 rate limited",
  [StatusClass.STATUS_CLASS_5XX]: "5xx",
  [StatusClass.STATUS_CLASS_NONE]: "No response",
};

export const auditActionLabels: Record<AuditAction, string> = {
  [AuditAction.UNSPECIFIED]: "Other change",
  [AuditAction.RESOURCE_CREATED]: "Resource created",
  [AuditAction.RESOURCE_UPDATED]: "Resource updated",
  [AuditAction.RESOURCE_DELETED]: "Resource deleted",
  [AuditAction.CREDENTIAL_REGISTERED]: "Credential added",
  [AuditAction.CREDENTIAL_REMOVED]: "Credential removed",
  [AuditAction.CALL_KEY_ISSUED]: "Call key issued",
  [AuditAction.CALL_KEY_REVOKED]: "Call key revoked",
  [AuditAction.ACL_CHANGED]: "Access changed",
  [AuditAction.ROUTING_CHANGED]: "Routing changed",
  [AuditAction.RESOURCE_BUDGET_CHANGED]: "Resource budget changed",
  [AuditAction.TENANT_BUDGET_CHANGED]: "Tenant budget changed",
};

export const actorKindLabels: Record<ActorKind, string> = {
  [ActorKind.UNSPECIFIED]: "Unknown",
  [ActorKind.CONSOLE]: "Console",
  [ActorKind.MCP]: "MCP",
  [ActorKind.PERSONAL_KEY]: "Personal key",
  [ActorKind.SYSTEM]: "System",
};
