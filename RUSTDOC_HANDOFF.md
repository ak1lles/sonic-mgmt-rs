# Rustdoc Handoff

All documentation for this project lives in Rust doc comments. A generation
script (`docs/scripts/generate.sh`) extracts crate-level `//!` comments into
Fumadocs MDX pages and copies `cargo doc` HTML into the site. The source of
truth is always the code.

Run `makers docs` to regenerate. Run `makers docs-serve` to preview.

## Style

Follow standard Rust documentation style. Write like the standard library docs.

- Lead with what the item does, not what it is
- First sentence is the summary shown in module-level listings
- Use `#` sections inside `//!` for structure (Examples, Panics, Errors)
- Reference other items with intra-doc links: [`DeviceType`], [`Connection::open`]
- No excessive punctuation
- No contrastive negation ("is not X, it is Y")
- Short sentences. One idea per sentence.
- Code examples use ```` ```no_run ```` or ```` ```ignore ```` when they need a device

## What needs writing

### sonic-core (highest priority)

This crate is the foundation. Every other crate links to its types. Good docs
here pay off across the entire workspace.

**types.rs** -- No module doc. Most public enums and structs lack doc comments.

Items that need `///` docs:

| Item | What to document |
|------|-----------------|
| `DeviceType` enum + variants | What each device type represents, when each is used |
| `ConnectionType` enum + variants | Transport protocols, when to choose each |
| `Platform` enum + variants | ASIC vendors, what switch families they cover |
| `TopologyType` enum + variants | What each topology looks like, VM count, port count |
| `VmType` enum + variants | Virtual machine flavors for neighbor simulation |
| `RebootType` enum + variants | Cold/warm/fast reboot semantics |
| `TestOutcome` enum + variants | Pass/fail/skip/error/xfail/xpass meaning |
| `TestbedState` enum + variants | Lifecycle states |
| `HealthStatus` enum + variants | Health check result levels |
| `Credentials` struct + fields | Authentication data, builder methods |
| `DeviceInfo` struct + fields | Full device identity, how to construct |
| `CommandResult` struct + fields | Remote command output, success() helper |
| `IpPair` struct | Dual-stack address pair |
| `PortInfo`, `BgpNeighborInfo`, `ConsoleInfo` | Supporting detail types |

Add `//!` module doc at the top of types.rs explaining that this module defines
the shared domain model for the entire workspace.

**error.rs** -- No module doc. `SonicError` variants have `#[error(...)]`
format strings but no `///` explaining when each variant is returned.

Items that need `///` docs:

| Item | What to document |
|------|-----------------|
| `SonicError` enum | Overview of the error hierarchy |
| Each variant | When this error is returned, what the caller should do |
| `connection()`, `timeout()`, `config()`, etc. | Constructor helpers |
| `Result<T>` type alias | Note that all crates return this |

### sonic-device (second priority)

The device crate has the most surface area. Focus on the public API that
users interact with.

**connection/mod.rs** -- Needs `//!` module doc explaining the connection
abstraction and listing the available transports.

**connection/ssh.rs** -- `SshConnection` methods need docs:
- `new()` -- parameters, defaults
- `with_connect_timeout()`, `with_command_timeout()` -- what the timeouts control
- Trait impl methods are covered by the trait docs in sonic-core

**connection/telnet.rs** -- Similar to ssh.rs. Document `TelnetConnection::new()`
and the builder methods.

**connection/console.rs** -- Document `ConsoleConnection::new()` and the
conserver protocol details.

**hosts/mod.rs** -- Needs `//!` module doc. Document `create_host()` factory
explaining dispatch by `DeviceType`.

**hosts/sonic.rs** -- `SonicHost` is the primary device type. Document:
- `new()` -- what it takes, what it sets up
- `config_reload()` -- what reload types are supported
- `load_minigraph()` -- what it pushes and how
- `get_running_config()` -- return format
- `get_container_status()` -- return type
- All service management methods

**hosts/eos.rs**, **hosts/cisco.rs**, **hosts/ptf.rs**, **hosts/fanout.rs** --
Each needs the same treatment as sonic.rs. Focus on what makes each host
type different (enable mode for EOS, privilege levels for Cisco, port
operations for fanout, file transfer for PTF).

**facts/mod.rs** -- Needs `//!` module doc explaining the facts collection
pipeline and caching.

### sonic-mgmt-cli (third priority)

**commands/mod.rs** -- Needs `//!` module doc listing available subcommand
modules.

## What is already well-documented

These crates have good coverage and only need incremental work:

- **sonic-config** -- all modules, types, and functions documented
- **sonic-testbed** -- all modules, types, and accessors documented
- **sonic-topology** -- all modules, types, generators, and renderers documented
- **sonic-testing** -- all modules, discovery, execution, fixtures, results documented
- **sonic-reporting** -- all modules, JUnit parsing, storage, upload documented
- **sonic-sdn** -- all modules, gNMI/gNOI/P4Runtime clients documented
- **sonic-integration-tests** -- crate-level doc present

## Verification

After writing docs, run:

```sh
cargo doc --workspace --no-deps 2>&1 | grep warning
```

Zero warnings means all intra-doc links resolve. Then run `makers docs` to
regenerate the Fumadocs site content.
