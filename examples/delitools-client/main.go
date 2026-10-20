// SPDX-License-Identifier: Apache-2.0
package main

import (
	"connectrpc.com/connect"
	"context"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/delinoio/apps/protos/delitools/v1/delitoolsv1connect"
	"log/slog"
	"net/http"
	"os"
	"time"
)

func main() {
	if err := run(); err != nil {
		slog.Error("DeliTools example failed", "code", connect.CodeOf(err).String())
		os.Exit(1)
	}
}
func run() error {
	base := os.Getenv("DELITOOLS_URL")
	if base == "" {
		base = "http://127.0.0.1:46322"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	client := delitoolsv1connect.NewDeliToolsServiceClient(&http.Client{}, base)
	req := connect.NewRequest(&pb.TranslateTextArrayRequest{TenantId: os.Getenv("DELITOOLS_TENANT_ID"), Model: "openai/gpt-6-luna", OutputLanguage: "ko", Texts: []string{"Hello", "Good morning"}, IdempotencyKey: os.Getenv("DELITOOLS_IDEMPOTENCY_KEY")})
	req.Header().Set("Authorization", "Bearer "+os.Getenv("DELITOOLS_TOKEN"))
	response, err := client.TranslateTextArray(ctx, req)
	if err != nil {
		return err
	}
	slog.Info("translation completed", "request_id", response.Msg.RequestId, "items", len(response.Msg.Texts))
	return nil
}
