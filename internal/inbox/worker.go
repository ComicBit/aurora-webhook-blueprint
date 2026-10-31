package inbox

import (
	"context"
	"errors"
	"time"

	"github.com/ComicBit/aurora-webhook-blueprint/internal/dispatch"
	bolt "go.etcd.io/bbolt"
)

// Run dispatches one claimed event at a time. An ambiguous attempt is never retried automatically.
// Only one Run may execute for a Store; Serve owns that lifecycle in the binary.
func (s *Store) Run(ctx context.Context) error {
	return s.run(ctx, func(ctx context.Context, raw []byte) ([]byte, error) { return dispatch.Run(ctx, s.config.Worker, raw) })
}

func (s *Store) run(ctx context.Context, execute func(context.Context, []byte) ([]byte, error)) error {
	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()
	s.notify()
	for {
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
			if err := s.db.Update(func(tx *bolt.Tx) error { return prune(tx, time.Now().Unix()) }); err != nil {
				return errors.New("retention transaction failed")
			}
		case <-s.wake:
			if s.config.Worker.Agent == "" {
				continue
			}
			for {
				if ctx.Err() != nil {
					return nil
				}
				record, raw, err := s.claim()
				if err != nil {
					return errors.New("claim transaction failed")
				}
				if record == nil {
					break
				}
				timeout := time.Duration(s.config.Worker.TimeoutSeconds) * time.Second
				if timeout <= 0 {
					timeout = 5 * time.Minute
				}
				attemptCtx, cancel := context.WithTimeout(ctx, timeout)
				output, executionErr := execute(attemptCtx, raw)
				// Cancellation is ambiguous even if a subprocess reports success while shutting down.
				success := executionErr == nil && attemptCtx.Err() == nil && int64(len(output)) <= maxResultBytes
				cancel()
				if !success {
					output = nil
				}
				if err = s.finish(record.ID, output, success); err != nil {
					return errors.New("completion transaction failed")
				}
			}
		}
	}
}
