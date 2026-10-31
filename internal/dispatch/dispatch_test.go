package dispatch

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The smoke executable is a copy of this test binary, not a real agent. No test
// invokes a model, a shell, or the network.
func init() {
	if filepath.Base(os.Args[0]) != "dispatch-fake" {
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "--fake-child" {
		for {
			file, err := os.OpenFile("child-heartbeat", os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
			if err != nil {
				os.Exit(3)
			}
			_, _ = file.WriteString(".")
			_ = file.Close()
			time.Sleep(25 * time.Millisecond)
		}
	}
	mode, err := os.ReadFile(".fake-mode")
	if err != nil {
		os.Exit(3)
	}
	switch string(mode) {
	case "capture":
		input, _ := io.ReadAll(os.Stdin)
		captured, _ := json.Marshal(struct {
			Args  []string
			Env   []string
			Input string
		}{os.Args[1:], os.Environ(), string(input)})
		if os.WriteFile("observed.json", captured, 0600) != nil {
			os.Exit(3)
		}
	case "failure":
		_, _ = os.Stdout.WriteString("SENSITIVE_MODEL_OUTPUT")
		_, _ = os.Stderr.WriteString("SENSITIVE_PROVIDER_ERROR")
		os.Exit(7)
	case "output":
		_, _ = os.Stdout.Write(bytes.Repeat([]byte("x"), maxOutputBytes+1))
		time.Sleep(30 * time.Second)
	case "wait":
		time.Sleep(30 * time.Second)
	case "child":
		child := exec.Command(os.Args[0], "--fake-child")
		child.Stdout, child.Stderr = os.Stdout, os.Stderr
		if child.Start() != nil {
			os.Exit(3)
		}
		time.Sleep(30 * time.Second)
	default:
		os.Exit(3)
	}
	_, _ = os.Stdout.WriteString("SAFE_FAKE_RESULT")
	_, _ = os.Stderr.WriteString("PRIVATE_FAKE_DIAGNOSTIC")
	os.Exit(0)
}

func fakeConfig(t *testing.T, mode string) Config {
	t.Helper()
	setFakeHermesRoute(t)
	workspace := t.TempDir()
	binary := filepath.Join(t.TempDir(), "dispatch-fake")
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	source, err := os.Open(self)
	if err != nil {
		t.Fatal(err)
	}
	defer source.Close()
	destination, err := os.OpenFile(binary, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0700)
	if err != nil {
		t.Fatal(err)
	}
	_, copyErr := io.Copy(destination, source)
	closeErr := destination.Close()
	if copyErr != nil || closeErr != nil {
		t.Fatalf("copying fake executable: %v / %v", copyErr, closeErr)
	}
	if err := os.WriteFile(filepath.Join(workspace, ".fake-mode"), []byte(mode), 0600); err != nil {
		t.Fatal(err)
	}
	return Config{Agent: "claude", Binary: binary, Workspace: workspace, Task: "Extract action items.", TimeoutSeconds: 5, CloudApproved: true}
}

func TestRunUsesStdinAndAllowlistedEnvironment(t *testing.T) {
	for _, name := range []string{"claude", "codex", "hermes"} {
		t.Run(name, func(t *testing.T) {
			cfg := fakeConfig(t, "capture")
			cfg.Agent = name
			t.Setenv("AURORA_WEBHOOK_SECRET", "never-forward-this")
			t.Setenv("AURORA_BEARER_TOKEN", "never-forward-this-either")
			t.Setenv("UNRELATED_SECRET", "never-forward-this-too")
			t.Setenv("NODE_OPTIONS", "--dangerous-inherited-hook")
			raw := []byte(`{"id":"event-123","data":{"transcript":"UNTRUSTED_TRANSCRIPT_MARKER"}}`)
			result, err := Run(context.Background(), cfg, raw)
			if err != nil {
				t.Fatal(err)
			}
			if string(result) != "SAFE_FAKE_RESULT" {
				t.Fatal("successful stdout was not preserved or stderr leaked into result")
			}
			captured, err := os.ReadFile(filepath.Join(cfg.Workspace, "observed.json"))
			if err != nil {
				t.Fatal(err)
			}
			var got struct {
				Args  []string
				Env   []string
				Input string
			}
			if err := json.Unmarshal(captured, &got); err != nil {
				t.Fatal(err)
			}
			if strings.Contains(strings.Join(got.Args, " "), "UNTRUSTED_TRANSCRIPT_MARKER") || strings.Contains(strings.Join(got.Args, " "), cfg.Task) {
				t.Fatal("event or task exposed in argv")
			}
			if !strings.Contains(got.Input, string(raw)) || !strings.Contains(got.Input, `Event ID: "event-123"`) || !strings.Contains(got.Input, "untrusted data") {
				t.Fatal("stdin does not preserve event and trust instructions")
			}
			for _, entry := range got.Env {
				if strings.HasPrefix(entry, "AURORA_") || strings.HasPrefix(entry, "UNRELATED_SECRET=") || strings.HasPrefix(entry, "NODE_OPTIONS=") {
					t.Fatal("unrelated environment reached worker")
				}
			}
		})
	}
}

func TestRunDiscardsDiagnostics(t *testing.T) {
	result, err := Run(context.Background(), fakeConfig(t, "failure"), []byte(`{"id":"event-123"}`))
	if err == nil || result != nil || strings.Contains(err.Error(), "SENSITIVE") {
		t.Fatalf("expected generic execution error, got %v", err)
	}
}

func TestRunBoundsOutput(t *testing.T) {
	result, err := Run(context.Background(), fakeConfig(t, "output"), []byte(`{"id":"event-123"}`))
	if err == nil || result != nil || err.Error() != "agent output limit exceeded" {
		t.Fatalf("expected bounded-output error, got %v", err)
	}
}

func TestRunTimeout(t *testing.T) {
	cfg := fakeConfig(t, "wait")
	cfg.TimeoutSeconds = 1
	start := time.Now()
	_, err := Run(context.Background(), cfg, []byte(`{"id":"event-123"}`))
	if !errors.Is(err, context.DeadlineExceeded) || time.Since(start) > 4*time.Second {
		t.Fatalf("expected prompt timeout, got %v", err)
	}
}

func TestCancellationKillsChildProcessGroup(t *testing.T) {
	cfg := fakeConfig(t, "child")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	result := make(chan error, 1)
	go func() {
		_, err := Run(ctx, cfg, []byte(`{"id":"event-123"}`))
		result <- err
	}()
	heartbeat := filepath.Join(cfg.Workspace, "child-heartbeat")
	deadline := time.Now().Add(3 * time.Second)
	for {
		if info, err := os.Stat(heartbeat); err == nil && info.Size() > 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("fake child did not start")
		}
		time.Sleep(10 * time.Millisecond)
	}
	cancel()
	select {
	case err := <-result:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("expected cancellation, got %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("cancellation did not stop worker")
	}
	before, err := os.ReadFile(heartbeat)
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(150 * time.Millisecond)
	after, err := os.ReadFile(heartbeat)
	if err != nil || !bytes.Equal(before, after) {
		t.Fatal("child survived process-group cancellation")
	}
}

func TestValidateRequiresExplicitSafeConfiguration(t *testing.T) {
	if err := Validate(Config{}); err != nil {
		t.Fatal(err)
	}
	if result, err := Run(context.Background(), Config{}, nil); err != nil || result != nil {
		t.Fatal(err)
	}
	base := fakeConfig(t, "capture")
	cases := []struct {
		name   string
		change func(*Config)
	}{
		{"unknown agent", func(c *Config) { c.Agent = "unknown" }},
		{"disabled with settings", func(c *Config) { c.Agent = "" }},
		{"missing consent", func(c *Config) { c.CloudApproved = false }},
		{"relative binary", func(c *Config) { c.Binary = "claude" }},
		{"missing binary", func(c *Config) { c.Binary += ".missing" }},
		{"relative workspace", func(c *Config) { c.Workspace = "." }},
		{"root workspace", func(c *Config) { c.Workspace = "/" }},
		{"empty task", func(c *Config) { c.Task = " " }},
		{"zero timeout", func(c *Config) { c.TimeoutSeconds = 0 }},
		{"excessive timeout", func(c *Config) { c.TimeoutSeconds = 3601 }},
	}
	for _, test := range cases {
		t.Run(test.name, func(t *testing.T) {
			cfg := base
			test.change(&cfg)
			if Validate(cfg) == nil {
				t.Fatal("unsafe config accepted")
			}
		})
	}
}

func setFakeHermesRoute(t *testing.T) {
	t.Helper()
	t.Setenv("HERMES_INFERENCE_MODEL", "fake-model")
	t.Setenv("HERMES_INFERENCE_PROVIDER", "custom")
	t.Setenv("CUSTOM_BASE_URL", "http://127.0.0.1:9/v1")
}

func TestHermesRequiresExplicitRoute(t *testing.T) {
	tests := []struct {
		name  string
		value string
	}{
		{"HERMES_INFERENCE_MODEL", ""},
		{"HERMES_INFERENCE_PROVIDER", ""},
		{"HERMES_INFERENCE_PROVIDER", "auto"},
		{"CUSTOM_BASE_URL", ""},
	}
	for _, test := range tests {
		t.Run(test.name+"="+test.value, func(t *testing.T) {
			cfg := fakeConfig(t, "capture")
			cfg.Agent = "hermes"
			t.Setenv(test.name, test.value)
			if Validate(cfg) == nil {
				t.Fatal("Hermes implicit processing route accepted")
			}
		})
	}
}

func TestHermesRejectsCredentialBearingOrInvalidEndpoints(t *testing.T) {
	endpoints := []string{
		"http://user:fake-secret@localhost:8000/v1",
		"https://model.example/v1?api_key=fake-secret",
		"https://model.example/v1?",
		"https://model.example/v1#fake-secret",
		"https://model.example/v1#",
		"file:///tmp/model",
		"localhost:8000/v1",
		"https:///v1",
		"https://:8000/v1",
		"http://localhost:bad/v1",
	}
	for _, endpoint := range endpoints {
		t.Run(endpoint, func(t *testing.T) {
			setFakeHermesRoute(t)
			t.Setenv("CUSTOM_BASE_URL", endpoint)
			_, err := arguments("hermes")
			if err == nil {
				t.Fatal("unsafe endpoint accepted into argv")
			}
			if strings.Contains(err.Error(), "fake-secret") {
				t.Fatal("endpoint credential disclosed in validation error")
			}
		})
	}
}

func TestHermesAcceptsApprovedHTTPSEndpoints(t *testing.T) {
	for _, endpoint := range []string{"http://127.0.0.1:11434/v1", "http://[::1]:8080/v1", "https://model.example/v1"} {
		t.Run(endpoint, func(t *testing.T) {
			setFakeHermesRoute(t)
			t.Setenv("CUSTOM_BASE_URL", endpoint)
			_, _, actual, err := hermesRoute()
			if err != nil || actual != endpoint {
				t.Fatalf("approved HTTP(S) route rejected: %v", err)
			}
		})
	}
}
