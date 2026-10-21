// SPDX-License-Identifier: Apache-2.0

// Command delcore-client demonstrates a Delcore consumer without an SDK
// package: optional tenant discovery with a user token, then the metering flow
// of a registered service with its M2M token. The service authorizes a feature
// for one group before execution, reports the execution as an event, corrects
// it, and reads the asynchronously updated aggregates and processing status.
//
// The metering flow matches the sample development catalog: the example-api
// service, its requests meter grouped by region, and its monthly-requests
// metered feature.
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
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/google/uuid"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// options configures one example run. Tokens come from the environment and
// are never printed.
type options struct {
	accessToken  string
	serviceToken string
	serviceID    string
	tenantID     string
	userID       string
	featureKey   string
	meterKey     string
	region       string
	attempts     int
	retryDelay   time.Duration
}

func main() {
	endpoint := flag.String("endpoint", "http://127.0.0.1:8080", "delcore API endpoint")
	service := flag.String("service", "", "registered ID of the calling service; required for the metering flow")
	tenant := flag.String("tenant", "", "tenant ID; defaults to the first tenant discovered with DELCORE_ACCESS_TOKEN")
	user := flag.String("user", "", "user ID to authorize; defaults to the user of DELCORE_ACCESS_TOKEN")
	feature := flag.String("feature", "monthly-requests", "metered feature key declared by the calling service")
	meter := flag.String("meter", "requests", "meter key to read aggregates from")
	region := flag.String("region", "us-east", "region attribute and feature group value")
	flag.Parse()
	o := options{
		accessToken:  os.Getenv("DELCORE_ACCESS_TOKEN"),
		serviceToken: os.Getenv("DELCORE_SERVICE_TOKEN"),
		serviceID:    *service,
		tenantID:     *tenant,
		userID:       *user,
		featureKey:   *feature,
		meterKey:     *meter,
		region:       *region,
		attempts:     3,
		retryDelay:   time.Second,
	}
	if o.accessToken == "" && o.serviceToken == "" {
		fmt.Fprintln(os.Stderr, "DELCORE_ACCESS_TOKEN or DELCORE_SERVICE_TOKEN is required")
		os.Exit(1)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	client := delcorev1connect.NewDelcoreServiceClient(&http.Client{Timeout: 15 * time.Second}, *endpoint)
	if e := run(ctx, client, o, os.Stdout); e != nil {
		// Print only the typed reason and code, never response content.
		fmt.Fprintf(os.Stderr, "delcore example failed: %s %s\n", connect.CodeOf(e), reason(e))
		os.Exit(1)
	}
}

func authorized[T any](message *T, token string) *connect.Request[T] {
	r := connect.NewRequest(message)
	r.Header().Set("Authorization", "Bearer "+token)
	return r
}

// newKey returns a UUID v7. Delcore request keys must be UUID v7; event keys
// only need to be stable for one measured execution.
func newKey() string { return uuid.Must(uuid.NewV7()).String() }

// reason extracts the typed Delcore error reason.
func reason(err error) pb.ErrorReason {
	var ce *connect.Error
	if !errors.As(err, &ce) {
		return pb.ErrorReason_ERROR_REASON_UNSPECIFIED
	}
	for _, detail := range ce.Details() {
		if value, e := detail.Value(); e == nil {
			if typed, ok := value.(*pb.ErrorDetail); ok {
				return typed.Reason
			}
		}
	}
	return pb.ErrorReason_ERROR_REASON_UNSPECIFIED
}

// retry repeats a call with the same request and event keys after transport
// or availability failures. Repeating the same keys is safe: Delcore replays
// the original result without repeated effects.
func retry[T any](ctx context.Context, o options, call func() (T, error)) (T, error) {
	var result T
	var err error
	for attempt := 1; attempt <= max(o.attempts, 1); attempt++ {
		result, err = call()
		code := connect.CodeOf(err)
		if err == nil || code != connect.CodeUnavailable && code != connect.CodeDeadlineExceeded && code != connect.CodeUnknown {
			return result, err
		}
		select {
		case <-ctx.Done():
			return result, ctx.Err()
		case <-time.After(o.retryDelay):
		}
	}
	return result, err
}

func run(ctx context.Context, client delcorev1connect.DelcoreServiceClient, o options, out io.Writer) error {
	if o.accessToken != "" {
		me, e := client.GetCurrentUser(ctx, authorized(&pb.GetCurrentUserRequest{}, o.accessToken))
		if e != nil {
			return e
		}
		fmt.Fprintf(out, "Discovered %d tenants; correlation %s\n", len(me.Msg.Tenants), me.Msg.Metadata.GetCorrelationId())
		if o.tenantID == "" && len(me.Msg.Tenants) > 0 {
			o.tenantID = me.Msg.Tenants[0].Id
		}
		if o.userID == "" {
			o.userID = me.Msg.UserId
		}
	}
	if o.serviceToken == "" {
		fmt.Fprintln(out, "Set DELCORE_SERVICE_TOKEN to run the service metering flow.")
		return nil
	}
	if o.serviceID == "" || o.tenantID == "" || o.userID == "" {
		return errors.New("the metering flow requires -service and a tenant and user from -tenant and -user or DELCORE_ACCESS_TOKEN")
	}
	groups := map[string]*pb.AttributeValue{"region": {Value: &pb.AttributeValue_Text{Text: o.region}}}

	// 1. Preflight authorization. Permission and billing checks always apply;
	// the feature result adds the pinned entitlement and the last applied usage
	// of this region group. Treat an error as a denial of new paid execution.
	decision, e := client.Authorize(ctx, authorized(&pb.AuthorizeRequest{TenantId: o.tenantID, UserId: o.userID, Capability: "execute", Paid: true, FeatureKey: o.featureKey, GroupValues: groups}, o.serviceToken))
	if e != nil {
		return e
	}
	feature := decision.Msg.Feature
	fmt.Fprintf(out, "Authorize allowed=%t reasons=%v entitled=%t usage=%s limit=%s\n", decision.Msg.Allowed, decision.Msg.Reasons, feature.GetEntitled(), feature.GetUsage(), feature.GetLimit())
	if !decision.Msg.Allowed {
		fmt.Fprintln(out, "Execution blocked; no event is reported.")
		return nil
	}

	// 2. Execute, then report the execution once. Keep the event key with the
	// pending report and reuse it, and the request key, for every retry.
	eventKey, requestKey := "example-"+newKey(), newKey()
	// The service ID must name the calling service; Delcore rejects another
	// service's ID.
	event := &pb.Event{TenantId: o.tenantID, ServiceId: o.serviceID, EventKey: eventKey, EventType: "request.completed", OccurredAt: timestamppb.Now(), Attributes: map[string]*pb.AttributeValue{
		"outcome": {Value: &pb.AttributeValue_Text{Text: "success"}},
		"region":  groups["region"],
		"user":    {Value: &pb.AttributeValue_Text{Text: o.userID}},
		"status":  {Value: &pb.AttributeValue_Number{Number: "200"}},
	}}
	accepted, e := retry(ctx, o, func() (*connect.Response[pb.IngestEventResponse], error) {
		return client.IngestEvent(ctx, authorized(&pb.IngestEventRequest{RequestKey: requestKey, Event: event}, o.serviceToken))
	})
	if e != nil {
		return e
	}
	// Acknowledgement means durable acceptance only. Aggregates, rating, and
	// feature usage update asynchronously, so an immediate read can still
	// return the previous value. This is not read-after-write.
	fmt.Fprintf(out, "IngestEvent accepted revision %d; aggregates update asynchronously\n", accepted.Msg.Revision)

	// 3. Correct the event by replacing its complete content. The expected
	// revision prevents silent overwrites; after a stale revision, re-read the
	// latest revision with GetEvent and decide again.
	replacement := map[string]*pb.AttributeValue{}
	for name, value := range event.Attributes {
		replacement[name] = value
	}
	replacement["outcome"] = &pb.AttributeValue{Value: &pb.AttributeValue_Text{Text: "cached"}}
	expected := accepted.Msg.Revision
	var corrected *connect.Response[pb.CorrectEventResponse]
	for range 2 {
		correctionKey := newKey()
		corrected, e = retry(ctx, o, func() (*connect.Response[pb.CorrectEventResponse], error) {
			return client.CorrectEvent(ctx, authorized(&pb.CorrectEventRequest{RequestKey: correctionKey, TenantId: o.tenantID, EventKey: eventKey, ExpectedRevision: expected, Kind: pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, EventType: event.EventType, Attributes: replacement}, o.serviceToken))
		})
		if reason(e) != pb.ErrorReason_ERROR_REASON_STALE_REVISION {
			break
		}
		latest, err := client.GetEvent(ctx, authorized(&pb.GetEventRequest{TenantId: o.tenantID, EventKey: eventKey}, o.serviceToken))
		if err != nil {
			return err
		}
		fmt.Fprintf(out, "Stale revision %d; latest is %d\n", expected, latest.Msg.Revision)
		expected = latest.Msg.Revision
	}
	if e != nil {
		return e
	}
	fmt.Fprintf(out, "CorrectEvent accepted revision %d\n", corrected.Msg.Revision)

	// 4. Read billing-period aggregates and processing status. The new revision
	// may not be applied yet; compare the status with the aggregates instead of
	// assuming the event is included.
	aggregates, e := client.ListMeterAggregates(ctx, authorized(&pb.ListMeterAggregatesRequest{TenantId: o.tenantID, MeterKey: o.meterKey, Page: &pb.Page{Size: 10}}, o.serviceToken))
	if e != nil {
		return e
	}
	fmt.Fprintf(out, "ListMeterAggregates returned %d groups in the current billing period\n", len(aggregates.Msg.Aggregates))
	status, e := client.GetMeteringStatus(ctx, authorized(&pb.GetMeteringStatusRequest{TenantId: o.tenantID}, o.serviceToken))
	if e != nil {
		return e
	}
	fmt.Fprintf(out, "GetMeteringStatus unapplied=%d pending=%d lag=%ds failed=%d\n", status.Msg.UnappliedEvents, status.Msg.PendingJobs, status.Msg.LagSeconds, status.Msg.FailedJobs)
	if status.Msg.UnappliedEvents > 0 {
		fmt.Fprintln(out, "Some accepted events are not applied yet; authorization uses the last applied aggregates until processing catches up.")
	}
	return nil
}
