// export-go-inbox reads a stopped v0.1.0 bbolt inbox and emits lossless migration JSONL.
// It is not a receiver, dispatcher, or compatibility fallback. Output is confidential.
package main

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"time"

	bolt "go.etcd.io/bbolt"
)

type diskRecord struct {
	ID         string `json:"id"`
	State      string `json:"state"`
	ReceivedAt int64  `json:"received_at"`
	ExpiresAt  int64  `json:"expires_at"`
	Test       bool   `json:"test"`
	Digest     string `json:"digest"`
}
type migrationRecord struct {
	FormatVersion int     `json:"format_version"`
	EventID       string  `json:"event_id"`
	ReceivedAt    int64   `json:"received_at"`
	ExpiresAt     int64   `json:"expires_at"`
	State         string  `json:"state"`
	Digest        string  `json:"digest"`
	BodyBase64    *string `json:"body_base64"`
	ResultBase64  *string `json:"result_base64"`
	Test          bool    `json:"test"`
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func run() error {
	path := flag.String("input", "", "stopped Go inbox.db")
	output := flag.String("output", "", "new private migration JSONL file")
	flag.Parse()
	if *path == "" || *output == "" || flag.NArg() != 0 {
		return errors.New("use --input stopped-inbox.db --output NEW-private-file.jsonl")
	}
	info, err := os.Lstat(*path)
	if err != nil || !info.Mode().IsRegular() || info.Mode()&os.ModeSymlink != 0 {
		return errors.New("input must be an existing regular database file")
	}
	db, err := bolt.Open(*path, 0600, &bolt.Options{ReadOnly: true, Timeout: time.Second})
	if err != nil {
		return errors.New("cannot read legacy database; stop the old receiver first")
	}
	defer db.Close()
	out, err := os.OpenFile(*output, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return errors.New("output must be a new private file")
	}
	defer out.Close()
	count := 0
	err = db.View(func(tx *bolt.Tx) error {
		records := tx.Bucket([]byte("records"))
		payloads := tx.Bucket([]byte("payloads"))
		results := tx.Bucket([]byte("results"))
		if records == nil || payloads == nil || results == nil {
			return errors.New("not a recognized v0.1 inbox")
		}
		enc := json.NewEncoder(out)
		return records.ForEach(func(key, value []byte) error {
			var r diskRecord
			if json.Unmarshal(value, &r) != nil || r.ID != string(key) || len(r.Digest) != 64 {
				return errors.New("invalid legacy metadata; export stopped")
			}
			m := migrationRecord{1, r.ID, r.ReceivedAt, r.ExpiresAt, r.State, r.Digest, nil, nil, r.Test}
			if body := payloads.Get(key); body != nil {
				digest := sha256.Sum256(body)
				if hex.EncodeToString(digest[:]) != r.Digest {
					return errors.New("legacy body digest mismatch; export stopped")
				}
				encoded := base64.StdEncoding.EncodeToString(body)
				m.BodyBase64 = &encoded
			}
			if result := results.Get(key); result != nil {
				encoded := base64.StdEncoding.EncodeToString(result)
				m.ResultBase64 = &encoded
			}
			if err := enc.Encode(m); err != nil {
				return errors.New("cannot write migration record")
			}
			count++
			return nil
		})
	})
	if err != nil {
		return err
	}
	if err = out.Sync(); err != nil {
		return errors.New("cannot sync migration output")
	}
	fmt.Fprintf(os.Stderr, "Exported %d legacy records; no dispatch or source modification. Retain old backup and reconcile previous external work.\n", count)
	return nil
}
