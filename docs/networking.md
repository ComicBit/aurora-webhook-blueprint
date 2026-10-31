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

Start the receiver bound to `127.0.0.1:8787`, then run:

```sh
tailscale serve --bg http://127.0.0.1:8787
tailscale serve status
```

Use the actual hostname reported by Serve, with this path:

```text
https://<machine>.<tailnet>.ts.net/webhooks/aurora
```

Serve defaults to HTTPS and proxies to the loopback HTTP backend. `--bg`
persists Serve configuration across Tailscale restarts/reboots; it does **not**
start or supervise the receiver. Have an authorized administrator restrict
TCP 443 access to the intended sending identities/devices. Grants are additive:
adding a narrow rule does not override an existing broad grant or ACL. Review
the entire effective policy. Port 8787 stays loopback-only.

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

Aurora's current validator permits HTTPS DNS names such as `*.ts.net` and
rejects literal private/tailnet IPs. **End-to-end delivery from Aurora iOS and
macOS over this private setup remains unverified.** Both apps need their own
Send test while connected to the intended VPN. A successful local signed smoke
test or a syntactically accepted URL is not this evidence. No public relay is
part of this blueprint.

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
