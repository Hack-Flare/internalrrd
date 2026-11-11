# InternalRRD

## Internal Rolling Release Deployments

InternalRRD is a lightweight, application-agnostic rolling deployment system designed to manage application porcesses inside persistent Docker containers.

Its primary goal is to enable zero-downtime application updates without restarting or replacing the container itself.

Instead of relying on external deployment platforms or container orchestration systems, InternalRRD manages application lifecycles directly.

## Features

These are planned and not necessarily up to date.

- [ ] Rolling Deployments: Run multiple application instances simultaneously during updates.
- [ ] Zero-downtime updates: Switch incoming traffic between application instances without restarting the container
- [ ] Multi-server deployments: Coordinate releases across multiple registered servers
- [ ] Version tracking: Track deployed application versions and avoid unnecessary deployments
- [ ] Automatic health checks: Verify replacement instances before switching traffic
- [ ] Rollback support: Retain previous application versions for recovery
- [ ] Artifact distribution: Retrieve prebuilt application binaries from configured release sources
- [ ] Artifact building: Build application binaries from source code if prebuilt artifacts are unavailable
- [ ] Authenticated deployments: Verify deployment notifcations using asymmetric cryptography
- [ ] Lightweight: Designed to have a highly minimal footprint
- [ ] Metrics: Expose metrics of both itself and the managed application for monitoring and observability

## Architecture

InternalRRD runs inside the same Docker container as the application(s) it manages.

It maintains control over application process lifecycles and provides a stable interface for incoming traffic.

Applications can run as multiple concurrent instances, allowing InternalRRD to transition between versions without interrupting the container.

A typical deployment should follow this sequence:
1. Recieve a deployment notification
2. Verify the notification's authenticity
3. Compare the requested release against the currently deployed version
4. Retrieve the new application binary
5. Start a replacement application instance
6. erify the replacement is healthy
7. Redirect incomming traffic to the replacement
8. Drain outstanding work from the previous instance
9. Terminate the previous instance

The docker container remaings fully running and operational throughout the entire process.

## Multi-server coordination

InternalRRD is designed to work across multiple servers.

A central control plane can communicate with registered InternalRRD instances through their management APIs.

The control plane is responsible for coordinating deployment notifications and collecting deployment results from each server.

Each server independently handles its own application update.

This allows deployments to be performed sequentially or according to configurable rollout policies.

## Deployment notifications

InternalRRD supports verisioned deployment notificaitons.

Notifications identify the intended application release and its associated artifact or build instructions.

Servers can reject unnecessary deployments based on their current deployment state.

Deployment notifications are authenticated using asymmetric cryptographic signatures to ensure that only authorized sources can trigger updates.

The design also considers post-quantum algorithms for long-term security.

## Build and distribution

InternalRRD is designed around prebuilt application binaries. Support for building application binaries from source code is also planned.

Build infrastructure produces application artifacts that are distributed to registered servers. Artifact integrity is also verified before deployment.

This provides consistent and reproducible deployments across all servers.

## Technology

The planned implementation uses:

- Rust: Core deployment system and process management
- Elixir: Optional central control-plane implementation
- Runners: CI/CD pipelines for building and distributing application artifacts
- Docker: Persistant application runtime environment

## Project goals

InternalRRD aims to provide a small, dependable deployment system without requiring a complex and full container orchestration platform.

Long term goals:

[ ] Application-agnostic process management  
[ ] Multi-server deployment coordination
[ ] Rolling application updates
[ ] Automatic rollback
[ ] Deployment history and reporting
[ ] Configurable rollout policies
[ ] Server registration and management
[ ] Post-quantum authenticated deployment notifications
[ ] Publicly documented management protocols and APIs

## Philosophy

InternalRRd is built around an idea that application updates should not require restarting the infrastructure hosting them.

A persistent container should be able to host, update, and replace its application process independently of its own lifecycle.

The result should be a simpler deployment system, with more predictable updates and fewer moving parts.

## License

InternalRRD is licensed under the Apache License, Version 2.0. See the LICENSE file for details.