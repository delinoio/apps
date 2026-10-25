// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"log/slog"
	"strconv"
	"time"

	"connectrpc.com/connect"
	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"github.com/delinoio/apps/protos/lane/v1/lanev1connect"
	"github.com/google/uuid"
	"google.golang.org/protobuf/encoding/protojson"
)

// Store pins one existing space and checks its registry and format on each transaction.
type Store interface {
	SpaceID() string
	DeviceID() string
	Root() string
	WithRead(context.Context, func(*sql.Conn) error) error
	WithWrite(context.Context, func(*sql.Conn) error) error
}

var (
	ErrDisconnected      = errors.New("synchronization is disabled; reconnect explicitly")
	ErrLeaseHeld         = errors.New("another process owns synchronization for this space")
	ErrLeaseLost         = errors.New("synchronization ownership expired or changed")
	ErrAccountMismatch   = errors.New("credential does not belong to this account space")
	ErrStaleEpoch        = errors.New("server generation changed; reconnect explicitly before synchronization")
	ErrUnsupportedFormat = errors.New("unsupported synchronization format; update Lane")
	ErrInvalidResponse   = errors.New("invalid synchronization response")
)

type Options struct {
	AccountID      string
	TenantID       string
	LeaseDuration  time.Duration
	RequestTimeout time.Duration
	PollInterval   time.Duration
	BatchSize      int
	Logger         *slog.Logger
	// CheckEnabled must check the pinned registry identity, not the active UI selection.
	CheckEnabled func(context.Context) error
	// OnAccountDeleted runs after durable redaction and lease release, outside SQL transactions.
	OnAccountDeleted func(context.Context) error
}

type Worker struct {
	store   Store
	client  lanev1connect.SyncServiceClient
	options Options
	owner   string
}

func NewWorker(s Store, client lanev1connect.SyncServiceClient, o Options) (*Worker, error) {
	if s == nil || client == nil || o.AccountID == "" || o.TenantID == "" {
		return nil, ErrAccountMismatch
	}
	if o.LeaseDuration == 0 {
		o.LeaseDuration = time.Minute
	}
	if o.RequestTimeout == 0 {
		o.RequestTimeout = 20 * time.Second
	}
	if o.PollInterval == 0 {
		o.PollInterval = time.Second
	}
	if o.BatchSize == 0 {
		o.BatchSize = 100
	}
	if o.LeaseDuration <= 0 || o.RequestTimeout <= 0 || o.RequestTimeout >= o.LeaseDuration/2 || o.PollInterval <= 0 || o.BatchSize < 1 || o.BatchSize > 1000 {
		return nil, errors.New("invalid synchronization options")
	}
	if o.Logger == nil {
		o.Logger = slog.Default()
	}
	id, err := uuid.NewV7()
	if err != nil {
		return nil, err
	}
	return &Worker{store: s, client: client, options: o, owner: id.String()}, nil
}

func meta(ctx context.Context, c *sql.Conn, key string) (string, error) {
	var v string
	err := c.QueryRowContext(ctx, "SELECT value FROM store_meta WHERE key=?", key).Scan(&v)
	if errors.Is(err, sql.ErrNoRows) {
		return "", nil
	}
	return v, err
}
func putMeta(ctx context.Context, c *sql.Conn, key, value string) error {
	_, err := c.ExecContext(ctx, "INSERT INTO store_meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", key, value)
	return err
}
func (w *Worker) lease(ctx context.Context) error {
	if w.options.CheckEnabled != nil {
		if err := w.options.CheckEnabled(ctx); err != nil {
			return errors.Join(ErrDisconnected, err)
		}
	}
	return w.store.WithWrite(ctx, func(c *sql.Conn) error {
		removed, e := meta(ctx, c, "account_deleted")
		if e != nil {
			return e
		}
		if removed == "1" {
			return ErrAccountDeleted
		}
		owner, err := meta(ctx, c, "sync_owner")
		if err != nil {
			return err
		}
		expiry, err := meta(ctx, c, "sync_owner_expires_at_us")
		if err != nil {
			return err
		}
		until, _ := strconv.ParseInt(expiry, 10, 64)
		now := time.Now()
		if owner != "" && owner != w.owner && until > now.UnixMicro() {
			return ErrLeaseHeld
		}
		if err = putMeta(ctx, c, "sync_owner", w.owner); err != nil {
			return err
		}
		return putMeta(ctx, c, "sync_owner_expires_at_us", strconv.FormatInt(now.Add(w.options.LeaseDuration).UnixMicro(), 10))
	})
}
func (w *Worker) guard(ctx context.Context, c *sql.Conn) error {
	if w.options.CheckEnabled != nil {
		if e := w.options.CheckEnabled(ctx); e != nil {
			return errors.Join(ErrDisconnected, e)
		}
	}
	owner, err := meta(ctx, c, "sync_owner")
	if err != nil {
		return err
	}
	expiry, err := meta(ctx, c, "sync_owner_expires_at_us")
	if err != nil {
		return err
	}
	until, _ := strconv.ParseInt(expiry, 10, 64)
	if owner != w.owner || until <= time.Now().UnixMicro() {
		return ErrLeaseLost
	}
	return nil
}
func (w *Worker) release() error {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	return w.store.WithWrite(ctx, func(c *sql.Conn) error {
		owner, err := meta(ctx, c, "sync_owner")
		if err != nil {
			return err
		}
		if owner != w.owner {
			return nil
		}
		if err = putMeta(ctx, c, "sync_owner", ""); err != nil {
			return err
		}
		return putMeta(ctx, c, "sync_owner_expires_at_us", "0")
	})
}

// Run drains durable work. Once runs one bounded upload batch and all available pull pages.
// Authentication and network errors never remove pending mutations.
func (w *Worker) Run(ctx context.Context, once bool) (err error) {
	if err = w.lease(ctx); err != nil {
		if errors.Is(err, ErrAccountDeleted) && w.options.OnAccountDeleted != nil {
			return errors.Join(err, w.options.OnAccountDeleted(ctx))
		}
		return err
	}
	defer func() {
		err = errors.Join(err, w.release())
		if errors.Is(err, ErrAccountDeleted) && w.options.OnAccountDeleted != nil {
			err = errors.Join(err, w.options.OnAccountDeleted(ctx))
		}
	}()
	watchCtx, stopWatch := context.WithCancel(ctx)
	watching := false
	watchDone := make(chan struct{})
	defer func() {
		stopWatch()
		if watching {
			<-watchDone
		}
	}()
	updates := make(chan struct{}, 1)
	delay := w.options.PollInterval
	for {
		err = w.cycle(ctx)
		if err != nil && ctx.Err() == nil && !errors.Is(err, ErrAccountDeleted) {
			err = errors.Join(err, w.recordFailure(ctx, err))
		}
		if once || ctx.Err() != nil {
			return err
		}
		if err != nil {
			if errors.Is(err, ErrReconnectRequired) || errors.Is(err, ErrAccountDeleted) || errors.Is(err, ErrDisconnected) || errors.Is(err, ErrLeaseLost) || errors.Is(err, ErrLeaseHeld) || errors.Is(err, ErrStaleEpoch) || errors.Is(err, ErrUnsupportedFormat) || errors.Is(err, ErrAccountMismatch) || connect.CodeOf(err) == connect.CodeUnauthenticated || connect.CodeOf(err) == connect.CodePermissionDenied {
				return err
			}
			w.options.Logger.Warn("lane sync attempt failed", "code", connect.CodeOf(err).String())
			delay = min(delay*2, 30*time.Second)
		} else {
			delay = w.options.PollInterval
			if !watching {
				watching = true
				go func() { defer close(watchDone); w.watch(watchCtx, updates) }()
			}
		}
		timer := time.NewTimer(delay)
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-updates:
			timer.Stop()
		case <-timer.C:
		}
	}
}
func (w *Worker) cycle(ctx context.Context) error {
	if e := w.store.WithRead(ctx, func(c *sql.Conn) error {
		required, e := meta(ctx, c, "reconnect_required")
		if e != nil {
			return e
		}
		if required == "1" {
			return ErrReconnectRequired
		}
		return nil
	}); e != nil {
		return e
	}
	if err := w.lease(ctx); err != nil {
		return err
	}
	call, cancel := context.WithTimeout(ctx, w.options.RequestTimeout)
	info, err := w.client.GetSyncInfo(call, connect.NewRequest(&lanev1.GetSyncInfoRequest{}))
	cancel()
	if err != nil {
		return err
	}
	if info.Msg.FormatVersion != 1 {
		return ErrUnsupportedFormat
	}
	if info.Msg.AccountId != w.options.AccountID || info.Msg.TenantId != w.options.TenantID {
		return ErrAccountMismatch
	}
	if info.Msg.Epoch == "" {
		return ErrInvalidResponse
	}
	epoch := info.Msg.Epoch
	err = w.store.WithWrite(ctx, func(c *sql.Conn) error {
		if err := w.guard(ctx, c); err != nil {
			return err
		}
		previous, err := meta(ctx, c, "account_epoch")
		if err != nil {
			return err
		}
		if previous != "" && previous != epoch {
			if e := putMeta(ctx, c, "reconnect_required", "1"); e != nil {
				return e
			}
			if e := putMeta(ctx, c, "sync_error_reason", lanev1.ErrorReason_ERROR_REASON_STALE_EPOCH.String()); e != nil {
				return e
			}
			return nil
		}
		return putMeta(ctx, c, "account_epoch", epoch)
	})
	if err != nil {
		return err
	}
	var required string
	if e := w.store.WithRead(ctx, func(c *sql.Conn) error { var e error; required, e = meta(ctx, c, "reconnect_required"); return e }); e != nil {
		return e
	}
	if required == "1" {
		return ErrStaleEpoch
	}
	if err = w.push(ctx, epoch); err != nil {
		return err
	}
	if err = w.pull(ctx, epoch); err != nil {
		return err
	}
	return w.store.WithWrite(ctx, func(c *sql.Conn) error {
		if err := w.guard(ctx, c); err != nil {
			return err
		}
		if e := putMeta(ctx, c, "sync_error_reason", ""); e != nil {
			return e
		}
		if err := putMeta(ctx, c, "last_sync_at_us", strconv.FormatInt(time.Now().UnixMicro(), 10)); err != nil {
			return err
		}
		_, err := c.ExecContext(ctx, "INSERT INTO changes(change_kind) VALUES(4)")
		return err
	})
}
func (w *Worker) push(ctx context.Context, epoch string) error {
	var items []*lanev1.SyncMutation
	err := w.store.WithRead(ctx, func(c *sql.Conn) error {
		rows, err := c.QueryContext(ctx, "SELECT request_id,request_json FROM outbox WHERE state=1 ORDER BY created_at_us,rowid LIMIT ?", w.options.BatchSize)
		if err != nil {
			return err
		}
		defer rows.Close()
		for rows.Next() {
			var id, body string
			if err = rows.Scan(&id, &body); err != nil {
				return err
			}
			m := new(lanev1.SyncMutation)
			if err = protojson.Unmarshal([]byte(body), m); err != nil {
				return ErrInvalidResponse
			}
			if m.GetMeta().GetRequestId() != id {
				return ErrInvalidResponse
			}
			items = append(items, m)
		}
		return rows.Err()
	})
	if err != nil || len(items) == 0 {
		return err
	}
	if err = w.lease(ctx); err != nil {
		return err
	}
	call, cancel := context.WithTimeout(ctx, w.options.RequestTimeout)
	response, err := w.client.PushChanges(call, connect.NewRequest(&lanev1.PushChangesRequest{FormatVersion: 1, Epoch: epoch, DeviceId: w.store.DeviceID(), Mutations: items}))
	cancel()
	if err != nil {
		return err
	}
	if len(response.Msg.Results) != len(items) {
		return ErrInvalidResponse
	}
	for i, r := range response.Msg.Results {
		if r.GetRequestId() != items[i].GetMeta().GetRequestId() || (r.Result == nil) == (r.Error == nil) {
			return ErrInvalidResponse
		}
	}
	return w.store.WithWrite(ctx, func(c *sql.Conn) error {
		if err := w.guard(ctx, c); err != nil {
			return err
		}
		if err := applyMappings(ctx, c, response.Msg.RepositoryMappings); err != nil {
			return err
		}
		for i, r := range response.Msg.Results {
			if r.Error != nil {
				if r.Error.Reason == lanev1.ErrorReason_ERROR_REASON_STALE_EPOCH {
					return ErrStaleEpoch
				}
				if r.Error.Reason == lanev1.ErrorReason_ERROR_REASON_UNSUPPORTED_FORMAT {
					return ErrUnsupportedFormat
				}
				if r.Error.Reason == lanev1.ErrorReason_ERROR_REASON_STALE_RESOLUTION {
					if _, e := c.ExecContext(ctx, "DELETE FROM heads WHERE entity_id=? AND version_id=?", items[i].GetVersion().GetEntityId(), items[i].GetVersion().GetVersionId()); e != nil {
						return e
					}
				}
				state := 2
				if r.Error.Retryable {
					state = 1
				}
				if _, err := c.ExecContext(ctx, "UPDATE outbox SET state=?,error_reason=? WHERE request_id=?", state, r.Error.Reason.String(), r.RequestId); err != nil {
					return err
				}
				continue
			}
			if err := ApplySyncResult(ctx, c, r.Result); err != nil {
				return err
			}
			if _, err := c.ExecContext(ctx, "DELETE FROM outbox WHERE request_id=?", r.RequestId); err != nil {
				return err
			}
		}
		_, err := c.ExecContext(ctx, "INSERT INTO changes(change_kind) VALUES(4)")
		return err
	})
}
func (w *Worker) pull(ctx context.Context, epoch string) error {
	for {
		var cursor string
		if err := w.store.WithRead(ctx, func(c *sql.Conn) error { var e error; cursor, e = meta(ctx, c, "pull_cursor"); return e }); err != nil {
			return err
		}
		if err := w.lease(ctx); err != nil {
			return err
		}
		call, cancel := context.WithTimeout(ctx, w.options.RequestTimeout)
		response, err := w.client.PullChanges(call, connect.NewRequest(&lanev1.PullChangesRequest{FormatVersion: 1, Epoch: epoch, Cursor: cursor, PageSize: uint32(w.options.BatchSize)}))
		cancel()
		if err != nil {
			return err
		}
		if response.Msg.HasMore && response.Msg.NextCursor == cursor {
			return ErrInvalidResponse
		}
		err = w.store.WithWrite(ctx, func(c *sql.Conn) error {
			if err := w.guard(ctx, c); err != nil {
				return err
			}
			for _, change := range response.Msg.Changes {
				if d := change.GetDeletion(); d != nil && d.Kind == lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT && d.Id != w.options.AccountID {
					return ErrAccountMismatch
				}
				if err := applyChange(ctx, c, change); err != nil {
					return err
				}
			}
			return putMeta(ctx, c, "pull_cursor", response.Msg.NextCursor)
		})
		if err != nil {
			return err
		}
		for _, change := range response.Msg.Changes {
			if d := change.GetDeletion(); d != nil && d.Kind == lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT {
				return ErrAccountDeleted
			}
		}
		if !response.Msg.HasMore {
			return nil
		}
	}
}

// RetryBlocked explicitly retries rejected items after the cause has been corrected.
func RetryBlocked(ctx context.Context, s Store) error {
	return s.WithWrite(ctx, func(c *sql.Conn) error {
		_, err := c.ExecContext(ctx, "UPDATE outbox SET state=1,error_reason=NULL WHERE state=2")
		return err
	})
}

func invalid(message string) error { return fmt.Errorf("%w: %s", ErrInvalidResponse, message) }

// Watch notifications are hints. Polling still discovers local writes and repairs
// missed notifications; only PullChanges may advance the durable cursor.
func (w *Worker) watch(ctx context.Context, updates chan<- struct{}) {
	for ctx.Err() == nil {
		var epoch, cursor string
		err := w.store.WithRead(ctx, func(c *sql.Conn) error {
			var e error
			epoch, e = meta(ctx, c, "account_epoch")
			if e != nil {
				return e
			}
			cursor, e = meta(ctx, c, "pull_cursor")
			return e
		})
		if err != nil {
			return
		}
		call, cancel := context.WithTimeout(ctx, 30*time.Second)
		stream, e := w.client.WatchChanges(call, connect.NewRequest(&lanev1.WatchChangesRequest{Epoch: epoch, Cursor: cursor}))
		if e == nil {
			for stream.Receive() {
				select {
				case updates <- struct{}{}:
				default:
				}
			}
			e = stream.Err()
			stream.Close()
		}
		cancel()
		if connect.CodeOf(e) == connect.CodeUnimplemented {
			w.options.Logger.Warn("lane live invalidations unavailable; polling remains active")
			return
		}
		if ctx.Err() != nil {
			return
		}
		timer := time.NewTimer(w.options.PollInterval)
		select {
		case <-ctx.Done():
			timer.Stop()
			return
		case <-timer.C:
		}
	}
}

func (w *Worker) recordFailure(ctx context.Context, cause error) error {
	reason := lanev1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE
	switch {
	case errors.Is(cause, ErrStaleEpoch) || errors.Is(cause, ErrReconnectRequired):
		reason = lanev1.ErrorReason_ERROR_REASON_STALE_EPOCH
	case errors.Is(cause, ErrUnsupportedFormat):
		reason = lanev1.ErrorReason_ERROR_REASON_UNSUPPORTED_FORMAT
	case errors.Is(cause, ErrAccountMismatch) || errors.Is(cause, ErrDisconnected):
		reason = lanev1.ErrorReason_ERROR_REASON_FORBIDDEN
	case connect.CodeOf(cause) == connect.CodeUnauthenticated:
		reason = lanev1.ErrorReason_ERROR_REASON_UNAUTHENTICATED
	case connect.CodeOf(cause) == connect.CodePermissionDenied:
		reason = lanev1.ErrorReason_ERROR_REASON_FORBIDDEN
	case errors.Is(cause, ErrLeaseLost) || errors.Is(cause, ErrLeaseHeld):
		return nil
	}
	return w.store.WithWrite(ctx, func(c *sql.Conn) error {
		if e := w.guard(ctx, c); e != nil {
			return e
		}
		if reason == lanev1.ErrorReason_ERROR_REASON_STALE_EPOCH {
			if e := putMeta(ctx, c, "reconnect_required", "1"); e != nil {
				return e
			}
		}
		if e := putMeta(ctx, c, "sync_error_reason", reason.String()); e != nil {
			return e
		}
		_, e := c.ExecContext(ctx, "INSERT INTO changes(change_kind) VALUES(4)")
		return e
	})
}
