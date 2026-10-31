package inbox

import (
	"bytes"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"fmt"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

func testConfig(t *testing.T) Config {
	t.Helper()
	dir, e := filepath.EvalSymlinks(t.TempDir())
	if e != nil {
		t.Fatal(e)
	}
	c := DefaultConfig()
	c.DataDir = filepath.Join(dir, "data")
	return c
}
func testStore(t *testing.T) *Store {
	t.Helper()
	s, e := Open(testConfig(t))
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { s.Close() })
	return s
}
func synthetic(id string, test bool) []byte {
	return []byte(fmt.Sprintf(`{"id":%q,"type":"meeting.completed","schema_version":"1.0","timestamp":"2026-01-01T00:00:00Z","test":%t,"data":{"content_scope":"transcript","meeting":{"id":"00000000-0000-4000-8000-000000000001","title":"Synthetic meeting","started_at":null,"ended_at":null,"duration_seconds":null,"languages":[]},"speakers":[],"source":{"app":"Aurora","app_version":"synthetic","platform":"macos"},"transcript":{"text":"Entirely synthetic content.","turns":[]}}}`, id, test))
}

var testKey = bytes.Repeat([]byte{0x42}, 32)

func signedRequest(id string, body []byte, now time.Time) *http.Request {
	timestamp := strconv.FormatInt(now.Unix(), 10)
	mac := hmac.New(sha256.New, testKey)
	mac.Write([]byte(id + "." + timestamp + "."))
	mac.Write(body)
	r := httptest.NewRequest(http.MethodPost, "http://127.0.0.1/webhooks/aurora", bytes.NewReader(body))
	r.Header.Set("webhook-id", id)
	r.Header.Set("webhook-timestamp", timestamp)
	r.Header.Set("webhook-signature", "v1,"+base64.StdEncoding.EncodeToString(mac.Sum(nil)))
	return r
}
func response(h http.Handler, r *http.Request) int {
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w.Code
}

func TestSignedDeliveryDeduplicatesAndConflicts(t *testing.T) {
	s := testStore(t)
	now := time.Now()
	h := handler(s, Secrets{Key: testKey}, func() time.Time { return now })
	body := synthetic("evt_delivery", false)
	for i := range 2 {
		if got := response(h, signedRequest("evt_delivery", body, now)); got != 204 {
			t.Fatalf("delivery %d: %d", i, got)
		}
	}
	changed := bytes.Replace(body, []byte("Synthetic meeting"), []byte("Different meeting"), 1)
	if got := response(h, signedRequest("evt_delivery", changed, now)); got != 409 {
		t.Fatalf("conflict: %d", got)
	}
	if got := response(h, signedRequest("evt_delivery", []byte("malformed changed bytes"), now)); got != 409 {
		t.Fatalf("malformed conflicting retry: %d", got)
	}
	records, e := s.Status()
	if e != nil || len(records) != 1 || records[0].State != "pending" {
		t.Fatalf("status: %+v %v", records, e)
	}
}

func TestSignatureBeforeParsingAndTimestampWindow(t *testing.T) {
	s := testStore(t)
	now := time.Now()
	h := handler(s, Secrets{Key: testKey}, func() time.Time { return now })
	bad := signedRequest("evt_bad", []byte("not JSON"), now)
	bad.Header.Set("webhook-signature", "v1,invalid")
	if got := response(h, bad); got != 401 {
		t.Fatalf("unverified malformed JSON: %d", got)
	}
	if got := response(h, signedRequest("evt_bad", []byte("not JSON"), now)); got != 400 {
		t.Fatalf("verified malformed JSON: %d", got)
	}
	for _, offset := range []int{-301, 301} {
		r := signedRequest("evt_skew", synthetic("evt_skew", false), now.Add(time.Duration(offset)*time.Second))
		if got := response(h, r); got != 401 {
			t.Fatalf("skew %d: %d", offset, got)
		}
	}
	for _, offset := range []int{-300, 300} {
		id := fmt.Sprintf("evt_boundary_%d", offset)
		if got := response(h, signedRequest(id, synthetic(id, false), now.Add(time.Duration(offset)*time.Second))); got != 204 {
			t.Fatalf("boundary %d: %d", offset, got)
		}
	}
}

func TestUniqueHeadersRotationBearerAndBodyBound(t *testing.T) {
	s := testStore(t)
	now := time.Now()
	h := handler(s, Secrets{Key: testKey, Bearer: strings.Repeat("x", 32)}, func() time.Time { return now })
	makeRequest := func() *http.Request {
		r := signedRequest("evt_auth", synthetic("evt_auth", false), now)
		r.Header.Set("Authorization", "Bearer "+strings.Repeat("x", 32))
		return r
	}
	for _, header := range []string{"webhook-id", "webhook-timestamp", "webhook-signature", "Authorization"} {
		r := makeRequest()
		r.Header.Add(header, r.Header.Get(header))
		if got := response(h, r); got != 401 {
			t.Fatalf("duplicate %s: %d", header, got)
		}
	}
	r := makeRequest()
	r.Header.Del("Authorization")
	if got := response(h, r); got != 401 {
		t.Fatalf("missing bearer: %d", got)
	}
	r = makeRequest()
	r.Header.Set("webhook-signature", "v1,"+base64.StdEncoding.EncodeToString(make([]byte, 32))+" "+r.Header.Get("webhook-signature"))
	if got := response(h, r); got != 204 {
		t.Fatalf("rotation: %d", got)
	}
	r = makeRequest()
	r.ContentLength = MaxBodyBytes + 1
	if got := response(h, r); got != 413 {
		t.Fatalf("body limit: %d", got)
	}
	r = makeRequest()
	r.Header.Set("webhook-timestamp", "+"+r.Header.Get("webhook-timestamp"))
	if got := response(h, r); got != 401 {
		t.Fatalf("nondecimal timestamp: %d", got)
	}
}

func TestEnvelopeValidationAndAdditiveCompatibility(t *testing.T) {
	base := synthetic("evt_schema", false)
	cases := []struct {
		name  string
		body  []byte
		valid bool
	}{
		{"base", base, true},
		{"additive", bytes.Replace(base, []byte(`"schema_version":"1.0"`), []byte(`"schema_version":"1.24","new_field":{"anything":true}`), 1), true},
		{"major", bytes.Replace(base, []byte(`"schema_version":"1.0"`), []byte(`"schema_version":"2.0"`), 1), false},
		{"scope", bytes.Replace(base, []byte(`"content_scope":"transcript"`), []byte(`"content_scope":"summary"`), 1), false},
		{"null transcript", bytes.Replace(base, []byte(`"transcript":{"text":"Entirely synthetic content.","turns":[]}`), []byte(`"transcript":null`), 1), false},
		{"missing meeting field", bytes.Replace(base, []byte(`"started_at":null,`), nil, 1), false},
		{"null test", bytes.Replace(base, []byte(`"test":false`), []byte(`"test":null`), 1), false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, e := parseEvent(tc.body, "evt_schema")
			if (e == nil) != tc.valid {
				t.Fatalf("valid=%v, error=%v", tc.valid, e)
			}
		})
	}
}
