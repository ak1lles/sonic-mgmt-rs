//! SDN protocol commands.
//!
//! Interact with devices via gNMI (Get/Set/Subscribe), gNOI (reboot, ping),
//! and P4Runtime (table writes).

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use colored::Colorize;

use sonic_sdn::{GnmiClient, GnmiPath, GnoiClient, P4RuntimeClient, SubscriptionMode};
use sonic_sdn::gnoi::RebootMethod;
use sonic_sdn::p4rt::{ActionParam, MatchField, MatchType};

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct SdnCmd {
    #[command(subcommand)]
    pub action: SdnAction,
}

#[derive(Subcommand, Debug)]
pub enum SdnAction {
    /// gNMI operations (Get, Set, Subscribe)
    Gnmi(GnmiCmd),

    /// gNOI operations (reboot, ping)
    Gnoi(GnoiCmd),

    /// P4Runtime operations
    P4rt(P4rtCmd),
}

// -- gNMI ------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct GnmiCmd {
    #[command(subcommand)]
    pub action: GnmiAction,
}

#[derive(Subcommand, Debug)]
pub enum GnmiAction {
    /// Retrieve state/config via gNMI Get
    Get {
        /// Target host (hostname:port or IP:port)
        host: String,

        /// gNMI path (e.g. /interfaces/interface[name=Ethernet0]/state)
        path: String,

        /// Request encoding
        #[arg(long, value_enum, default_value = "json-ietf")]
        encoding: GnmiEncoding,
    },

    /// Update state/config via gNMI Set
    Set {
        /// Target host
        host: String,

        /// gNMI path
        path: String,

        /// JSON value to set
        value: String,

        /// Set operation type
        #[arg(long, value_enum, default_value = "update")]
        operation: SetOperation,
    },

    /// Subscribe to gNMI notifications
    Subscribe {
        /// Target host
        host: String,

        /// gNMI path
        path: String,

        /// Subscription mode
        #[arg(long, short = 'm', value_enum, default_value = "once")]
        mode: SubscribeMode,

        /// Sample interval in milliseconds (for stream mode)
        #[arg(long, default_value = "10000")]
        interval: u64,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum GnmiEncoding {
    #[value(name = "json-ietf")]
    JsonIetf,
    Json,
    Proto,
    Ascii,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum SetOperation {
    Update,
    Replace,
    Delete,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum SubscribeMode {
    Once,
    Stream,
    Poll,
}

// -- gNOI ------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct GnoiCmd {
    #[command(subcommand)]
    pub action: GnoiAction,
}

#[derive(Subcommand, Debug)]
pub enum GnoiAction {
    /// Reboot a device via gNOI
    Reboot {
        /// Target host
        host: String,

        /// Reboot method
        #[arg(long, short = 't', value_enum, default_value = "cold")]
        reboot_type: GnoiRebootType,

        /// Reboot message / reason
        #[arg(long, short = 'm', default_value = "CLI-initiated reboot")]
        message: String,
    },

    /// Ping a destination from a device via gNOI
    Ping {
        /// Target host (the device to run ping from)
        host: String,

        /// Ping destination address
        destination: String,

        /// Number of ping packets
        #[arg(long, short = 'c', default_value = "5")]
        count: u32,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum GnoiRebootType {
    Cold,
    Warm,
}

// -- P4Runtime -------------------------------------------------------------

#[derive(Args, Debug)]
pub struct P4rtCmd {
    #[command(subcommand)]
    pub action: P4rtAction,
}

#[derive(Subcommand, Debug)]
pub enum P4rtAction {
    /// Write a table entry via P4Runtime
    Write {
        /// Target host
        host: String,

        /// P4 table name
        #[arg(long)]
        table: String,

        /// Match fields as key=value pairs (comma-separated)
        #[arg(long, value_name = "FIELD=VALUE,...")]
        r#match: String,

        /// Action name
        #[arg(long)]
        action: String,

        /// Action parameters as key=value pairs (comma-separated)
        #[arg(long, default_value = "")]
        params: String,
    },
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn handle(cmd: SdnCmd) -> Result<()> {
    match cmd.action {
        SdnAction::Gnmi(gnmi) => handle_gnmi(gnmi).await,
        SdnAction::Gnoi(gnoi) => handle_gnoi(gnoi).await,
        SdnAction::P4rt(p4rt) => handle_p4rt(p4rt).await,
    }
}

// ---------------------------------------------------------------------------
// gNMI handlers
// ---------------------------------------------------------------------------

async fn handle_gnmi(cmd: GnmiCmd) -> Result<()> {
    match cmd.action {
        GnmiAction::Get {
            host,
            path,
            encoding,
        } => gnmi_get(&host, &path, encoding).await,
        GnmiAction::Set {
            host,
            path,
            value,
            operation,
        } => gnmi_set(&host, &path, &value, operation).await,
        GnmiAction::Subscribe {
            host,
            path,
            mode,
            interval,
        } => gnmi_subscribe(&host, &path, mode, interval).await,
    }
}

async fn gnmi_get(host: &str, path: &str, _encoding: GnmiEncoding) -> Result<()> {
    println!(
        "{} gNMI Get on {} path: {}",
        "=>".green().bold(),
        host.cyan(),
        path.yellow(),
    );

    let endpoint = format!("https://{}", host);
    let mut client = GnmiClient::new(&endpoint, "admin", "admin");
    client
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to connect via gNMI")?;

    let gnmi_path = GnmiPath::from_str(path, "openconfig");

    let response = client
        .get(&gnmi_path)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("gNMI Get failed")?;

    println!("\n{}", "Response:".bold().underline());
    println!("{}", serde_json::to_string_pretty(&response)
        .unwrap_or_else(|_| format!("{:?}", response)));

    Ok(())
}

async fn gnmi_set(
    host: &str,
    path: &str,
    value: &str,
    operation: SetOperation,
) -> Result<()> {
    let op_name = match operation {
        SetOperation::Update => "Update",
        SetOperation::Replace => "Replace",
        SetOperation::Delete => "Delete",
    };

    println!(
        "{} gNMI Set ({}) on {} path: {}",
        "=>".green().bold(),
        op_name.yellow(),
        host.cyan(),
        path.yellow(),
    );

    let endpoint = format!("https://{}", host);
    let mut client = GnmiClient::new(&endpoint, "admin", "admin");
    client
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to connect via gNMI")?;

    let gnmi_path = GnmiPath::from_str(path, "openconfig");
    let json_value: serde_json::Value = serde_json::from_str(value)
        .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));

    let response = client
        .set(&gnmi_path, json_value)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("gNMI Set failed")?;

    println!(
        "\n{} Set operation completed successfully.",
        "OK".green().bold()
    );
    println!("{}", serde_json::to_string_pretty(&response)
        .unwrap_or_else(|_| format!("{:?}", response)));

    Ok(())
}

async fn gnmi_subscribe(
    host: &str,
    path: &str,
    mode: SubscribeMode,
    _interval_ms: u64,
) -> Result<()> {
    let mode_name = match mode {
        SubscribeMode::Once => "ONCE",
        SubscribeMode::Stream => "STREAM",
        SubscribeMode::Poll => "POLL",
    };

    println!(
        "{} gNMI Subscribe ({}) on {} path: {}",
        "=>".green().bold(),
        mode_name.yellow(),
        host.cyan(),
        path.yellow(),
    );

    let endpoint = format!("https://{}", host);
    let mut client = GnmiClient::new(&endpoint, "admin", "admin");
    client
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to connect via gNMI")?;

    let sub_mode = match mode {
        SubscribeMode::Once => SubscriptionMode::Once,
        SubscribeMode::Stream => SubscriptionMode::Stream,
        SubscribeMode::Poll => SubscriptionMode::Poll,
    };

    let gnmi_path = GnmiPath::from_str(path, "openconfig");

    println!(
        "\n{} Receiving notifications (Ctrl-C to stop):\n",
        "=>".dimmed()
    );

    let mut rx = client
        .subscribe(&[gnmi_path], sub_mode)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("gNMI Subscribe failed")?;

    while let Some(notification) = rx.recv().await {
        let timestamp = chrono::Utc::now().format("%H:%M:%S%.3f");
        println!(
            "[{}] {}",
            timestamp.to_string().dimmed(),
            serde_json::to_string(&notification)
                .unwrap_or_else(|_| format!("{:?}", notification)),
        );
    }

    println!(
        "\n{} Subscription ended.",
        "=>".green().bold()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// gNOI handlers
// ---------------------------------------------------------------------------

async fn handle_gnoi(cmd: GnoiCmd) -> Result<()> {
    match cmd.action {
        GnoiAction::Reboot {
            host,
            reboot_type,
            message,
        } => gnoi_reboot(&host, reboot_type, &message).await,
        GnoiAction::Ping {
            host,
            destination,
            count,
        } => gnoi_ping(&host, &destination, count).await,
    }
}

async fn gnoi_reboot(host: &str, reboot_type: GnoiRebootType, message: &str) -> Result<()> {
    let method_name = match reboot_type {
        GnoiRebootType::Cold => "COLD",
        GnoiRebootType::Warm => "WARM",
    };

    println!(
        "{} gNOI Reboot ({}) on {} -- {}",
        "=>".yellow().bold(),
        method_name.yellow(),
        host.cyan(),
        message.dimmed(),
    );

    let endpoint = format!("https://{}", host);
    let mut client = GnoiClient::new(&endpoint, "admin", "admin");
    client
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to connect via gNOI")?;

    let method = match reboot_type {
        GnoiRebootType::Cold => RebootMethod::Cold,
        GnoiRebootType::Warm => RebootMethod::Warm,
    };

    client
        .system_reboot(method, 0)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("gNOI reboot failed")?;

    println!(
        "{} Reboot command sent successfully.",
        "OK".green().bold()
    );
    Ok(())
}

async fn gnoi_ping(host: &str, destination: &str, count: u32) -> Result<()> {
    println!(
        "{} gNOI Ping from {} to {} ({} packets)",
        "=>".green().bold(),
        host.cyan(),
        destination.yellow(),
        count,
    );

    let endpoint = format!("https://{}", host);
    let mut client = GnoiClient::new(&endpoint, "admin", "admin");
    client
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to connect via gNOI")?;

    let result = client
        .system_ping(destination, count)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("gNOI ping failed")?;

    println!("\n{}", "Ping Results:".bold().underline());
    println!(
        "  {} packets transmitted, {} received, {:.1}% loss",
        result.sent,
        result.received,
        result.packet_loss_pct,
    );
    println!(
        "  rtt min/avg/max = {:.2}/{:.2}/{:.2} ms",
        result.min_rtt_ms, result.avg_rtt_ms, result.max_rtt_ms,
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// P4Runtime handlers
// ---------------------------------------------------------------------------

async fn handle_p4rt(cmd: P4rtCmd) -> Result<()> {
    match cmd.action {
        P4rtAction::Write {
            host,
            table,
            r#match,
            action,
            params,
        } => p4rt_write(&host, &table, &r#match, &action, &params).await,
    }
}

async fn p4rt_write(
    host: &str,
    _table: &str,
    match_fields: &str,
    _action: &str,
    params: &str,
) -> Result<()> {
    println!(
        "{} P4Runtime Write on {} table: {}",
        "=>".green().bold(),
        host.cyan(),
        _table.yellow(),
    );
    println!("  Match:  {}", match_fields);
    println!("  Action: {}", _action);
    if !params.is_empty() {
        println!("  Params: {}", params);
    }

    let endpoint = format!("https://{}", host);
    let mut client = P4RuntimeClient::new(&endpoint, 1);
    client
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to connect via P4Runtime")?;

    // Parse match fields from "field_id=value,..." into MatchField structs
    let p4_match_fields: Vec<MatchField> = match_fields
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|kv| {
            let mut parts = kv.splitn(2, '=');
            let key = parts.next()?.trim();
            let val = parts.next()?.trim();
            Some(MatchField {
                field_id: key.parse().unwrap_or(0),
                match_type: MatchType::Exact,
                value: val.as_bytes().to_vec(),
                mask: None,
                range_low: None,
                range_high: None,
            })
        })
        .collect();

    let p4_action_params: Vec<ActionParam> = params
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|kv| {
            let mut parts = kv.splitn(2, '=');
            let key = parts.next()?.trim();
            let val = parts.next()?.trim();
            Some(ActionParam {
                param_id: key.parse().unwrap_or(0),
                value: val.as_bytes().to_vec(),
            })
        })
        .collect();

    // Use table_id=0, action_id=0 as placeholders (real usage would resolve names)
    client
        .write_table_entry(0, p4_match_fields, 0, p4_action_params)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("P4Runtime write failed")?;

    println!(
        "\n{} Table entry written successfully.",
        "OK".green().bold()
    );
    Ok(())
}
