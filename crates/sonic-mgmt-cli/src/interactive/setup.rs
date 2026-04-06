//! First-time interactive setup wizard.
//!
//! Guides the user through creating a `sonic-mgmt.toml` configuration file
//! with sensible defaults for every section: testbed, connection, testing,
//! reporting, topology, and logging.

use std::path::PathBuf;

use anyhow::{Context, Result};
use colored::Colorize;
use console::Term;
use inquire::{Confirm, Select, Text};

use sonic_config::app::{
    AppConfig, ConnectionSection, LogFormat, LoggingSection, ReportingSection, TestbedSection,
    TestingSection, TopologySection,
};
use sonic_core::{AuthMethod, ReportFormat};

use super::prompts;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Runs the full guided first-time setup wizard and writes
/// `sonic-mgmt.toml`.
pub async fn run_initial_setup() -> Result<()> {
    let term = Term::stdout();

    // ---- Welcome --------------------------------------------------------
    term.clear_screen().ok();
    println!("{}", "=".repeat(60).cyan());
    println!(
        "{}",
        "  SONiC Management Framework -- Initial Setup".cyan().bold()
    );
    println!("{}", "=".repeat(60).cyan());
    println!();
    println!(
        "This wizard will create a {} configuration file.",
        "sonic-mgmt.toml".yellow().bold()
    );
    println!("You can re-run this at any time with `sonic-mgmt init`.");
    println!();

    // 1. Project name (used as the testbed active name placeholder)
    let project_name = Text::new("Project / testbed name:")
        .with_default("sonic-testbed")
        .with_help_message("A short identifier for this testbed setup")
        .prompt()?;

    // 2. Default topology type
    let topo_type = prompts::prompt_topology_type()
        .context("topology selection failed")?;

    // 3. Management network CIDR
    let mgmt_network = Text::new("Management network CIDR:")
        .with_default("10.250.0.0/24")
        .with_help_message("IP range used for VM management interfaces")
        .with_validator(|val: &str| {
            Ok(val.parse::<ipnetwork::IpNetwork>()
                .map(|_| inquire::validator::Validation::Valid)
                .unwrap_or_else(|_| {
                    inquire::validator::Validation::Invalid(
                        "Enter a valid CIDR (e.g. 10.250.0.0/24).".into(),
                    )
                }))
        })
        .prompt()?;

    // 4. VM type
    let vm_type = prompts::prompt_vm_type()
        .context("VM type selection failed")?;

    // 5. DUT configuration (optional)
    let configure_duts = Confirm::new("Configure DUT devices now?")
        .with_default(false)
        .with_help_message("You can add DUTs later in the testbed config file")
        .prompt()?;

    let mut dut_hostnames: Vec<String> = Vec::new();
    if configure_duts {
        loop {
            println!("\n{}", "--- DUT Configuration ---".bold());

            let hostname = Text::new("DUT hostname:")
                .with_help_message("Hostname as it appears in SONiC DEVICE_METADATA")
                .prompt()?;

            let _mgmt_ip_str = Text::new("DUT management IP:")
                .with_help_message("IPv4 or IPv6 address for SSH access")
                .with_validator(|val: &str| {
                    Ok(val.parse::<std::net::IpAddr>()
                        .map(|_| inquire::validator::Validation::Valid)
                        .unwrap_or_else(|_| {
                            inquire::validator::Validation::Invalid(
                                "Not a valid IP address.".into(),
                            )
                        }))
                })
                .prompt()?;

            let _platform = prompts::prompt_platform()?;

            let _hwsku = Text::new("Hardware SKU:")
                .with_default("ACS-MSN2700")
                .with_help_message("e.g. ACS-MSN2700, Force10-S6000, ...")
                .prompt()?;

            let _username = Text::new("SSH username:")
                .with_default("admin")
                .prompt()?;

            let _password = Text::new("SSH password (leave blank to skip):")
                .with_help_message("Press Enter if using key-based auth")
                .prompt()?;

            dut_hostnames.push(hostname);

            let add_more = Confirm::new("Add another DUT?")
                .with_default(false)
                .prompt()?;

            if !add_more {
                break;
            }
        }
    }

    // 6. Logging level
    let log_level = prompts::prompt_log_level()
        .context("log level selection failed")?;

    // 7. Report backend
    let report_options = vec!["Kusto (Azure Data Explorer)", "Local file storage", "None"];
    let report_choice = Select::new("Reporting backend:", report_options.clone())
        .with_starting_cursor(2) // default to "None"
        .with_help_message("Where to upload test results for analytics")
        .prompt()?;

    let mut backend_url: Option<String> = None;
    let mut database = String::new();
    let mut table = String::new();
    let mut auth_method = AuthMethod::AzureDefault;

    if report_choice == report_options[0] {
        // Kusto
        let url = Text::new("Kusto cluster URL:")
            .with_help_message("e.g. https://mycluster.kusto.windows.net")
            .prompt()?;
        backend_url = Some(url);

        database = Text::new("Kusto database name:")
            .with_default("SonicTestResults")
            .prompt()?;

        table = Text::new("Kusto table name:")
            .with_default("TestResults")
            .prompt()?;

        let auth_options = vec![
            "Azure Default",
            "Azure CLI",
            "Managed Identity",
            "Device Code",
            "App Key",
        ];
        let auth_choice = Select::new("Authentication method:", auth_options.clone())
            .with_starting_cursor(0)
            .with_help_message("How to authenticate to the Kusto cluster")
            .prompt()?;

        auth_method = match auth_choice {
            s if s == auth_options[0] => AuthMethod::AzureDefault,
            s if s == auth_options[1] => AuthMethod::AzureCli,
            s if s == auth_options[2] => AuthMethod::ManagedIdentity,
            s if s == auth_options[3] => AuthMethod::DeviceCode,
            s if s == auth_options[4] => AuthMethod::AppKey,
            _ => AuthMethod::AzureDefault,
        };
    }

    // ---- Build config ---------------------------------------------------
    let vm_base_ip = mgmt_network
        .split('/')
        .next()
        .unwrap_or("10.250.0.2")
        .to_string();
    // Offset base IP by 2 so .0 = network, .1 = gateway
    let base_octets: Vec<&str> = vm_base_ip.split('.').collect();
    let adjusted_base = if base_octets.len() == 4 {
        format!(
            "{}.{}.{}.{}",
            base_octets[0],
            base_octets[1],
            base_octets[2],
            base_octets[3]
                .parse::<u32>()
                .unwrap_or(0)
                .saturating_add(2)
        )
    } else {
        vm_base_ip.clone()
    };

    let config = AppConfig {
        testbed: TestbedSection {
            active: project_name.clone(),
            file: PathBuf::from("testbed.toml"),
        },
        connection: ConnectionSection {
            default_ssh_port: 22,
            timeout_secs: 30,
            retries: 3,
            key_path: None,
        },
        testing: TestingSection {
            parallel_workers: 1,
            timeout_secs: 900,
            output_dir: PathBuf::from("output"),
            report_format: ReportFormat::JunitXml,
        },
        reporting: ReportingSection {
            backend_url,
            auth_method,
            database,
            table,
        },
        topology: TopologySection {
            vm_base_ip: adjusted_base,
            vlan_base: 1000,
            ip_offset: 1,
        },
        logging: LoggingSection {
            level: log_level.clone(),
            file: None,
            format: LogFormat::Full,
        },
    };

    // ---- Write to disk --------------------------------------------------
    let config_path = "sonic-mgmt.toml";
    config
        .save(config_path)
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to write configuration file")?;

    // ---- Summary --------------------------------------------------------
    println!("\n{}", "=".repeat(60).green());
    println!("{}", "  Setup Complete!".green().bold());
    println!("{}", "=".repeat(60).green());
    println!();
    println!(
        "  Configuration written to: {}",
        config_path.yellow().bold()
    );
    println!("  Project name:             {}", project_name.cyan());
    println!("  Topology:                 {}", topo_type);
    println!("  VM type:                  {}", vm_type);
    println!("  Management network:       {}", mgmt_network);
    println!("  Log level:                {}", log_level);
    if !dut_hostnames.is_empty() {
        println!(
            "  DUTs configured:          {}",
            dut_hostnames.join(", ").cyan()
        );
    }
    println!();
    println!("{}", "Next steps:".bold().underline());
    println!(
        "  1. Create a testbed file:  {}",
        "sonic-mgmt testbed wizard".yellow()
    );
    println!(
        "  2. Deploy topology:        {}",
        format!("sonic-mgmt testbed deploy {}", project_name).yellow()
    );
    println!(
        "  3. Run tests:              {}",
        "sonic-mgmt test run --path tests/".yellow()
    );
    println!();

    Ok(())
}
