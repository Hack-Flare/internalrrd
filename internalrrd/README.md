# internalrrd

The service component of InternalRRD, an in-container rolling deployment project.

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
command = "/usr/bin/example"
args = ["--serve"]
working_directory = "/srv/example"
shutdown_timeout_secs = 30

[application.environment]
RUST_LOG = "info"
```

- `management.port` is the port reserved for the management protocol and defaults to `54`.
- `application.name` is a human readable name used in logs.
- `application.command` is the executable started for each instance.
- `application.args` are optional arguments passed to the executable.
- `application.working_directory` is optional and defaults to the working directory of `internalrrd`.
- `application.environment` holds optional environment variables passed to every instance.
- `application.shutdown_timeout_secs` controls how long an instance has to exit after `SIGTERM` before it is killed.
