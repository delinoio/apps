// SPDX-License-Identifier: Apache-2.0

// Command devbase-client demonstrates the DevBase end-user directory without an SDK package.
//
// A tenant Owner or Admin logs in and creates a project and a CRUD API key. The
// application then registers, looks up, updates, lists and deletes an end user with that
// key. The program removes everything it created before it exits.
//
// Environment:
//
//	DEVBASE_URL        API base URL, default http://127.0.0.1:46317
//	DEVBASE_TOKEN      Delino login token of a tenant Owner or Admin
//	DEVBASE_TENANT_ID  tenant that owns the example project
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
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/protos/devbase/v1/devbasev1connect"
	"github.com/google/uuid"
	"google.golang.org/protobuf/types/known/fieldmaskpb"
	"google.golang.org/protobuf/types/known/structpb"
)

func main() {
	if e := run(context.Background(), os.Getenv, os.Stdout); e != nil {
		fmt.Fprintln(os.Stderr, "DevBase example failed:", e)
		os.Exit(1)
	}
}

// authorized attaches a bearer token. Mutations also need an idempotency key, which lets
// a client repeat a request safely when a response is lost.
func authorized[T any](token string, message *T) *connect.Request[T] {
	r := connect.NewRequest(message)
	r.Header().Set("Authorization", "Bearer "+token)
	return r
}

func idempotencyKey() string { return uuid.NewString() }

func run(ctx context.Context, env func(string) string, out io.Writer) error {
	base, token, tenant := env("DEVBASE_URL"), env("DEVBASE_TOKEN"), env("DEVBASE_TENANT_ID")
	if base == "" {
		base = "http://127.0.0.1:46317"
	}
	if token == "" || tenant == "" {
		return errors.New("DEVBASE_TOKEN and DEVBASE_TENANT_ID are required")
	}
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	client := devbasev1connect.NewDevBaseServiceClient(&http.Client{Timeout: 15 * time.Second}, base)

	// 1. Developer: create a project and a key with a login token.
	created, e := client.CreateProject(ctx, authorized(token, &pb.CreateProjectRequest{IdempotencyKey: idempotencyKey(), TenantId: tenant, Name: "example-" + uuid.NewString()[:8]}))
	if e != nil {
		return fmt.Errorf("create project: %w", e)
	}
	project := created.Msg.Project
	fmt.Fprintf(out, "Created project %s (version %d)\n", project.Id, project.Version)
	defer func() {
		// Deleting the project removes its users and keys.
		if _, e := client.DeleteProject(context.WithoutCancel(ctx), authorized(token, &pb.DeleteProjectRequest{IdempotencyKey: idempotencyKey(), ProjectId: project.Id, Version: project.Version})); e != nil {
			fmt.Fprintln(out, "Cleanup failed; delete the example project manually:", project.Id)
		}
	}()
	issued, e := client.IssueApiKey(ctx, authorized(token, &pb.IssueApiKeyRequest{IdempotencyKey: idempotencyKey(), ProjectId: project.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD}))
	if e != nil {
		return fmt.Errorf("issue key: %w", e)
	}
	// The secret is shown once. Store it in your application's secret store; never print it.
	secret := issued.Msg.Secret
	fmt.Fprintf(out, "Issued CRUD key %s\n", issued.Msg.ApiKey.Id)

	// 2. Application: manage end users with the project key.
	metadata, e := structpb.NewStruct(map[string]any{"plan": "pro"})
	if e != nil {
		return e
	}
	user, e := client.CreateEndUser(ctx, authorized(secret, &pb.CreateEndUserRequest{IdempotencyKey: idempotencyKey(), ProjectId: project.Id, ExternalId: "user-1001", Name: "Ada", Email: "ada@example.com", Metadata: metadata}))
	if e != nil {
		return fmt.Errorf("create end user: %w", e)
	}
	fmt.Fprintf(out, "Registered end user %s for external ID user-1001\n", user.Msg.EndUser.Id)

	// Pass the stable internal ID to other services. Look users up by either identifier.
	byExternal, e := client.LookupEndUser(ctx, authorized(secret, &pb.LookupEndUserRequest{ProjectId: project.Id, ExternalId: "user-1001"}))
	if e != nil {
		return fmt.Errorf("lookup end user: %w", e)
	}
	if byExternal.Msg.EndUser.Id != user.Msg.EndUser.Id {
		return errors.New("lookup returned a different user")
	}

	// Update with a field mask and the current version. A stale version fails with "aborted".
	updated, e := client.UpdateEndUser(ctx, authorized(secret, &pb.UpdateEndUserRequest{
		IdempotencyKey: idempotencyKey(), ProjectId: project.Id, EndUserId: user.Msg.EndUser.Id,
		Name: "Ada Lovelace", UpdateMask: &fieldmaskpb.FieldMask{Paths: []string{"name"}}, Version: user.Msg.EndUser.Version,
	}))
	if e != nil {
		return fmt.Errorf("update end user: %w", e)
	}
	fmt.Fprintf(out, "Updated end user to version %d\n", updated.Msg.EndUser.Version)

	// Lists use cursors: repeat with next_page_token until it is empty.
	total, token2 := 0, ""
	for {
		page, e := client.ListEndUsers(ctx, authorized(secret, &pb.ListEndUsersRequest{ProjectId: project.Id, PageSize: 50, PageToken: token2}))
		if e != nil {
			return fmt.Errorf("list end users: %w", e)
		}
		total += len(page.Msg.EndUsers)
		if token2 = page.Msg.NextPageToken; token2 == "" {
			break
		}
	}
	fmt.Fprintf(out, "Listed %d end user(s)\n", total)

	if _, e = client.DeleteEndUser(ctx, authorized(secret, &pb.DeleteEndUserRequest{IdempotencyKey: idempotencyKey(), ProjectId: project.Id, EndUserId: user.Msg.EndUser.Id, Version: updated.Msg.EndUser.Version})); e != nil {
		return fmt.Errorf("delete end user: %w", e)
	}
	fmt.Fprintln(out, "Deleted the end user")

	// 3. Developer: rotate by revoking the key. Revocation takes effect immediately.
	if _, e = client.RevokeApiKey(ctx, authorized(token, &pb.RevokeApiKeyRequest{IdempotencyKey: idempotencyKey(), ProjectId: project.Id, ApiKeyId: issued.Msg.ApiKey.Id})); e != nil {
		return fmt.Errorf("revoke key: %w", e)
	}
	if _, e = client.ListEndUsers(ctx, authorized(secret, &pb.ListEndUsersRequest{ProjectId: project.Id})); connect.CodeOf(e) != connect.CodeUnauthenticated {
		return errors.New("a revoked key was still accepted")
	}
	fmt.Fprintln(out, "Revoked the key; it is rejected as unauthenticated")
	return nil
}
