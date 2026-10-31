package dispatch

import (
	"errors"
	"net/url"
	"os"
	"strings"
)

// Policy arguments are fixed. Only Hermes model/provider routing comes from
// explicit operator-owned environment; event content never controls argv.
func arguments(agent string) ([]string, error) {
	switch agent {
	case "claude":
		return []string{
			"--print", "--safe-mode", "--restricted",
			"--input-format", "text", "--output-format", "json",
			"--tools", "", "--disallowedTools", "mcp__*",
			"--strict-mcp-config", "--mcp-config", `{"mcpServers":{}}`,
			"--disable-slash-commands", "--no-chrome",
			"--permission-mode", "dontAsk", "--permission-prompts", "none",
			"--no-session-persistence",
		}, nil
	case "codex":
		return []string{
			"exec", "--sandbox", "read-only", "--ignore-user-config", "--ignore-rules",
			"--skip-git-repo-check", "--ephemeral", "--color", "never", "--json",
			"--config", `approval_policy="never"`,
			"--config", `approvals_reviewer="user"`,
			"--config", "features.shell_tool=false",
			"--config", "features.hooks=false",
			"--config", "features.apps=false",
			"--config", "features.multi_agent=false",
			"--config", "features.plugins=false",
			"--config", `web_search="disabled"`,
			"-",
		}, nil
	case "hermes":
		model, provider, endpoint, err := hermesRoute()
		if err != nil {
			return nil, err
		}
		args := []string{
			"chat", "--query-file", "-", "--oneshot", "--quiet",
			"--safe-mode", "--toolsets", "",
			"--model", model, "--provider", provider,
		}
		if provider == "custom" {
			// The endpoint is explicit; never fall back to a configured cloud route.
			args = append(args, "--base-url", endpoint)
		}
		return args, nil
	default:
		return nil, errors.New("worker agent must be claude, codex, or hermes")
	}
}

func hermesRoute() (model, provider, endpoint string, err error) {
	model = strings.TrimSpace(os.Getenv("HERMES_INFERENCE_MODEL"))
	provider = strings.ToLower(strings.TrimSpace(os.Getenv("HERMES_INFERENCE_PROVIDER")))
	if model == "" || provider == "" || provider == "auto" {
		return "", "", "", errors.New("Hermes requires explicit HERMES_INFERENCE_MODEL and HERMES_INFERENCE_PROVIDER (not auto)")
	}
	if provider == "custom" {
		endpoint = strings.TrimSpace(os.Getenv("CUSTOM_BASE_URL"))
		if endpoint == "" {
			return "", "", "", errors.New("Hermes custom provider requires an explicitly approved CUSTOM_BASE_URL")
		}
		parsed, parseErr := url.Parse(endpoint)
		if parseErr != nil || (parsed.Scheme != "http" && parsed.Scheme != "https") || parsed.Hostname() == "" ||
			parsed.User != nil || parsed.RawQuery != "" || parsed.ForceQuery || strings.Contains(endpoint, "#") {
			return "", "", "", errors.New("Hermes CUSTOM_BASE_URL must be an HTTP(S) endpoint with a host and without credentials, query, or fragment")
		}
	}
	return model, provider, endpoint, nil
}
