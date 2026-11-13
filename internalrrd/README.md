# internalrrd

The service component of InternalRRD, an in-container rolling deployment project.

## Current state

`internalrrd` loads a TOML configuration file, manages application instances, serves the management protocol, and runs an L4 proxy for blue-green traffic switching.

It currently supports:

- Loading releases from signed deployment messages.
- Downloading artifacts over HTTP, HTTPS, or `file://`.
- Verifying artifact SHA-256 checksums.
- Extracting `tar.gz` releases into versioned directories.
- Starting multiple instances per release on private ports.
- Switching TCP and UDP traffic to a new release without restarting the container.
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
8. Start `application.instances` new processes on free private ports.
9. Wait for `application.startup_grace_secs` and confirm the new processes are still alive.
10. Switch the proxy to the new instance ports.
11. Stop the old instances with `SIGTERM`, then `SIGKILL` after the shutdown timeout.
12. Update the `current` release link.

If a new instance exits during the startup grace period, the deployment is rejected, the new instances are stopped, their ports are released, and the old release keeps serving.

## L4 proxy

Each `proxy.listeners` entry binds a public TCP or UDP port and forwards it to the active release.

- TCP connections are distributed round-robin across the release instances.
- TCP connections that are already open continue to the old release until they close.
- UDP sessions are tracked per client address and distributed round-robin.
- UDP sessions expire after `proxy.udp_session_timeout_secs`.
- New TCP connections and UDP sessions use the new release immediately after the switch.

Backends are always `127.0.0.1:<instance port>`.
Applications receive their private port through placeholders in `application.args` and `application.environment`.

## Command templates

The following placeholders are expanded for every instance:

| Placeholder        | Value                                  |
| ------------------ | -------------------------------------- |
| `{port}`           | The private instance port.             |
| `{address}`        | `127.0.0.1`.                           |
| `{listen_address}` | `127.0.0.1:<port>`.                    |
| `{release_dir}`    | The extracted release directory.       |

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

See [`config.example.toml`](config.example.toml) for a complete example.

```toml
[management]
port = 54

[application]
name = "example"
command = "{release_dir}/bin/example"
args = ["--listen", "{listen_address}"]
working_directory = "{release_dir}"
shutdown_timeout_secs = 30
instances = 2
startup_grace_secs = 2
release_directory = "/var/lib/internalrrd/releases"
max_artifact_bytes = 536870912

[application.environment]
LISTEN_ADDR = "{listen_address}"

[trust]
keys = [
  { id = "release-2026", algorithm = 1, public_key = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a" },
]

[proxy]
udp_session_timeout_secs = 30

[proxy.ports]
start = 30000
end = 31000

[[proxy.listeners]]
protocol = "tcp"
bind = "0.0.0.0"
port = 8080

[[proxy.listeners]]
protocol = "udp"
bind = "0.0.0.0"
port = 53
```

- `management.port` is the port reserved for the management protocol and defaults to `54`.
- `application.name` is used in logs and must match the deployment application name.
- `application.command` is the executable or entrypoint template for each instance.
- `application.args` are optional arguments passed to the executable.
- `application.working_directory` is optional and defaults to the release directory when a release is active.
- `application.environment` holds optional environment variables passed to every instance.
- `application.shutdown_timeout_secs` controls how long an instance has to exit after `SIGTERM` before it is killed.
- `application.instances` is the number of instances started per release and defaults to `1`.
- `application.startup_grace_secs` is how long new instances must stay alive before traffic switches and defaults to `2`.
- `application.release_directory` is where artifacts and releases are stored.
- `application.max_artifact_bytes` limits downloaded artifact size and defaults to 512 MiB.
- `trust.keys` lists trusted release signing keys.
- `proxy.listeners` defines public TCP and UDP listeners.
- `proxy.ports` defines the private instance port range; ephemeral ports are used when absent.
- `proxy.udp_session_timeout_secs` controls UDP session expiry.
