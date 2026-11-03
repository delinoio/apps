// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

// Pool is the connection pool handle that Store exposes. It satisfies DB and
// pgx.BeginFunc's beginner interface.
//
// With a positive timeout (ModeServe) each call is bounded by a context
// deadline that covers waiting for a free connection as well as running the
// statement, so a saturated pool or a blocked statement fails with
// context.DeadlineExceeded instead of waiting for the request context. The
// deadline of Query and QueryRow lasts until the rows are closed or scanned.
// Begin and Acquire bound only the wait for a connection; the statements of
// a transaction are bounded by the server's statement_timeout and
// lock_timeout, which Open sets in the same mode. A zero timeout
// (ModeMaintenance) passes every call through unchanged.
type Pool struct {
	raw     *pgxpool.Pool
	timeout time.Duration
}

var _ DB = (*Pool)(nil)

// bound derives the per-call context. The returned cancel is never nil.
func (p *Pool) bound(ctx context.Context) (context.Context, context.CancelFunc) {
	if p.timeout <= 0 {
		return ctx, func() {}
	}
	return context.WithTimeout(ctx, p.timeout)
}

// Exec runs a statement outside a transaction.
func (p *Pool) Exec(ctx context.Context, sql string, args ...any) (pgconn.CommandTag, error) {
	ctx, cancel := p.bound(ctx)
	defer cancel()
	return p.raw.Exec(ctx, sql, args...)
}

// Query runs a query outside a transaction. Close the rows to release the
// connection and the deadline.
func (p *Pool) Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error) {
	ctx, cancel := p.bound(ctx)
	rows, err := p.raw.Query(ctx, sql, args...)
	if err != nil {
		cancel()
		return nil, err
	}
	return &boundedRows{Rows: rows, cancel: cancel}, nil
}

// QueryRow runs a single-row query outside a transaction.
func (p *Pool) QueryRow(ctx context.Context, sql string, args ...any) pgx.Row {
	ctx, cancel := p.bound(ctx)
	return &boundedRow{Row: p.raw.QueryRow(ctx, sql, args...), cancel: cancel}
}

// Begin starts a transaction. Only the wait for a connection is bounded; the
// transaction itself lives as long as the caller's context.
func (p *Pool) Begin(ctx context.Context) (pgx.Tx, error) {
	acquireCtx, cancel := p.bound(ctx)
	defer cancel()
	return p.raw.Begin(acquireCtx)
}

// Acquire checks out a connection. Only the wait for a connection is bounded.
func (p *Pool) Acquire(ctx context.Context) (*pgxpool.Conn, error) {
	acquireCtx, cancel := p.bound(ctx)
	defer cancel()
	return p.raw.Acquire(acquireCtx)
}

type boundedRows struct {
	pgx.Rows
	cancel context.CancelFunc
}

func (r *boundedRows) Close() {
	r.Rows.Close()
	r.cancel()
}

type boundedRow struct {
	pgx.Row
	cancel context.CancelFunc
}

func (r *boundedRow) Scan(dest ...any) error {
	defer r.cancel()
	return r.Row.Scan(dest...)
}
