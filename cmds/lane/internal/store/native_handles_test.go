// SPDX-License-Identifier: Apache-2.0
package store

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

// Run with the native engine driver built against the shared lifetime-lock protocol.
// An external process is necessary: Go-only lock tests do not establish native acceptance.
func nativeDriver(t *testing.T, root string) func(any) any {
	driver := os.Getenv("LANE_NATIVE_STORE_DRIVER")
	if driver == "" {
		t.Skip("set LANE_NATIVE_STORE_DRIVER to the built native store driver")
	}
	processCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
	t.Cleanup(cancel)
	cmd := exec.CommandContext(processCtx, driver, root)
	in, e := cmd.StdinPipe()
	if e != nil {
		t.Fatal(e)
	}
	out, e := cmd.StdoutPipe()
	if e != nil {
		t.Fatal(e)
	}
	cmd.Stderr = os.Stderr
	if e = cmd.Start(); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() {
		in.Close()
		if e := cmd.Wait(); e != nil {
			t.Errorf("native driver exit: %v", e)
		}
	})
	encoder, decoder := json.NewEncoder(in), json.NewDecoder(out)
	exchange := func(request any) any {
		t.Helper()
		if e := encoder.Encode(request); e != nil {
			t.Fatal(e)
		}
		var response any
		if e := decoder.Decode(&response); e != nil {
			t.Fatal(e)
		}
		return response
	}

	return exchange
}
func TestNativeHandleRemoval(t *testing.T) {
	s := testStore(t)
	create(t, s)
	exchange := nativeDriver(t, s.Root())
	var e error
	opened := exchange(map[string]any{"method": "open", "request": map[string]any{"formatVersion": 1, "spaceId": s.SpaceID()}})
	if ok, exists := opened.(map[string]any)["ok"].(bool); exists && !ok {
		t.Fatalf("native open failed: %v", opened)
	}
	if e = s.RemoveSpace(ctx); reason(e) != 9 {
		t.Fatal("native idle handle did not block physical deletion", e)
	}
	dir := filepath.Join(s.Root(), "spaces", s.SpaceID())
	if _, e = os.Stat(filepath.Join(dir, "lane.sqlite")); e != nil {
		t.Fatal("database removed beneath native handle", e)
	}
	exchange(map[string]any{"method": "poll"})
	retry, e := OpenForRemoval(s.Root(), s.SpaceID())
	if e != nil {
		t.Fatal(e)
	}
	defer retry.Close()
	if e = retry.RemoveSpace(ctx); e != nil {
		t.Fatal("native poll did not release handle", e)
	}
	if _, e = os.Stat(dir); !os.IsNotExist(e) {
		t.Fatal("physical directory remains", e)
	}
}

func TestNativeRemovalWaitsForGoHandle(t *testing.T) {
	s := testStore(t)
	create(t, s)
	exchange := nativeDriver(t, s.Root())
	opened := exchange(map[string]any{"method": "open", "request": map[string]any{"formatVersion": 1, "spaceId": s.SpaceID()}}).(map[string]any)
	if opened["ok"] != true {
		t.Fatalf("native open failed: %v", opened)
	}
	request := map[string]any{"method": "invoke", "operation": "RemoveDeviceData", "spaceId": s.SpaceID(), "request": map[string]any{}}
	response := exchange(request).(map[string]any)
	if response["ok"] != false || response["error"].(map[string]any)["reason"] != "ERROR_REASON_STORAGE_BUSY" {
		t.Fatalf("native removed beneath Go handle: %v", response)
	}
	dir := filepath.Join(s.Root(), "spaces", s.SpaceID())
	if _, e := os.Stat(filepath.Join(dir, "lane.sqlite")); e != nil {
		t.Fatal(e)
	}
	if _, e := s.Sequence(ctx); reason(e) != 7 {
		t.Fatal("Go poll did not close marked handle", e)
	}
	response = exchange(request).(map[string]any)
	if response["ok"] != true {
		t.Fatalf("native retry failed: %v", response)
	}
	if _, e := os.Stat(dir); !os.IsNotExist(e) {
		t.Fatal("native did not physically remove directory", e)
	}
}
