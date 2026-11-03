// SPDX-License-Identifier: Apache-2.0
import {
  create,
  fromBinary,
  toBinary,
  type DescMessage,
  type DescMethodUnary,
  type MessageInitShape,
} from "@bufbuild/protobuf";
import { base64Decode, base64Encode } from "@bufbuild/protobuf/wire";
import { Code, ConnectError, type Transport } from "@connectrpc/connect";
import { errorFromJson } from "@connectrpc/connect/protocol-connect";
import type { ClientKind } from "@repo/protos/anydash/v1/anydash_pb";
import {
  APP_VERSION,
  clientKindHeader,
  hostCall,
  HostMethod,
  RpcTarget,
  type RpcReply,
  type RpcRequest,
} from "./host";
import { logError } from "./log";

export const CORRELATION_HEADER = "x-anydash-correlation-id";

const httpCode = (status: number) => {
  switch (status) {
    case 400:
      return Code.InvalidArgument;
    case 401:
      return Code.Unauthenticated;
    case 403:
      return Code.PermissionDenied;
    case 404:
      return Code.Unimplemented;
    case 409:
      return Code.Aborted;
    case 429:
      return Code.ResourceExhausted;
    case 502:
    case 503:
    case 504:
      return Code.Unavailable;
    default:
      return Code.Unknown;
  }
};

/**
 * Connect unary transport over the platform host. The host performs the HTTP
 * request: it adds the Logto bearer token for the API, or uses the OS-user
 * restricted collector socket for LocalCollectorService, so tokens stay in the
 * host. Bodies cross the bridge as base64 binary Protobuf.
 */
export function createHostTransport(
  target: RpcTarget,
  kind: () => ClientKind,
  send: (request: RpcRequest) => Promise<RpcReply> = (request) =>
    hostCall<RpcReply>(HostMethod.Rpc, request),
): Transport {
  return {
    async unary<I extends DescMessage, O extends DescMessage>(
      method: DescMethodUnary<I, O>,
      signal: AbortSignal | undefined,
      _timeoutMs: number | undefined,
      header: HeadersInit | undefined,
      input: MessageInitShape<I>,
    ) {
      "background only";
      if (signal?.aborted)
        throw new ConnectError("The request was cancelled.", Code.Canceled);
      const headers: Record<string, string> = {
        "X-AnyDash-Client-Kind": clientKindHeader(kind()),
        "X-AnyDash-Client-Version": APP_VERSION,
      };
      if (header)
        new Headers(header).forEach((value, key) => (headers[key] = value));
      const message = create(method.input, input);
      let reply: RpcReply;
      try {
        reply = await send({
          target,
          service: method.parent.typeName,
          method: method.name,
          body: base64Encode(toBinary(method.input, message)),
          headers,
        });
      } catch (error) {
        throw new ConnectError(
          error instanceof Error ? error.message : "The request failed.",
          Code.Unavailable,
        );
      }
      const metadata = new Headers();
      if (reply.correlationId)
        metadata.set(CORRELATION_HEADER, reply.correlationId);
      const bytes = base64Decode(reply.body || "");
      if (reply.status !== 200) {
        const fallback = new ConnectError(
          `HTTP ${reply.status}`,
          httpCode(reply.status),
          metadata,
        );
        let error = fallback;
        try {
          error = errorFromJson(
            JSON.parse(new TextDecoder().decode(bytes)),
            metadata,
            fallback,
          );
        } catch {
          error = fallback;
        }
        logError("anydash_rpc_failed", {
          service: method.parent.typeName,
          method: method.name,
          code: Code[error.code],
          correlationId: reply.correlationId,
        });
        throw error;
      }
      return {
        stream: false as const,
        service: method.parent,
        method,
        header: metadata,
        trailer: new Headers(),
        message: fromBinary(method.output, bytes),
      };
    },
    stream() {
      return Promise.reject(
        new ConnectError(
          "Streaming is not used by AnyDash.",
          Code.Unimplemented,
        ),
      );
    },
  };
}
