//go:build !darwin && !linux

package dispatch

import (
	"errors"
	"os/exec"
)

func configureProcess(*exec.Cmd) error {
	return errors.New("agent subprocess isolation is supported only on macOS and Linux")
}

func killProcessGroup(cmd *exec.Cmd) error {
	if cmd.Process == nil {
		return nil
	}
	return cmd.Process.Kill()
}
