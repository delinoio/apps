// SPDX-License-Identifier: Apache-2.0
package store

import (
	"context"
	"database/sql"
	_ "embed"
	"errors"

	"github.com/google/uuid"
	_ "modernc.org/sqlite"
	"net/url"
	"os"
	"path/filepath"
	"sync"
	"time"
)

//go:embed schema.sql
var schema string

//go:embed registry.sql
var registrySchema string

const applicationID = 1279348293

type Store struct {
	db, registry        *sql.DB
	root, space, device string
	lifeMu              sync.RWMutex
	handles             *os.File
	physicalClosed      bool
}

func (s *Store) SpaceID() string  { return s.space }
func (s *Store) DeviceID() string { return s.device }
func (s *Store) Root() string     { return s.root }
func (s *Store) Close() error {
	s.lifeMu.Lock()
	defer s.lifeMu.Unlock()
	return errors.Join(s.closePhysical(), s.registry.Close())
}
func NewID() string { return uuid.Must(uuid.NewV7()).String() }
func validID(s string) bool {
	u, e := uuid.Parse(s)
	return e == nil && u.Version() == 7 && u.String() == s
}

func openDB(path string, storeDatabase bool) (*sql.DB, error) {
	f, e := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0600)
	if e != nil {
		return nil, e
	}
	f.Close()
	u := url.URL{Scheme: "file", Path: path}
	q := u.Query()
	for _, p := range []string{"foreign_keys(1)", "busy_timeout(5000)", "synchronous(FULL)", "secure_delete(1)"} {
		q.Add("_pragma", p)
	}
	u.RawQuery = q.Encode()
	db, e := sql.Open("sqlite", u.String())
	if e != nil {
		return nil, e
	}
	db.SetMaxOpenConns(4)

	// Read all compatibility fields in one snapshot. Concurrent first-open writers
	// must not expose a mixture of the empty and initialized database identities.
	e = transaction(context.Background(), db, false, func(c *sql.Conn) error {
		var app, version, tables int
		if e := c.QueryRowContext(context.Background(), "PRAGMA application_id").Scan(&app); e != nil {
			return domainError(8, "Invalid database format", "")
		}
		if e := c.QueryRowContext(context.Background(), "PRAGMA user_version").Scan(&version); e != nil {
			return e
		}
		if e := c.QueryRowContext(context.Background(), "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'").Scan(&tables); e != nil {
			return e
		}
		if !(app == applicationID && version == 1) && !(app == 0 && version == 0 && tables == 0) {
			return domainError(8, "Update Lane to open this database", "")
		}
		if storeDatabase && tables > 0 {
			var format string
			if e := c.QueryRowContext(context.Background(), "SELECT value FROM store_meta WHERE key='format_version'").Scan(&format); e != nil || format != "1" {
				return domainError(8, "Update Lane to open this store format", "")
			}
		}
		return nil
	})
	if e != nil {
		db.Close()
		return nil, e
	}

	if e = protectPath(path, false); e != nil {
		db.Close()
		return nil, e
	}

	// Concurrent first opens can race while converting DELETE to WAL. SQLite can
	// reject that lock upgrade immediately despite busy_timeout. No domain
	// transaction has started: reread the mode and retry only this idempotent
	// initialization step within the same five-second budget.
	deadline := time.Now().Add(5 * time.Second)
	for {
		var mode string
		e = db.QueryRow("PRAGMA journal_mode").Scan(&mode)
		if e == nil && mode == "wal" {
			break
		}
		if e == nil {
			e = db.QueryRow("PRAGMA journal_mode=WAL").Scan(&mode)
			if e == nil && mode == "wal" {
				break
			}
		}
		var sqliteError interface{ Code() int }
		if e == nil || !errors.As(e, &sqliteError) || (sqliteError.Code()&255 != 5 && sqliteError.Code()&255 != 6) || !time.Now().Before(deadline) {
			db.Close()
			if e == nil {
				return nil, domainError(8, "WAL mode is unavailable", "")
			}
			return nil, storageError(e)
		}
		time.Sleep(10 * time.Millisecond)
	}
	return db, nil
}
func transaction(ctx context.Context, db *sql.DB, write bool, fn func(*sql.Conn) error) error {
	c, e := db.Conn(ctx)
	if e != nil {
		return e
	}
	defer c.Close()
	begin := "BEGIN"
	if write {
		begin = "BEGIN IMMEDIATE"
	}
	if _, e = c.ExecContext(ctx, begin); e != nil {
		return storageError(e)
	}
	defer c.ExecContext(context.Background(), "ROLLBACK")
	if e = fn(c); e != nil {
		return storageError(e)
	}
	_, e = c.ExecContext(ctx, "COMMIT")
	return storageError(e)
}
func compatible(ctx context.Context, c *sql.Conn, init string) error {
	var a, v, n int
	if e := c.QueryRowContext(ctx, "PRAGMA application_id").Scan(&a); e != nil {
		return e
	}
	if e := c.QueryRowContext(ctx, "PRAGMA user_version").Scan(&v); e != nil {
		return e
	}
	if a == applicationID && v == 1 {
		return nil
	}
	if init != "" && a == 0 && v == 0 {
		if e := c.QueryRowContext(ctx, "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'").Scan(&n); e != nil {
			return e
		}
		if n == 0 {
			_, e := c.ExecContext(ctx, init+"; PRAGMA application_id=1279348293; PRAGMA user_version=1;")
			return e
		}
	}
	return domainError(8, "Update Lane to open this database", "")
}
func Open(root, space string) (*Store, error) { return openStore(root, space, false) }

// OpenForRemoval resumes a removal whose checkpoint was blocked by an older reader.
// Other operations on the returned handle still reject the removed marker.
func OpenForRemoval(root, space string) (*Store, error) { return openStore(root, space, true) }
func openStore(root, space string, removal bool) (s *Store, err error) {
	if root == "" {
		root, err = DefaultRoot()
		if err != nil {
			return nil, err
		}
	}
	root, err = filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	if space != "" && !validID(space) {
		return nil, domainError(1, "Invalid space ID", "")
	}
	if err = os.MkdirAll(root, 0700); err != nil {
		return nil, err
	}
	if err = protectPath(root, true); err != nil {
		return nil, err
	}
	r, err := openDB(filepath.Join(root, "registry.sqlite"), false)
	if err != nil {
		return nil, err
	}
	s = &Store{registry: r, root: root}
	opened := s
	defer func() {
		if err != nil {
			r.Close()
			opened.closePhysical()
		}
	}()
	ctx := context.Background()
	err = transaction(ctx, r, true, func(c *sql.Conn) error {
		if e := compatible(ctx, c, registrySchema); e != nil {
			return e
		}

		if space == "" {
			e := c.QueryRowContext(ctx, "SELECT value FROM registry_meta WHERE key='active_space_id'").Scan(&space)
			if errors.Is(e, sql.ErrNoRows) {
				e = c.QueryRowContext(ctx, "SELECT id FROM spaces WHERE account_key='local'").Scan(&space)
				if errors.Is(e, sql.ErrNoRows) {
					space = NewID()
					_, e = c.ExecContext(ctx, "INSERT INTO spaces(id,account_key) VALUES(?,'local')", space)
				}
				if e != nil {
					return e
				}
				if _, e = c.ExecContext(ctx, "INSERT INTO registry_meta(key,value) VALUES('active_space_id',?)", space); e != nil {
					return e
				}
				if _, e = c.ExecContext(ctx, "INSERT OR IGNORE INTO registry_meta(key,value) VALUES('device_id',?)", NewID()); e != nil {
					return e
				}
			} else if e != nil {
				return e
			}
		}

		s.space = space
		var unavailable int
		if e := c.QueryRowContext(ctx, "SELECT count(*) FROM registry_meta WHERE key=? AND value='1'", "unavailable:"+space).Scan(&unavailable); e != nil {
			return e
		}
		if unavailable > 0 && !removal {
			return domainError(7, "Space is unavailable", space)
		}
		var id string
		if e := c.QueryRowContext(ctx, "SELECT id FROM spaces WHERE id=?", space).Scan(&id); e != nil && !(removal && unavailable > 0) {
			return domainError(2, "Space does not exist", space)
		}

		return c.QueryRowContext(ctx, "SELECT value FROM registry_meta WHERE key='device_id'").Scan(&s.device)
	})
	if err != nil {
		return nil, err
	}

	unavailable, e := s.registryUnavailable(ctx)
	if e != nil {
		return nil, e
	}
	if removal && unavailable {
		s.physicalClosed = true
		return s, nil
	}
	s.handles, err = s.acquireHandles(ctx, false)
	if err != nil {
		return nil, err
	}
	// A remover can mark the registry while this opener waits for the shared lock.
	unavailable, err = s.registryUnavailable(ctx)
	if err != nil {
		return nil, err
	}
	if unavailable {
		return nil, domainError(7, "Space is unavailable", space)
	}
	dir := filepath.Join(root, "spaces", space)
	if err = os.MkdirAll(dir, 0700); err != nil {
		return nil, err
	}
	if err = protectPath(filepath.Join(root, "spaces"), true); err != nil {
		return nil, err
	}
	if err = protectPath(dir, true); err != nil {
		return nil, err
	}
	s.db, err = openDB(filepath.Join(dir, "lane.sqlite"), true)
	if err != nil {
		return nil, err
	}
	err = transaction(ctx, s.db, true, func(c *sql.Conn) error {
		if e := compatible(ctx, c, schema); e != nil {
			return e
		}

		if removal {
			var removed int
			if e := c.QueryRowContext(ctx, "SELECT count(*) FROM store_meta WHERE key='removed' AND value='1'").Scan(&removed); e != nil {
				return e
			}
			if removed > 0 {
				return s.checkState(ctx, c, true)
			}
		}
		for k, v := range map[string]string{"space_id": space, "device_id": s.device, "format_version": "1", "account_epoch": "", "pull_cursor": "", "notification_enabled": "0", "notification_content": "DETAIL", "sync_owner": "", "sync_owner_expires_at_us": "0"} {
			if _, e := c.ExecContext(ctx, "INSERT OR IGNORE INTO store_meta(key,value) VALUES(?,?)", k, v); e != nil {
				return e
			}
		}
		return s.check(ctx, c)
	})
	return s, err
}
func (s *Store) check(ctx context.Context, c *sql.Conn) error { return s.checkState(ctx, c, false) }
func (s *Store) checkState(ctx context.Context, c *sql.Conn, allowRemoved bool) error {
	unavailable, e := s.registryUnavailable(ctx)
	if e != nil {
		return e
	}
	if unavailable && !allowRemoved {
		return domainError(7, "Space is unavailable", s.space)
	}
	if e := compatible(ctx, c, ""); e != nil {
		return e
	}

	var invalidHeads int
	if e := c.QueryRowContext(ctx, "SELECT count(*) FROM heads h LEFT JOIN versions v ON v.version_id=h.version_id WHERE v.version_id IS NULL OR h.entity_id<>v.entity_id").Scan(&invalidHeads); e != nil {
		return e
	}
	if invalidHeads > 0 {
		return domainError(1, "Invalid head linkage", s.space)
	}

	var registryApp, registryVersion int
	if e := s.registry.QueryRowContext(ctx, "PRAGMA application_id").Scan(&registryApp); e != nil {
		return e
	}
	if e := s.registry.QueryRowContext(ctx, "PRAGMA user_version").Scan(&registryVersion); e != nil {
		return e
	}
	if registryApp != applicationID || registryVersion != 1 {
		return domainError(8, "Update Lane to open this registry", s.space)
	}
	var device string
	if e := c.QueryRowContext(ctx, "SELECT value FROM store_meta WHERE key='device_id'").Scan(&device); e != nil {
		return e
	}
	if device != s.device {
		return domainError(8, "Store device does not match registry", s.space)
	}
	var space, format string
	if e := c.QueryRowContext(ctx, "SELECT value FROM store_meta WHERE key='space_id'").Scan(&space); e != nil {
		return e
	}
	if e := c.QueryRowContext(ctx, "SELECT value FROM store_meta WHERE key='format_version'").Scan(&format); e != nil {
		return e
	}
	var removed int
	if e := c.QueryRowContext(ctx, "SELECT count(*) FROM store_meta WHERE key='removed' AND value='1'").Scan(&removed); e != nil {
		return e
	}
	if removed > 0 && !allowRemoved {
		return domainError(7, "Space is unavailable", s.space)
	}
	if space != s.space || format != "1" {
		return domainError(8, "Incompatible space format", s.space)
	}
	var n int
	if e := s.registry.QueryRowContext(ctx, "SELECT count(*) FROM spaces WHERE id=?", s.space).Scan(&n); e != nil {
		return e
	}
	if n != 1 {
		return domainError(7, "Space is unavailable", s.space)
	}
	return nil
}

// WithWrite holds a short immediate transaction. Callbacks must not perform network I/O.
func (s *Store) WithWrite(ctx context.Context, fn func(*sql.Conn) error) error {
	return s.withTransaction(ctx, true, fn)
}
func (s *Store) WithRead(ctx context.Context, fn func(*sql.Conn) error) error {
	return s.withTransaction(ctx, false, fn)
}
func (s *Store) withTransaction(ctx context.Context, write bool, fn func(*sql.Conn) error) error {
	s.lifeMu.RLock()
	if s.physicalClosed {
		s.lifeMu.RUnlock()
		return domainError(7, "Space is unavailable", s.space)
	}
	e := transaction(ctx, s.db, write, func(c *sql.Conn) error {
		if e := s.check(ctx, c); e != nil {
			return e
		}
		return fn(c)
	})
	s.lifeMu.RUnlock()
	if unavailableError(e) {
		s.lifeMu.Lock()
		closeErr := s.closePhysical()
		s.lifeMu.Unlock()
		if closeErr != nil {
			return closeErr
		}
	}
	return e
}
func (s *Store) Sequence(ctx context.Context) (n uint64, e error) {
	e = s.WithRead(ctx, func(c *sql.Conn) error {
		return c.QueryRowContext(ctx, "SELECT coalesce(max(sequence),0) FROM changes").Scan(&n)
	})
	return
}
func storageError(e error) error {
	if e == nil {
		return nil
	}
	var se interface{ Code() int }
	if errors.As(e, &se) {
		switch se.Code() & 255 {
		case 5, 6:
			return domainError(9, "Storage is busy; retry the same request", "")
		case 13:
			return domainError(10, "Storage is full", "")
		}
		return domainError(14, "Local storage operation failed", "")
	}
	return e
}
