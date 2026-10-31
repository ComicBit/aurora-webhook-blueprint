package inbox

import (
	"bytes"
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"

	bolt "go.etcd.io/bbolt"
)

func TestRestartRecoversAmbiguousClaimWithoutRetry(t *testing.T) {
	c := testConfig(t)
	s, e := Open(c)
	if e != nil {
		t.Fatal(e)
	}
	raw := synthetic("evt_crash", false)
	if _, e = s.Accept("evt_crash", raw, false, time.Now()); e != nil {
		t.Fatal(e)
	}
	claimed, payload, e := s.claim()
	if e != nil || claimed == nil || !bytes.Equal(payload, raw) {
		t.Fatalf("claim: %+v %v", claimed, e)
	}
	if e = s.Close(); e != nil {
		t.Fatal(e)
	}
	s, e = Open(c)
	if e != nil {
		t.Fatal(e)
	}
	defer s.Close()
	records, e := s.Status()
	if e != nil || len(records) != 1 || records[0].State != "needs_review" || records[0].Attempts != 1 {
		t.Fatalf("recovery: %+v %v", records, e)
	}
	next, _, e := s.claim()
	if e != nil || next != nil {
		t.Fatalf("unexpected automatic retry: %+v %v", next, e)
	}
	if e = s.Retry("evt_crash"); e != nil {
		t.Fatal(e)
	}
	next, _, e = s.claim()
	if e != nil || next == nil || next.Attempts != 2 {
		t.Fatalf("manual retry: %+v %v", next, e)
	}
}

func TestCapacityBudgetAndDeletionTombstone(t *testing.T) {
	c := testConfig(t)
	c.MaxPending = 1
	s, e := Open(c)
	if e != nil {
		t.Fatal(e)
	}
	defer s.Close()
	now := time.Now()
	raw := synthetic("evt_one", false)
	if _, e = s.Accept("evt_one", raw, false, now); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Accept("evt_two", synthetic("evt_two", false), false, now); !errors.Is(e, ErrFull) {
		t.Fatalf("pending limit: %v", e)
	}
	if duplicate, e := s.Accept("evt_one", raw, false, now); e != nil || !duplicate {
		t.Fatalf("duplicate under pressure: %v %v", duplicate, e)
	}
	if e = s.Delete("evt_one"); e != nil {
		t.Fatal(e)
	}
	if duplicate, e := s.Accept("evt_one", raw, false, now); e != nil || !duplicate {
		t.Fatalf("deleted replay: %v %v", duplicate, e)
	}
	if _, e = s.Accept("evt_one", append(raw, ' '), false, now); !errors.Is(e, ErrConflict) {
		t.Fatalf("deleted conflict: %v", e)
	}
	if e = s.Retry("evt_one"); !errors.Is(e, ErrNotRetryable) {
		t.Fatalf("deleted retry: %v", e)
	}
	if e = s.db.View(func(tx *bolt.Tx) error {
		if tx.Bucket(payloadsBucket).Get([]byte("evt_one")) != nil {
			t.Error("deleted payload retained")
		}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Accept("evt_two", synthetic("evt_two", false), false, now); e != nil {
		t.Fatal(e)
	}
	// A test event bypasses the work queue limit but still consumes the storage budget.
	if _, e = s.Accept("evt_test", synthetic("evt_test", true), true, now); e != nil {
		t.Fatal(e)
	}
	if e = s.Retry("evt_test"); !errors.Is(e, ErrNotRetryable) {
		t.Fatalf("test retry: %v", e)
	}
}

func TestRetainedPayloadBudgetAndFiniteDedupe(t *testing.T) {
	c := testConfig(t)
	c.MaxStorageBytes = MaxBodyBytes + recordCharge
	s, e := Open(c)
	if e != nil {
		t.Fatal(e)
	}
	defer s.Close()
	raw := bytes.Repeat([]byte{'x'}, MaxBodyBytes)
	now := time.Now()
	if _, e = s.Accept("evt_large", raw, true, now); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Accept("evt_other", []byte("small"), true, now); !errors.Is(e, ErrFull) {
		t.Fatalf("storage budget: %v", e)
	}
	if e = s.Delete("evt_large"); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Accept("evt_other", []byte("small"), true, now); e != nil {
		t.Fatal(e)
	}
	afterRetention := now.Add(time.Duration(c.RetentionDays+1) * 24 * time.Hour)
	if duplicate, e := s.Accept("evt_large", []byte("new bytes after expiry"), true, afterRetention); e != nil || duplicate {
		t.Fatalf("expiry loses dedupe: %v %v", duplicate, e)
	}
}

func TestWorkerTestBypassAndDurableClaim(t *testing.T) {
	s := testStore(t)
	// The execution seam exercises queue transitions without invoking a real model or requiring credentials.
	s.config.Worker.Agent = "synthetic"
	s.config.Worker.TimeoutSeconds = 5
	if _, e := s.Accept("evt_test", synthetic("evt_test", true), true, time.Now()); e != nil {
		t.Fatal(e)
	}
	raw := synthetic("evt_real", false)
	if _, e := s.Accept("evt_real", raw, false, time.Now()); e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	executed := make(chan []byte, 1)
	done := make(chan error, 1)
	go func() {
		done <- s.run(ctx, func(_ context.Context, body []byte) ([]byte, error) {
			records, e := s.Status()
			if e != nil {
				return nil, e
			}
			found := false
			for _, r := range records {
				if r.ID == "evt_real" && r.State == "running" && r.Attempts == 1 {
					found = true
				}
			}
			if !found {
				return nil, errors.New("side effect was not durably claimed")
			}
			executed <- bytes.Clone(body)
			cancel()
			return []byte("must not retain failed output"), errors.New("synthetic ambiguous failure")
		})
	}()
	select {
	case e := <-done:
		if e != nil {
			t.Fatal(e)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("worker failed to stop")
	}
	select {
	case body := <-executed:
		if !bytes.Equal(body, raw) {
			t.Fatal("worker body changed")
		}
	default:
		t.Fatal("worker never executed")
	}
	if len(executed) != 0 {
		t.Fatal("test event executed")
	}
	records, e := s.Status()
	if e != nil {
		t.Fatal(e)
	}
	for _, r := range records {
		expected := "needs_review"
		if r.Test {
			expected = "test"
		}
		if r.State != expected {
			t.Fatalf("record: %+v", r)
		}
	}
	if _, e = s.Result("evt_real"); !errors.Is(e, ErrNotFound) {
		t.Fatalf("failed output retained: %v", e)
	}
}

func TestPrivatePathsAndExclusiveDatabase(t *testing.T) {
	c := testConfig(t)
	s, e := Open(c)
	if e != nil {
		t.Fatal(e)
	}
	if other, e := Open(c); e == nil {
		other.Close()
		t.Error("second database owner accepted")
	}
	s.Close()
	info, e := os.Stat(filepath.Join(c.DataDir, "inbox.db"))
	if e != nil || info.Mode().Perm()&0077 != 0 {
		t.Fatalf("database permissions: %v %v", info, e)
	}
	if e = os.Chmod(c.DataDir, 0755); e != nil {
		t.Fatal(e)
	}
	if other, e := Open(c); e == nil {
		other.Close()
		t.Error("non-private directory accepted")
	}
	if e = os.Chmod(c.DataDir, 0700); e != nil {
		t.Fatal(e)
	}
	linked := filepath.Join(c.DataDir, "linked")
	if e = os.Symlink(c.DataDir, linked); e != nil {
		t.Fatal(e)
	}
	c.DataDir = linked
	if other, e := Open(c); e == nil {
		other.Close()
		t.Error("symlink directory accepted")
	}
}

func TestSuccessfulResultIsAtomicRetainedAndDeleted(t *testing.T) {
	c := testConfig(t)
	s, e := Open(c)
	if e != nil {
		t.Fatal(e)
	}
	raw := synthetic("evt_result", false)
	if _, e = s.Accept("evt_result", raw, false, time.Now()); e != nil {
		t.Fatal(e)
	}
	if _, _, e = s.claim(); e != nil {
		t.Fatal(e)
	}
	output := []byte("{\"synthetic_result\":\"review only\"}\n")
	if e = s.finish("evt_result", output, true); e != nil {
		t.Fatal(e)
	}
	if e = s.Close(); e != nil {
		t.Fatal(e)
	}
	s, e = Open(c)
	if e != nil {
		t.Fatal(e)
	}
	defer s.Close()
	result, e := s.Result("evt_result")
	if e != nil || !bytes.Equal(result, output) {
		t.Fatalf("result: %q %v", result, e)
	}
	payload, e := s.Payload("evt_result")
	if e != nil || !bytes.Equal(payload, raw) {
		t.Fatalf("payload: %v", e)
	}
	records, e := s.Status()
	if e != nil || len(records) != 1 || records[0].State != "done" {
		t.Fatalf("status: %+v %v", records, e)
	}
	if e = s.Delete("evt_result"); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Result("evt_result"); !errors.Is(e, ErrNotFound) {
		t.Fatalf("deleted result: %v", e)
	}
	if _, e = s.Payload("evt_result"); !errors.Is(e, ErrNotFound) {
		t.Fatalf("deleted payload: %v", e)
	}
	if e = s.db.View(func(tx *bolt.Tx) error {
		if usage(tx, usageKey) != recordCharge {
			t.Errorf("tombstone budget = %d", usage(tx, usageKey))
		}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
}
