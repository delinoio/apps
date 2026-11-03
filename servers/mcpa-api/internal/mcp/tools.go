// SPDX-License-Identifier: FSL-1.1-ALv2

package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"strings"
	"time"
	"unicode"

	"connectrpc.com/connect"
	gomcp "github.com/modelcontextprotocol/go-sdk/mcp"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
)

// rpcCall decodes ProtoJSON arguments, calls one McpaService handler and
// returns the response message.
type rpcCall func(ctx context.Context, h mcpav1connect.McpaServiceHandler, args json.RawMessage) (proto.Message, error)

// bind adapts a McpaServiceHandler method expression to rpcCall.
func bind[Req, Resp any, PReq interface {
	*Req
	proto.Message
}, PResp interface {
	*Resp
	proto.Message
}](method func(mcpav1connect.McpaServiceHandler, context.Context, *connect.Request[Req]) (*connect.Response[Resp], error)) rpcCall {
	return func(ctx context.Context, h mcpav1connect.McpaServiceHandler, args json.RawMessage) (proto.Message, error) {
		req := PReq(new(Req))
		if err := unmarshalArgs(args, req); err != nil {
			return nil, err
		}
		resp, err := method(h, ctx, connect.NewRequest((*Req)(req)))
		if err != nil {
			return nil, err
		}
		if resp == nil || resp.Msg == nil {
			return nil, apperr.Internal(fmt.Errorf("%T returned no message", req))
		}
		return PResp(resp.Msg), nil
	}
}

var argsOptions = protojson.UnmarshalOptions{}

// unmarshalArgs decodes tool arguments in ProtoJSON. The parser error is not
// returned because it can quote argument values, such as API keys.
func unmarshalArgs(args json.RawMessage, msg proto.Message) error {
	trimmed := strings.TrimSpace(string(args))
	if trimmed == "" || trimmed == "null" {
		return nil
	}
	if err := argsOptions.Unmarshal([]byte(trimmed), msg); err != nil {
		return apperr.InvalidInput("arguments must be the ProtoJSON form of " + string(msg.ProtoReflect().Descriptor().FullName()) +
			" with the fields of the tool input schema")
	}
	return nil
}

var resultOptions = protojson.MarshalOptions{UseProtoNames: true}

// effect classifies what an RPC does to the server state, following the
// MCP ToolAnnotations semantics: a read-only tool does not modify its
// environment, and a non-destructive tool performs only additive updates.
// Each RPC is classified by its handler, not by its name.
type effect int

const (
	effectUnspecified effect = iota
	// effectRead changes no resource, credential, key, ACL, budget or flow.
	// Refreshing a cached Delcore group snapshot does not count as a change.
	effectRead
	// effectAdd only creates state or advances a flow, for example a
	// provider OAuth poll that registers a credential when it completes.
	effectAdd
	// effectReplace overwrites or clears existing settings, such as a
	// resource name, the model allowlist, a role or a budget limit.
	effectReplace
	// effectRemove deletes or revokes state. Repeating it has no further
	// effect: the target is already gone or revoked.
	effectRemove
)

// rpc is one McpaService method exposed as a tool.
type rpc struct {
	method      protoreflect.Name
	effect      effect
	description string
	call        rpcCall
}

// rpcs lists every McpaService method. TestEveryRPCHasATool keeps it
// complete, and TestToolAnnotations pins the effect of every tool.
var rpcs = []rpc{
	{"GetViewer", effectRead, "Return the caller's Delcore user ID, actor kind and the tenants it can act in.",
		bind(mcpav1connect.McpaServiceHandler.GetViewer)},
	{"ListTenantGroups", effectRead, "List the Delcore groups of a tenant, for ACL entries. Requires the access_resources capability.",
		bind(mcpav1connect.McpaServiceHandler.ListTenantGroups)},
	{"CreateResource", effectAdd, "Create a resource in a tenant. Requires the create_resources capability; the caller becomes a manager. request_key is a client-generated UUID v7 that makes retries return the original result.",
		bind(mcpav1connect.McpaServiceHandler.CreateResource)},
	{"GetResource", effectRead, "Get a resource with its routing settings, the OpenAI-, Claude- and Gemini-compatible base URLs and the caller's role. Requires the invoke role.",
		bind(mcpav1connect.McpaServiceHandler.GetResource)},
	{"ListResources", effectRead, "List the resources of a tenant on which the caller has a role.",
		bind(mcpav1connect.McpaServiceHandler.ListResources)},
	{"UpdateResource", effectReplace, "Change the name or description of a resource. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.UpdateResource)},
	{"DeleteResource", effectRemove, "Delete a resource. Its call keys, credentials and settings are removed immediately; usage, error and audit records stay until retention ends. confirm_name must equal the resource name. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.DeleteResource)},
	{"UpdateRouting", effectReplace, "Replace the routing settings of a resource: round-robin load balancing and the model allowlist (empty allows every model). Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.UpdateRouting)},
	{"ListAclEntries", effectRead, "List the user and group roles of a resource. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.ListAclEntries)},
	{"PutAclEntry", effectReplace, "Give a Delcore user or group the manage or invoke role on a resource. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.PutAclEntry)},
	{"RemoveAclEntry", effectRemove, "Remove the role of a user or group from a resource. The last direct manager cannot be removed unless a group holds manage. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.RemoveAclEntry)},
	{"ListCredentials", effectRead, "List the provider credentials of a resource without secrets. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.ListCredentials)},
	{"RegisterApiKeyCredential", effectAdd, "Register a provider API key for a resource. The key is stored encrypted and is never returned. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.RegisterApiKeyCredential)},
	{"RemoveCredential", effectRemove, "Remove a provider credential. New calls stop using it immediately. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.RemoveCredential)},
	{"StartProviderOAuth", effectAdd, "Start a provider OAuth flow that connects a provider account to a resource. Open authorization_url, or enter user_code at verification_uri for device-code flows. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.StartProviderOAuth)},
	{"CompleteProviderOAuth", effectAdd, "Complete an authorization-code provider OAuth flow with the full final redirect URL from the browser. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.CompleteProviderOAuth)},
	{"GetProviderOAuthFlow", effectAdd, "Get the state of a provider OAuth flow. Device-code flows are polled at the provider; a poll that finds the authorization granted registers the credential. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.GetProviderOAuthFlow)},
	{"IssueCallKey", effectAdd, "Issue a call key for model calls on a resource. The secret is returned only by the first execution of request_key, a client-generated UUID v7. Requires the invoke role.",
		bind(mcpav1connect.McpaServiceHandler.IssueCallKey)},
	{"ListCallKeys", effectRead, "List the call keys of a resource. Managers see every key; invokers see their own. Requires the invoke role.",
		bind(mcpav1connect.McpaServiceHandler.ListCallKeys)},
	{"RevokeCallKey", effectRemove, "Revoke a call key. New calls with it fail immediately. Invokers revoke their own keys; managers revoke any key of the resource.",
		bind(mcpav1connect.McpaServiceHandler.RevokeCallKey)},
	{"GetResourceBudget", effectRead, "Get the USD budget of a resource for the current Delcore billing period and whether it is exhausted. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.GetResourceBudget)},
	{"SetResourceBudget", effectReplace, "Set or remove (empty limit) the USD budget of a resource. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.SetResourceBudget)},
	{"GetTenantBudget", effectRead, "Get the USD budget of a tenant for the current Delcore billing period. Requires the manage_tenant_budget capability.",
		bind(mcpav1connect.McpaServiceHandler.GetTenantBudget)},
	{"SetTenantBudget", effectReplace, "Set or remove (empty limit) the USD budget of a tenant. Requires the manage_tenant_budget capability.",
		bind(mcpav1connect.McpaServiceHandler.SetTenantBudget)},
	{"GetUsage", effectRead, "Get usage: request and response bytes, total GiB and the estimated USD at USD 0.07 per GiB, with daily rows. Delcore issues the final bill. Requires the invoke role on each included resource.",
		bind(mcpav1connect.McpaServiceHandler.GetUsage)},
	{"ListCallRecords", effectRead, "List request-level call records with status, provider status class, error class and request ID. Records contain no request or response bodies. Requires the manage role on each included resource.",
		bind(mcpav1connect.McpaServiceHandler.ListCallRecords)},
	{"ListAuditEvents", effectRead, "List the audit events of a resource. Requires the manage role.",
		bind(mcpav1connect.McpaServiceHandler.ListAuditEvents)},
	{"ListTenantAuditEvents", effectRead, "List the tenant-level audit events of a tenant, such as tenant budget changes. Requires the manage_tenant_budget capability.",
		bind(mcpav1connect.McpaServiceHandler.ListTenantAuditEvents)},
}

// ToolName returns the snake_case tool name of an RPC method name, for
// example create_resource for CreateResource.
func ToolName(method protoreflect.Name) string {
	var b strings.Builder
	runes := []rune(string(method))
	for i, r := range runes {
		if unicode.IsUpper(r) {
			if i > 0 && (unicode.IsLower(runes[i-1]) || (i+1 < len(runes) && unicode.IsLower(runes[i+1]))) {
				b.WriteByte('_')
			}
			r = unicode.ToLower(r)
		}
		b.WriteRune(r)
	}
	return b.String()
}

// serviceDescriptor is mcpa.v1.McpaService.
func serviceDescriptor() protoreflect.ServiceDescriptor {
	return mcpav1.File_mcpa_v1_mcpa_proto.Services().ByName("McpaService")
}

// annotations returns the MCP hints of an effect. OpenWorldHint is false for
// every tool: tools act on the caller's Managed CPA resources and reach only
// the configured Delcore and provider endpoints.
func annotations(e effect) (*gomcp.ToolAnnotations, error) {
	yes, no := true, false
	switch e {
	case effectRead:
		return &gomcp.ToolAnnotations{ReadOnlyHint: true, IdempotentHint: true, OpenWorldHint: &no}, nil
	case effectAdd:
		return &gomcp.ToolAnnotations{DestructiveHint: &no, OpenWorldHint: &no}, nil
	case effectReplace:
		return &gomcp.ToolAnnotations{DestructiveHint: &yes, OpenWorldHint: &no}, nil
	case effectRemove:
		return &gomcp.ToolAnnotations{DestructiveHint: &yes, IdempotentHint: true, OpenWorldHint: &no}, nil
	default:
		return nil, fmt.Errorf("unclassified effect %d", e)
	}
}

// Tools builds the tool definitions and handlers of every RPC. It fails when
// the table names a method that the service does not have.
func Tools(h mcpav1connect.McpaServiceHandler, log *slog.Logger) ([]*gomcp.Tool, []gomcp.ToolHandler, error) {
	sd := serviceDescriptor()
	tools := make([]*gomcp.Tool, 0, len(rpcs))
	handlers := make([]gomcp.ToolHandler, 0, len(rpcs))
	for _, r := range rpcs {
		md := sd.Methods().ByName(r.method)
		if md == nil {
			return nil, nil, fmt.Errorf("mcp: McpaService has no method %s", r.method)
		}
		ann, err := annotations(r.effect)
		if err != nil {
			return nil, nil, fmt.Errorf("mcp: %s: %w", r.method, err)
		}
		name := ToolName(r.method)
		tools = append(tools, &gomcp.Tool{
			Name:         name,
			Description:  r.description,
			InputSchema:  MessageSchema(md.Input()),
			OutputSchema: MessageSchema(md.Output()),
			Annotations:  ann,
		})
		handlers = append(handlers, toolHandler(name, r.call, h, log))
	}
	return tools, handlers, nil
}

// principalKey is the TokenInfo.Extra key of the authenticated principal.
const principalKey = "mcpa.principal"

func principalOf(ctx context.Context, req *gomcp.CallToolRequest) *auth.Principal {
	if req != nil && req.Extra != nil && req.Extra.TokenInfo != nil {
		if p, ok := req.Extra.TokenInfo.Extra[principalKey].(*auth.Principal); ok {
			return p
		}
	}
	return auth.FromContext(ctx)
}

func toolHandler(name string, call rpcCall, h mcpav1connect.McpaServiceHandler, log *slog.Logger) gomcp.ToolHandler {
	return func(ctx context.Context, req *gomcp.CallToolRequest) (*gomcp.CallToolResult, error) {
		start := time.Now()
		if apperr.TraceID(ctx) == "" {
			ctx = apperr.WithTraceID(ctx, apperr.NewTraceID())
		}
		p := principalOf(ctx, req)
		var msg proto.Message
		var err error
		if p == nil {
			err = apperr.Unauthenticated("sign in to continue")
		} else {
			ctx = auth.WithPrincipal(ctx, p)
			var args json.RawMessage
			if req != nil && req.Params != nil {
				args = req.Params.Arguments
			}
			msg, err = call(ctx, h, args)
		}
		if err == nil {
			out, merr := resultOptions.Marshal(msg)
			if merr == nil {
				log.DebugContext(ctx, "mcp tool call", "tool", name, "actor_kind", p.Kind.String(), "duration_ms", time.Since(start).Milliseconds())
				return &gomcp.CallToolResult{
					Content:           []gomcp.Content{&gomcp.TextContent{Text: string(out)}},
					StructuredContent: json.RawMessage(out),
				}, nil
			}
			err = apperr.Internal(merr)
		}
		e := apperr.As(err)
		attrs := []any{"tool", name, "code", e.Code.String(), "reason", e.Reason.String(), "duration_ms", time.Since(start).Milliseconds()}
		if p != nil {
			attrs = append(attrs, "actor_kind", p.Kind.String())
		}
		switch e.Code {
		case connect.CodeInternal, connect.CodeUnavailable, connect.CodeUnknown, connect.CodeDataLoss:
			if e.Cause != nil {
				attrs = append(attrs, "error", e.Cause.Error())
			}
			log.ErrorContext(ctx, "mcp tool call failed", attrs...)
		default:
			log.InfoContext(ctx, "mcp tool call rejected", attrs...)
		}
		return ToolError(ctx, e), nil
	}
}

// ToolErrorBody is the JSON body of a failed tool result.
type ToolErrorBody struct {
	Error ToolErrorDetail `json:"error"`
}

// ToolErrorDetail carries the typed failure of a tool call.
type ToolErrorDetail struct {
	// Reason is the mcpa.v1.ErrorReason name.
	Reason              string `json:"reason"`
	Message             string `json:"message"`
	TraceID             string `json:"trace_id"`
	ReauthenticationURL string `json:"reauthentication_url,omitempty"`
}

// ToolError renders a typed error as an MCP tool error result.
func ToolError(ctx context.Context, e *apperr.Error) *gomcp.CallToolResult {
	body, _ := json.Marshal(ToolErrorBody{Error: ToolErrorDetail{
		Reason: e.Reason.String(), Message: e.Message, TraceID: apperr.TraceID(ctx), ReauthenticationURL: e.ReauthURL,
	}})
	return &gomcp.CallToolResult{IsError: true, Content: []gomcp.Content{&gomcp.TextContent{Text: string(body)}}}
}
