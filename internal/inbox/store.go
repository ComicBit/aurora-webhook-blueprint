package inbox

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"time"

	bolt "go.etcd.io/bbolt"
)

var (
	ErrConflict      = errors.New("event ID conflicts with retained event")
	ErrFull          = errors.New("inbox capacity reached")
	ErrNotFound      = errors.New("event not found")
	ErrNotRetryable  = errors.New("event cannot be retried")
	recordsBucket    = []byte("records")
	payloadsBucket   = []byte("payloads")
	resultsBucket    = []byte("results")
	expiryBucket     = []byte("expiry")
	pendingBucket    = []byte("pending")
	accountingBucket = []byte("accounting")
	usageKey         = []byte("usage")
	pendingKey       = []byte("pending")
)

// Budget charges include fixed per-record overhead, even after deletion.
const recordCharge int64 = 4096
const maxResultBytes int64 = 64 * 1024

func retainedCharge(r Record) int64 {
	charge := recordCharge + r.Size
	if !r.Test && r.State != "deleted" {
		charge += maxResultBytes
	}
	return charge
}

// Record intentionally contains no transcript, title, secret, or model output.
type Record struct {
	ID         string `json:"id"`
	State      string `json:"state"`
	ReceivedAt int64  `json:"received_at"`
	ExpiresAt  int64  `json:"expires_at"`
	Test       bool   `json:"test"`
	Attempts   int    `json:"attempts"`
	Hash       string `json:"-"`
	Size       int64  `json:"-"`
}

// diskRecord separates durable metadata from the deliberately redacted public representation.
type diskRecord struct {
	Record
	Digest       string `json:"digest"`
	PayloadBytes int64  `json:"payload_bytes"`
}

type Store struct {
	db        *bolt.DB
	config    Config
	wake      chan struct{}
	closeOnce sync.Once
}

func privateDirectory(path string) error {
	abs, err := filepath.Abs(path)
	if err != nil {
		return errors.New("invalid data directory")
	}
	// Reject symlinks anywhere in the configured path, including existing ancestors.
	current := string(filepath.Separator)
	for _, part := range bytes.Split([]byte(abs), []byte{filepath.Separator}) {
		if len(part) == 0 {
			continue
		}
		current = filepath.Join(current, string(part))
		info, e := os.Lstat(current)
		if os.IsNotExist(e) {
			if e = os.Mkdir(current, 0700); e != nil {
				return errors.New("cannot create data directory")
			}
			continue
		}
		if e != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			return errors.New("unsafe data directory")
		}
	}
	info, err := os.Lstat(abs)
	if err != nil || info.Mode().Perm()&0077 != 0 {
		return errors.New("data directory must be private (0700)")
	}
	return nil
}

func Open(c Config) (*Store, error) {
	if err := c.Validate(); err != nil {
		return nil, err
	}
	if err := privateDirectory(c.DataDir); err != nil {
		return nil, err
	}
	path := filepath.Join(c.DataDir, "inbox.db")
	if info, err := os.Lstat(path); err == nil {
		if !info.Mode().IsRegular() || info.Mode().Perm()&0077 != 0 {
			return nil, errors.New("database must be a private regular file (0600)")
		}
	} else if !os.IsNotExist(err) {
		return nil, errors.New("cannot inspect database")
	}
	db, err := bolt.Open(path, 0600, &bolt.Options{Timeout: time.Second})
	if err != nil {
		return nil, errors.New("cannot open inbox database; stop any other receiver or administrative command")
	}
	db.AllocSize = 1024 * 1024
	s := &Store{db: db, config: c, wake: make(chan struct{}, 1)}
	err = db.Update(func(tx *bolt.Tx) error {
		for _, name := range [][]byte{recordsBucket, payloadsBucket, resultsBucket} {
			if _, e := tx.CreateBucketIfNotExists(name); e != nil {
				return e
			}
		}
		// Rebuild small indexes and accounting once on open, including crash recovery.
		for _, name := range [][]byte{expiryBucket, pendingBucket, accountingBucket} {
			if tx.Bucket(name) != nil {
				if e := tx.DeleteBucket(name); e != nil {
					return e
				}
			}
			if _, e := tx.CreateBucket(name); e != nil {
				return e
			}
		}
		b := tx.Bucket(recordsBucket)
		var recovered []diskRecord
		var used, pending int64
		if err := b.ForEach(func(_, v []byte) error {
			r, e := decodeRecord(v)
			if e != nil {
				return e
			}
			if r.State == "running" {
				r.State = "needs_review"
				recovered = append(recovered, r)
			}
			used += retainedCharge(r.Record)
			if r.State == "pending" {
				pending++
				if e = tx.Bucket(pendingBucket).Put([]byte(r.ID), nil); e != nil {
					return e
				}
			}
			return tx.Bucket(expiryBucket).Put(expiryKey(r.Record), nil)
		}); err != nil {
			return err
		}
		for _, r := range recovered {
			if e := putRecord(b, r); e != nil {
				return e
			}
		}
		if e := changeUsage(tx, used, pending); e != nil {
			return e
		}
		return prune(tx, time.Now().Unix())
	})
	if err != nil {
		db.Close()
		return nil, errors.New("cannot initialize inbox database")
	}
	return s, nil
}

func (s *Store) Close() error {
	var err error
	s.closeOnce.Do(func() { err = s.db.Close() })
	return err
}
func (s *Store) notify() {
	select {
	case s.wake <- struct{}{}:
	default:
	}
}

func decodeRecord(v []byte) (diskRecord, error) {
	var r diskRecord
	err := json.Unmarshal(v, &r)
	r.Hash = r.Digest
	r.Size = r.PayloadBytes
	return r, err
}
func putRecord(b *bolt.Bucket, r diskRecord) error {
	r.Digest = r.Hash
	r.PayloadBytes = r.Size
	v, e := json.Marshal(r)
	if e != nil {
		return e
	}
	return b.Put([]byte(r.ID), v)
}

func expiryKey(r Record) []byte {
	key := make([]byte, 8+len(r.ID))
	binary.BigEndian.PutUint64(key, uint64(r.ExpiresAt))
	copy(key[8:], r.ID)
	return key
}
func usage(tx *bolt.Tx, key []byte) int64 {
	v := tx.Bucket(accountingBucket).Get(key)
	if len(v) != 8 {
		return 0
	}
	return int64(binary.BigEndian.Uint64(v))
}
func changeUsage(tx *bolt.Tx, bytesDelta, pendingDelta int64) error {
	b := tx.Bucket(accountingBucket)
	for _, item := range []struct {
		key   []byte
		delta int64
	}{{usageKey, bytesDelta}, {pendingKey, pendingDelta}} {
		value := usage(tx, item.key) + item.delta
		if value < 0 {
			return errors.New("invalid inbox accounting")
		}
		var encoded [8]byte
		binary.BigEndian.PutUint64(encoded[:], uint64(value))
		if e := b.Put(item.key, encoded[:]); e != nil {
			return e
		}
	}
	return nil
}
func prune(tx *bolt.Tx, now int64) error {
	b, p := tx.Bucket(recordsBucket), tx.Bucket(payloadsBucket)
	cur := tx.Bucket(expiryBucket).Cursor()
	for k, _ := cur.First(); k != nil; k, _ = cur.Next() {
		if len(k) < 9 {
			return errors.New("invalid retention index")
		}
		if int64(binary.BigEndian.Uint64(k[:8])) > now {
			break
		}
		id := k[8:]
		v := b.Get(id)
		if v == nil {
			return errors.New("invalid retention index")
		}
		r, e := decodeRecord(v)
		if e != nil {
			return e
		}
		// Retain identity while a side effect is still running.
		if r.State == "running" {
			continue
		}
		pendingDelta := int64(0)
		if r.State == "pending" {
			pendingDelta = -1
		}
		if e = changeUsage(tx, -retainedCharge(r.Record), pendingDelta); e != nil {
			return e
		}
		if e = p.Delete(id); e != nil {
			return e
		}
		if e = tx.Bucket(pendingBucket).Delete(id); e != nil {
			return e
		}
		if e = tx.Bucket(resultsBucket).Delete(id); e != nil {
			return e
		}
		if e = b.Delete(id); e != nil {
			return e
		}
		if e = cur.Delete(); e != nil {
			return e
		}
	}
	return nil
}

// Accept commits an already validated body once. Duplicate acknowledgements do not extend retention.
func (s *Store) Accept(id string, raw []byte, test bool, now time.Time) (bool, error) {
	return s.accept(id, raw, now, func() (bool, error) { return test, nil })
}

func (s *Store) acceptEvent(id string, raw []byte, now time.Time) (bool, error) {
	return s.accept(id, raw, now, func() (bool, error) { event, err := parseEvent(raw, id); return event.Test, err })
}

func (s *Store) accept(id string, raw []byte, now time.Time, classify func() (bool, error)) (bool, error) {
	if !validID(id) || len(raw) > MaxBodyBytes {
		return false, errEvent
	}
	digest := sha256.Sum256(raw)
	hash := hex.EncodeToString(digest[:])
	duplicate := false
	var test bool
	err := s.db.Update(func(tx *bolt.Tx) error {
		if e := prune(tx, now.Unix()); e != nil {
			return e
		}
		b := tx.Bucket(recordsBucket)
		if v := b.Get([]byte(id)); v != nil {
			r, e := decodeRecord(v)
			if e != nil {
				return e
			}
			if r.Hash != hash {
				return ErrConflict
			}
			duplicate = true
			return nil
		}
		var e error
		test, e = classify()
		if e != nil {
			return e
		}
		used, pending := usage(tx, usageKey), usage(tx, pendingKey)
		charge := recordCharge + int64(len(raw))
		if !test {
			charge += maxResultBytes
		}
		if (!test && pending >= int64(s.config.MaxPending)) || charge > s.config.MaxStorageBytes-used {
			return ErrFull
		}
		state := "pending"
		if test {
			state = "test"
		}
		r := diskRecord{Record: Record{ID: id, State: state, ReceivedAt: now.Unix(), ExpiresAt: now.Add(time.Duration(s.config.RetentionDays) * 24 * time.Hour).Unix(), Test: test, Hash: hash, Size: int64(len(raw))}}
		if e := tx.Bucket(payloadsBucket).Put([]byte(id), raw); e != nil {
			return e
		}
		if e := putRecord(b, r); e != nil {
			return e
		}
		if e := tx.Bucket(expiryBucket).Put(expiryKey(r.Record), nil); e != nil {
			return e
		}
		pendingDelta := int64(0)
		if !test {
			pendingDelta = 1
			if e := tx.Bucket(pendingBucket).Put([]byte(id), nil); e != nil {
				return e
			}
		}
		return changeUsage(tx, charge, pendingDelta)
	})
	if err == nil && !duplicate && !test {
		s.notify()
	}
	return duplicate, err
}

func (s *Store) Status() ([]Record, error) {
	result := make([]Record, 0)
	err := s.db.Update(func(tx *bolt.Tx) error {
		if e := prune(tx, time.Now().Unix()); e != nil {
			return e
		}
		return tx.Bucket(recordsBucket).ForEach(func(_, v []byte) error {
			r, e := decodeRecord(v)
			if e == nil {
				result = append(result, r.Record)
			}
			return e
		})
	})
	return result, err
}

func (s *Store) Retry(id string) error {
	err := s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(recordsBucket)
		v := b.Get([]byte(id))
		if v == nil {
			return ErrNotFound
		}
		r, e := decodeRecord(v)
		if e != nil {
			return e
		}
		if r.Test || r.State != "needs_review" || r.ExpiresAt <= time.Now().Unix() || tx.Bucket(payloadsBucket).Get([]byte(id)) == nil {
			return ErrNotRetryable
		}
		if usage(tx, pendingKey) >= int64(s.config.MaxPending) {
			return ErrFull
		}
		r.State = "pending"
		if e = putRecord(b, r); e != nil {
			return e
		}
		if e = tx.Bucket(pendingBucket).Put([]byte(id), nil); e != nil {
			return e
		}
		return changeUsage(tx, 0, 1)
	})
	if err == nil {
		s.notify()
	}
	return err
}

func (s *Store) Delete(id string) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(recordsBucket)
		v := b.Get([]byte(id))
		if v == nil {
			return ErrNotFound
		}
		r, e := decodeRecord(v)
		if e != nil {
			return e
		}
		if r.State == "running" {
			return errors.New("cannot delete a running event")
		}
		if e = tx.Bucket(payloadsBucket).Delete([]byte(id)); e != nil {
			return e
		}
		if e = tx.Bucket(resultsBucket).Delete([]byte(id)); e != nil {
			return e
		}
		pendingDelta := int64(0)
		if r.State == "pending" {
			pendingDelta = -1
		}
		if e = changeUsage(tx, -retainedCharge(r.Record)+recordCharge, pendingDelta); e != nil {
			return e
		}
		if e = tx.Bucket(pendingBucket).Delete([]byte(id)); e != nil {
			return e
		}
		if e = tx.Bucket(expiryBucket).Delete(expiryKey(r.Record)); e != nil {
			return e
		}
		r.Size = 0
		r.State = "deleted"
		expiry := time.Now().Add(time.Duration(s.config.RetentionDays) * 24 * time.Hour).Unix()
		if expiry > r.ExpiresAt {
			r.ExpiresAt = expiry
		}
		if e = tx.Bucket(expiryBucket).Put(expiryKey(r.Record), nil); e != nil {
			return e
		}
		return putRecord(b, r)
	})
}

func (s *Store) claim() (*Record, []byte, error) {
	var record *Record
	var raw []byte
	err := s.db.Update(func(tx *bolt.Tx) error {
		if e := prune(tx, time.Now().Unix()); e != nil {
			return e
		}
		b := tx.Bucket(recordsBucket)
		key, _ := tx.Bucket(pendingBucket).Cursor().First()
		if key == nil {
			return nil
		}
		id := bytes.Clone(key)
		r, e := decodeRecord(b.Get(id))
		if e != nil {
			return e
		}
		if r.State != "pending" || r.Test {
			return errors.New("invalid pending index")
		}
		payload := tx.Bucket(payloadsBucket).Get(id)
		if payload == nil {
			return errors.New("missing event payload")
		}
		raw = bytes.Clone(payload)
		r.State = "running"
		r.Attempts++
		if e = putRecord(b, r); e != nil {
			return e
		}
		if e = tx.Bucket(pendingBucket).Delete(id); e != nil {
			return e
		}
		record = &r.Record
		return nil
	})
	return record, raw, err
}

func (s *Store) finish(id string, output []byte, success bool) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(recordsBucket)
		v := b.Get([]byte(id))
		if v == nil {
			return ErrNotFound
		}
		r, e := decodeRecord(v)
		if e != nil {
			return e
		}
		if r.State != "running" {
			return errors.New("invalid event transition")
		}
		if int64(len(output)) > maxResultBytes {
			return errors.New("worker result exceeds reserved budget")
		}
		if success {
			if e = tx.Bucket(resultsBucket).Put([]byte(id), output); e != nil {
				return e
			}
		} else {
			if e = tx.Bucket(resultsBucket).Delete([]byte(id)); e != nil {
				return e
			}
		}
		r.State = "needs_review"
		if success {
			r.State = "done"
		}
		if e = putRecord(b, r); e != nil {
			return e
		}
		return changeUsage(tx, 0, -1)
	})
}

// Payload explicitly retrieves sensitive raw JSON; callers must not log it implicitly.
func (s *Store) Payload(id string) ([]byte, error) {
	var raw []byte
	err := s.db.Update(func(tx *bolt.Tx) error {
		if e := prune(tx, time.Now().Unix()); e != nil {
			return e
		}
		v := tx.Bucket(payloadsBucket).Get([]byte(id))
		if v != nil {
			raw = bytes.Clone(v)
		}
		return nil
	})
	if err != nil {
		return nil, errors.New("cannot read event payload")
	}
	if raw == nil {
		return nil, ErrNotFound
	}
	return raw, nil
}

// Result explicitly retrieves a successful agent's bounded output, which may be sensitive.
func (s *Store) Result(id string) ([]byte, error) {
	var output []byte
	found := false
	err := s.db.Update(func(tx *bolt.Tx) error {
		if e := prune(tx, time.Now().Unix()); e != nil {
			return e
		}
		v := tx.Bucket(recordsBucket).Get([]byte(id))
		if v == nil {
			return nil
		}
		r, e := decodeRecord(v)
		if e != nil {
			return e
		}
		if r.State != "done" {
			return nil
		}
		result := tx.Bucket(resultsBucket).Get([]byte(id))
		if result == nil {
			return errors.New("missing worker result")
		}
		output = bytes.Clone(result)
		found = true
		return nil
	})
	if err != nil {
		return nil, errors.New("cannot read worker result")
	}
	if !found {
		return nil, ErrNotFound
	}
	return output, nil
}
