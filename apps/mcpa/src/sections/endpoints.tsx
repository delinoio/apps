// SPDX-License-Identifier: Apache-2.0
import { EndpointFormat, type Resource } from "@repo/protos/mcpa/v1/mcpa_pb";
import { endpointFormatLabels } from "../lib/labels";
import { CopyButton } from "../components/copy-button";
import { EmptyState } from "../components/states";

const formatOrder = [
  EndpointFormat.OPENAI,
  EndpointFormat.CLAUDE,
  EndpointFormat.GEMINI,
];

const formatHints: Record<EndpointFormat, string> = {
  [EndpointFormat.UNSPECIFIED]: "",
  [EndpointFormat.OPENAI]:
    "Chat Completions, Completions, Responses and model listing. Use this as the OpenAI SDK base URL.",
  [EndpointFormat.CLAUDE]:
    "Messages and token counting. Use this as the Anthropic SDK base URL; clients add /v1/messages.",
  [EndpointFormat.GEMINI]:
    "generateContent, streamGenerateContent and countTokens. Clients add /v1beta.",
};

/** Builds a short curl example that reads the call key from $MCPA_KEY. */
export function curlExample(format: EndpointFormat, baseUrl: string): string {
  switch (format) {
    case EndpointFormat.OPENAI:
      return [
        `curl "${baseUrl}/chat/completions" \\`,
        `  -H "Authorization: Bearer $MCPA_KEY" \\`,
        `  -H "Content-Type: application/json" \\`,
        `  -d '{"model": "MODEL", "messages": [{"role": "user", "content": "Hello"}]}'`,
      ].join("\n");
    case EndpointFormat.CLAUDE:
      return [
        `curl "${baseUrl}/v1/messages" \\`,
        `  -H "x-api-key: $MCPA_KEY" \\`,
        `  -H "anthropic-version: 2023-06-01" \\`,
        `  -H "Content-Type: application/json" \\`,
        `  -d '{"model": "MODEL", "max_tokens": 256, "messages": [{"role": "user", "content": "Hello"}]}'`,
      ].join("\n");
    case EndpointFormat.GEMINI:
      return [
        `curl "${baseUrl}/v1beta/models/MODEL:generateContent" \\`,
        `  -H "x-goog-api-key: $MCPA_KEY" \\`,
        `  -H "Content-Type: application/json" \\`,
        `  -d '{"contents": [{"parts": [{"text": "Hello"}]}]}'`,
      ].join("\n");
    default:
      return "";
  }
}

export function EndpointsView({ resource }: { resource: Resource }) {
  const endpoints = [...resource.endpoints].sort(
    (a, b) => formatOrder.indexOf(a.format) - formatOrder.indexOf(b.format),
  );
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>Endpoints</h2>
          <p className="muted">
            Point your client at a base URL and authenticate with a call key.
            Replace MODEL with a model your credentials can serve.
          </p>
        </div>
      </div>
      {endpoints.length ? (
        <ul className="endpoint-list">
          {endpoints.map((endpoint) => {
            const label = endpointFormatLabels[endpoint.format] ?? "Endpoint";
            const example = curlExample(endpoint.format, endpoint.baseUrl);
            return (
              <li key={endpoint.format} className="card-block">
                <div className="endpoint-head">
                  <h3>{label}</h3>
                  <CopyButton
                    value={endpoint.baseUrl}
                    label="Copy URL"
                    accessibleLabel={`Copy ${label} base URL`}
                  />
                </div>
                <code className="url">{endpoint.baseUrl}</code>
                <p className="muted small">{formatHints[endpoint.format]}</p>
                {example && (
                  <details>
                    <summary>Example request</summary>
                    <pre>
                      <code>{example}</code>
                    </pre>
                    <CopyButton
                      value={example}
                      label="Copy example"
                      accessibleLabel={`Copy ${label} example`}
                    />
                  </details>
                )}
              </li>
            );
          })}
        </ul>
      ) : (
        <EmptyState title="No endpoints available" />
      )}
      <p className="muted small">
        Set <span className="mono">MCPA_KEY</span> to a call key from the Call
        keys tab. Calls are billed by traffic at USD 0.07 per GiB.
      </p>
    </div>
  );
}
