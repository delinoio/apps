// SPDX-License-Identifier: Apache-2.0

// Command delitools-client demonstrates the DeliTools public API without an SDK
// package. It translates a short text array and extracts structured data from
// a sentence through the synchronous Connect methods TranslateTextArray and
// ExtractDataFromText.
//
// Environment:
//
//	DELITOOLS_URL        API base URL, default http://127.0.0.1:46322
//	DELITOOLS_API_KEY    DeliTools API key (preferred), or
//	DELITOOLS_TOKEN      Logto user access token
//	DELITOOLS_TENANT_ID  tenant that receives the usage charge
//
// Credentials and response content are never printed. Successful runs print
// counts and request IDs; failures print only the Connect code, the stable
// reason and the request ID.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/delinoio/apps/protos/delitools/v1/delitoolsv1connect"
	"github.com/google/uuid"
)

const defaultURL = "http://127.0.0.1:46322"

// extractionSchema uses the supported schema subset: an object root, every
// property required and additionalProperties set to false.
const extractionSchema = `{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "Full name of the person"},
    "city": {"type": "string", "description": "City where the person lives"}
  },
  "required": ["name", "city"],
  "additionalProperties": false
}`

// options configures one example run.
type options struct {
	baseURL    string
	credential string
	tenantID   string
	outputLang string
	inputLang  string
	model      string
}

func main() {
	output := flag.String("to", "ko", "BCP 47 output language of the translation")
	input := flag.String("from", "", "BCP 47 input language; omit to detect it")
	model := flag.String("model", "", "translation model from the operator allowlist; required")
	flag.Parse()
	o, e := optionsFromEnv(os.Getenv)
	if e != nil {
		fmt.Fprintln(os.Stderr, "DeliTools example failed:", e)
		os.Exit(1)
	}
	o.outputLang, o.inputLang, o.model = *output, *input, *model
	if o.model == "" {
		fmt.Fprintln(os.Stderr, "DeliTools example failed: -model is required")
		os.Exit(1)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	// Calls are synchronous and can take as long as the operator's execution
	// timeout, so the HTTP client has no short overall timeout; the context
	// bounds the run.
	client := delitoolsv1connect.NewDeliToolsServiceClient(http.DefaultClient, o.baseURL)
	if e := run(ctx, client, o, os.Stdout); e != nil {
		fmt.Fprintln(os.Stderr, "DeliTools example failed:", describe(e))
		os.Exit(1)
	}
}

// optionsFromEnv reads the base URL, credential and tenant from the environment.
func optionsFromEnv(env func(string) string) (options, error) {
	o := options{baseURL: env("DELITOOLS_URL"), credential: env("DELITOOLS_API_KEY"), tenantID: env("DELITOOLS_TENANT_ID")}
	if o.baseURL == "" {
		o.baseURL = defaultURL
	}
	if o.credential == "" {
		o.credential = env("DELITOOLS_TOKEN")
	}
	if o.credential == "" || o.tenantID == "" {
		return o, errors.New("DELITOOLS_API_KEY (or DELITOOLS_TOKEN) and DELITOOLS_TENANT_ID are required")
	}
	return o, nil
}

// authorized attaches the credential. Both a DeliTools API key and a Logto
// access token use the Bearer scheme.
func authorized[T any](credential string, message *T) *connect.Request[T] {
	r := connect.NewRequest(message)
	r.Header().Set("Authorization", "Bearer "+credential)
	return r
}

// describe reports the Connect code, the stable reason and the request ID of a
// typed DeliTools error, and never any response content.
func describe(err error) string {
	var ce *connect.Error
	if !errors.As(err, &ce) {
		return "non-Connect error"
	}
	reason, requestID := pb.ErrorReason_ERROR_REASON_UNSPECIFIED, ""
	for _, detail := range ce.Details() {
		if value, e := detail.Value(); e == nil {
			if typed, ok := value.(*pb.ErrorDetail); ok {
				reason, requestID = typed.Reason, typed.RequestId
			}
		}
	}
	return fmt.Sprintf("code=%s reason=%s request_id=%s", ce.Code(), reason, requestID)
}

func run(ctx context.Context, client delitoolsv1connect.DeliToolsServiceClient, o options, out io.Writer) error {
	// 1. Translate. The idempotency key is optional. Reusing it for a retry of
	// the same input within 30 days prevents a second execution and charge, but
	// a repeated call returns an error such as ALREADY_PROCESSED; the result is
	// never replayed. Without a key every call is a new, billable execution.
	translated, e := client.TranslateTextArray(ctx, authorized(o.credential, &pb.TranslateTextArrayRequest{
		TenantId:       o.tenantID,
		OutputLanguage: o.outputLang,
		InputLanguage:  o.inputLang,
		Model:          o.model,
		Texts:          []string{"Good morning", "Where is the station?"},
		IdempotencyKey: uuid.Must(uuid.NewV7()).String(),
	}))
	if e != nil {
		return fmt.Errorf("translate: %w", e)
	}
	// Results are application data. The example reports only content-free
	// metadata; a real consumer hands the result to its own code instead.
	fmt.Fprintf(out, "Translated %d texts (request %s)\n", len(translated.Msg.Texts), translated.Msg.RequestId)

	// 2. Extract data that the text states explicitly. The model is fixed by the
	// service. Information missing from the text returns EXTRACTION_IMPOSSIBLE.
	extracted, e := client.ExtractDataFromText(ctx, authorized(o.credential, &pb.ExtractDataFromTextRequest{
		TenantId:       o.tenantID,
		Text:           "Ada Lovelace lives in London.",
		JsonSchema:     extractionSchema,
		IdempotencyKey: uuid.Must(uuid.NewV7()).String(),
	}))
	if e != nil {
		return fmt.Errorf("extract: %w", e)
	}
	fmt.Fprintf(out, "Extracted %d bytes of JSON data (request %s)\n", len(extracted.Msg.JsonData), extracted.Msg.RequestId)
	// Usage and charges are not part of the responses; inspect them through the
	// billing interface.
	return nil
}
