// SPDX-License-Identifier: FSL-1.1-ALv2

// Package api implements the DevBase Connect service.
package api

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"reflect"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/protos/devbase/v1/devbasev1connect"
	"github.com/delinoio/apps/servers/devbase-api/internal/config"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
	"github.com/delinoio/apps/servers/devbase-api/internal/limit"
	"github.com/delinoio/apps/servers/devbase-api/internal/metrics"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
	"google.golang.org/protobuf/proto"
)

// Group classifies an operation for credential permissions and rate limiting.
type Group int

const (
	GroupProject Group = iota
	GroupKey
	GroupUserRead
	GroupUserWrite
)

func (g Group) management() bool { return g == GroupProject || g == GroupKey }

// Op describes one procedure.
type Op struct {
	Name     string
	Group    Group
	Mutation bool
	// Degraded operations remain available to login Owners and Admins while the
	// tenant is BLOCKED or DELETING.
	Degraded bool
}

// Target identifies the tenant or project a request addresses.
type Target struct{ TenantID, ProjectID string }

type Server struct {
	devbasev1connect.UnimplementedDevBaseServiceHandler
	Pool      *pgxpool.Pool
	Authority delcore.Authority
	Config    config.Limits
	Log       *slog.Logger
	Metrics   *metrics.Metrics
	Now       func() time.Time

	users      *limit.Limiter
	management *limit.Limiter
}

func New(pool *pgxpool.Pool, authority delcore.Authority, cfg config.Limits, log *slog.Logger, m *metrics.Metrics, now func() time.Time) *Server {
	if now == nil {
		now = time.Now
	}
	if log == nil {
		log = slog.Default()
	}
	s := &Server{Pool: pool, Authority: authority, Config: cfg, Log: log, Metrics: m, Now: now}
	s.users = limit.New(cfg.UserRate, cfg.UserBurst, now)
	s.management = limit.New(cfg.ManagementRate, cfg.ManagementBurst, now)
	return s
}

func (s *Server) Handler() (string, http.Handler) {
	return devbasev1connect.NewDevBaseServiceHandler(s, connect.WithReadMaxBytes(256<<10))
}

func (s *Server) now() time.Time { return s.Now().UTC().Truncate(time.Microsecond) }

func newID() string { return uuid.Must(uuid.NewV7()).String() }

// Querier is implemented by the pool and by transactions.
type Querier interface {
	Exec(ctx context.Context, sql string, args ...any) (pgconn.CommandTag, error)
	Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

// Call carries the verified request context into an operation.
type Call struct {
	Ctx       context.Context
	DB        Querier
	Now       time.Time
	Trace     string
	Actor     string
	TenantID  string
	ProjectID string
	Config    config.Limits

	replayUser string
	redact     func(proto.Message)
}

// Audit records one successful change in the caller's transaction.
func (c *Call) Audit(action, target string) error {
	_, e := c.DB.Exec(c.Ctx, "INSERT INTO audit_events(id,occurred_at,action,actor,target_id,trace_id) VALUES($1,$2,$3,$4,$5,$6)", newID(), c.Now, action, c.Actor, target, c.Trace)
	return e
}

// handle runs the common pipeline: credential, target, authorization, tenant
// state, rate limit, validation and execution.
func handle[Q any, R any](s *Server, ctx context.Context, r *connect.Request[Q], op Op, out *R, target func() (Target, error), validate func() error, run func(*Call) error) (*connect.Response[R], error) {
	trace := newID()
	start := time.Now()
	ctx, cancel := context.WithTimeout(ctx, s.Config.RequestTimeout)
	defer cancel()
	err := s.execute(ctx, r.Header(), trace, op, any(r.Msg).(proto.Message), any(out).(proto.Message), target, validate, run)
	code := "ok"
	if err != nil {
		code = connect.CodeOf(render(err, trace)).String()
	}
	s.observe(ctx, op, code, trace, start, err)
	if err != nil {
		return nil, render(err, trace)
	}
	reflect.ValueOf(out).Elem().FieldByName("Metadata").Set(reflect.ValueOf(&pb.Metadata{TraceId: trace}))
	resp := connect.NewResponse(out)
	resp.Header().Set("X-Devbase-Trace-Id", trace)
	return resp, nil
}

func (s *Server) observe(ctx context.Context, op Op, code, trace string, start time.Time, err error) {
	if s.Metrics != nil {
		s.Metrics.Requests.WithLabelValues(op.Name, code).Inc()
		s.Metrics.Duration.WithLabelValues(op.Name).Observe(time.Since(start).Seconds())
	}
	var f *failure
	if err != nil && !errors.As(err, &f) {
		var database *pgconn.PgError
		if errors.As(err, &database) {
			s.Log.ErrorContext(ctx, "database operation failed", "trace_id", trace, "procedure", op.Name, "sqlstate", database.Code)
		} else {
			s.Log.ErrorContext(ctx, "dependency failed", "trace_id", trace, "procedure", op.Name, "category", "dependency")
		}
	}
	s.Log.InfoContext(ctx, "request completed", "trace_id", trace, "procedure", op.Name, "code", code, "duration_ms", time.Since(start).Milliseconds())
}

func (s *Server) execute(ctx context.Context, header http.Header, trace string, op Op, msg, out proto.Message, target func() (Target, error), validate func() error, run func(*Call) error) error {
	cred, e := parseCredential(header)
	if e != nil {
		return e
	}
	t, e := target()
	if e != nil {
		return e
	}
	var key string
	if op.Mutation {
		key = msg.(interface{ GetIdempotencyKey() string }).GetIdempotencyKey()
	}
	who, e := s.authenticate(ctx, cred, op, t, key)
	if e != nil {
		return e
	}
	if e = s.admitRate(op, who); e != nil {
		return e
	}
	if e = requestSize(msg); e != nil {
		return e
	}
	if op.Mutation {
		if e = idempotencyKey(key); e != nil {
			return e
		}
	}
	if e = validate(); e != nil {
		return e
	}
	c := &Call{Ctx: ctx, Now: s.now(), Trace: trace, Actor: who.actor, TenantID: who.tenantID, ProjectID: who.projectID, Config: s.Config}
	if !op.Mutation {
		c.DB = s.Pool
		return run(c)
	}
	return s.mutate(ctx, c, op, who, msg, out, key, run)
}

func (s *Server) admitRate(op Op, who identity) error {
	limiter, id, kind := s.users, who.projectID, "user_api"
	if op.Group.management() {
		limiter, id, kind = s.management, who.tenantID, "management_api"
	}
	if ok, wait := limiter.Allow(id); !ok {
		if s.Metrics != nil {
			s.Metrics.LimitRejections.WithLabelValues(kind).Inc()
		}
		return &failure{code: connect.CodeResourceExhausted, reason: pb.ErrorReason_ERROR_REASON_RATE_LIMITED, text: "rate limit exceeded; retry later", retry: wait}
	}
	return nil
}
