//! Device management commands.
//!
//! Interact with individual devices in the inventory: list, inspect, execute
//! remote commands, reboot, and collect device facts.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use colored::Colorize;

use sonic_config::{DeviceEntry, InventoryConfig};
use sonic_core::{
    BasicFacts, BgpFacts, ConfigFacts, Device, DeviceInfo,
    FactsProvider, InterfaceFacts, LacpMode, RebootType,
};
use sonic_device::{create_host, SonicHost};

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct DeviceCmd {
    #[command(subcommand)]
    pub action: DeviceAction,

    /// Path to the inventory file
    #[arg(long, short = 'i', default_value = "config/inventory.toml", global = true)]
    pub inventory: PathBuf,
}

#[derive(Subcommand, Debug)]
pub enum DeviceAction {
    /// List all devices in the inventory
    List,

    /// Show detailed information about a device
    Show {
        /// Device hostname
        hostname: String,
    },

    /// Display SSH connection information for a device
    Connect {
        /// Device hostname
        hostname: String,
    },

    /// Execute a command on a remote device
    Exec {
        /// Device hostname
        hostname: String,

        /// Command to execute
        #[arg(trailing_var_arg = true, num_args = 1..)]
        command: Vec<String>,
    },

    /// Reboot a device
    Reboot {
        /// Device hostname
        hostname: String,

        /// Reboot type
        #[arg(long, short = 't', value_enum, default_value = "cold")]
        reboot_type: RebootArg,
    },

    /// Collect and display device facts
    Facts {
        /// Device hostname
        hostname: String,

        /// Type of facts to collect
        #[arg(long, short = 't', value_enum, default_value = "basic")]
        facts_type: FactsType,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum RebootArg {
    Cold,
    Warm,
    Fast,
}

impl From<RebootArg> for RebootType {
    fn from(arg: RebootArg) -> Self {
        match arg {
            RebootArg::Cold => RebootType::Cold,
            RebootArg::Warm => RebootType::Warm,
            RebootArg::Fast => RebootType::Fast,
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum FactsType {
    Basic,
    Bgp,
    Interface,
    Config,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn handle(cmd: DeviceCmd, _config_path: &str) -> Result<()> {
    let inventory_path = &cmd.inventory;
    match cmd.action {
        DeviceAction::List => list_devices(inventory_path).await,
        DeviceAction::Show { hostname } => show_device(&hostname, inventory_path).await,
        DeviceAction::Connect { hostname } => connect_device(&hostname, inventory_path).await,
        DeviceAction::Exec { hostname, command } => {
            exec_device(&hostname, &command.join(" "), inventory_path).await
        }
        DeviceAction::Reboot {
            hostname,
            reboot_type,
        } => reboot_device(&hostname, reboot_type.into(), inventory_path).await,
        DeviceAction::Facts {
            hostname,
            facts_type,
        } => collect_facts(&hostname, facts_type, inventory_path).await,
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

async fn list_devices(inventory_path: &PathBuf) -> Result<()> {
    let inventory = load_inventory(inventory_path)?;

    if inventory.devices.is_empty() {
        println!(
            "{}",
            "No devices found. Use `sonic-mgmt init` or add devices to your inventory.".yellow()
        );
        return Ok(());
    }

    println!(
        "{:<20} {:<18} {:<12} {:<14} {:<16}",
        "HOSTNAME".bold(),
        "MGMT IP".bold(),
        "TYPE".bold(),
        "PLATFORM".bold(),
        "HWSKU".bold(),
    );
    println!("{}", "-".repeat(82));

    let mut hostnames = inventory.hostnames();
    hostnames.sort();

    for hostname in hostnames {
        let entry = &inventory.devices[hostname.as_str()];
        println!(
            "{:<20} {:<18} {:<12} {:<14} {:<16}",
            hostname.cyan(),
            entry.mgmt_ip.to_string(),
            entry.device_type,
            entry.platform,
            if entry.hwsku.is_empty() {
                "(unknown)".to_string()
            } else {
                entry.hwsku.clone()
            },
        );
    }

    println!(
        "\n{} device(s) in inventory.",
        inventory.device_count().to_string().green().bold()
    );

    // Groups summary
    if !inventory.groups.is_empty() {
        println!("\n{}", "Groups:".bold().underline());
        for group_name in inventory.group_names() {
            let members = inventory.groups.get(group_name.as_str()).map(|v| v.len()).unwrap_or(0);
            println!("  {}: {} device(s)", group_name.yellow(), members);
        }
    }

    Ok(())
}

async fn show_device(hostname: &str, inventory_path: &PathBuf) -> Result<()> {
    let inventory = load_inventory(inventory_path)?;
    let entry = inventory
        .get_device(hostname)
        .ok_or_else(|| anyhow::anyhow!("device `{}` not found in inventory", hostname))?;

    println!("{}", format!("Device: {}", hostname).cyan().bold());
    println!("{:<18} {}", "Management IP:".bold(), entry.mgmt_ip);
    println!("{:<18} {}", "Device Type:".bold(), entry.device_type);
    println!("{:<18} {}", "Platform:".bold(), entry.platform);
    println!(
        "{:<18} {}",
        "HWSKU:".bold(),
        if entry.hwsku.is_empty() {
            "(unknown)"
        } else {
            &entry.hwsku
        }
    );
    println!("{:<18} {}", "Username:".bold(), entry.credentials.username);
    println!(
        "{:<18} {}",
        "Auth Method:".bold(),
        if entry.credentials.key_path.is_some() {
            "SSH key"
        } else if entry.credentials.password.is_some() {
            "password"
        } else {
            "none configured"
        }
    );
    println!(
        "{:<18} {} (port {})",
        "Connection:".bold(),
        entry.connection.connection_type,
        entry.connection.port,
    );

    if let Some(ref console) = entry.console {
        println!(
            "{:<18} {}:{} ({})",
            "Console:".bold(),
            console.server,
            console.port,
            console.protocol,
        );
    }

    if !entry.metadata.is_empty() {
        println!("\n{}", "Metadata:".bold().underline());
        let mut keys: Vec<&String> = entry.metadata.keys().collect();
        keys.sort();
        for key in keys {
            println!("  {}: {}", key.yellow(), entry.metadata[key]);
        }
    }

    // Show group memberships
    let groups: Vec<&String> = inventory
        .groups
        .iter()
        .filter(|(_, members)| members.iter().any(|m| m == hostname))
        .map(|(name, _)| name)
        .collect();

    if !groups.is_empty() {
        println!(
            "\n{:<18} {}",
            "Groups:".bold(),
            groups
                .iter()
                .map(|g| g.yellow().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(())
}

async fn connect_device(hostname: &str, inventory_path: &PathBuf) -> Result<()> {
    let inventory = load_inventory(inventory_path)?;
    let entry = inventory
        .get_device(hostname)
        .ok_or_else(|| anyhow::anyhow!("device `{}` not found in inventory", hostname))?;

    println!("{}", "SSH Connection Information".cyan().bold());
    println!("{}", "=".repeat(40));
    println!(
        "  {}  ssh {}@{} -p {}",
        "Command:".bold(),
        entry.credentials.username,
        entry.mgmt_ip,
        entry.connection.port,
    );

    if let Some(ref key_path) = entry.credentials.key_path {
        println!(
            "  {}  ssh -i {} {}@{} -p {}",
            "With key:".bold(),
            key_path.display(),
            entry.credentials.username,
            entry.mgmt_ip,
            entry.connection.port,
        );
    }

    if let Some(ref console) = entry.console {
        println!(
            "\n  {} ssh {}:{} (console)",
            "Console:".bold(),
            console.server,
            console.port,
        );
    }

    println!(
        "\n{}",
        "Tip: Use `sonic-mgmt device exec <hostname> <cmd>` to run commands directly."
            .dimmed()
    );

    Ok(())
}

async fn exec_device(hostname: &str, command: &str, inventory_path: &PathBuf) -> Result<()> {
    let inventory = load_inventory(inventory_path)?;
    let entry = inventory
        .get_device(hostname)
        .ok_or_else(|| anyhow::anyhow!("device `{}` not found in inventory", hostname))?;

    println!(
        "{} Executing on {}: {}",
        "=>".green().bold(),
        hostname.cyan(),
        command.yellow(),
    );

    let device_info = build_device_info(hostname, entry);
    let mut host = create_host(device_info);

    host.connect()
        .await
        .context(format!("failed to connect to {}", hostname))?;

    let result = host
        .execute(command)
        .await
        .context(format!("command execution failed on {}", hostname))?;

    host.disconnect().await.ok();

    // Display output
    if !result.stdout.is_empty() {
        println!("{}", result.stdout);
    }
    if !result.stderr.is_empty() {
        eprintln!("{}", result.stderr.red());
    }

    if result.success() {
        println!(
            "\n{} exit code: {} ({})",
            "=>".green().bold(),
            result.exit_code,
            format!("{:.2?}", result.duration).dimmed(),
        );
    } else {
        println!(
            "\n{} exit code: {} ({})",
            "=>".red().bold(),
            result.exit_code.to_string().red(),
            format!("{:.2?}", result.duration).dimmed(),
        );
    }

    Ok(())
}

async fn reboot_device(
    hostname: &str,
    reboot_type: RebootType,
    inventory_path: &PathBuf,
) -> Result<()> {
    let inventory = load_inventory(inventory_path)?;
    let entry = inventory
        .get_device(hostname)
        .ok_or_else(|| anyhow::anyhow!("device `{}` not found in inventory", hostname))?;

    println!(
        "{} Rebooting {} ({} reboot) ...",
        "=>".yellow().bold(),
        hostname.cyan(),
        reboot_type.to_string().yellow(),
    );

    let device_info = build_device_info(hostname, entry);
    let mut host = create_host(device_info);

    host.connect()
        .await
        .context(format!("failed to connect to {}", hostname))?;

    host.reboot(reboot_type)
        .await
        .context(format!("reboot failed on {}", hostname))?;

    println!(
        "{} Reboot command sent. Waiting for device to come back...",
        "=>".green().bold()
    );

    host.wait_ready(300)
        .await
        .context(format!("device {} did not come back within timeout", hostname))?;

    println!(
        "{} Device {} is back online.",
        "OK".green().bold(),
        hostname.cyan(),
    );

    Ok(())
}

async fn collect_facts(
    hostname: &str,
    facts_type: FactsType,
    inventory_path: &PathBuf,
) -> Result<()> {
    let inventory = load_inventory(inventory_path)?;
    let entry = inventory
        .get_device(hostname)
        .ok_or_else(|| anyhow::anyhow!("device `{}` not found in inventory", hostname))?;

    println!(
        "{} Collecting {:?} facts from {} ...\n",
        "=>".green().bold(),
        facts_type,
        hostname.cyan(),
    );

    // Build a SonicHost directly so we have access to the FactsProvider trait.
    // create_host returns a trait object; for facts we use the concrete type.
    let device_info = build_device_info(hostname, entry);
    let mut host = SonicHost::new(device_info);
    host.connect()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context(format!("failed to connect to {}", hostname))?;

    match facts_type {
        FactsType::Basic => {
            let facts = host
                .basic_facts()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .context("failed to collect basic facts")?;
            print_basic_facts(&facts);
        }
        FactsType::Bgp => {
            let facts = host
                .bgp_facts()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .context("failed to collect BGP facts")?;
            print_bgp_facts(&facts);
        }
        FactsType::Interface => {
            let facts = host
                .interface_facts()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .context("failed to collect interface facts")?;
            print_interface_facts(&facts);
        }
        FactsType::Config => {
            let facts = host
                .config_facts()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .context("failed to collect config facts")?;
            print_config_facts(&facts);
        }
    }

    host.disconnect()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

fn print_basic_facts(facts: &BasicFacts) {
    println!("{}", "Basic Facts".cyan().bold());
    println!("{}", "=".repeat(40));
    println!("{:<20} {}", "Hostname:".bold(), facts.hostname);
    println!("{:<20} {}", "HWSKU:".bold(), facts.hwsku);
    println!("{:<20} {}", "Platform:".bold(), facts.platform);
    println!("{:<20} {}", "OS Version:".bold(), facts.os_version);
    println!("{:<20} {}", "Serial:".bold(), facts.serial_number);
    println!("{:<20} {}", "Model:".bold(), facts.model);
    println!("{:<20} {}", "MAC Address:".bold(), facts.mac_address);
    println!("{:<20} {}s", "Uptime:".bold(), facts.uptime);
    println!("{:<20} {}", "ASIC Type:".bold(), facts.asic_type);
    println!("{:<20} {}", "Kernel:".bold(), facts.kernel_version);
}

fn print_bgp_facts(facts: &BgpFacts) {
    println!("{}", "BGP Facts".cyan().bold());
    println!("{}", "=".repeat(60));
    println!("{:<20} {}", "Router ID:".bold(), facts.router_id);
    println!("{:<20} {}", "Local AS:".bold(), facts.local_as);

    if !facts.neighbors.is_empty() {
        println!("\n{}", "Neighbors:".bold().underline());
        println!(
            "  {:<18} {:<10} {:<10} {:<14} {:<8} {:<8}",
            "ADDRESS".bold(),
            "REMOTE AS".bold(),
            "LOCAL AS".bold(),
            "STATE".bold(),
            "RX PFX".bold(),
            "TX PFX".bold(),
        );
        println!("  {}", "-".repeat(70));
        for n in &facts.neighbors {
            let state_colored = match n.state {
                sonic_core::BgpState::Established => n.state.to_string().green(),
                sonic_core::BgpState::Active | sonic_core::BgpState::Connect => {
                    n.state.to_string().yellow()
                }
                _ => n.state.to_string().red(),
            };
            println!(
                "  {:<18} {:<10} {:<10} {:<14} {:<8} {:<8}",
                n.address,
                n.remote_as,
                n.local_as,
                state_colored,
                n.prefixes_received,
                n.prefixes_sent,
            );
        }
    }
}

fn print_interface_facts(facts: &InterfaceFacts) {
    println!("{}", "Interface Facts".cyan().bold());
    println!("{}", "=".repeat(60));

    if !facts.ports.is_empty() {
        println!("\n{}", "Ports:".bold().underline());
        println!(
            "  {:<16} {:<10} {:<12} {:<8} {:<8}",
            "NAME".bold(),
            "SPEED".bold(),
            "ADMIN".bold(),
            "OPER".bold(),
            "MTU".bold(),
        );
        println!("  {}", "-".repeat(56));
        for p in &facts.ports {
            let oper_colored = match p.oper_status {
                sonic_core::PortStatus::Up => "up".green(),
                sonic_core::PortStatus::Down => "down".red(),
                sonic_core::PortStatus::NotPresent => "n/a".dimmed(),
            };
            println!(
                "  {:<16} {:<10} {:<12} {:<8} {:<8}",
                p.name,
                format_speed(p.speed),
                p.admin_status,
                oper_colored,
                p.mtu,
            );
        }
    }

    if !facts.vlans.is_empty() {
        println!("\n{} ({} total)", "VLANs:".bold().underline(), facts.vlans.len());
        for v in &facts.vlans {
            println!(
                "  VLAN {} ({}): {} member(s)",
                v.id.to_string().yellow(),
                v.name,
                v.members.len(),
            );
        }
    }

    if !facts.lags.is_empty() {
        println!("\n{} ({} total)", "LAGs:".bold().underline(), facts.lags.len());
        for lag in &facts.lags {
            println!(
                "  {} ({}): {} member(s), min-links={}",
                lag.name.yellow(),
                format_lacp_mode(&lag.lacp_mode),
                lag.members.len(),
                lag.min_links,
            );
        }
    }
}

fn print_config_facts(facts: &ConfigFacts) {
    println!("{}", "Configuration Facts".cyan().bold());
    println!("{}", "=".repeat(50));

    if !facts.features.is_empty() {
        println!("\n{}", "Features:".bold().underline());
        println!(
            "  {:<24} {:<12} {:<14}",
            "NAME".bold(),
            "STATE".bold(),
            "AUTO-RESTART".bold(),
        );
        println!("  {}", "-".repeat(52));
        let mut features: Vec<_> = facts.features.values().collect();
        features.sort_by(|a, b| a.name.cmp(&b.name));
        for f in features {
            let state_colored = if f.state == "enabled" {
                f.state.green()
            } else {
                f.state.red()
            };
            println!(
                "  {:<24} {:<12} {:<14}",
                f.name,
                state_colored,
                if f.auto_restart { "yes".green() } else { "no".dimmed() },
            );
        }
    }

    if !facts.services.is_empty() {
        println!("\n{}", "Services:".bold().underline());
        for svc in &facts.services {
            let status_colored = if svc.status == "running" {
                svc.status.green()
            } else {
                svc.status.red()
            };
            println!(
                "  {:<24} {} (pid: {})",
                svc.name,
                status_colored,
                svc.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            );
        }
    }

    println!(
        "\n  Running config tables: {}",
        facts.running_config.len().to_string().green()
    );
    println!(
        "  Startup config tables: {}",
        facts.startup_config.len().to_string().green()
    );
}

fn format_speed(speed_bps: u64) -> String {
    match speed_bps {
        s if s >= 100_000_000_000 => format!("{}G", s / 1_000_000_000),
        s if s >= 1_000_000_000 => format!("{}G", s / 1_000_000_000),
        s if s >= 1_000_000 => format!("{}M", s / 1_000_000),
        s => format!("{s}"),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_inventory(path: &PathBuf) -> Result<InventoryConfig> {
    sonic_config::inventory::load_inventory(path)
        .context(format!("failed to load inventory from {}", path.display()))
}

fn build_device_info(hostname: &str, entry: &DeviceEntry) -> DeviceInfo {
    let creds = entry.to_core_credentials();
    let mut info = DeviceInfo::new(hostname, entry.mgmt_ip, entry.device_type, creds);
    info.platform = entry.platform;
    info.hwsku = entry.hwsku.clone();
    info.port = entry.connection.port;
    info.connection_type = entry.connection.connection_type;
    info
}

fn format_lacp_mode(mode: &LacpMode) -> &'static str {
    match mode {
        LacpMode::Active => "active",
        LacpMode::Passive => "passive",
        LacpMode::On => "on",
    }
}
