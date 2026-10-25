// SPDX-License-Identifier: Apache-2.0
package locallog

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

func TestCombinedRetention(t *testing.T) {
	dir := t.TempDir()
	now := time.Now()
	for i, name := range []string{"native-old.log", "native-new.log", "cli-new.log"} {
		p := filepath.Join(dir, name)
		if e := os.WriteFile(p, []byte("1234567890"), 0600); e != nil {
			t.Fatal(e)
		}
		age := time.Duration(3-i) * time.Hour
		if i == 0 {
			age = MaxAge
		}
		os.Chtimes(p, now.Add(-age), now.Add(-age))
	}
	if e := prune(dir, now, 10); e != nil {
		t.Fatal(e)
	}
	files, _ := os.ReadDir(dir)
	if len(files) != 1 || files[0].Name() != "cli-new.log" {
		t.Fatal(files)
	}
}
func TestConcurrentMetadataWrites(t *testing.T) {
	root := t.TempDir()
	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if e := Record(context.Background(), root, "GetTask", "APPLIED", "id"); e != nil {
				t.Error(e)
			}
		}()
	}
	wg.Wait()
	files, _ := filepath.Glob(filepath.Join(root, "logs", "*.log"))
	if len(files) != 1 {
		t.Fatal(files)
	}
}
