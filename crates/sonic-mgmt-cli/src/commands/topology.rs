//! Topology commands.
//!
//! List supported topologies, inspect their details, generate topology
//! definition files, and render topologies into different output formats.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use colored::Colorize;

use sonic_core::{TopologyGenerator, TopologyType, VmType};
use sonic_topology::{DefaultTopologyGenerator, TopologyRenderer};

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct TopologyCmd {
    #[command(subcommand)]
    pub action: TopologyAction,
}

#[derive(Subcommand, Debug)]
pub enum TopologyAction {
    /// List all supported topology types
    List,

    /// Show details of a topology type
    Show {
        /// Topology type (e.g. t0, t1, t1-lag, dualtor)
        #[arg(value_enum)]
        topo_type: TopoArg,
    },

    /// Generate a topology definition and write it to a TOML file
    Generate {
        /// Topology type
        #[arg(value_enum)]
        topo_type: TopoArg,

        /// Output file path
        #[arg(long, short = 'o', default_value = "topology.toml")]
        output: PathBuf,
    },

    /// Render a topology to a specific output format
    Render {
        /// Topology type
        #[arg(value_enum)]
        topo_type: TopoArg,

        /// Output format
        #[arg(long, short = 'f', value_enum, default_value = "inventory")]
        format: RenderFormat,
    },
}

/// CLI-friendly topology type enum that maps to `TopologyType`.
#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum TopoArg {
    T0,
    #[value(name = "t0-64")]
    T064,
    #[value(name = "t0-116")]
    T0116,
    T1,
    #[value(name = "t1-64")]
    T164,
    #[value(name = "t1-lag")]
    T1Lag,
    T2,
    Dualtor,
    #[value(name = "mgmt-tor")]
    MgmtTor,
    #[value(name = "m0-vlan")]
    M0Vlan,
    #[value(name = "ptf-32")]
    Ptf32,
    #[value(name = "ptf-64")]
    Ptf64,
    Ptf,
}

impl From<TopoArg> for TopologyType {
    fn from(arg: TopoArg) -> Self {
        match arg {
            TopoArg::T0 => TopologyType::T0,
            TopoArg::T064 => TopologyType::T064,
            TopoArg::T0116 => TopologyType::T0116,
            TopoArg::T1 => TopologyType::T1,
            TopoArg::T164 => TopologyType::T164,
            TopoArg::T1Lag => TopologyType::T1Lag,
            TopoArg::T2 => TopologyType::T2,
            TopoArg::Dualtor => TopologyType::Dualtor,
            TopoArg::MgmtTor => TopologyType::MgmtTor,
            TopoArg::M0Vlan => TopologyType::M0Vlan,
            TopoArg::Ptf32 => TopologyType::Ptf32,
            TopoArg::Ptf64 => TopologyType::Ptf64,
            TopoArg::Ptf => TopologyType::Ptf,
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum RenderFormat {
    Inventory,
    Minigraph,
    #[value(name = "config-db")]
    ConfigDb,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn handle(cmd: TopologyCmd, config_path: &str) -> Result<()> {
    match cmd.action {
        TopologyAction::List => list_topologies().await,
        TopologyAction::Show { topo_type } => show_topology(topo_type.into()).await,
        TopologyAction::Generate { topo_type, output } => {
            generate_topology(topo_type.into(), &output, config_path).await
        }
        TopologyAction::Render { topo_type, format } => {
            render_topology(topo_type.into(), format, config_path).await
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

async fn list_topologies() -> Result<()> {
    let generator = DefaultTopologyGenerator::new(VmType::Veos);
    let supported = generator.supported_topologies();

    println!(
        "{:<16} {:<8} {:<12} {}",
        "TOPOLOGY".bold(),
        "VMs".bold(),
        "PTF-ONLY".bold(),
        "DESCRIPTION".bold(),
    );
    println!("{}", "-".repeat(64));

    for topo in &supported {
        let desc = topology_description(*topo);
        println!(
            "{:<16} {:<8} {:<12} {}",
            topo.to_string().cyan(),
            topo.vm_count(),
            if topo.is_ptf_only() {
                "yes".yellow()
            } else {
                "no".dimmed()
            },
            desc,
        );
    }

    println!(
        "\n{} topology type(s) supported.",
        supported.len().to_string().green().bold()
    );
    Ok(())
}

async fn show_topology(topo_type: TopologyType) -> Result<()> {
    let generator = DefaultTopologyGenerator::new(VmType::Veos);
    let topo_def = generator
        .generate(topo_type)
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to generate topology definition")?;

    println!("{}", format!("Topology: {}", topo_type).cyan().bold());
    println!("{:<18} {}", "Type:".bold(), topo_type);
    println!("{:<18} {}", "VM Count:".bold(), topo_def.vms.len());
    println!("{:<18} {}", "VLAN Count:".bold(), topo_def.vlans.len());
    println!(
        "{:<18} {}",
        "Host Interfaces:".bold(),
        topo_def.host_interfaces.len()
    );
    println!("{:<18} {}", "LAG Links:".bold(), topo_def.lag_links.len());
    println!(
        "{:<18} {}",
        "IP Pairs:".bold(),
        topo_def.ip_pairs.len()
    );

    // VMs
    if !topo_def.vms.is_empty() {
        println!("\n{}", "Virtual Machines:".bold().underline());
        println!(
            "  {:<20} {:<10} {:<8} {:<18}",
            "NAME".bold(),
            "TYPE".bold(),
            "OFFSET".bold(),
            "MGMT IP".bold(),
        );
        println!("  {}", "-".repeat(58));
        for vm in &topo_def.vms {
            println!(
                "  {:<20} {:<10} {:<8} {:<18}",
                vm.name.green(),
                vm.vm_type,
                vm.vm_offset,
                vm.mgmt_ip,
            );
        }
    }

    // VLANs
    if !topo_def.vlans.is_empty() {
        println!("\n{}", "VLANs:".bold().underline());
        println!(
            "  {:<8} {:<16} {:<20} {}",
            "ID".bold(),
            "NAME".bold(),
            "PREFIX".bold(),
            "INTERFACES".bold(),
        );
        println!("  {}", "-".repeat(60));
        for vlan in &topo_def.vlans {
            println!(
                "  {:<8} {:<16} {:<20} {}",
                vlan.id.to_string().yellow(),
                vlan.name,
                vlan.prefix
                    .as_deref()
                    .unwrap_or("(none)"),
                if vlan.intfs.is_empty() {
                    "(none)".to_string()
                } else {
                    vlan.intfs.join(", ")
                },
            );
        }
    }

    // Host Interfaces
    if !topo_def.host_interfaces.is_empty() {
        println!("\n{}", "Host Interfaces:".bold().underline());
        println!(
            "  {:<10} {:<10} {:<16} {:<16}",
            "VM IDX".bold(),
            "PORT IDX".bold(),
            "DUT PORT".bold(),
            "PTF PORT".bold(),
        );
        println!("  {}", "-".repeat(54));
        for hi in topo_def.host_interfaces.iter().take(20) {
            println!(
                "  {:<10} {:<10} {:<16} {:<16}",
                hi.vm_index, hi.port_index, hi.dut_port, hi.ptf_port,
            );
        }
        if topo_def.host_interfaces.len() > 20 {
            println!(
                "  ... and {} more",
                topo_def.host_interfaces.len() - 20
            );
        }
    }

    Ok(())
}

async fn generate_topology(
    topo_type: TopologyType,
    output: &PathBuf,
    _config_path: &str,
) -> Result<()> {
    let generator = DefaultTopologyGenerator::new(VmType::Veos);
    let topo_def = generator
        .generate(topo_type)
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to generate topology")?;

    let toml_str = toml::to_string_pretty(&topo_def)
        .context("failed to serialise topology to TOML")?;

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .context("failed to create output directory")?;
    }

    std::fs::write(output, &toml_str)
        .context(format!("failed to write topology to {}", output.display()))?;

    println!(
        "{} Topology {} written to {}",
        "OK".green().bold(),
        topo_type.to_string().cyan(),
        output.display().to_string().yellow(),
    );
    println!(
        "  {} VMs, {} VLANs, {} host interfaces",
        topo_def.vms.len(),
        topo_def.vlans.len(),
        topo_def.host_interfaces.len(),
    );

    Ok(())
}

async fn render_topology(
    topo_type: TopologyType,
    format: RenderFormat,
    _config_path: &str,
) -> Result<()> {
    let generator = DefaultTopologyGenerator::new(VmType::Veos);
    let topo_def = generator
        .generate(topo_type)
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to generate topology")?;

    let format_name = match format {
        RenderFormat::Inventory => "inventory",
        RenderFormat::Minigraph => "minigraph",
        RenderFormat::ConfigDb => "config-db",
    };

    println!(
        "{} Rendering topology {} as {} ...\n",
        "=>".green().bold(),
        topo_type.to_string().cyan(),
        format_name.yellow(),
    );

    let renderer = TopologyRenderer::new()
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to create topology renderer")?;

    let rendered = match format {
        RenderFormat::Inventory => renderer.render_inventory(&topo_def),
        RenderFormat::Minigraph => renderer.render_minigraph(&topo_def),
        RenderFormat::ConfigDb => renderer.render_config_db(&topo_def),
    }
    .map_err(|e| anyhow::anyhow!("{}", e))
    .context("rendering failed")?;

    println!("{}", rendered);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn topology_description(topo: TopologyType) -> &'static str {
    match topo {
        TopologyType::T0 => "Leaf switch with 4 BGP neighbors",
        TopologyType::T064 => "Leaf switch with 64 BGP neighbors",
        TopologyType::T0116 => "Leaf switch with 116 BGP neighbors",
        TopologyType::T1 => "Spine switch with 32 BGP neighbors",
        TopologyType::T164 => "Spine switch with 64 BGP neighbors",
        TopologyType::T1Lag => "Spine switch with 32 LAG-connected neighbors",
        TopologyType::T2 => "Super-spine with 64 neighbors",
        TopologyType::Dualtor => "Dual ToR configuration (2 DUTs)",
        TopologyType::MgmtTor => "Management ToR topology",
        TopologyType::M0Vlan => "M0 VLAN-based topology",
        TopologyType::Ptf32 => "PTF-only with 32 ports",
        TopologyType::Ptf64 => "PTF-only with 64 ports",
        TopologyType::Ptf => "PTF-only (default port count)",
        TopologyType::Any => "Any topology (wildcard)",
    }
}
