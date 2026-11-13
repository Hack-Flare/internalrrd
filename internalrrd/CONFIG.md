# Configuration

`internalrrd` reads a single TOML file at startup.
The default path is `/etc/internalrrd/config.toml` and can be overridden with `--config`.
The file is read once, so changing it requires a restart.
Unknown keys are rejected, which turns typos into startup errors.

This document covers every field, its default, and how the configuration drives zero-downtime rolling releases.
See [`config.example.toml`](config.example.toml) for a copy of the multi-port example below.

## Minimal configuration

```toml
[application]
name = "example"
command = "/usr/bin/example"
```

This starts one instance, allows it to use TCP and UDP on a single private port, and does not start a proxy.

## Top-level sections

| Section       | Required | Purpose                                                     |
| ------------- | -------- | ----------------------------------------------------------- |
| `management`  | No       | Management protocol listener.                               |
| `application` | Yes      | Application identity, process template, and release storage. |
| `trust`       | No       | Public keys that may sign deployments.                      |
| `proxy`       | No       | Public L4 listeners and private port allocation.            |

## `management`

| Key    | Type    | Default | Description                              |
| ------ | ------- | ------- | ---------------------------------------- |
| `port` | integer | `54`    | TCP port for the management protocol.    |

The management listener always binds `0.0.0.0`.
The management protocol is a length-prefixed postcard protocol, not HTTP.
See [`../protocol/README.md`](../protocol/README.md) for the wire format.

Port `54` is below 1024, so binding it may require root or `CAP_NET_BIND_SERVICE`.

## `application`

| Key                     | Type              | Default                        | Description                                                        |
| ----------------------- | ----------------- | ------------------------------ | ------------------------------------------------------------------ |
| `name`                  | string            | required                       | Human readable name used in logs and deployment checks.            |
| `command`               | string (path)     | required                       | Entrypoint template for every instance.                            |
| `args`                  | array of strings  | `[]`                           | Arguments passed to the entrypoint.                                |
| `working_directory`     | string (path)     | release directory when present | Working directory for every instance.                              |
| `environment`           | table of strings  | `{}`                           | Extra environment variables passed to every instance.              |
| `shutdown_timeout_secs` | integer           | `30`                           | Seconds an instance has to exit after `SIGTERM` before `SIGKILL`.   |
| `instances`             | integer           | `1`                            | Number of instances started for each release.                      |
| `startup_grace_secs`    | integer           | `2`                            | Seconds new instances must stay alive before they receive traffic. |
| `release_directory`     | string (path)     | `/var/lib/internalrrd/releases`| Directory that stores downloads and extracted releases.            |
| `max_artifact_bytes`    | integer           | `536870912` (512 MiB)          | Maximum downloaded artifact size.                                  |
| `ports`                 | array of tables   | `[]`                           | Named private ports assigned to every instance.                    |

`name` must match the `application` field in a signed deployment or the deployment is rejected.

`command`, `args`, `working_directory`, and every environment value are templates.
See [Template placeholders](#template-placeholders).

When `command` contains `{release_dir}` and no release is installed, `internalrrd` starts zero instances and waits for the first deployment.
When `command` does not contain `{release_dir}`, the first instance starts without a release directory.
Using `{release_dir}` in any other template while no release is installed fails startup.

`working_directory` defaults to the active release directory.
If no release is active and `working_directory` is unset, the instance inherits the working directory of `internalrrd`.

`max_artifact_bytes` guards the download before extraction.
It must be greater than zero.

`release_directory` contains:

- `downloads/<sha256>.tar.gz`: verified artifact downloads.
- `releases/<version>-<sha12>/`: extracted releases.
- `releases/<version>-<sha12>/.internalrrd-release`: version and issue time.
- `tmp/`: in-progress extraction directories.
- `current`: a symlink to the active release.

### `application.ports`

When `application.ports` is empty, `internalrrd` allocates one implicit private port per instance.
The implicit port is named `default` and accepts both TCP and UDP.

Each entry declares one named private port:

| Key         | Type             | Description                                                        |
| ----------- | ---------------- | ------------------------------------------------------------------ |
| `name`      | string           | Unique name used by placeholders and proxy listener backends.      |
| `protocols` | array of strings | Transports that may reach the port, either `"tcp"` or `"udp"`.     |

Every instance receives one private port per configured name.
A port name with both protocols uses the same port number for TCP and UDP, which DNS requires.

Port names must be non-empty and unique.
Each entry must declare at least one protocol and must not repeat a protocol.

### `application.environment`

A table of environment variables added to the process environment.
Values are templates and may reference named ports.

```toml
[application.environment]
RUST_LOG = "info"
DNS_LISTEN_ADDR = "{listen_address:dns}"
```

When `application.ports` is empty, use `{listen_address}` instead of a named form.

## `trust`

| Key    | Type            | Default | Description                                    |
| ------ | --------------- | ------- | ---------------------------------------------- |
| `keys` | array of tables | `[]`    | Public keys that may sign deployments.         |

When `keys` is empty, every deployment is rejected.

### `trust.keys`

| Key          | Type    | Default  | Description                                          |
| ------------ | ------- | -------- | ---------------------------------------------------- |
| `id`         | string  | required | Identifier referenced by a signed deployment.        |
| `algorithm`  | integer | `1`      | Signature algorithm. `1` is Ed25519.                 |
| `public_key` | string  | required | 32-byte Ed25519 public key as hexadecimal, 64 chars. |

Key identifiers must be non-empty and unique.
Only algorithm `1` (Ed25519) is currently accepted.

## `proxy`

| Key                        | Type            | Default | Description                                        |
| -------------------------- | --------------- | ------- | -------------------------------------------------- |
| `listeners`                | array of tables | `[]`    | Public L4 listeners.                               |
| `ports`                    | table           | absent  | Private port range for application instances.      |
| `udp_session_timeout_secs` | integer         | `30`    | Seconds an idle UDP client session is kept open.   |

When `listeners` is empty, no proxy is started and instances are reachable only on their private ports.

### `proxy.listeners`

| Key        | Type    | Default    | Description                                                 |
| ---------- | ------- | ---------- | ----------------------------------------------------------- |
| `protocol` | string  | required   | `"tcp"` or `"udp"`.                                         |
| `bind`     | string  | `"0.0.0.0"`| Local IP address to bind.                                   |
| `port`     | integer | required   | Public port to bind. `0` selects an ephemeral port.         |
| `backend`  | string  | `"default"`| Name of the application port that supplies backends.        |

The `backend` must name a configured `application.ports` entry, or the implicit `default` port when no application ports are configured.
The listener `protocol` must be declared by that application port.
A TCP listener and a UDP listener may share the same public port number.

Each listener forwards to every instance that owns the named private port.
TCP connections are distributed round-robin.
UDP sessions are tracked per client address, distributed round-robin on first datagram, and expire after `udp_session_timeout_secs` of inactivity.

### `proxy.ports`

| Key     | Type    | Description                       |
| ------- | ------- | --------------------------------- |
| `start` | integer | First private port to allocate.   |
| `end`   | integer | Last private port to allocate.    |

When `ports` is absent, ephemeral ports are allocated instead.
Ports are checked for availability on both TCP and UDP before allocation, even when only one transport is needed.

The range must not include the management port or any non-zero listener port.
`start` must be greater than zero and must not be greater than `end`.

During a blue-green deployment both the old and new instance sets are alive at the same time.
The range must therefore hold at least:

```text
instances * 2 * configured_application_ports
```

For example, `instances = 2` with four named ports needs 16 free ports during a deployment.

## Template placeholders

Every supported placeholder has an unnamed and a named form:

| Placeholder               | Value                                            |
| ------------------------- | ------------------------------------------------ |
| `{port}`                  | Private port of the `default` application port.  |
| `{address}`               | `127.0.0.1`.                                     |
| `{listen_address}`        | `127.0.0.1:<default port>`.                      |
| `{port:name}`             | Private port of the named application port.      |
| `{address:name}`          | `127.0.0.1`, after checking the name exists.     |
| `{listen_address:name}`   | `127.0.0.1:<named port>`.                        |
| `{release_dir}`           | Extracted release directory.                     |

The unnamed forms resolve to the application port named `default`.
With a multi-port configuration, use the named forms.
Referencing an unknown port name fails the startup or rejects the deployment.
`{release_dir}` is only available once a release is installed.

Unknown placeholders are left unchanged.
This lets an application receive literal braces, but it also hides a misspelled placeholder.
Prefer the named forms and check the expanded process arguments in the logs when debugging.

## Multi-port example: NTP, DNS, and web

This example serves:

- NTP on UDP `123`.
- DNS on UDP `53` and TCP `53`.
- HTTP on TCP `80`.
- HTTPS on TCP `443`.

The application receives four private ports, one each for NTP, DNS, HTTP, and HTTPS.
DNS receives a single private port for both UDP and TCP.

```toml
[management]
port = 54

[application]
name = "example"
command = "{release_dir}/bin/example"
args = [
  "--ntp-port", "{port:ntp}",
  "--dns-port", "{port:dns}",
  "--http-port", "{port:http}",
  "--https-port", "{port:https}",
]
working_directory = "{release_dir}"
shutdown_timeout_secs = 30
instances = 2
startup_grace_secs = 2
release_directory = "/var/lib/internalrrd/releases"
max_artifact_bytes = 536870912

[application.environment]
RUST_LOG = "info"
NTP_LISTEN_ADDR = "{listen_address:ntp}"
DNS_LISTEN_ADDR = "{listen_address:dns}"
HTTP_LISTEN_ADDR = "{listen_address:http}"
HTTPS_LISTEN_ADDR = "{listen_address:https}"

[[application.ports]]
name = "ntp"
protocols = ["udp"]

[[application.ports]]
name = "dns"
protocols = ["udp", "tcp"]

[[application.ports]]
name = "http"
protocols = ["tcp"]

[[application.ports]]
name = "https"
protocols = ["tcp"]

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
protocol = "udp"
bind = "0.0.0.0"
port = 123
backend = "ntp"

[[proxy.listeners]]
protocol = "udp"
bind = "0.0.0.0"
port = 53
backend = "dns"

[[proxy.listeners]]
protocol = "tcp"
bind = "0.0.0.0"
port = 53
backend = "dns"

[[proxy.listeners]]
protocol = "tcp"
bind = "0.0.0.0"
port = 80
backend = "http"

[[proxy.listeners]]
protocol = "tcp"
bind = "0.0.0.0"
port = 443
backend = "https"
```

With this configuration each instance might receive:

| Port name | Private port | Public listeners        |
| --------- | ------------ | ----------------------- |
| `ntp`     | `30000`      | UDP `123`               |
| `dns`     | `30001`      | UDP `53` and TCP `53`   |
| `http`    | `30002`      | TCP `80`                |
| `https`   | `30003`      | TCP `443`               |

The second instance receives four different ports, and both instances are backends for every listener that uses their port name.

The application must listen on `127.0.0.1` for each private port.
It must not bind the public ports, because `internalrrd` owns those.
Ports `53`, `80`, `123`, and `443` require root or `CAP_NET_BIND_SERVICE`.

## Zero-downtime rolling releases

A deployment replaces all instances of a release as a blue-green switch:

1. The control plane signs a deployment and sends it over the management protocol.
2. `internalrrd` verifies the signature against `trust.keys` and checks `application.name`.
3. It downloads the artifact, verifies its size and SHA-256, and extracts it into `release_directory`.
4. It allocates every named private port for `application.instances` new instances.
5. It starts the new instances with templates expanded for their private ports.
6. It waits `startup_grace_secs` and confirms the new processes are still alive.
7. It replaces the proxy backend map in one step, so every listener switches to the new set together.
8. It sends `SIGTERM` to the old instances and waits up to `shutdown_timeout_secs` before `SIGKILL`.
9. It releases the old private ports and updates the `current` symlink.

If anything fails before step 7, the new instances are stopped, their ports are released, and the old release keeps serving.

New TCP connections use the new release immediately after the switch.
Connections that were already open continue to the old instance until they close or the old instance exits.
Open UDP sessions keep their old backend until they idle out or the old instance exits.

The health check is process liveness, not application readiness.
Set `startup_grace_secs` high enough for the application to bind every private port.

## Common configuration errors

| Message                                                        | Cause                                                          |
| -------------------------------------------------------------- | -------------------------------------------------------------- |
| `references unknown application port`                          | A listener `backend` does not match any `application.ports`.   |
| `does not declare the tcp protocol`                            | A listener uses a protocol missing from its backend port.      |
| `is defined more than once`                                    | Duplicate port names, trust key IDs, or listener addresses.    |
| `must declare at least one protocol`                           | An `application.ports` entry has an empty `protocols` array.   |
| `range includes the management port`                           | `proxy.ports` overlaps `management.port`.                      |
| `range includes listener port`                                 | `proxy.ports` overlaps a public listener port.                 |
| `no free application ports remain`                             | The range is too small for old and new instances together.     |
| `template references unknown application port`                 | A template uses a name that is not in `application.ports`.     |
| `template requires {release_dir} but no release is installed`  | A template uses `{release_dir}` before a release is available. |

## Validation summary

`internalrrd` refuses to start when:

- `application.name` or `application.command` is empty.
- `application.shutdown_timeout_secs`, `application.instances`, or `application.max_artifact_bytes` is zero.
- `application.release_directory` is empty.
- An application port name is empty, duplicated, or has no protocols.
- An application port repeats a protocol.
- A trust key ID is empty or duplicated.
- A trust key uses an unsupported algorithm or a public key that is not 32 bytes of hexadecimal.
- A proxy listener references an unknown application port or an undeclared protocol.
- A proxy listener uses the management port or duplicates another listener.
- `proxy.udp_session_timeout_secs` is zero.
- The private port range starts at zero, is inverted, or overlaps a listener or the management port.
