# sonic-mgmt-rs

A Rust-based network management framework for SONiC (Software for Open Networking in the Cloud) switches. This project replaces the Python-based [sonic-mgmt](https://github.com/sonic-net/sonic-mgmt) with a type-safe, high-performance Rust implementation, providing a unified CLI tool for managing testbeds, devices, topologies, tests, and SDN operations.

## Architecture

The project is organized as a Cargo workspace with nine crates:

| Crate | Description |
|---|---|
| `sonic-core` | Core types, traits, and error definitions shared across all crates |
| `sonic-config` | TOML/YAML configuration loading, validation, and management |
| `sonic-device` | Device abstraction layer with SSH, Telnet, and console transport |
| `sonic-testbed` | Testbed management, provisioning, and lifecycle operations |
| `sonic-topology` | Network topology definitions, generation, and rendering |
| `sonic-testing` | Test discovery, execution framework, and result collection |
| `sonic-reporting` | JUnit XML parsing, test analytics, and coverage reporting |
| `sonic-sdn` | SDN protocol clients for gNMI, gNOI, and P4Runtime |
| `sonic-mgmt-cli` | Interactive CLI binary (`sonic-mgmt`) tying everything together |

## Prerequisites

- **Rust 1.75+** and Cargo
- Optional: SSH access to SONiC DUT devices
- Optional: Arista vEOS images for virtual testbeds
- Optional: Docker for the containerized test runner

## Installation

Build from source:

```sh
cargo build --release
```

The compiled binary is located at `target/release/sonic-mgmt`.

## Quick Start

```sh
# 1. Run the first-time setup wizard
sonic-mgmt init

# 2. Create a testbed configuration interactively
sonic-mgmt testbed wizard

# 3. Deploy a topology to the testbed
sonic-mgmt testbed deploy <name>

# 4. Run tests
sonic-mgmt test run --path tests/
```

## CLI Reference

```
sonic-mgmt <command> [options]
```

### Commands

| Command | Subcommands | Description |
|---|---|---|
| `init` | | First-time setup wizard |
| `testbed` | `list`, `show`, `deploy`, `teardown`, `health`, `refresh`, `upgrade`, `wizard` | Manage testbed lifecycle |
| `device` | `list`, `show`, `connect`, `exec`, `reboot`, `facts`, `wizard` | Interact with network devices |
| `topology` | `list`, `show`, `generate`, `render` | Define and visualize topologies |
| `test` | `discover`, `run`, `wizard`, `results` | Discover, execute, and review tests |
| `report` | `parse`, `upload`, `coverage` | Parse JUnit results, upload reports, view coverage |
| `config` | `show`, `edit`, `validate`, `init` | Manage configuration files |
| `sdn` | `gnmi` (`get`, `set`, `subscribe`), `gnoi` (`reboot`, `ping`), `p4rt` (`write`) | SDN protocol operations |
| `docker` | `build`, `up`, `down`, `status`, `shell`, `test` | Manage the containerized test runner |

## Configuration

### Main Configuration

The primary configuration file is `sonic-mgmt.toml`, containing sections for:

- `testbed` -- testbed defaults and paths
- `connection` -- SSH/transport settings and credentials
- `testing` -- test runner options and timeouts
- `reporting` -- report output format and upload targets
- `topology` -- topology generation parameters
- `logging` -- log level and output configuration

### File Formats

- **Testbed definitions:** TOML or YAML (see `config/testbed.example.toml`)
- **Inventory files:** TOML or YAML (see `config/inventory.example.toml`)
- **Topology templates:** located in `config/topology/`

### Environment Variables

| Variable | Description |
|---|---|
| `SONIC_CONFIG` | Override path to `sonic-mgmt.toml` |
| `SONIC_LOG` | Set log level (`trace`, `debug`, `info`, `warn`, `error`) |

## Supported Topologies

| Topology | Description |
|---|---|
| `t0` | Leaf switch with 32 servers |
| `t0-64` | Leaf switch with 64 servers |
| `t0-116` | Leaf switch with 116 servers |
| `t1` | Spine switch with 32 leaf connections |
| `t1-64` | Spine switch with 64 leaf connections |
| `t1-lag` | Spine switch with LAG leaf connections |
| `t2` | Super-spine with spine connections |
| `dualtor` | Dual top-of-rack redundancy topology |
| `mgmt-tor` | Management top-of-rack topology |
| `m0-vlan` | M0 topology with VLAN segmentation |
| `ptf-32` | PTF-only topology with 32 ports |
| `ptf-64` | PTF-only topology with 64 ports |
| `ptf` | Minimal PTF topology |

## Supported Devices

| Platform | Notes |
|---|---|
| SONiC | Primary target -- all SONiC NOS versions |
| Arista EOS | Fanout and neighbor device support |
| Cisco | Neighbor device support |
| Fanout | VLAN-based fanout switches |
| PTF | Packet Test Framework containers |
| K8s Master | Kubernetes-managed testbed orchestration |

## Development

```sh
# Unit tests
cargo test

# Integration tests against a real testbed
SONIC_TESTBED=my-testbed makers integration-test

# Or run inside the Docker test runner
makers docker-build
SONIC_TESTBED=my-testbed makers docker-test

# Lint and format
cargo clippy --workspace
cargo fmt --all

# Build documentation
cargo doc --workspace --no-deps
```

### Environment Variables

| Variable | Default | Purpose |
|---|---|---|
| `SONIC_CONFIG` | `sonic-mgmt.toml` | Path to application config |
| `SONIC_TESTBED` | | Active testbed name for integration tests |
| `SONIC_SSH_HOST` | | Override target host directly |
| `SONIC_SSH_PORT` | `22` | Override target SSH port |
| `SONIC_SSH_USER` | `admin` | Override SSH username |
| `SONIC_SSH_PASS` | `password` | Override SSH password |
| `SONIC_LOG` | `info` | Log level |

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
