# internalrrd

The service component of InternalRRD, an in-container rolling deployment project.

## Current state

`internalrrd` loads a TOML configuration file, manages application instances, serves the management protocol, and runs an L4 proxy for blue-green traffic switching.

It currently supports:

- Loading releases from signed deployment messages.
- Downloading artifacts over HTTP, HTTPS, or `file://`.
- Verifying artifact SHA-256 checksums.
- Extracting `tar.gz` releases into versioned directories.
- Starting multiple instances per release on named private ports.
- Switching TCP and UDP traffic to a new release without restarting the container.
- Routing each public listener to a different application port.
- Gracefully stopping the previous release after the switch.

## Management protocol

`internalrrd` listens on TCP port `54` by default and binds to `0.0.0.0`.
The port is configurable with `management.port`.
Binding port `54` may require root or `CAP_NET_BIND_SERVICE` inside the container.

It handles these protocol requests:

- `Hello`: returns the application name, capabilities, and deployed version.
- `Status`: returns the running application instances and deployed version.
- `Metrics`: returns InternalRRD, application, and per-instance metrics.
- `Deploy`: verifies, installs, and activates a signed release.

The wire format and message definitions live in [`protocol/README.md`](../protocol/README.md).

## Deployment flow

A `Deploy` request is processed in this order:

1. Verify the Ed25519 signature against a configured trusted key.
2. Check that the deployment targets this application.
3. Reject an already deployed version.
4. Reject a deployment whose `issued_at_unix` is not newer than the active deployment.
5. Download the artifact and enforce `application.max_artifact_bytes`.
6. Verify the artifact SHA-256 and size.
7. Extract the `tar.gz` artifact into `releases/<version>-<sha12>/`.
8. Start `application.instances` new processes with a private port for every `application.ports` entry.
9. Wait for `application.startup_grace_secs` and confirm the new processes are still alive.
10. Switch the proxy backend map so every listener uses the new instances.
11. Stop the old instances with `SIGTERM`, then `SIGKILL` after the shutdown timeout.
12. Release the old private ports and update the `current` release link.

If a new instance exits during the startup grace period, the deployment is rejected, the new instances are stopped, their ports are released, and the old release keeps serving.

## L4 proxy

Each `proxy.listeners` entry binds a public TCP or UDP port and forwards it to one named application port.
The listener `backend` selects the name, with `default` used when it is omitted.

- TCP connections are distributed round-robin across the instances of that application port.
- TCP connections that are already open continue to the old release until they close.
- UDP sessions are tracked per client address and distributed round-robin.
- UDP sessions expire after `proxy.udp_session_timeout_secs`.
- New TCP connections and UDP sessions use the new release immediately after the switch.

Backends are always `127.0.0.1:<private port>`.
Applications receive their private ports through placeholders in `application.command`, `application.args`, and `application.environment`.
A named port can serve both TCP and UDP on the same private port number, which DNS requires.

## Command templates

The following placeholders are expanded for every instance:

| Placeholder             | Value                                           |
| ----------------------- | ----------------------------------------------- |
| `{port}`                | The private `default` port.                     |
| `{address}`             | `127.0.0.1`.                                    |
| `{listen_address}`      | `127.0.0.1:<default port>`.                     |
| `{port:name}`           | The private port of the named application port. |
| `{address:name}`        | `127.0.0.1`, after checking the name exists.    |
| `{listen_address:name}` | `127.0.0.1:<named port>`.                       |
| `{release_dir}`         | The extracted release directory.                |

The unnamed forms resolve to the application port named `default`.
With multiple application ports, use the named forms.
See [`CONFIG.md`](CONFIG.md#template-placeholders) for the complete rules.

`{release_dir}` is available for deployments.
When `application.command` contains `{release_dir}` and no release is installed, `internalrrd` starts no instances and waits for the first deployment.
After a successful deployment, the `current` symlink lets a restarted `internalrrd` start from the most recent release.

## Release directory layout

The `application.release_directory` contains:

- `downloads/<sha256>.tar.gz`: verified artifact downloads.
- `releases/<version>-<sha12>/`: extracted releases.
- `releases/<version>-<sha12>/.internalrrd-release`: version and issue time used for restart checks.
- `tmp/`: in-progress extraction directories.
- `current`: a symlink to the active release.

## Trust

`trust.keys` lists the public keys that may sign deployments.
Only Ed25519 is currently supported, with algorithm identifier `1`.
A deployment whose key identifier is not configured is rejected.

## Metrics

Metrics include:

- InternalRRD CPU, resident memory, and storage usage.
- Application aggregate metrics and per-instance metrics.
- The deployed application version.
- Zeroed traffic counters until request instrumentation is added.

CPU usage is the average since process start.
Storage usage is read from the filesystem that contains the release directory.

## Limitations

- Health checks use process liveness during the startup grace period, not application-level readiness.
- Traffic counters remain at zero until request instrumentation is added.
- UDP sessions that are already open stay on the old release until they idle out.
- Artifact URLs support `http://`, `https://`, and `file://`.

## Build and run

From the repository root, build or run it with:

```sh
cargo build -p internalrrd
cargo run -p internalrrd -- --config internalrrd/config.example.toml
```

The default configuration path is `/etc/internalrrd/config.toml`.

## Configuration

[`CONFIG.md`](CONFIG.md) is the complete reference for every field, default, validation rule, and placeholder.
[`config.example.toml`](config.example.toml) contains a working multi-port configuration for NTP, DNS, HTTP, and HTTPS.

In short:

- `management.port` is the management protocol port and defaults to `54`.
- `application.name` must match the deployment application name.
- `application.command` is the entrypoint template for each instance.
- `application.ports` declares the named private ports that every instance receives.
- `trust.keys` lists trusted release signing keys.
- `proxy.listeners` maps public TCP and UDP ports to named application ports.
- `proxy.ports` defines the private port range; ephemeral ports are used when absent.
- `proxy.udp_session_timeout_secs` controls UDP session expiry.
