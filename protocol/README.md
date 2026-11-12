# Protocol

The management protocol used between an InternalRRD control plane and registered servers.

The crate is a library.
It defines the wire format, message types, framing codec, and signed deployment notifications.

## Transport

The protocol runs over TCP and uses port `54` by default.

Every message is one frame:

```text
+----------------------+-------------------------------+
| 4 byte length prefix | postcard encoded WireFrame    |
| big endian u32       | length prefix excludes prefix |
+----------------------+-------------------------------+
```

The maximum encoded frame payload is 1 MiB.
A `WireFrame` contains:

- `version`: the protocol version.
- `request_id`: the identifier shared by a request and its response.
- `message_type`: the explicit type tag.
- `payload`: the postcard encoded message body.

Message bodies are encoded separately from the frame so that unknown message types can be rejected without losing the request identifier.

## Versioning

`ProtocolVersion` has a `major` and `minor` component.
Peers are compatible when their major versions match.
The minor version may grow without breaking the wire format.

Message type tags and capability values are explicit and stable.
Unknown values are preserved where the protocol allows forward compatibility.

## Message types

Requests use the `0x00xx` range and responses use the `0x80xx` range.

| Tag      | Direction | Name                 | Body                 |
| -------- | --------- | -------------------- | -------------------- |
| `0x0001` | request   | Hello                | `HelloRequest`       |
| `0x0002` | request   | Deploy               | `SignedDeployment`   |
| `0x0003` | request   | Status               | none                 |
| `0x8001` | response  | Hello                | `HelloResponse`      |
| `0x8002` | response  | DeploymentAccepted   | `DeploymentAccepted` |
| `0x8003` | response  | DeploymentRejected   | `DeploymentRejected` |
| `0x8004` | response  | Status               | `StatusReport`       |
| `0x8005` | response  | Error                | `ProtocolError`      |

A typical session is:

1. The control plane sends `Hello`.
2. The server replies with `Hello` and its current deployment state.
3. The control plane sends `Deploy` with a signed deployment notification.
4. The server replies with `DeploymentAccepted` or `DeploymentRejected`.
5. The control plane may query `Status` at any time.

## Capabilities

Capabilities are advertised in `HelloRequest` and `HelloResponse`.

| Value | Name                |
| ----- | ------------------- |
| `1`   | `SignedDeployments` |
| `2`   | `StatusReporting`   |

Unknown capability values are preserved on the wire.

## Deployment signatures

A `SignedDeployment` contains a `Deployment` and a detached `DeploymentSignature`.

The signed bytes are the ASCII domain separator `internalrrd/deployment/v1` followed by the postcard encoding of the `Deployment`.

Ed25519 is algorithm value `1`.
The `key_id` selects the public key trusted by the server.
Unknown algorithm values are preserved so that newer schemes can be negotiated later, including post-quantum algorithms.

The `Deployment` covers the application name, release version, artifact URL and SHA-256 digest, artifact size, and issue time.

Servers are responsible for checking `issued_at_unix` freshness and their current deployment state before accepting a notification.

## Rejection and error codes

`DeploymentRejected` carries a `RejectionCode`:

| Value | Name                  |
| ----- | --------------------- |
| `1`   | `InvalidSignature`    |
| `2`   | `UnsupportedVersion`  |
| `3`   | `OutdatedRelease`     |
| `4`   | `AlreadyDeployed`     |
| `5`   | `ArtifactUnavailable` |
| `6`   | `HealthCheckFailed`   |
| `7`   | `Internal`            |

`ProtocolError` carries a `ProtocolErrorCode`:

| Value | Name                 |
| ----- | -------------------- |
| `1`   | `UnsupportedVersion` |
| `2`   | `InvalidMessage`     |
| `3`   | `UnknownMessageType` |
| `4`   | `Unauthorized`       |
| `5`   | `Internal`           |

## Rust usage

```rust
use protocol::{Command, Envelope, Request, RequestId};

let request: Request = Envelope::current(RequestId::new(1), Command::Status);
let frame = request.encode()?;
let decoded = Request::decode(&frame)?;
```

For asynchronous TCP, use `RequestCodec` on the control plane side and `ResponseCodec` on the server side.

## Build and test

From the repository root:

```sh
cargo build -p protocol
cargo test -p protocol
```
