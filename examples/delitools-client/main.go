// SPDX-License-Identifier: Apache-2.0

// Command delitools-client demonstrates the DeliTools public API without an SDK
// package. It translates a short text array and extracts structured data from
// a sentence through the synchronous Connect methods TranslateTextArray and
// ExtractDataFromText.
//
// The default operation, all, runs both calls and needs a key or token that
// holds both the translator and the extractor capability; with a single
// capability the first call succeeds and is billed, and the second one fails
// with a permission error. Select -operation translate or -operation extract
// to run one call with a single-capability key.
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

const (
	defaultURL = "http://127.0.0.1:46322"
	// defaultCallTimeout is zero: no client deadline. The operator's execution
	// timeout (up to an hour) is enforced by the server and returns a typed
	// timeout error, while a shorter client default would cancel an otherwise
	// valid call whose usage is still billed.
	defaultCallTimeout time.Duration = 0
)

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

// operation selects which of the two DeliTools calls an example run performs.
type operation int

const (
	// operationAll runs the translation and then the extraction. The zero value
	// keeps the default behavior.
	operationAll operation = iota
	operationTranslate
	operationExtract
)

// parseOperation converts the -operation flag value.
func parseOperation(value string) (operation, error) {
	switch value {
	case "all":
		return operationAll, nil
	case "translate":
		return operationTranslate, nil
	case "extract":
		return operationExtract, nil
	}
	return operationAll, fmt.Errorf("-operation must be all, translate or extract, not %q", value)
}

func (op operation) translates() bool { return op == operationAll || op == operationTranslate }
func (op operation) extracts() bool   { return op == operationAll || op == operationExtract }

// options configures one example run.
type options struct {
	baseURL    string
	credential string
	tenantID   string
	outputLang string
	inputLang  string
	model      string
	operation  operation
	// translateKey and extractKey are the per-operation idempotency keys. An
	// empty key is replaced with a fresh UUID v7 for that run.
	translateKey string
	extractKey   string
	// timeout bounds each synchronous call separately; zero or less means no
	// client-side deadline.
	timeout time.Duration
}

func main() {
	output := flag.String("to", "ko", "BCP 47 output language of the translation")
	input := flag.String("from", "", "BCP 47 input language; omit to detect it")
	model := flag.String("model", "", "translation model from the operator allowlist; required for the translate and all operations")
	operationName := flag.String("operation", "all", "call to run: all (needs both capabilities), translate or extract")
	timeout := flag.Duration("timeout", defaultCallTimeout, "client deadline of each call; zero (default) sets none and leaves the limit to the server's execution timeout. A deadline below that timeout cancels the call, and its usage is still billed")
	translateKey := flag.String("translate-key", "", "idempotency key of the translation; reuse it to retry an uncertain run")
	extractKey := flag.String("extract-key", "", "idempotency key of the extraction; reuse it to retry an uncertain run")
	flag.Parse()
	o, e := optionsFromEnv(os.Getenv)
	if e != nil {
		fmt.Fprintln(os.Stderr, "DeliTools example failed:", e)
		os.Exit(1)
	}
	o.outputLang, o.inputLang, o.model = *output, *input, *model
	o.translateKey, o.extractKey, o.timeout = *translateKey, *extractKey, *timeout
	if o.operation, e = parseOperation(*operationName); e != nil {
		fmt.Fprintln(os.Stderr, "DeliTools example failed:", e)
		os.Exit(1)
	}
	if o.operation.translates() && o.model == "" {
		fmt.Fprintln(os.Stderr, "DeliTools example failed: -model is required to translate")
		os.Exit(1)
	}
	// Calls are synchronous and can take as long as the operator's execution
	// timeout, so the HTTP client has no short overall timeout. run applies
	// the -timeout deadline to each call separately.
	ctx := context.Background()
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

// requestIDHeader is the response header that carries the request ID of every
// DeliTools response. It equals ErrorDetail.request_id when a detail exists.
const requestIDHeader = "X-Delitools-Request-Id"

// describe reports the Connect code, the stable reason and the request ID of a
// DeliTools error, and never any response content. The request ID comes from
// the typed ErrorDetail and falls back to the response header, which is the
// only place it appears when the error has no detail.
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
	if requestID == "" {
		requestID = ce.Meta().Get(requestIDHeader)
	}
	return fmt.Sprintf("code=%s reason=%s request_id=%s", ce.Code(), reason, requestID)
}

// callContext derives the context of one call. Each paid call gets a fresh
// deadline, so time spent in one call never cancels the next one; usage that a
// canceled call already incurred is still billed.
func callContext(ctx context.Context, timeout time.Duration) (context.Context, context.CancelFunc) {
	if timeout <= 0 {
		return context.WithCancel(ctx)
	}
	return context.WithTimeout(ctx, timeout)
}

// keyOrNew returns the caller-supplied idempotency key, or a new UUID v7 when
// the caller supplied none.
func keyOrNew(key string) string {
	if key != "" {
		return key
	}
	return uuid.Must(uuid.NewV7()).String()
}

func run(ctx context.Context, client delitoolsv1connect.DeliToolsServiceClient, o options, out io.Writer) error {
	if o.operation.translates() {
		if e := translate(ctx, client, o, out); e != nil {
			return e
		}
	}
	if o.operation.extracts() {
		if e := extract(ctx, client, o, out); e != nil {
			return e
		}
	}
	// Usage and charges are not part of the responses; inspect them through the
	// billing interface.
	return nil
}

func translate(ctx context.Context, client delitoolsv1connect.DeliToolsServiceClient, o options, out io.Writer) error {
	// Translate. The idempotency key is optional. Reusing it for a retry of
	// the same input within 30 days prevents a second execution and charge, but
	// a repeated call returns an error such as ALREADY_PROCESSED; the result is
	// never replayed. Without a key every call is a new, billable execution.
	// The example always sends a key and prints it, so a run whose outcome is
	// uncertain (timeout, dropped connection) can be repeated with the same
	// key through -translate-key. A key is not a secret.
	translateKey := keyOrNew(o.translateKey)
	fmt.Fprintf(out, "Translation idempotency key: %s\n", translateKey)
	translateCtx, cancelTranslate := callContext(ctx, o.timeout)
	defer cancelTranslate()
	translated, e := client.TranslateTextArray(translateCtx, authorized(o.credential, &pb.TranslateTextArrayRequest{
		TenantId:       o.tenantID,
		OutputLanguage: o.outputLang,
		InputLanguage:  o.inputLang,
		Model:          o.model,
		Texts:          []string{"Good morning", "Where is the station?"},
		IdempotencyKey: translateKey,
	}))
	if e != nil {
		return fmt.Errorf("translate: %w", e)
	}
	// Results are application data. The example reports only content-free
	// metadata; a real consumer hands the result to its own code instead.
	fmt.Fprintf(out, "Translated %d texts (request %s)\n", len(translated.Msg.Texts), translated.Msg.RequestId)
	return nil
}

func extract(ctx context.Context, client delitoolsv1connect.DeliToolsServiceClient, o options, out io.Writer) error {
	// Extract data that the text states explicitly. The model is fixed by the
	// service. Information missing from the text returns EXTRACTION_IMPOSSIBLE.
	extractKey := keyOrNew(o.extractKey)
	fmt.Fprintf(out, "Extraction idempotency key: %s\n", extractKey)
	extractCtx, cancelExtract := callContext(ctx, o.timeout)
	defer cancelExtract()
	extracted, e := client.ExtractDataFromText(extractCtx, authorized(o.credential, &pb.ExtractDataFromTextRequest{
		TenantId:       o.tenantID,
		Text:           "Ada Lovelace lives in London.",
		JsonSchema:     extractionSchema,
		IdempotencyKey: extractKey,
	}))
	if e != nil {
		return fmt.Errorf("extract: %w", e)
	}
	fmt.Fprintf(out, "Extracted %d bytes of JSON data (request %s)\n", len(extracted.Msg.JsonData), extracted.Msg.RequestId)
	return nil
}
