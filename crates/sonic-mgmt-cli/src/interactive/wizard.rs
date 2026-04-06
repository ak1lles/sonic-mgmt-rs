//! Interactive wizards for testbed creation, inventory setup, and test
//! planning.
//!
//! Each wizard function collects user input via `inquire` prompts, validates
//! inputs at every step, builds the appropriate config struct, serialises it
//! to TOML, and writes the result to disk.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use colored::Colorize;
use inquire::{Confirm, CustomType, MultiSelect, Select, Text};

use sonic_config::inventory::{
    ConnectionInfo, DeviceEntry, InventoryConfig, InventoryCredentials,
};
use sonic_config::testbed::{DutConfig, DutCredentials, TestbedConfig};
use sonic_core::{ConnectionType, TopologyType};

use super::prompts;

// ---------------------------------------------------------------------------
// Testbed wizard
// ---------------------------------------------------------------------------

/// Interactive wizard that creates a complete testbed definition file.
pub async fn run_testbed_wizard() -> Result<()> {
    println!("\n{}", "=".repeat(50).cyan());
    println!(
        "{}",
        "  Testbed Creation Wizard".cyan().bold()
    );
    println!("{}", "=".repeat(50).cyan());
    println!();

    // 1. Testbed name
    let name = Text::new("Testbed name:")
        .with_default("vms-t0")
        .with_help_message("Unique identifier for this testbed (e.g. vms-t0, lab-t1)")
        .with_validator(|val: &str| {
            if val.trim().is_empty() {
                Ok(inquire::validator::Validation::Invalid(
                    "Testbed name must not be empty.".into(),
                ))
            } else if val.contains(' ') {
                Ok(inquire::validator::Validation::Invalid(
                    "Testbed name must not contain spaces.".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()?;

    // 2. Topology type
    let topo_type = prompts::prompt_topology_type()
        .context("topology selection failed")?;

    // 3. VM type
    let _vm_type = prompts::prompt_vm_type()
        .context("VM type selection failed")?;

    // 4. DUT configuration loop
    println!("\n{}", "--- DUT Configuration ---".bold());
    let mut duts: Vec<DutConfig> = Vec::new();

    loop {
        println!(
            "\n{} DUT #{}\n",
            "=>".green().bold(),
            duts.len() + 1
        );

        let hostname = Text::new("DUT hostname:")
            .with_help_message("Hostname as configured in SONiC DEVICE_METADATA")
            .with_validator(|val: &str| {
                if val.trim().is_empty() {
                    Ok(inquire::validator::Validation::Invalid(
                        "Hostname must not be empty.".into(),
                    ))
                } else {
                    Ok(inquire::validator::Validation::Valid)
                }
            })
            .prompt()?;

        let mgmt_ip = prompts::prompt_ip_address("DUT management IP:")?;
        let platform = prompts::prompt_platform()?;

        let hwsku = Text::new("Hardware SKU:")
            .with_default("ACS-MSN2700")
            .with_help_message("e.g. ACS-MSN2700, Force10-S6000, Arista-7060CX-32S")
            .prompt()?;

        let username = Text::new("SSH username:")
            .with_default("admin")
            .with_help_message("Login username for the DUT")
            .prompt()?;

        let password_raw = inquire::Password::new("SSH password:")
            .without_confirmation()
            .with_help_message("Press Enter to skip if using key-based auth")
            .prompt()?;
        let password = if password_raw.is_empty() {
            None
        } else {
            Some(password_raw)
        };

        let key_path_str = Text::new("SSH key path (leave blank to skip):")
            .with_help_message("Absolute path to the private key file")
            .prompt()?;
        let key_path = if key_path_str.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(key_path_str))
        };

        duts.push(DutConfig {
            hostname,
            mgmt_ip,
            hwsku,
            platform,
            credentials: DutCredentials {
                username,
                password,
                key_path,
            },
            metadata: HashMap::new(),
        });

        let add_more = Confirm::new("Add another DUT?")
            .with_default(false)
            .prompt()?;

        if !add_more {
            break;
        }
    }

    // 5. PTF configuration
    println!("\n{}", "--- PTF Configuration ---".bold());
    let ptf_image = Text::new("PTF container image:")
        .with_default("docker-ptf")
        .with_help_message("Docker image name for the PTF container")
        .prompt()?;

    let ptf_ip_str = Text::new("PTF management IP (leave blank for auto):")
        .with_help_message("IPv4 address for the PTF container")
        .prompt()?;
    let ptf_ip: Option<IpAddr> = if ptf_ip_str.trim().is_empty() {
        None
    } else {
        Some(
            ptf_ip_str
                .parse()
                .context("invalid PTF IP address")?,
        )
    };

    // 6. Server hostname
    let server = Text::new("Server hostname:")
        .with_default("server-1")
        .with_help_message("Physical server hosting VMs for this testbed")
        .prompt()?;

    // 7. VM base name
    let vm_base = Text::new("VM base name:")
        .with_default("VM0100")
        .with_help_message("Base name for neighbor VMs (e.g. VM0100)")
        .prompt()?;

    // 8. Review
    let save = Confirm::new("Review and save this testbed?")
        .with_default(true)
        .prompt()?;

    if !save {
        println!("{}", "Testbed creation cancelled.".yellow());
        return Ok(());
    }

    // Build config
    let testbed = TestbedConfig {
        name: name.clone(),
        group: String::new(),
        topo: topo_type,
        ptf_image,
        ptf_ip,
        server,
        vm_base,
        duts,
        neighbors: vec![],
        comment: format!("Created by testbed wizard"),
        metadata: HashMap::new(),
    };

    // Validate
    testbed
        .validate()
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("testbed validation failed")?;

    // 9. Summary and write
    let output_path = format!("config/{}.toml", name);
    let toml_str =
        toml::to_string_pretty(&testbed).context("failed to serialise testbed to TOML")?;

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).context("failed to create config directory")?;
    }
    std::fs::write(&output_path, &toml_str)
        .context(format!("failed to write {}", output_path))?;

    println!("\n{}", "=".repeat(50).green());
    println!("{}", "  Testbed Created!".green().bold());
    println!("{}", "=".repeat(50).green());
    println!();
    println!("  Name:       {}", testbed.name.cyan());
    println!("  Topology:   {}", testbed.topo);
    println!("  DUTs:       {}", testbed.duts.len());
    println!("  VMs needed: {}", testbed.expected_vm_count());
    println!("  Written to: {}", output_path.yellow());
    println!();
    println!(
        "  Deploy with: {}",
        format!("sonic-mgmt testbed deploy {}", testbed.name).yellow()
    );
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Inventory wizard
// ---------------------------------------------------------------------------

/// Interactive wizard that creates a device inventory file.
pub async fn run_inventory_wizard() -> Result<()> {
    println!("\n{}", "=".repeat(50).cyan());
    println!(
        "{}",
        "  Inventory Creation Wizard".cyan().bold()
    );
    println!("{}", "=".repeat(50).cyan());
    println!();

    // 1. Inventory name
    let inv_name = Text::new("Inventory name:")
        .with_default("lab")
        .with_help_message("Short name for this inventory file")
        .prompt()?;

    // 2. Device entry loop
    let mut devices: HashMap<String, DeviceEntry> = HashMap::new();

    loop {
        println!(
            "\n{} Device #{}\n",
            "=>".green().bold(),
            devices.len() + 1
        );

        let hostname = Text::new("Device hostname:")
            .with_validator(|val: &str| {
                if val.trim().is_empty() {
                    Ok(inquire::validator::Validation::Invalid(
                        "Hostname must not be empty.".into(),
                    ))
                } else {
                    Ok(inquire::validator::Validation::Valid)
                }
            })
            .prompt()?;

        let mgmt_ip = prompts::prompt_ip_address("Management IP:")?;
        let device_type = prompts::prompt_device_type()?;
        let platform = prompts::prompt_platform()?;

        let hwsku = Text::new("Hardware SKU:")
            .with_default("ACS-MSN2700")
            .with_help_message("Hardware SKU string")
            .prompt()?;

        let username = Text::new("SSH username:")
            .with_default("admin")
            .prompt()?;

        let password_raw = inquire::Password::new("SSH password (leave blank to skip):")
            .without_confirmation()
            .prompt()?;
        let password = if password_raw.is_empty() {
            None
        } else {
            Some(password_raw)
        };

        let key_path_str = Text::new("SSH key path (leave blank to skip):")
            .prompt()?;
        let key_path = if key_path_str.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(key_path_str))
        };

        let port = prompts::prompt_port("SSH port:", 22)?;

        devices.insert(
            hostname,
            DeviceEntry {
                mgmt_ip,
                device_type,
                platform,
                hwsku,
                credentials: InventoryCredentials {
                    username,
                    password,
                    key_path,
                },
                connection: ConnectionInfo {
                    connection_type: ConnectionType::Ssh,
                    port,
                    timeout_secs: None,
                },
                console: None,
                metadata: HashMap::new(),
            },
        );

        let add_more = Confirm::new("Add another device?")
            .with_default(false)
            .prompt()?;

        if !add_more {
            break;
        }
    }

    // 3. Group creation
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    let all_hostnames: Vec<String> = devices.keys().cloned().collect();

    let create_groups = Confirm::new("Create device groups?")
        .with_default(true)
        .with_help_message("Groups let you target multiple devices at once")
        .prompt()?;

    if create_groups {
        // Always create an "all" group
        groups.insert("all".to_string(), all_hostnames.clone());

        loop {
            let group_name = Text::new("Group name:")
                .with_help_message("e.g. duts, fanouts, ptf-hosts")
                .prompt()?;

            if group_name.trim().is_empty() {
                break;
            }

            let selected = MultiSelect::new(
                &format!("Select devices for group '{}':", group_name),
                all_hostnames.clone(),
            )
            .with_help_message("Use Space to select, Enter to confirm")
            .prompt()?;

            if !selected.is_empty() {
                groups.insert(group_name, selected);
            }

            let add_more = Confirm::new("Create another group?")
                .with_default(false)
                .prompt()?;

            if !add_more {
                break;
            }
        }
    }

    // 4. Build and write
    let inventory = InventoryConfig { devices, groups };

    inventory
        .validate()
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("inventory validation failed")?;

    let output_path = format!("config/{}-inventory.toml", inv_name);
    let toml_str =
        toml::to_string_pretty(&inventory).context("failed to serialise inventory to TOML")?;

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, &toml_str)
        .context(format!("failed to write {}", output_path))?;

    println!("\n{}", "=".repeat(50).green());
    println!("{}", "  Inventory Created!".green().bold());
    println!("{}", "=".repeat(50).green());
    println!();
    println!("  Devices:    {}", inventory.device_count());
    println!(
        "  Groups:     {}",
        inventory.groups.len()
    );
    println!("  Written to: {}", output_path.yellow());
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Test wizard
// ---------------------------------------------------------------------------

/// Interactive wizard that plans a test execution session.
pub async fn run_test_wizard() -> Result<()> {
    println!("\n{}", "=".repeat(50).cyan());
    println!(
        "{}",
        "  Test Execution Wizard".cyan().bold()
    );
    println!("{}", "=".repeat(50).cyan());
    println!();

    // 1. Test directory
    let test_dir = Text::new("Test directory path:")
        .with_default("tests")
        .with_help_message("Directory containing test definition files (.toml)")
        .prompt()?;

    // 2. Filter pattern
    let filter = Text::new("Filter pattern (leave blank for all):")
        .with_help_message("Glob or regex to filter test names (e.g. test_bgp*)")
        .prompt()?;
    let filter = if filter.trim().is_empty() {
        None
    } else {
        Some(filter)
    };

    // 3. Tags
    let available_tags = vec![
        "sanity".to_string(),
        "nightly".to_string(),
        "t0".to_string(),
        "t1".to_string(),
        "bgp".to_string(),
        "acl".to_string(),
        "ecmp".to_string(),
        "vlan".to_string(),
        "lag".to_string(),
        "qos".to_string(),
        "platform".to_string(),
        "warm-reboot".to_string(),
    ];

    let selected_tags = MultiSelect::new("Select test tags:", available_tags)
        .with_help_message("Use Space to select, Enter to confirm. Leave empty for all tags.")
        .prompt()?;

    // 4. Topology filter
    let topo_options = vec![
        "Any (no filter)".to_string(),
        "t0".to_string(),
        "t1".to_string(),
        "t2".to_string(),
        "dualtor".to_string(),
        "ptf".to_string(),
    ];
    let topo_choice = Select::new("Topology filter:", topo_options)
        .with_starting_cursor(0)
        .with_help_message("Only run tests for this topology type")
        .prompt()?;

    let _topo_filter: Option<TopologyType> = match topo_choice.as_str() {
        "t0" => Some(TopologyType::T0),
        "t1" => Some(TopologyType::T1),
        "t2" => Some(TopologyType::T2),
        "dualtor" => Some(TopologyType::Dualtor),
        "ptf" => Some(TopologyType::Ptf),
        _ => None,
    };

    // 5. Parallel workers
    let parallel = CustomType::<u32>::new("Number of parallel workers:")
        .with_default(1)
        .with_help_message("How many tests to run concurrently (1 = sequential)")
        .with_error_message("Enter a positive integer")
        .with_validator(|val: &u32| {
            if *val == 0 {
                Ok(inquire::validator::Validation::Invalid(
                    "At least 1 worker is required.".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()?;

    // 6. Fail-fast
    let fail_fast = Confirm::new("Stop on first failure (fail-fast)?")
        .with_default(false)
        .with_help_message("If yes, the run will abort after the first test failure")
        .prompt()?;

    // 7. Output format
    let format_options = vec![
        "JSON".to_string(),
        "TOML".to_string(),
        "JUnit XML".to_string(),
        "HTML".to_string(),
    ];
    let format_choice = Select::new("Output format:", format_options)
        .with_starting_cursor(0)
        .with_help_message("Format for the results report")
        .prompt()?;

    // 8. Display plan and confirm
    println!("\n{}", "--- Execution Plan ---".bold().underline());
    println!("  Test directory:  {}", test_dir.cyan());
    if let Some(ref f) = filter {
        println!("  Filter:          {}", f.yellow());
    } else {
        println!("  Filter:          (all tests)");
    }
    if !selected_tags.is_empty() {
        println!("  Tags:            {}", selected_tags.join(", ").yellow());
    } else {
        println!("  Tags:            (all)");
    }
    println!("  Topology:        {}", topo_choice);
    println!("  Workers:         {}", parallel);
    println!(
        "  Fail-fast:       {}",
        if fail_fast { "yes".yellow() } else { "no".dimmed() }
    );
    println!("  Output format:   {}", format_choice);
    println!();

    let proceed = Confirm::new("Proceed with test execution?")
        .with_default(true)
        .prompt()?;

    if !proceed {
        println!("{}", "Test execution cancelled.".yellow());
        return Ok(());
    }

    // Build the CLI command that would be equivalent.
    let mut cmd_parts = vec![
        "sonic-mgmt test run".to_string(),
        format!("--path {}", test_dir),
    ];
    if let Some(ref f) = filter {
        cmd_parts.push(format!("--filter '{}'", f));
    }
    for tag in &selected_tags {
        cmd_parts.push(format!("--tag {}", tag));
    }
    cmd_parts.push(format!("--parallel {}", parallel));
    if fail_fast {
        cmd_parts.push("--fail-fast".to_string());
    }

    println!(
        "\n{} Equivalent CLI command:\n  {}\n",
        "=>".green().bold(),
        cmd_parts.join(" ").yellow(),
    );

    println!(
        "{} To execute, run the command above or use `sonic-mgmt test run` directly.",
        "=>".green().bold()
    );

    Ok(())
}
