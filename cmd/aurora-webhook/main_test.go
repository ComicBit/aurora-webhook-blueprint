package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ComicBit/aurora-webhook-blueprint/internal/inbox"
)

func TestAdministrativeCommandsDoNotRequireSecrets(t *testing.T) {
	t.Setenv("AURORA_WEBHOOK_SECRET", "")
	t.Setenv("AURORA_BEARER_TOKEN", "")
	dir, e := filepath.EvalSymlinks(t.TempDir())
	if e != nil {
		t.Fatal(e)
	}
	config := inbox.DefaultConfig()
	config.DataDir = filepath.Join(dir, "data")
	encoded, e := json.Marshal(config)
	if e != nil {
		t.Fatal(e)
	}
	path := filepath.Join(dir, "config.json")
	if e = os.WriteFile(path, encoded, 0600); e != nil {
		t.Fatal(e)
	}
	store, e := inbox.Open(config)
	if e != nil {
		t.Fatal(e)
	}
	raw := []byte(`{"id":"evt_cli","synthetic_private_content":"never implicit output"}`)
	if _, e = store.Accept("evt_cli", raw, true, time.Now()); e != nil {
		t.Fatal(e)
	}
	if e = store.Close(); e != nil {
		t.Fatal(e)
	}
	var out bytes.Buffer
	if e = run([]string{"status", "-config", path}, &out); e != nil {
		t.Fatal(e)
	}
	if !strings.Contains(out.String(), "evt_cli") || strings.Contains(out.String(), "synthetic_private_content") {
		t.Fatalf("unsafe status: %s", out.String())
	}
	out.Reset()
	if e = run([]string{"show", "-config", path, "evt_cli"}, &out); e != nil || !bytes.Equal(out.Bytes(), raw) {
		t.Fatalf("explicit show: %s %v", out.String(), e)
	}
	out.Reset()
	if e = run([]string{"retry", "-config", path, "evt_cli"}, &out); e == nil || !strings.Contains(e.Error(), "acknowledge") {
		t.Fatalf("retry consent: %v", e)
	}
	if e = run([]string{"retry", "-config", path, "--acknowledge-duplicate-risk", "evt_cli"}, &out); e == nil {
		t.Fatal("test event retried")
	}
	if e = run([]string{"result", "-config", path, "evt_cli"}, &out); e == nil {
		t.Fatal("test event has result")
	}
	if e = run([]string{"delete", "-config", path, "evt_cli"}, &out); e != nil {
		t.Fatal(e)
	}
	if e = run([]string{"show", "-config", path, "evt_cli"}, &out); e == nil {
		t.Fatal("deleted payload exported")
	}
	out.Reset()
	if e = run([]string{"status", "-config", path}, &out); e != nil || !strings.Contains(out.String(), "deleted") {
		t.Fatalf("tombstone status: %s %v", out.String(), e)
	}
}
