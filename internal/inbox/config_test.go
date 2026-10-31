package inbox

import (
	"bytes"
	"encoding/base64"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRelativeDataPathAndUnsafeConfiguration(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.json")
	if e := os.WriteFile(path, []byte(`{"data_dir":"queue"}`), 0600); e != nil {
		t.Fatal(e)
	}
	c, e := LoadConfig(path)
	if e != nil {
		t.Fatal(e)
	}
	if c.DataDir != filepath.Join(dir, "queue") {
		t.Fatalf("relative storage path: %q", c.DataDir)
	}
	for _, raw := range []string{`{"unknown":true}`, `{"worker":{"unknown":true}}`, `null`, `{} {}`, `{"listen":"0.0.0.0:8787"}`, `{"listen":"localhost:8787"}`, `{"listen":"192.168.1.10:8787"}`, `{"max_pending":0}`, `{"max_concurrent_requests":0}`, `{"retention_days":0}`, `{"data_dir":""}`, `{}` + strings.Repeat(" ", 64*1024)} {
		if e = os.WriteFile(path, []byte(raw), 0600); e != nil {
			t.Fatal(e)
		}
		if _, e = LoadConfig(path); e == nil {
			t.Fatalf("unsafe configuration accepted: %.80s", raw)
		}
	}
	c = DefaultConfig()
	c.Listen = "[::1]:8787"
	if e = c.Validate(); e != nil {
		t.Fatalf("IPv6 loopback: %v", e)
	}
}

func TestSecretEnvironmentValidation(t *testing.T) {
	good := "whsec_" + base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x42}, 32))
	t.Setenv("AURORA_BEARER_TOKEN", "")
	for _, secret := range []string{"", "whsec_invalid", "whsec_" + base64.StdEncoding.EncodeToString(make([]byte, 31)), good + "\n"} {
		t.Setenv("AURORA_WEBHOOK_SECRET", secret)
		if _, e := LoadSecrets(); e == nil {
			t.Fatal("invalid secret accepted")
		}
	}
	for _, secret := range []string{good, strings.TrimPrefix(good, "whsec_")} {
		t.Setenv("AURORA_WEBHOOK_SECRET", secret)
		if s, e := LoadSecrets(); e != nil || len(s.Key) != 32 {
			t.Fatalf("valid secret: %v", e)
		}
	}
	for _, bearer := range []string{"short", strings.Repeat("x", 32) + "\n", strings.Repeat("x", 32) + " "} {
		t.Setenv("AURORA_BEARER_TOKEN", bearer)
		if _, e := LoadSecrets(); e == nil {
			t.Fatal("invalid bearer accepted")
		}
	}
	t.Setenv("AURORA_BEARER_TOKEN", strings.Repeat("x", 32))
	if _, e := LoadSecrets(); e != nil {
		t.Fatal(e)
	}
}
