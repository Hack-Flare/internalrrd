# InternalRRD

InternalRRD (Internal Rolling Release Deployments) is a project for updating application processes inside persistent containers without restarting the containers themselves.
It aims to provide rolling releases and coordinated deployments across servers without requiring a full container orchestration platform.

## How it is intended to work

A deployment controller notifies registered servers of a release. 
Each server verifies the request, obtains the application artifact, starts a replacement process, checks its health, switches traffic to it, and then stops the old process.
The container remains running throughout.
The controller can coordinate rollout order and collect results; each server performs its own application update.

The design also calls for signed deployment notifications, release tracking, health checks, rollback, and artifact integrity checks. 

## Repository components

- **`internalrrd/`**, Rust component intended to run on each server and manage application processes and releases. See [`internalrrd/README.md`](internalrrd/README.md).
- **`protocol/`**. Rust component for the deployment communication protocol. See [`protocol/README.md`](protocol/README.md).
- **`console/`**, Odin component intended for the deployment controller/console. See [`console/README.md`](console/README.md).

## License

InternalRRD is licensed under the Apache License, Version 2.0. See [LICENSE.md](LICENSE.md).
