package inbox

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"io"
	"log"
	"net"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Handler bounds concurrent request bodies and authenticates before JSON parsing.
func Handler(store *Store, secrets Secrets) http.Handler {
	return handler(store, secrets, time.Now)
}

func handler(store *Store, secrets Secrets, now func() time.Time) http.Handler {
	slots := make(chan struct{}, store.config.MaxConcurrentRequests)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("X-Content-Type-Options", "nosniff")
		if r.URL.Path != "/webhooks/aurora" {
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		if r.Method != http.MethodPost {
			w.Header().Set("Allow", "POST")
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		select {
		case slots <- struct{}{}:
			defer func() { <-slots }()
		default:
			w.Header().Set("Retry-After", "30")
			http.Error(w, "receiver busy", http.StatusServiceUnavailable)
			return
		}
		id, ok := uniqueHeader(r, "webhook-id")
		if !ok || !validID(id) {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		timestamp, ok := uniqueHeader(r, "webhook-timestamp")
		if !ok || len(timestamp) > 20 || timestamp == "" {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		for _, c := range timestamp {
			if c < '0' || c > '9' {
				http.Error(w, "unauthorized", http.StatusUnauthorized)
				return
			}
		}
		seconds, e := strconv.ParseInt(timestamp, 10, 64)
		current := now().Unix()
		if e != nil || seconds < current-300 || seconds > current+300 {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		signature, ok := uniqueHeader(r, "webhook-signature")
		if !ok || len(signature) > 8192 {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		if secrets.Bearer != "" {
			authorization, ok := uniqueHeader(r, "Authorization")
			actual := sha256.Sum256([]byte(authorization))
			expected := sha256.Sum256([]byte("Bearer " + secrets.Bearer))
			if !ok || subtle.ConstantTimeCompare(actual[:], expected[:]) != 1 {
				http.Error(w, "unauthorized", http.StatusUnauthorized)
				return
			}
		}
		if r.ContentLength > MaxBodyBytes {
			http.Error(w, "request too large", http.StatusRequestEntityTooLarge)
			return
		}
		r.Body = http.MaxBytesReader(w, r.Body, MaxBodyBytes)
		raw, e := io.ReadAll(r.Body)
		if e != nil {
			var limit *http.MaxBytesError
			if errors.As(e, &limit) {
				http.Error(w, "request too large", http.StatusRequestEntityTooLarge)
			} else {
				http.Error(w, "invalid request", http.StatusBadRequest)
			}
			return
		}
		mac := hmac.New(sha256.New, secrets.Key)
		mac.Write([]byte(id))
		mac.Write([]byte("."))
		mac.Write([]byte(timestamp))
		mac.Write([]byte("."))
		mac.Write(raw)
		expected := mac.Sum(nil)
		matched := 0
		for _, entry := range strings.Fields(signature) {
			version, encoded, found := strings.Cut(entry, ",")
			if !found || version != "v1" {
				continue
			}
			decoded, e := base64.StdEncoding.Strict().DecodeString(encoded)
			if e != nil {
				continue
			}
			matched |= subtle.ConstantTimeCompare(decoded, expected)
		}
		if matched != 1 {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		_, e = store.acceptEvent(id, raw, now())
		switch {
		case errors.Is(e, ErrConflict):
			http.Error(w, "event conflict", http.StatusConflict)
		case errors.Is(e, errEvent):
			http.Error(w, "invalid event", http.StatusBadRequest)
		case errors.Is(e, ErrFull):
			w.Header().Set("Retry-After", "30")
			http.Error(w, "inbox full", http.StatusServiceUnavailable)
		case e != nil:
			http.Error(w, "storage unavailable", http.StatusServiceUnavailable)
		default:
			w.WriteHeader(http.StatusNoContent)
		}
	})
}

func uniqueHeader(r *http.Request, name string) (string, bool) {
	values := r.Header.Values(name)
	if len(values) != 1 || values[0] == "" {
		return "", false
	}
	return values[0], true
}

// Serve owns the listener and worker lifecycle; the caller closes Store after it returns.
func Serve(ctx context.Context, store *Store, secrets Secrets) error {
	if len(secrets.Key) != 32 {
		return errors.New("invalid webhook signing key")
	}
	listener, err := net.Listen("tcp", store.config.Listen)
	if err != nil {
		return errors.New("cannot bind configured loopback listener")
	}
	return serveListener(ctx, listener, store, secrets)
}

func serveListener(ctx context.Context, listener net.Listener, store *Store, secrets Secrets) error {
	var err error
	runCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	workerDone := make(chan error, 1)
	go func() { workerDone <- store.Run(runCtx) }()
	server := &http.Server{Handler: Handler(store, secrets), ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 20 * time.Second, WriteTimeout: 20 * time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 16 * 1024, ErrorLog: log.New(io.Discard, "", 0), BaseContext: func(net.Listener) context.Context { return runCtx }}
	serverDone := make(chan error, 1)
	go func() { serverDone <- server.Serve(limitConnections(listener, store.config.MaxConcurrentRequests*2)) }()
	var result error
	workerStopped := false
	select {
	case <-ctx.Done():
	case result = <-serverDone:
	case result = <-workerDone:
		workerStopped = true
	}
	cancel()
	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), 25*time.Second)
	defer shutdownCancel()
	if err = server.Shutdown(shutdownCtx); err != nil {
		server.Close()
		result = errors.New("HTTP shutdown timed out")
	}
	if !workerStopped {
		if err = <-workerDone; err != nil {
			result = err
		}
	}
	if result != nil && !errors.Is(result, http.ErrServerClosed) {
		return errors.New("receiver stopped due to an internal error")
	}
	return nil
}

type limitedListener struct {
	net.Listener
	slots chan struct{}
	done  chan struct{}
	once  sync.Once
}

func limitConnections(listener net.Listener, count int) net.Listener {
	return &limitedListener{Listener: listener, slots: make(chan struct{}, count), done: make(chan struct{})}
}
func (l *limitedListener) Accept() (net.Conn, error) {
	select {
	case l.slots <- struct{}{}:
	case <-l.done:
		return nil, net.ErrClosed
	}
	connection, err := l.Listener.Accept()
	if err != nil {
		<-l.slots
		return nil, err
	}
	return &limitedConnection{Conn: connection, release: func() { <-l.slots }}, nil
}
func (l *limitedListener) Close() error {
	l.once.Do(func() { close(l.done) })
	return l.Listener.Close()
}

type limitedConnection struct {
	net.Conn
	release func()
	once    sync.Once
}

func (c *limitedConnection) Close() error {
	err := c.Conn.Close()
	c.once.Do(c.release)
	return err
}
