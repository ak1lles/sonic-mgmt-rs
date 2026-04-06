//! Reusable prompt helpers built on the `inquire` crate.
//!
//! Every helper validates user input and returns a strongly-typed value, keeping
//! the wizard and setup modules free of low-level validation logic.

use std::net::IpAddr;

use anyhow::Result;
use inquire::{Confirm, CustomType, Password, Select, Text};
use ipnetwork::IpNetwork;

use sonic_core::{Credentials, DeviceType, Platform, TopologyType, VmType};

// ---------------------------------------------------------------------------
// Internal helper -- select by index
// ---------------------------------------------------------------------------

/// Presents a `Select` prompt with `labels` and returns the positional index
/// the user chose.  This avoids accessing internal fields of `inquire::Select`.
fn select_index(message: &str, labels: Vec<String>, help: &str) -> Result<usize> {
    let snapshot = labels.clone();
    let answer = Select::new(message, labels)
        .with_help_message(help)
        .prompt()?;
    let idx = snapshot
        .iter()
        .position(|o| *o == answer)
        .unwrap_or(0);
    Ok(idx)
}

/// Like `select_index` but allows setting the default cursor position.
#[allow(dead_code)]
fn select_index_with_default(
    message: &str,
    labels: Vec<String>,
    help: &str,
    default_cursor: usize,
) -> Result<usize> {
    let snapshot = labels.clone();
    let answer = Select::new(message, labels)
        .with_help_message(help)
        .with_starting_cursor(default_cursor)
        .prompt()?;
    let idx = snapshot
        .iter()
        .position(|o| *o == answer)
        .unwrap_or(0);
    Ok(idx)
}

// ---------------------------------------------------------------------------
// IP / Network prompts
// ---------------------------------------------------------------------------

/// Prompt for an IP address (v4 or v6) with inline validation.
pub fn prompt_ip_address(message: &str) -> Result<IpAddr> {
    let input = Text::new(message)
        .with_help_message("Enter an IPv4 or IPv6 address (e.g. 10.0.0.1 or fc00::1)")
        .with_validator(|val: &str| {
            Ok(val.parse::<IpAddr>()
                .map(|_| inquire::validator::Validation::Valid)
                .unwrap_or_else(|_| {
                    inquire::validator::Validation::Invalid(
                        "Not a valid IP address. Use dotted-decimal (IPv4) or colon-hex (IPv6)."
                            .into(),
                    )
                }))
        })
        .prompt()?;

    Ok(input.parse::<IpAddr>()?)
}

/// Prompt for a CIDR notation network (e.g. `10.0.0.0/24`).
pub fn prompt_cidr(message: &str) -> Result<IpNetwork> {
    let input = Text::new(message)
        .with_help_message("Enter a network in CIDR notation (e.g. 10.250.0.0/24)")
        .with_validator(|val: &str| {
            Ok(val.parse::<IpNetwork>()
                .map(|_| inquire::validator::Validation::Valid)
                .unwrap_or_else(|_| {
                    inquire::validator::Validation::Invalid(
                        "Not a valid CIDR network. Use address/prefix format (e.g. 10.0.0.0/24)."
                            .into(),
                    )
                }))
        })
        .prompt()?;

    Ok(input.parse::<IpNetwork>()?)
}

/// Prompt for a TCP/UDP port number with an optional default.
pub fn prompt_port(message: &str, default: u16) -> Result<u16> {
    let port = CustomType::<u16>::new(message)
        .with_help_message("Enter a port number (1-65535)")
        .with_default(default)
        .with_error_message("Please enter a valid port number (1-65535)")
        .with_validator(|val: &u16| {
            if *val == 0 {
                Ok(inquire::validator::Validation::Invalid(
                    "Port must be non-zero.".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()?;

    Ok(port)
}

// ---------------------------------------------------------------------------
// Credential prompts
// ---------------------------------------------------------------------------

/// Prompt for a username + password and/or key path, returning a `Credentials`
/// value.
pub fn prompt_credentials() -> Result<Credentials> {
    let username = Text::new("SSH username:")
        .with_default("admin")
        .with_help_message("Login username for the device")
        .prompt()?;

    let password_raw = Password::new("SSH password (leave blank to skip):")
        .without_confirmation()
        .with_help_message("Press Enter to skip if using key-based auth")
        .prompt()?;
    let password = if password_raw.is_empty() {
        None
    } else {
        Some(password_raw)
    };

    let key_path_raw = Text::new("SSH key path (leave blank to skip):")
        .with_help_message("Path to the private key file (e.g. ~/.ssh/id_rsa)")
        .prompt()?;
    let key_path = if key_path_raw.trim().is_empty() {
        None
    } else {
        Some(key_path_raw)
    };

    let mut creds = Credentials::new(username);
    if let Some(pw) = password {
        creds = creds.with_password(pw);
    }
    if let Some(key) = key_path {
        creds = creds.with_key(key);
    }

    Ok(creds)
}

// ---------------------------------------------------------------------------
// Enum selectors
// ---------------------------------------------------------------------------

/// Prompt the user to select a `DeviceType`.
pub fn prompt_device_type() -> Result<DeviceType> {
    let options = vec![
        DeviceType::Sonic,
        DeviceType::Eos,
        DeviceType::Cisco,
        DeviceType::Fanout,
        DeviceType::Ptf,
        DeviceType::K8sMaster,
        DeviceType::Aos,
        DeviceType::Cumulus,
        DeviceType::Onie,
    ];

    let labels: Vec<String> = options.iter().map(|d| d.to_string()).collect();
    let idx = select_index(
        "Device type:",
        labels,
        "Select the type of network operating system",
    )?;

    Ok(options[idx])
}

/// Prompt the user to select a `Platform`.
pub fn prompt_platform() -> Result<Platform> {
    let options = vec![
        Platform::Broadcom,
        Platform::Mellanox,
        Platform::Barefoot,
        Platform::Marvell,
        Platform::Nokia,
        Platform::Cisco,
        Platform::Centec,
        Platform::Virtual,
        Platform::Unknown,
    ];

    let labels: Vec<String> = options.iter().map(|p| p.to_string()).collect();
    let idx = select_index(
        "Platform / ASIC vendor:",
        labels,
        "Select the hardware platform",
    )?;

    Ok(options[idx])
}

/// Prompt the user to select a `TopologyType`.
pub fn prompt_topology_type() -> Result<TopologyType> {
    let options = vec![
        TopologyType::T0,
        TopologyType::T064,
        TopologyType::T0116,
        TopologyType::T1,
        TopologyType::T164,
        TopologyType::T1Lag,
        TopologyType::T2,
        TopologyType::Dualtor,
        TopologyType::MgmtTor,
        TopologyType::M0Vlan,
        TopologyType::Ptf32,
        TopologyType::Ptf64,
        TopologyType::Ptf,
    ];

    let labels: Vec<String> = options
        .iter()
        .map(|t| format!("{} ({} VMs)", t, t.vm_count()))
        .collect();

    let idx = select_index(
        "Topology type:",
        labels,
        "Select the network topology to deploy",
    )?;

    Ok(options[idx])
}

/// Prompt the user to select a `VmType`.
pub fn prompt_vm_type() -> Result<VmType> {
    let options = vec![
        VmType::Veos,
        VmType::Ceos,
        VmType::Vsonic,
        VmType::Vcisco,
        VmType::Csonic,
    ];

    let labels: Vec<String> = options.iter().map(|v| v.to_string()).collect();
    let idx = select_index(
        "VM type:",
        labels,
        "Select the virtualization type for neighbor VMs",
    )?;

    Ok(options[idx])
}

// ---------------------------------------------------------------------------
// Generic prompts
// ---------------------------------------------------------------------------

/// Yes/no confirmation prompt.
pub fn prompt_confirm(message: &str) -> Result<bool> {
    let answer = Confirm::new(message).with_default(true).prompt()?;
    Ok(answer)
}

/// Text prompt that returns `None` if the user enters an empty string.
pub fn prompt_optional_text(message: &str) -> Result<Option<String>> {
    let input = Text::new(message)
        .with_help_message("Press Enter to skip")
        .prompt()?;

    if input.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(input))
    }
}

/// Select a log level from common tracing directive strings.
pub fn prompt_log_level() -> Result<String> {
    let options = vec![
        "error".to_string(),
        "warn".to_string(),
        "info".to_string(),
        "debug".to_string(),
        "trace".to_string(),
    ];

    let level = Select::new("Log level:", options)
        .with_starting_cursor(2) // default to "info"
        .with_help_message("Controls verbosity of framework logging")
        .prompt()?;

    Ok(level)
}
