// SPDX-License-Identifier: Apache-2.0
package main

import (
	"connectrpc.com/connect"
	"context"
	"fmt"
	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"github.com/delinoio/apps/protos/gittracker/v1/gittrackerv1connect"
	"google.golang.org/protobuf/encoding/protojson"
	"net/http"
	"os"
	"time"
)

func main() {
	if e := run(); e != nil {
		fmt.Fprintln(os.Stderr, "Git Tracker request failed; check inputs and permissions")
		os.Exit(1)
	}
}
func run() error {
	if len(os.Args) != 2 {
		return fmt.Errorf("usage: git-tracker-client upload.json")
	}
	raw, e := os.ReadFile(os.Args[1])
	if e != nil {
		return e
	}
	input := &pb.UploadRequest{}
	if e = protojson.Unmarshal(raw, input); e != nil {
		return e
	}
	token := os.Getenv("GIT_TRACKER_TOKEN")
	if token == "" {
		return fmt.Errorf("GIT_TRACKER_TOKEN required")
	}
	client := gittrackerv1connect.NewGitTrackerServiceClient(&http.Client{Timeout: 30 * time.Second}, "https://git-tracker.api.delino.io")
	q := connect.NewRequest(input)
	q.Header().Set("Authorization", "Bearer "+token)
	ctx := context.Background()
	if _, e = client.Upload(ctx, q); e != nil {
		return e
	}
	complete := connect.NewRequest(&pb.CompleteRequest{Context: input.Context})
	complete.Header().Set("Authorization", "Bearer "+token)
	if _, e = client.Complete(ctx, complete); e != nil {
		return e
	}
	fmt.Println("Measurement upload and completion succeeded.")
	return nil
}
