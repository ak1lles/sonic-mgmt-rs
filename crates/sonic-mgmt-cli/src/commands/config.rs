//! Configuration management commands.
//!
//! View, edit, validate, and interactively initialise configuration files.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use colored::Colorize;

use sonic_config::AppConfig;

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ConfigCmd {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Display the current application configuration
    Show,

    /// Open the configuration file in your default editor
    Edit,

    /// Validate a configuration file
    Validate {
        /// Path to the configuration file to validate
        path: PathBuf,
    },

    /// Run the interactive configuration wizard
    Init,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn handle(cmd: ConfigCmd, config_path: &str) -> Result<()> {
    match cmd.action {
        ConfigAction::Show => show_config(config_path).await,
        ConfigAction::Edit => edit_config(config_path).await,
        ConfigAction::Validate { path } => validate_config(&path).await,
        ConfigAction::Init => {
            crate::interactive::setup::run_initial_setup().await?;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

async fn show_config(config_path: &str) -> Result<()> {
    let config = AppConfig::load_or_default(config_path)
        .context("failed to load application config")?;

    let path = std::path::Path::new(config_path);
    if path.exists() {
        println!(
            "{} Loaded from: {}\n",
            "=>".green().bold(),
            config_path.cyan(),
        );
    } else {
        println!(
            "{} No config file found at {}; showing defaults.\n",
            "=>".yellow().bold(),
            config_path.yellow(),
        );
    }

    // Testbed section
    println!("{}", "[testbed]".bold().underline());
    println!(
        "  {:<22} {}",
        "active:".bold(),
        if config.testbed.active.is_empty() {
            "(none)".dimmed().to_string()
        } else {
            config.testbed.active.clone()
        }
    );
    println!(
        "  {:<22} {}",
        "file:".bold(),
        config.testbed.file.display()
    );

    // Connection section
    println!("\n{}", "[connection]".bold().underline());
    println!(
        "  {:<22} {}",
        "default_ssh_port:".bold(),
        config.connection.default_ssh_port
    );
    println!(
        "  {:<22} {}s",
        "timeout_secs:".bold(),
        config.connection.timeout_secs
    );
    println!(
        "  {:<22} {}",
        "retries:".bold(),
        config.connection.retries
    );
    println!(
        "  {:<22} {}",
        "key_path:".bold(),
        config
            .connection
            .key_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".into())
    );

    // Testing section
    println!("\n{}", "[testing]".bold().underline());
    println!(
        "  {:<22} {}",
        "parallel_workers:".bold(),
        config.testing.parallel_workers
    );
    println!(
        "  {:<22} {}s",
        "timeout_secs:".bold(),
        config.testing.timeout_secs
    );
    println!(
        "  {:<22} {}",
        "output_dir:".bold(),
        config.testing.output_dir.display()
    );
    println!(
        "  {:<22} {:?}",
        "report_format:".bold(),
        config.testing.report_format
    );

    // Reporting section
    println!("\n{}", "[reporting]".bold().underline());
    println!(
        "  {:<22} {}",
        "backend_url:".bold(),
        config
            .reporting
            .backend_url
            .as_deref()
            .unwrap_or("(none)")
    );
    println!(
        "  {:<22} {:?}",
        "auth_method:".bold(),
        config.reporting.auth_method
    );
    println!(
        "  {:<22} {}",
        "database:".bold(),
        if config.reporting.database.is_empty() {
            "(none)"
        } else {
            &config.reporting.database
        }
    );
    println!(
        "  {:<22} {}",
        "table:".bold(),
        if config.reporting.table.is_empty() {
            "(none)"
        } else {
            &config.reporting.table
        }
    );

    // Topology section
    println!("\n{}", "[topology]".bold().underline());
    println!(
        "  {:<22} {}",
        "vm_base_ip:".bold(),
        config.topology.vm_base_ip
    );
    println!(
        "  {:<22} {}",
        "vlan_base:".bold(),
        config.topology.vlan_base
    );
    println!(
        "  {:<22} {}",
        "ip_offset:".bold(),
        config.topology.ip_offset
    );

    // Logging section
    println!("\n{}", "[logging]".bold().underline());
    println!("  {:<22} {}", "level:".bold(), config.logging.level);
    println!(
        "  {:<22} {}",
        "file:".bold(),
        config
            .logging
            .file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(stderr only)".into())
    );
    println!("  {:<22} {}", "format:".bold(), config.logging.format);

    Ok(())
}

async fn edit_config(config_path: &str) -> Result<()> {
    let path = std::path::Path::new(config_path);

    // Ensure the file exists -- create defaults if not.
    if !path.exists() {
        println!(
            "{} Config file not found. Creating defaults at {} ...",
            "=>".yellow().bold(),
            config_path.cyan(),
        );
        let config = AppConfig::default();
        config
            .save(config_path)
            .map_err(|e| anyhow::anyhow!("{}", e))
            .context("failed to write default config")?;
    }

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    println!(
        "{} Opening {} in {} ...",
        "=>".green().bold(),
        config_path.cyan(),
        editor.yellow(),
    );

    let status = std::process::Command::new(&editor)
        .arg(config_path)
        .status()
        .context(format!("failed to launch editor `{}`", editor))?;

    if !status.success() {
        anyhow::bail!("editor exited with non-zero status");
    }

    // Validate after editing
    match AppConfig::load(config_path) {
        Ok(_) => {
            println!(
                "{} Configuration is valid.",
                "OK".green().bold()
            );
        }
        Err(e) => {
            println!(
                "{} Configuration has errors: {}",
                "WARN".yellow().bold(),
                e
            );
            println!(
                "{}",
                "The file was saved but may need corrections.".dimmed()
            );
        }
    }

    Ok(())
}

async fn validate_config(path: &PathBuf) -> Result<()> {
    println!(
        "{} Validating {} ...\n",
        "=>".green().bold(),
        path.display().to_string().cyan(),
    );

    // Attempt to load as AppConfig
    match AppConfig::load(path) {
        Ok(config) => {
            println!(
                "{} Application config is valid.",
                "OK".green().bold()
            );
            println!(
                "  Testbed: {}, Workers: {}, Timeout: {}s",
                if config.testbed.active.is_empty() {
                    "(none)".to_string()
                } else {
                    config.testbed.active
                },
                config.testing.parallel_workers,
                config.testing.timeout_secs,
            );
            return Ok(());
        }
        Err(app_err) => {
            // Try as testbed config
            match sonic_config::testbed::load_testbed(path) {
                Ok(testbeds) => {
                    println!(
                        "{} Testbed config is valid. {} testbed(s) defined.",
                        "OK".green().bold(),
                        testbeds.len(),
                    );
                    for tb in &testbeds {
                        println!(
                            "  - {} (topo: {}, {} DUT(s))",
                            tb.name.cyan(),
                            tb.topo,
                            tb.duts.len(),
                        );
                    }
                    return Ok(());
                }
                Err(_) => {}
            }

            // Try as inventory config
            match sonic_config::inventory::load_inventory(path) {
                Ok(inv) => {
                    println!(
                        "{} Inventory config is valid. {} device(s), {} group(s).",
                        "OK".green().bold(),
                        inv.device_count(),
                        inv.groups.len(),
                    );
                    return Ok(());
                }
                Err(_) => {}
            }

            // Nothing matched
            println!(
                "{} Validation failed.",
                "FAIL".red().bold()
            );
            println!("  {}", app_err);
            anyhow::bail!("configuration file is not valid");
        }
    }
}
