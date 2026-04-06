//! `sonic-mgmt` -- the main CLI binary for the SONiC management framework.
//!
//! Dispatches every subcommand to the corresponding handler module and sets up
//! global tracing, error handling, and colored terminal output.

mod commands;
mod interactive;

use std::process::ExitCode;

use clap::Parser;
use colored::Colorize;
use tracing_subscriber::EnvFilter;

use commands::{
    config::ConfigCmd, device::DeviceCmd, report::ReportCmd, sdn::SdnCmd, test::TestCmd,
    testbed::TestbedCmd, topology::TopologyCmd,
};

// ---------------------------------------------------------------------------
// Top-level CLI definition
// ---------------------------------------------------------------------------

/// SONiC Network Management CLI
///
/// A unified command-line interface for managing SONiC testbeds, devices,
/// topologies, test execution, reporting, and SDN operations.
#[derive(Parser, Debug)]
#[command(
    name = "sonic-mgmt",
    version,
    about = "SONiC Network Management CLI",
    long_about = "A unified command-line interface for managing SONiC testbeds, devices, \
                  topologies, test execution, reporting, and SDN operations.",
    propagate_version = true
)]
struct Cli {
    /// Path to the configuration file.
    #[arg(
        long,
        short = 'c',
        global = true,
        default_value = "sonic-mgmt.toml",
        env = "SONIC_CONFIG"
    )]
    config: String,

    /// Override the log level (e.g. debug, info, warn, error).
    #[arg(long, global = true, env = "SONIC_LOG")]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Manage testbeds (deploy, teardown, health-check, ...)
    Testbed(TestbedCmd),

    /// Manage individual devices (list, show, exec, reboot, ...)
    Device(DeviceCmd),

    /// Generate and inspect network topologies
    Topology(TopologyCmd),

    /// Discover and run tests
    Test(TestCmd),

    /// Parse, upload, and analyse test reports
    Report(ReportCmd),

    /// View and edit framework configuration
    Config(ConfigCmd),

    /// SDN operations (gNMI, gNOI, P4Runtime)
    Sdn(SdnCmd),

    /// Run the interactive first-time setup wizard
    Init,
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Initialise tracing.
    //
    // Priority: --log-level flag > SONIC_LOG env var (via clap `env`) > "info".
    let filter = cli
        .log_level
        .clone()
        .unwrap_or_else(|| "info".to_owned());

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_new(&filter).unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    if let Err(err) = dispatch(cli).await {
        eprintln!("{} {:#}", "error:".red().bold(), err);
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

async fn dispatch(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Testbed(cmd) => commands::testbed::handle(cmd, &cli.config).await,
        Command::Device(cmd) => commands::device::handle(cmd, &cli.config).await,
        Command::Topology(cmd) => commands::topology::handle(cmd, &cli.config).await,
        Command::Test(cmd) => commands::test::handle(cmd, &cli.config).await,
        Command::Report(cmd) => commands::report::handle(cmd).await,
        Command::Config(cmd) => commands::config::handle(cmd, &cli.config).await,
        Command::Sdn(cmd) => commands::sdn::handle(cmd).await,
        Command::Init => {
            interactive::setup::run_initial_setup().await?;
            Ok(())
        }
    }
}
