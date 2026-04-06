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
    config::ConfigCmd, device::DeviceCmd, docker::DockerCmd, report::ReportCmd, sdn::SdnCmd,
    test::TestCmd, testbed::TestbedCmd, topology::TopologyCmd,
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

    /// Manage the sonic-mgmt test runner container
    Docker(DockerCmd),

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
        Command::Docker(cmd) => commands::docker::handle(cmd).await,
        Command::Init => {
            interactive::setup::run_initial_setup().await?;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Helper: parse args and return the result.
    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    // -- testbed subcommands ------------------------------------------------

    #[test]
    fn testbed_list() {
        let cli = parse(&["sonic-mgmt", "testbed", "list"]).unwrap();
        assert!(matches!(cli.command, Command::Testbed(_)));
        if let Command::Testbed(cmd) = cli.command {
            assert!(matches!(
                cmd.action,
                commands::testbed::TestbedAction::List { .. }
            ));
        }
    }

    #[test]
    fn testbed_show() {
        let cli = parse(&["sonic-mgmt", "testbed", "show", "mybed"]).unwrap();
        if let Command::Testbed(cmd) = cli.command {
            match cmd.action {
                commands::testbed::TestbedAction::Show { name } => {
                    assert_eq!(name, "mybed");
                }
                other => panic!("expected TestbedAction::Show, got {:?}", other),
            }
        } else {
            panic!("expected Command::Testbed");
        }
    }

    #[test]
    fn testbed_deploy() {
        let cli = parse(&["sonic-mgmt", "testbed", "deploy", "mybed"]).unwrap();
        if let Command::Testbed(cmd) = cli.command {
            match cmd.action {
                commands::testbed::TestbedAction::Deploy { name } => {
                    assert_eq!(name, "mybed");
                }
                other => panic!("expected TestbedAction::Deploy, got {:?}", other),
            }
        } else {
            panic!("expected Command::Testbed");
        }
    }

    #[test]
    fn testbed_wizard() {
        let cli = parse(&["sonic-mgmt", "testbed", "wizard"]).unwrap();
        if let Command::Testbed(cmd) = cli.command {
            assert!(matches!(
                cmd.action,
                commands::testbed::TestbedAction::Wizard
            ));
        } else {
            panic!("expected Command::Testbed");
        }
    }

    // -- device subcommands -------------------------------------------------

    #[test]
    fn device_list() {
        let cli = parse(&["sonic-mgmt", "device", "list"]).unwrap();
        assert!(matches!(cli.command, Command::Device(_)));
        if let Command::Device(cmd) = cli.command {
            assert!(matches!(
                cmd.action,
                commands::device::DeviceAction::List
            ));
        }
    }

    #[test]
    fn device_show() {
        let cli = parse(&["sonic-mgmt", "device", "show", "myhost"]).unwrap();
        if let Command::Device(cmd) = cli.command {
            match cmd.action {
                commands::device::DeviceAction::Show { hostname } => {
                    assert_eq!(hostname, "myhost");
                }
                other => panic!("expected DeviceAction::Show, got {:?}", other),
            }
        } else {
            panic!("expected Command::Device");
        }
    }

    #[test]
    fn device_wizard() {
        let cli = parse(&["sonic-mgmt", "device", "wizard"]).unwrap();
        if let Command::Device(cmd) = cli.command {
            assert!(matches!(
                cmd.action,
                commands::device::DeviceAction::Wizard
            ));
        } else {
            panic!("expected Command::Device");
        }
    }

    // -- topology subcommands -----------------------------------------------

    #[test]
    fn topology_list() {
        let cli = parse(&["sonic-mgmt", "topology", "list"]).unwrap();
        assert!(matches!(cli.command, Command::Topology(_)));
        if let Command::Topology(cmd) = cli.command {
            assert!(matches!(
                cmd.action,
                commands::topology::TopologyAction::List
            ));
        }
    }

    #[test]
    fn topology_show() {
        let cli = parse(&["sonic-mgmt", "topology", "show", "t0"]).unwrap();
        if let Command::Topology(cmd) = cli.command {
            match cmd.action {
                commands::topology::TopologyAction::Show { topo_type } => {
                    assert!(matches!(
                        topo_type,
                        commands::topology::TopoArg::T0
                    ));
                }
                other => panic!("expected TopologyAction::Show, got {:?}", other),
            }
        } else {
            panic!("expected Command::Topology");
        }
    }

    // -- test subcommands ---------------------------------------------------

    #[test]
    fn test_discover() {
        let cli = parse(&["sonic-mgmt", "test", "discover"]).unwrap();
        assert!(matches!(cli.command, Command::Test(_)));
        if let Command::Test(cmd) = cli.command {
            assert!(matches!(
                cmd.action,
                commands::test::TestAction::Discover { .. }
            ));
        }
    }

    #[test]
    fn test_wizard() {
        let cli = parse(&["sonic-mgmt", "test", "wizard"]).unwrap();
        if let Command::Test(cmd) = cli.command {
            assert!(matches!(
                cmd.action,
                commands::test::TestAction::Wizard
            ));
        } else {
            panic!("expected Command::Test");
        }
    }

    // -- report subcommands -------------------------------------------------

    #[test]
    fn report_parse() {
        let cli = parse(&["sonic-mgmt", "report", "parse", "results.xml"]).unwrap();
        if let Command::Report(cmd) = cli.command {
            match cmd.action {
                commands::report::ReportAction::Parse { path } => {
                    assert_eq!(path.to_str().unwrap(), "results.xml");
                }
                other => panic!("expected ReportAction::Parse, got {:?}", other),
            }
        } else {
            panic!("expected Command::Report");
        }
    }

    // -- config subcommands -------------------------------------------------

    #[test]
    fn config_show() {
        let cli = parse(&["sonic-mgmt", "config", "show"]).unwrap();
        if let Command::Config(cmd) = cli.command {
            assert!(matches!(
                cmd.action,
                commands::config::ConfigAction::Show
            ));
        } else {
            panic!("expected Command::Config");
        }
    }

    // -- sdn subcommands ----------------------------------------------------

    #[test]
    fn sdn_gnmi_get() {
        let cli = parse(&[
            "sonic-mgmt", "sdn", "gnmi", "get", "host:8080", "/path",
        ])
        .unwrap();
        if let Command::Sdn(cmd) = cli.command {
            match cmd.action {
                commands::sdn::SdnAction::Gnmi(gnmi) => match gnmi.action {
                    commands::sdn::GnmiAction::Get { host, path, .. } => {
                        assert_eq!(host, "host:8080");
                        assert_eq!(path, "/path");
                    }
                    other => panic!("expected GnmiAction::Get, got {:?}", other),
                },
                other => panic!("expected SdnAction::Gnmi, got {:?}", other),
            }
        } else {
            panic!("expected Command::Sdn");
        }
    }

    // -- init command -------------------------------------------------------

    #[test]
    fn init_command() {
        let cli = parse(&["sonic-mgmt", "init"]).unwrap();
        assert!(matches!(cli.command, Command::Init));
    }

    // -- global flags -------------------------------------------------------

    #[test]
    fn global_config_flag() {
        let cli = parse(&[
            "sonic-mgmt", "-c", "custom.toml", "testbed", "list",
        ])
        .unwrap();
        assert_eq!(cli.config, "custom.toml");
        assert!(matches!(cli.command, Command::Testbed(_)));
    }

    #[test]
    fn global_log_level_flag() {
        let cli = parse(&[
            "sonic-mgmt", "--log-level", "debug", "testbed", "list",
        ])
        .unwrap();
        assert_eq!(cli.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn default_config_value() {
        let cli = parse(&["sonic-mgmt", "init"]).unwrap();
        assert_eq!(cli.config, "sonic-mgmt.toml");
        assert!(cli.log_level.is_none());
    }

    // -- error cases --------------------------------------------------------

    #[test]
    fn invalid_subcommand_fails() {
        let result = parse(&["sonic-mgmt", "nonexistent"]);
        assert!(result.is_err());
    }

    #[test]
    fn missing_required_arg_fails() {
        // `testbed show` requires a name argument
        let result = parse(&["sonic-mgmt", "testbed", "show"]);
        assert!(result.is_err());
    }

    #[test]
    fn no_subcommand_fails() {
        let result = parse(&["sonic-mgmt"]);
        assert!(result.is_err());
    }
}
