# Console

The control plane component of InternalRRD.
It signs deployment notifications and sends them to registered servers over the management protocol.

## Current state

The console supports:

- Loading and validating a TOML configuration file.
- Signing deployments with an Ed25519 key.
- Connecting to a server and completing the `Hello` handshake.
- Deploying to one server or every configured server in order.
- Querying `Status` and `Metrics`.
- Stopping a multi-server deployment at the first failure.

The console does not upload artifacts.
A deployment names an artifact URL, its SHA-256 digest, and its size.

## Configuration

The default path is `/etc/internalrrd/console.toml` and can be overridden with `--config`.
Unknown keys are rejected.
See [`config.example.toml`](config.example.toml) for a working configuration.

| Key                      | Type            | Default  | Description                                      |
| ------------------------ | --------------- | -------- | ------------------------------------------------ |
| `application.name`       | string          | required | Application name that deployments target.        |
| `signing.key_id`         | string          | required | Key identifier in every server's trust store.    |
| `signing.private_key`    | string          | required | 32-byte Ed25519 private key as hexadecimal.      |
| `network.timeout_secs`   | integer         | `10`     | Seconds a connection or response may take.       |
| `servers[].name`         | string          | required | Unique name used to select the server.           |
| `servers[].address`      | string          | required | Management protocol address, as `ip:port`.       |
| `servers`                | array of tables | required | Servers in deployment order. Must not be empty.  |

`signing.private_key` is secret.
Restrict the configuration file to the user that runs the console.
The example key corresponds to the public key in [`../internalrrd/config.example.toml`](../internalrrd/config.example.toml).

Server names and addresses must be unique.
`network.timeout_secs` applies to connecting, sending, and waiting for each response.

## Commands

```sh
# Verify the handshake against every configured server.
console --config console.toml hello

# Print deployed versions and instances.
console --config console.toml status

# Query one server and one instance.
console --config console.toml metrics --server web-1 --instance 0

# Sign and deploy to every server in configuration order.
console --config console.toml deploy \
  --version 1.2.3 \
  --artifact https://artifacts.example/example-1.2.3.tar.gz \
  --sha256 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef \
  --size 4096
```

Pass `--server <name>` to any command to target one configured server.
Pass `--issued-at <unix>` to `deploy` to set the issue time explicitly.
Otherwise the console uses the current Unix time.

`deploy` signs one deployment and sends the same notification to every selected server.
It stops at the first rejection or connection failure so later servers keep their current release.
Servers that already accepted the deployment stay on the new release.
Each server re-checks the signature, the application name, the version, and the issue time.

## Build and run

From the repository root:

```sh
cargo build -p console
cargo run -p console -- --config console/config.example.toml status
```

## Trust

The server accepts a deployment only when `signing.key_id` names a key in its `trust.keys` list.
Generate an Ed25519 key pair, put the public key in each server configuration, and keep the private key in the console configuration.
