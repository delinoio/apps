// SPDX-License-Identifier: Apache-2.0
package locallog

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"time"
)

const MaxBytes int64 = 100 << 20
const MaxAge = 30 * 24 * time.Hour

// Record writes metadata only. Rotation and writes share a cross-process lock.
func Record(ctx context.Context, root, operation, reason, correlationID string) error {
	dir := filepath.Join(root, "logs")
	if err := os.MkdirAll(dir, 0700); err != nil {
		return err
	}
	lock, err := os.OpenFile(filepath.Join(dir, ".rotation.lock"), os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return err
	}
	defer lock.Close()
	if err = acquire(ctx, lock); err != nil {
		return err
	}
	defer release(lock)
	now := time.Now().UTC()
	name := fmt.Sprintf("cli-%s-%d.log", now.Format("20060102"), os.Getpid())
	f, err := os.OpenFile(filepath.Join(dir, name), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	logger := slog.New(slog.NewJSONHandler(f, nil))
	logger.InfoContext(ctx, "operation", slog.String("operation", operation), slog.String("reason", reason), slog.String("correlationId", correlationID))
	err = f.Close()
	if err != nil {
		return err
	}
	return prune(dir, now, MaxBytes)
}
func prune(dir string, now time.Time, limit int64) error {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return err
	}
	type file struct {
		name  string
		size  int64
		mtime time.Time
	}
	var files []file
	var total int64
	for _, entry := range entries {
		if entry.Name() == ".rotation.lock" || entry.IsDir() {
			continue
		}
		info, e := entry.Info()
		if e != nil {
			return e
		}
		if !info.Mode().IsRegular() {
			continue
		}
		if now.Sub(info.ModTime()) >= MaxAge {
			if e = os.Remove(filepath.Join(dir, entry.Name())); e != nil {
				return e
			}
			continue
		}
		files = append(files, file{entry.Name(), info.Size(), info.ModTime()})
		total += info.Size()
	}
	sort.Slice(files, func(i, j int) bool {
		if files[i].mtime.Equal(files[j].mtime) {
			return files[i].name < files[j].name
		}
		return files[i].mtime.Before(files[j].mtime)
	})
	for _, f := range files {
		if total <= limit {
			break
		}
		if err = os.Remove(filepath.Join(dir, f.name)); err != nil {
			return err
		}
		total -= f.size
	}
	return nil
}
func wait(ctx context.Context) error {
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-time.After(25 * time.Millisecond):
		return nil
	}
}

var errBusy = errors.New("log rotation busy")
