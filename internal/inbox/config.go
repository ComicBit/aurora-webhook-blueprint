// Package inbox implements authenticated webhook intake and a durable, reviewable inbox.
package inbox

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/ComicBit/aurora-webhook-blueprint/internal/dispatch"
)

const MaxBodyBytes = 8 * 1024 * 1024

// Config contains non-secret settings. Relative data directories resolve beside the config file.
type Config struct {
	Listen                string          `json:"listen"`
	DataDir               string          `json:"data_dir"`
	MaxPending            int             `json:"max_pending"`
	MaxStorageBytes       int64           `json:"max_storage_bytes"`
	RetentionDays         int             `json:"retention_days"`
	MaxConcurrentRequests int             `json:"max_concurrent_requests"`
	Worker                dispatch.Config `json:"worker"`
}

func DefaultConfig() Config {
	return Config{Listen: "127.0.0.1:8787", DataDir: "./data", MaxPending: 100, MaxStorageBytes: 256 * 1024 * 1024, RetentionDays: 7, MaxConcurrentRequests: 8}
}

func LoadConfig(path string) (Config, error) {
	c := DefaultConfig()
	f, err := os.Open(path)
	if err != nil {
		return c, errors.New("cannot open configuration")
	}
	defer f.Close()
	raw, err := io.ReadAll(io.LimitReader(f, 64*1024+1))
	if err != nil || len(raw) > 64*1024 {
		return c, errors.New("invalid configuration")
	}
	d := json.NewDecoder(bytes.NewReader(raw))
	d.DisallowUnknownFields()
	target := &c
	if err = d.Decode(&target); err != nil || target == nil {
		return c, errors.New("invalid configuration")
	}
	if err = d.Decode(new(any)); err != io.EOF {
		return c, errors.New("invalid configuration")
	}
	if strings.TrimSpace(c.DataDir) == "" {
		return c, errors.New("data_dir must not be empty")
	}
	if !filepath.IsAbs(c.DataDir) {
		c.DataDir = filepath.Join(filepath.Dir(path), c.DataDir)
	}
	return c, c.Validate()
}

func (c Config) Validate() error {
	host, port, err := net.SplitHostPort(c.Listen)
	ip := net.ParseIP(host)
	n, pErr := strconv.Atoi(port)
	if err != nil || ip == nil || !ip.IsLoopback() || pErr != nil || n < 1 || n > 65535 {
		return errors.New("listen must be a literal loopback IP and port")
	}
	if strings.TrimSpace(c.DataDir) == "" || c.MaxPending < 1 || c.MaxPending > 1000000 || c.MaxStorageBytes < MaxBodyBytes+recordCharge || c.RetentionDays < 1 || c.RetentionDays > 3650 || c.MaxConcurrentRequests < 1 || c.MaxConcurrentRequests > 1024 {
		return errors.New("invalid inbox limits")
	}
	if err := dispatch.Validate(c.Worker); err != nil {
		return errors.New("invalid worker configuration")
	}
	return nil
}

// Secrets are deliberately excluded from Config and all status output.
type Secrets struct {
	Key    []byte
	Bearer string
}

func LoadSecrets() (Secrets, error) {
	var s Secrets
	value := os.Getenv("AURORA_WEBHOOK_SECRET")
	key, err := base64.StdEncoding.Strict().DecodeString(strings.TrimPrefix(value, "whsec_"))
	if err != nil || len(key) != 32 || bytes.ContainsAny([]byte(value), "\r\n") {
		return s, errors.New("AURORA_WEBHOOK_SECRET must encode a 32-byte key")
	}
	s.Key = key
	s.Bearer = os.Getenv("AURORA_BEARER_TOKEN")
	if s.Bearer != "" && (len(s.Bearer) < 32 || len(s.Bearer) > 4096 || strings.ContainsAny(s.Bearer, " \t\r\n")) {
		return Secrets{}, errors.New("AURORA_BEARER_TOKEN must be a whitespace-free token of at least 32 bytes")
	}
	return s, nil
}
