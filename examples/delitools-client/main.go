// SPDX-License-Identifier: Apache-2.0
package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/delinoio/apps/protos/delitools/v1/delitoolsv1connect"
	"google.golang.org/protobuf/encoding/protojson"
)

const usage = "usage: delitools-client translate|extract request.json"

// main reads one ProtoJSON request, calls the DeliTools API with a Logto user
// token or a Delcore API key, and prints the result or the stable error reason.
func main() {
	if err := run(context.Background(), os.Args[1:], os.Getenv, os.Stdout, http.DefaultClient); err != nil {
		fmt.Fprintln(os.Stderr, describe(err))
		os.Exit(1)
	}
}

// describe reports the stable reason code and request ID of an API error
// without echoing request content or credentials.
func describe(err error) string {
	var connectErr *connect.Error
	if errors.As(err, &connectErr) {
		for _, detail := range connectErr.Details() {
			value, valueErr := detail.Value()
			if valueErr != nil {
				continue
			}
			if info, ok := value.(*pb.ErrorDetail); ok {
				return fmt.Sprintf("DeliTools request failed: %s (request %s)", info.GetReason(), info.GetRequestId())
			}
		}
		return fmt.Sprintf("DeliTools request failed: %s", connectErr.Code())
	}
	return "DeliTools request failed; check the arguments and environment"
}

func run(ctx context.Context, args []string, getenv func(string) string, out io.Writer, httpClient connect.HTTPClient) error {
	if len(args) != 2 {
		return errors.New(usage)
	}
	token := getenv("DELITOOLS_TOKEN")
	baseURL := getenv("DELITOOLS_URL")
	if token == "" || baseURL == "" {
		return errors.New("DELITOOLS_TOKEN and DELITOOLS_URL are required")
	}
	raw, err := os.ReadFile(args[1])
	if err != nil {
		return err
	}
	if httpClient == http.DefaultClient {
		httpClient = &http.Client{Timeout: 150 * time.Second}
	}
	client := delitoolsv1connect.NewDelitoolsServiceClient(httpClient, baseURL)
	switch args[0] {
	case "translate":
		input := &pb.TranslateRequest{}
		if err := protojson.Unmarshal(raw, input); err != nil {
			return err
		}
		request := connect.NewRequest(input)
		request.Header().Set("Authorization", "Bearer "+token)
		response, err := client.Translate(ctx, request)
		if err != nil {
			return err
		}
		for _, translation := range response.Msg.GetTranslations() {
			fmt.Fprintln(out, translation)
		}
		return nil
	case "extract":
		input := &pb.ExtractRequest{}
		if err := protojson.Unmarshal(raw, input); err != nil {
			return err
		}
		request := connect.NewRequest(input)
		request.Header().Set("Authorization", "Bearer "+token)
		response, err := client.Extract(ctx, request)
		if err != nil {
			return err
		}
		fmt.Fprintln(out, response.Msg.GetJson())
		return nil
	default:
		return errors.New(usage)
	}
}
