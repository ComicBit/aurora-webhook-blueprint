# Private HTTPS ingress

Reviewed 2026-10-31 against the primary sources below. Private transport and
model inference location are separate decisions. A local agent CLI can still
send content to a cloud provider.

## Use an existing VPN when suitable

A trusted existing VPN is sufficient if the sending iPhone/Mac can resolve and
reach a private HTTPS DNS hostname, its certificate is trusted, and access is
limited to the intended senders. Configure your existing HTTPS reverse proxy
to reach the loopback receiver and preserve the body and signature headers.
Do not assume a custom hostname can simply replace a Tailscale Serve hostname.
This is deployment guidance, not an app compatibility result.

## No VPN yet: recommend Tailscale Serve

Tailscale provides an encrypted private overlay between authenticated devices.
Packets may traverse the internet and DERP relays; DERP cannot decrypt the
WireGuard payload. It is not offline delivery, a model sandbox or a guarantee
that data never uses third-party infrastructure.

Obtain permission before installing, signing in, enabling HTTPS or changing
tailnet policy. Authenticate both the receiving host and the actual sending
device, approve OS VPN permissions and device registration as required. Enable
MagicDNS and tailnet HTTPS. Choose a nonsensitive host name **before** issuing
a certificate: the full certificate hostname is published in public Certificate
Transparency logs. This exposes name metadata, not the webhook body, and does
not make the service public.

Start the receiver bound to `127.0.0.1:8787`. Inspect routes **before** changing
Serve; never reset or overwrite existing services:

```sh
tailscale serve status
```

If HTTPS 443 is free and authorized:

```sh
tailscale serve --bg --https=443 http://127.0.0.1:8787
tailscale serve status
```

If occupied, choose an explicit free HTTPS port (for example 8443, only after
checking it is unused), using `--https=8443` instead. If loopback 8787 is also
occupied, choose a free receiver port in config, review/apply it offline and
use that same backend port. Do not displace another listener.

Use the **actual** HTTPS hostname and port reported by Serve, with the complete
webhook path; this is a placeholder, not a URL to paste unchanged:

```text
https://<machine>.<tailnet>.ts.net:8443/webhooks/aurora
```

For default HTTPS use port 443 instead. Serve proxies to loopback HTTP.
`--bg` persists Serve configuration across Tailscale restarts/reboots; it does
**not** start or supervise the receiver. Have an authorized administrator
restrict the selected HTTPS port to intended sending identities/devices.
Grants are additive: a narrow rule does not override broad grants/ACLs. Review
the entire effective policy. The backend port stays loopback-only.

`GET /healthz` is a health probe, not Aurora's delivery URL. Aurora sends
`POST /webhooks/aurora`. A 405 suggests route/method mismatch: check the full
hostname, port, path and method before credentials. A 401 on the correct POST
calls for checking signing/bearer copies and timestamp freshness. Never guess
another URL, regenerate the app key blindly or turn off protections.

**Never use Funnel as a fallback.** Serve is tailnet-private; Funnel exposes a
service to the public internet. The last mode configured on a port determines
its exposure. If private delivery fails, diagnose connectivity, do not make
the receiver public or weaken Aurora's URL validation.

## Availability and compatibility checks

Keep the host awake, receiver supervised, tailnet connected and node credentials
valid. A webhook can start work in a running service, not power on a sleeping
computer. Linux `tailscaled` commonly runs as a system service. macOS GUI
variants depend on login/session behavior; official macOS-variants and
unattended guidance differ on daemon arrangements. Do not assume a universal
supported pre-login recipe. Validate reboot/login behavior for your deployment.

Aurora's validator permits HTTPS DNS names such as `*.ts.net` and rejects
literal private/tailnet IPs. A user reported a successful manual app test after
correcting the URL; the platform was not specified. This is not independently
measured iOS/macOS coverage. Each intended app/device needs its own Send test
on the intended VPN. A loopback signed check or accepted URL is not that proof.
No public relay is part of this blueprint.

## Primary sources

- [Serve CLI](https://tailscale.com/docs/reference/tailscale-cli/serve) and
  [Serve overview](https://tailscale.com/docs/features/tailscale-serve): commands,
  private access, persistence, certificate provisioning and limitations.
- [Funnel](https://tailscale.com/docs/features/tailscale-funnel): public exposure.
- [MagicDNS](https://tailscale.com/docs/features/magicdns) and
  [HTTPS certificates](https://tailscale.com/docs/how-to/set-up-https-certificates):
  prerequisites and certificate-transparency disclosure.
- [DERP](https://tailscale.com/docs/reference/derp-servers): encrypted relaying.
- [Grants](https://tailscale.com/docs/reference/syntax/grants) and
  [tailnet policy](https://tailscale.com/docs/features/tailnet-policy-file):
  additive rules and administrative roles.
- [Auth keys](https://tailscale.com/docs/features/access-control/auth-keys):
  registration, expiry, device approval and Tailnet Lock.
- [Unattended operation](https://tailscale.com/docs/how-to/run-unattended) and
  [macOS variants](https://tailscale.com/docs/concepts/macos-variants): deployment
  caveats; verify current OS-specific support before installing a daemon.
