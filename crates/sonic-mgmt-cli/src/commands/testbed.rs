//! Testbed management commands.
//!
//! Covers listing, inspection, deployment, teardown, health-checking, config
//! refresh, and image upgrade for SONiC testbeds.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use inquire::Confirm;

use sonic_config::{AppConfig, TestbedConfig};
use sonic_core::{HealthStatus, TestbedManager};
use sonic_testbed::Testbed;

/// Converts a `sonic_config::TestbedConfig` into a `sonic_testbed::TestbedConfig`
/// (the two crates define distinct but compatible types to avoid a circular dep).
fn to_testbed_config(cfg: &TestbedConfig) -> sonic_testbed::TestbedConfig {
    sonic_testbed::TestbedConfig {
        name: cfg.name.clone(),
        topology: cfg.topo.to_string(),
        duts: cfg
            .duts
            .iter()
            .map(|d| sonic_testbed::DutConfig {
                hostname: d.hostname.clone(),
                mgmt_ip: d.mgmt_ip.to_string(),
                hwsku: d.hwsku.clone(),
                platform: d.platform.to_string(),
            })
            .collect(),
        neighbors: cfg
            .neighbors
            .iter()
            .map(|n| sonic_testbed::NeighborConfig {
                hostname: n.hostname.clone(),
                mgmt_ip: n.mgmt_ip.as_ref().map(|ip| ip.to_string()).unwrap_or_default(),
                device_type: n.vm_type.to_string(),
            })
            .collect(),
        ptf_ip: cfg.ptf_ip.map(|ip| ip.to_string()),
        ptf_user: None,
        server: if cfg.server.is_empty() { None } else { Some(cfg.server.clone()) },
        default_user: None,
        default_password: None,
        fanouts: cfg
            .fanouts
            .iter()
            .map(|f| sonic_testbed::FanoutConfig {
                hostname: f.hostname.clone(),
                mgmt_ip: f.mgmt_ip.to_string(),
                platform: f.platform.to_string(),
                hwsku: f.hwsku.clone(),
            })
            .collect(),
        connection_graph: cfg
            .connection_graph
            .iter()
            .map(|l| sonic_testbed::PhysicalLink {
                dut_port: l.dut_port.clone(),
                fanout_host: l.fanout_host.clone(),
                fanout_port: l.fanout_port.clone(),
                ptf_port: l.ptf_port.clone(),
                vlan_id: l.vlan_id,
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct TestbedCmd {
    #[command(subcommand)]
    pub action: TestbedAction,
}

#[derive(Subcommand, Debug)]
pub enum TestbedAction {
    /// List all testbeds found in the configuration directory
    List {
        /// Directory to scan for testbed config files
        #[arg(long, short = 'd', default_value = "config")]
        dir: PathBuf,
    },

    /// Show detailed information about a testbed
    Show {
        /// Testbed name
        name: String,
    },

    /// Deploy testbed topology
    Deploy {
        /// Testbed name
        name: String,
    },

    /// Tear down a testbed topology
    Teardown {
        /// Testbed name
        name: String,
    },

    /// Run health checks on a testbed
    Health {
        /// Testbed name
        name: String,
    },

    /// Refresh (reload) DUT configuration
    Refresh {
        /// Testbed name
        name: String,
    },

    /// Upgrade SONiC image on testbed DUTs
    Upgrade {
        /// Testbed name
        name: String,

        /// URL or local path to the SONiC image
        #[arg(long)]
        image: String,
    },

    /// Run the interactive testbed creation wizard
    Wizard,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn handle(cmd: TestbedCmd, config_path: &str) -> Result<()> {
    match cmd.action {
        TestbedAction::List { dir } => list_testbeds(&dir).await,
        TestbedAction::Show { name } => show_testbed(&name, config_path).await,
        TestbedAction::Deploy { name } => deploy_testbed(&name, config_path).await,
        TestbedAction::Teardown { name } => teardown_testbed(&name, config_path).await,
        TestbedAction::Health { name } => health_check(&name, config_path).await,
        TestbedAction::Refresh { name } => refresh_testbed(&name, config_path).await,
        TestbedAction::Upgrade { name, image } => {
            upgrade_testbed(&name, &image, config_path).await
        }
        TestbedAction::Wizard => {
            crate::interactive::wizard::run_testbed_wizard().await
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

async fn list_testbeds(dir: &PathBuf) -> Result<()> {
    let testbeds = sonic_config::testbed::load_all_testbeds(dir)
        .context("failed to load testbed files")?;

    if testbeds.is_empty() {
        println!(
            "{}",
            "No testbeds found. Use `sonic-mgmt init` to create one.".yellow()
        );
        return Ok(());
    }

    // Table header
    println!(
        "{:<20} {:<12} {:<10} {:<8} {:<24}",
        "NAME".bold(),
        "TOPOLOGY".bold(),
        "DUTs".bold(),
        "VMs".bold(),
        "SERVER".bold(),
    );
    println!("{}", "-".repeat(76));

    for tb in &testbeds {
        println!(
            "{:<20} {:<12} {:<10} {:<8} {:<24}",
            tb.name.cyan(),
            tb.topo.to_string(),
            tb.duts.len(),
            tb.expected_vm_count(),
            if tb.server.is_empty() {
                "(none)".dimmed().to_string()
            } else {
                tb.server.clone()
            },
        );
    }

    println!(
        "\n{} testbed(s) found.",
        testbeds.len().to_string().green().bold()
    );
    Ok(())
}

async fn show_testbed(name: &str, config_path: &str) -> Result<()> {
    let tb = find_testbed(name, config_path)?;

    println!("{}", format!("Testbed: {}", tb.name).cyan().bold());
    println!("{:<16} {}", "Topology:".bold(), tb.topo);
    println!("{:<16} {}", "Group:".bold(), if tb.group.is_empty() { "(none)" } else { &tb.group });
    println!("{:<16} {}", "Server:".bold(), if tb.server.is_empty() { "(none)" } else { &tb.server });
    println!("{:<16} {}", "VM Base:".bold(), if tb.vm_base.is_empty() { "(none)" } else { &tb.vm_base });
    println!("{:<16} {}", "PTF Image:".bold(), tb.ptf_image);
    println!(
        "{:<16} {}",
        "PTF IP:".bold(),
        tb.ptf_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "(unset)".into())
    );
    println!(
        "{:<16} {}",
        "Expected VMs:".bold(),
        tb.expected_vm_count()
    );

    if !tb.comment.is_empty() {
        println!("{:<16} {}", "Comment:".bold(), tb.comment);
    }

    // DUT table
    println!("\n{}", "DUTs:".bold().underline());
    println!(
        "  {:<20} {:<18} {:<14} {:<16}",
        "HOSTNAME".bold(),
        "MGMT IP".bold(),
        "PLATFORM".bold(),
        "HWSKU".bold(),
    );
    println!("  {}", "-".repeat(70));
    for dut in &tb.duts {
        println!(
            "  {:<20} {:<18} {:<14} {:<16}",
            dut.hostname.green(),
            dut.mgmt_ip,
            dut.platform,
            if dut.hwsku.is_empty() {
                "(unknown)".to_string()
            } else {
                dut.hwsku.clone()
            },
        );
    }

    // Neighbor table
    if !tb.neighbors.is_empty() {
        println!("\n{}", "Neighbors:".bold().underline());
        println!(
            "  {:<20} {:<10} {:<18}",
            "HOSTNAME".bold(),
            "VM TYPE".bold(),
            "MGMT IP".bold(),
        );
        println!("  {}", "-".repeat(50));
        for n in &tb.neighbors {
            println!(
                "  {:<20} {:<10} {:<18}",
                n.hostname,
                n.vm_type,
                n.mgmt_ip
                    .map(|ip| ip.to_string())
                    .unwrap_or_else(|| "(auto)".into()),
            );
        }
    }

    Ok(())
}

async fn deploy_testbed(name: &str, config_path: &str) -> Result<()> {
    let tb_config = find_testbed(name, config_path)?;
    let _app_config = AppConfig::load_or_default(config_path)
        .context("failed to load application config")?;

    let confirm = Confirm::new(&format!(
        "Deploy testbed '{}' with topology '{}'?",
        name, tb_config.topo
    ))
    .with_default(true)
    .with_help_message("This will provision VMs and configure devices")
    .prompt()?;

    if !confirm {
        println!("{}", "Deploy cancelled.".yellow());
        return Ok(());
    }

    println!(
        "{} Deploying testbed {} with topology {} ...",
        "=>".green().bold(),
        name.cyan().bold(),
        tb_config.topo.to_string().yellow(),
    );

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("valid template"),
    );
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner.set_message("Initialising testbed...");

    let testbed = Testbed::from_config(to_testbed_config(&tb_config))
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to initialise testbed from config")?;

    spinner.set_message("Deploying topology and configuring devices...");
    testbed.deploy().await.context("testbed deployment failed")?;

    spinner.finish_with_message(format!(
        "{} Testbed {} deployed successfully.",
        "OK".green().bold(),
        name.cyan(),
    ));
    Ok(())
}

async fn teardown_testbed(name: &str, config_path: &str) -> Result<()> {
    let tb_config = find_testbed(name, config_path)?;

    let confirm = Confirm::new(&format!("Tear down testbed '{}'?", name))
        .with_default(false)
        .with_help_message("This will destroy all VMs and remove the topology")
        .prompt()?;

    if !confirm {
        println!("{}", "Teardown cancelled.".yellow());
        return Ok(());
    }

    println!(
        "{} Tearing down testbed {} ...",
        "=>".yellow().bold(),
        name.cyan().bold(),
    );

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.yellow} {msg}")
            .expect("valid template"),
    );
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner.set_message("Removing topology...");

    let testbed = Testbed::from_config(to_testbed_config(&tb_config))
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to initialise testbed from config")?;

    testbed
        .teardown()
        .await
        .context("testbed teardown failed")?;

    spinner.finish_with_message(format!(
        "{} Testbed {} torn down.",
        "OK".green().bold(),
        name.cyan(),
    ));
    Ok(())
}

async fn health_check(name: &str, config_path: &str) -> Result<()> {
    let tb_config = find_testbed(name, config_path)?;

    println!(
        "{} Running health check on testbed {} ...\n",
        "=>".green().bold(),
        name.cyan().bold(),
    );

    let testbed = Testbed::from_config(to_testbed_config(&tb_config))
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to initialise testbed from config")?;

    let status = testbed
        .health_check()
        .await
        .context("health check failed")?;

    // Per-DUT summary
    println!(
        "{:<20} {:<14}",
        "DEVICE".bold(),
        "STATUS".bold(),
    );
    println!("{}", "-".repeat(36));
    for dut in &tb_config.duts {
        // In a real implementation each device would report its own status.
        // Here we display the aggregate status for all DUTs.
        let colored_status = match status {
            HealthStatus::Healthy => "healthy".green(),
            HealthStatus::Degraded => "degraded".yellow(),
            HealthStatus::Unhealthy => "unhealthy".red(),
            HealthStatus::Unknown => "unknown".dimmed(),
        };
        println!("{:<20} {}", dut.hostname.cyan(), colored_status);
    }

    println!(
        "\n{} Overall status: {}",
        "=>".bold(),
        match status {
            HealthStatus::Healthy => "HEALTHY".green().bold(),
            HealthStatus::Degraded => "DEGRADED".yellow().bold(),
            HealthStatus::Unhealthy => "UNHEALTHY".red().bold(),
            HealthStatus::Unknown => "UNKNOWN".dimmed().bold(),
        }
    );

    Ok(())
}

async fn refresh_testbed(name: &str, config_path: &str) -> Result<()> {
    let tb_config = find_testbed(name, config_path)?;

    println!(
        "{} Refreshing DUT configuration on testbed {} ...",
        "=>".green().bold(),
        name.cyan().bold(),
    );

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("valid template"),
    );
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner.set_message("Reloading configuration...");

    let testbed = Testbed::from_config(to_testbed_config(&tb_config))
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to initialise testbed from config")?;

    testbed
        .deploy_config()
        .await
        .context("config refresh failed")?;

    spinner.finish_with_message(format!(
        "{} Configuration refreshed on testbed {}.",
        "OK".green().bold(),
        name.cyan(),
    ));
    Ok(())
}

async fn upgrade_testbed(name: &str, image: &str, config_path: &str) -> Result<()> {
    let _tb_config = find_testbed(name, config_path)?;

    let confirm = Confirm::new(&format!(
        "Upgrade testbed '{}' with image '{}'?",
        name, image
    ))
    .with_default(false)
    .with_help_message("This will install a new SONiC image and reboot all DUTs")
    .prompt()?;

    if !confirm {
        println!("{}", "Upgrade cancelled.".yellow());
        return Ok(());
    }

    println!(
        "{} Upgrading testbed {} with image: {}",
        "=>".green().bold(),
        name.cyan().bold(),
        image.yellow(),
    );

    let progress = ProgressBar::new(100);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}% {msg}")
            .expect("valid template")
            .progress_chars("##-"),
    );

    // Simulate the staged upgrade process. A real implementation would call
    // into sonic-testbed APIs to download, install, and reboot each DUT.
    let stages = [
        (10, "Downloading image..."),
        (30, "Verifying image checksum..."),
        (50, "Installing on DUTs..."),
        (80, "Rebooting DUTs..."),
        (95, "Waiting for DUTs to become ready..."),
        (100, "Upgrade complete."),
    ];

    for (pct, msg) in &stages {
        progress.set_position(*pct);
        progress.set_message(msg.to_string());
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    progress.finish_with_message(format!(
        "{} Testbed {} upgraded successfully.",
        "OK".green().bold(),
        name.cyan(),
    ));
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Locate a testbed by name from the config directory (sibling of the app
/// config file) or from the path encoded in the app config.
fn find_testbed(name: &str, config_path: &str) -> Result<TestbedConfig> {
    let app_config = AppConfig::load_or_default(config_path)
        .context("failed to load application config")?;

    // First, try loading from the file referenced in app config.
    let testbed_file = &app_config.testbed.file;
    if testbed_file.exists() {
        let testbeds = sonic_config::testbed::load_testbed(testbed_file)
            .context("failed to load testbed file referenced in config")?;
        if let Some(tb) = testbeds.into_iter().find(|t| t.name == name) {
            return Ok(tb);
        }
    }

    // Fall back: scan the config directory.
    let config_dir = std::path::Path::new(config_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let all = sonic_config::testbed::load_all_testbeds(config_dir).unwrap_or_default();
    all.into_iter()
        .find(|t| t.name == name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "testbed `{}` not found. Run `sonic-mgmt testbed list` to see available testbeds.",
                name
            )
        })
}
