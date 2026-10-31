// aurora-webhook is a loopback-only Aurora webhook inbox and optional single worker.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"

	"github.com/ComicBit/aurora-webhook-blueprint/internal/inbox"
)

func main() {
	if err := run(os.Args[1:], os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run(args []string, out io.Writer) error {
	if len(args) == 0 {
		return errors.New("usage: aurora-webhook serve|status|show|result|retry|delete -config PATH [--acknowledge-duplicate-risk] [ID]")
	}
	command := args[0]
	if command != "serve" && command != "status" && command != "show" && command != "result" && command != "retry" && command != "delete" {
		return errors.New("unknown command")
	}
	flags := flag.NewFlagSet(command, flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	path := flags.String("config", "config.json", "configuration file")
	acknowledge := flags.Bool("acknowledge-duplicate-risk", false, "acknowledge retry may duplicate external actions")
	if err := flags.Parse(args[1:]); err != nil {
		return errors.New("invalid command arguments")
	}
	if command == "retry" || command == "delete" || command == "show" || command == "result" {
		if flags.NArg() != 1 {
			return errors.New("command requires exactly one event ID")
		}
	} else if flags.NArg() != 0 {
		return errors.New("unexpected command arguments")
	}
	if command == "retry" && !*acknowledge {
		return errors.New("retry requires --acknowledge-duplicate-risk; external actions may already have happened")
	}
	if command != "retry" && *acknowledge {
		return errors.New("duplicate-risk acknowledgement is only valid with retry")
	}
	config, err := inbox.LoadConfig(*path)
	if err != nil {
		return err
	}
	var secrets inbox.Secrets
	if command == "serve" {
		secrets, err = inbox.LoadSecrets()
		if err != nil {
			return err
		}
	}
	store, err := inbox.Open(config)
	if err != nil {
		return err
	}
	defer store.Close()
	switch command {
	case "serve":
		ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
		defer stop()
		fmt.Fprintln(out, "starting loopback receiver")
		return inbox.Serve(ctx, store, secrets)
	case "status":
		records, err := store.Status()
		if err != nil {
			return errors.New("cannot read inbox status")
		}
		return json.NewEncoder(out).Encode(records)
	case "show", "result":
		var raw []byte
		if command == "show" {
			raw, err = store.Payload(flags.Arg(0))
		} else {
			raw, err = store.Result(flags.Arg(0))
		}
		if err != nil {
			return err
		}
		n, err := out.Write(raw)
		if err != nil {
			return err
		}
		if n != len(raw) {
			return io.ErrShortWrite
		}
	case "retry":
		if err = store.Retry(flags.Arg(0)); err != nil {
			return err
		}
		fmt.Fprintln(out, "event queued; retry may duplicate external actions")
	case "delete":
		if err = store.Delete(flags.Arg(0)); err != nil {
			return err
		}
		fmt.Fprintln(out, "payload removed; deduplication tombstone retained")
	}
	return nil
}
