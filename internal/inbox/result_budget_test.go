package inbox

import (
	"bytes"
	"errors"
	"testing"
	"time"

	bolt "go.etcd.io/bbolt"
)

func TestResultBudgetReservedBeforeWorkerIsEnabled(t *testing.T) {
	c := testConfig(t)
	c.MaxStorageBytes = MaxBodyBytes + recordCharge
	s, e := Open(c)
	if e != nil {
		t.Fatal(e)
	}
	defer s.Close()
	now := time.Now()
	if _, e = s.Accept("evt_exceeds_reserve", bytes.Repeat([]byte{'x'}, MaxBodyBytes), false, now); !errors.Is(e, ErrFull) {
		t.Fatalf("result reservation omitted: %v", e)
	}
	raw := bytes.Repeat([]byte{'x'}, MaxBodyBytes-int(maxResultBytes))
	if _, e = s.Accept("evt_reserved", raw, false, now); e != nil {
		t.Fatal(e)
	}
	if _, _, e = s.claim(); e != nil {
		t.Fatal(e)
	}
	result := bytes.Repeat([]byte{'y'}, int(maxResultBytes))
	if e = s.finish("evt_reserved", result, true); e != nil {
		t.Fatal(e)
	}
	if e = s.db.View(func(tx *bolt.Tx) error {
		if used := usage(tx, usageKey); used != c.MaxStorageBytes {
			t.Errorf("charged=%d, budget=%d", used, c.MaxStorageBytes)
		}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	retained, e := s.Result("evt_reserved")
	if e != nil || !bytes.Equal(retained, result) {
		t.Fatalf("reserved result: %v", e)
	}
	if e = s.Delete("evt_reserved"); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Result("evt_reserved"); !errors.Is(e, ErrNotFound) {
		t.Fatalf("deleted result: %v", e)
	}
}
