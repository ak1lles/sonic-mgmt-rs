//! Integration tests against a live SONiC device.
//!
//! These tests require a reachable SONiC DUT. They are marked `#[ignore]` so
//! `cargo test` skips them by default.
//!
//! Target resolution (first match wins):
//!   1. `SONIC_SSH_HOST` env var set -- use SONIC_SSH_* env vars directly
//!   2. `SONIC_TESTBED` env var set -- load testbed config, use primary DUT
//!   3. Fall back to 127.0.0.1:22 admin/password
//!
//! Run with:
//!   SONIC_TESTBED=lab1 makers integration-test
//!   SONIC_SSH_HOST=10.0.0.1 cargo test -p sonic-integration-tests --test integration -- --ignored

use std::net::IpAddr;

use sonic_config::AppConfig;
use sonic_core::{
    Connection, Credentials, Device, DeviceInfo, DeviceType, FactsProvider,
};
use sonic_device::SonicHost;
use sonic_device::SshConnection;

// ---------------------------------------------------------------------------
// Test target resolution
// ---------------------------------------------------------------------------

fn test_target() -> DeviceInfo {
    // 1. Explicit env vars
    if std::env::var("SONIC_SSH_HOST").is_ok() {
        return device_info_from_env();
    }

    // 2. Testbed config
    if let Ok(testbed_name) = std::env::var("SONIC_TESTBED") {
        if let Some(info) = device_info_from_testbed(&testbed_name) {
            return info;
        }
    }

    // 3. Defaults
    let creds = Credentials::new("admin").with_password("password");
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    DeviceInfo::new("sonic-dut", ip, DeviceType::Sonic, creds)
}

fn device_info_from_env() -> DeviceInfo {
    let host = std::env::var("SONIC_SSH_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("SONIC_SSH_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(22);
    let user = std::env::var("SONIC_SSH_USER").unwrap_or_else(|_| "admin".into());
    let pass = std::env::var("SONIC_SSH_PASS").unwrap_or_else(|_| "password".into());

    let ip: IpAddr = host.parse().expect("SONIC_SSH_HOST must be a valid IP");
    let creds = Credentials::new(user).with_password(pass);
    let mut info = DeviceInfo::new("sonic-dut", ip, DeviceType::Sonic, creds);
    info.port = port;
    info
}

fn device_info_from_testbed(testbed_name: &str) -> Option<DeviceInfo> {
    let config_path = std::env::var("SONIC_CONFIG").unwrap_or_else(|_| "sonic-mgmt.toml".into());
    let app = AppConfig::load_or_default(&config_path).ok()?;

    let testbed_file = &app.testbed.file;
    let testbeds = sonic_config::testbed::load_testbed(testbed_file).ok()?;
    let tb = testbeds.into_iter().find(|t| t.name == testbed_name)?;
    let dut = tb.primary_dut()?;

    let creds: Credentials = dut.credentials.clone().into();
    let mut info = DeviceInfo::new(&dut.hostname, dut.mgmt_ip, DeviceType::Sonic, creds);
    info.platform = dut.platform;
    info.hwsku = dut.hwsku.clone();
    Some(info)
}

// ---------------------------------------------------------------------------
// SSH Connection Tests
// ---------------------------------------------------------------------------

mod ssh {
    use super::*;

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn connect_and_disconnect() {
        let target = test_target();
        let creds = target.credentials.clone();
        let host = target.mgmt_ip.to_string();
        let mut conn = SshConnection::new(host, target.port, creds);

        conn.open().await.expect("SSH connect should succeed");
        assert!(conn.is_alive().await, "connection should be alive after open");

        conn.close().await.expect("SSH close should succeed");
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn execute_simple_command() {
        let target = test_target();
        let creds = target.credentials.clone();
        let host = target.mgmt_ip.to_string();
        let mut conn = SshConnection::new(host, target.port, creds);
        conn.open().await.expect("connect");

        let result = conn.send_command("echo hello").await.expect("echo should work");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");

        conn.close().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn execute_command_captures_exit_code() {
        let target = test_target();
        let creds = target.credentials.clone();
        let host = target.mgmt_ip.to_string();
        let mut conn = SshConnection::new(host, target.port, creds);
        conn.open().await.expect("connect");

        let result = conn.send_command("false").await.expect("false should return");
        assert_ne!(result.exit_code, 0, "exit code should be non-zero for `false`");

        conn.close().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn execute_command_captures_stderr() {
        let target = test_target();
        let creds = target.credentials.clone();
        let host = target.mgmt_ip.to_string();
        let mut conn = SshConnection::new(host, target.port, creds);
        conn.open().await.expect("connect");

        let result = conn
            .send_command("echo err >&2")
            .await
            .expect("should capture stderr");
        assert!(
            result.stderr.contains("err"),
            "stderr should contain 'err', got: {:?}",
            result.stderr
        );

        conn.close().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn wrong_password_fails() {
        let target = test_target();
        let host = target.mgmt_ip.to_string();
        let creds = Credentials::new(target.credentials.username.clone())
            .with_password("wrong_password");
        let mut conn = SshConnection::new(host, target.port, creds);

        let result = conn.open().await;
        assert!(result.is_err(), "should fail with wrong password");
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn multiple_commands_on_same_connection() {
        let target = test_target();
        let creds = target.credentials.clone();
        let host = target.mgmt_ip.to_string();
        let mut conn = SshConnection::new(host, target.port, creds);
        conn.open().await.expect("connect");

        for i in 0..5 {
            let result = conn
                .send_command(&format!("echo iteration-{i}"))
                .await
                .expect("command should succeed");
            assert_eq!(result.stdout.trim(), format!("iteration-{i}"));
        }

        conn.close().await.ok();
    }
}

// ---------------------------------------------------------------------------
// SonicHost / Device Trait Tests
// ---------------------------------------------------------------------------

mod device {
    use super::*;

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn connect_and_execute() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("SonicHost::connect should succeed");
        assert!(host.is_connected().await);

        let result = host
            .execute("hostname")
            .await
            .expect("hostname command should work");
        assert_eq!(result.exit_code, 0);
        assert!(!result.stdout.trim().is_empty(), "hostname should not be empty");

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn execute_checked_fails_on_error() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let result = host.execute_checked("false").await;
        assert!(result.is_err(), "execute_checked should fail on non-zero exit");

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn wait_ready() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        host.wait_ready(30)
            .await
            .expect("wait_ready should succeed on a running device");

        host.disconnect().await.ok();
    }
}

// ---------------------------------------------------------------------------
// SONiC CLI / Facts Tests
// ---------------------------------------------------------------------------

mod sonic_cli {
    use super::*;

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn show_version() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let result = host
            .execute("show version")
            .await
            .expect("show version should work");
        assert_eq!(result.exit_code, 0);

        let out = &result.stdout;
        assert!(
            out.contains("SONiC Software Version") || out.contains("sonic"),
            "show version should mention SONiC, got:\n{out}"
        );

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn show_interfaces_status() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let result = host
            .execute("show interfaces status")
            .await
            .expect("show interfaces status");
        assert_eq!(result.exit_code, 0);
        assert!(
            result.stdout.contains("Interface") || result.stdout.contains("Ethernet"),
            "should list interface columns or Ethernet ports"
        );

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn docker_ps() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let containers = host
            .get_container_status()
            .await
            .expect("get_container_status should work");

        let expected = ["database", "swss", "syncd", "bgp"];
        for name in &expected {
            assert!(
                containers.keys().any(|k| k.contains(name)),
                "expected container '{}' in {:?}",
                name,
                containers.keys().collect::<Vec<_>>()
            );
        }

        host.disconnect().await.ok();
    }
}

// ---------------------------------------------------------------------------
// Facts Provider Tests
// ---------------------------------------------------------------------------

mod facts {
    use super::*;

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn basic_facts() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let facts = host.basic_facts().await.expect("basic_facts should parse");
        assert!(!facts.hostname.is_empty(), "hostname should not be empty");
        assert!(
            !facts.os_version.is_empty(),
            "os_version should not be empty"
        );
        println!("SONiC version: {}", facts.os_version);
        println!("Platform:      {}", facts.platform);
        println!("HWSKU:         {}", facts.hwsku);

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn basic_facts_caching() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let facts1 = host.basic_facts().await.expect("first call");
        let facts2 = host.basic_facts().await.expect("second call (cached)");

        assert_eq!(facts1.hostname, facts2.hostname);
        assert_eq!(facts1.os_version, facts2.os_version);

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn interface_facts() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let facts = host.interface_facts().await.expect("interface_facts");
        assert!(
            !facts.ports.is_empty(),
            "device should have at least one interface"
        );
        println!("Found {} interfaces", facts.ports.len());
        for port in facts.ports.iter().take(3) {
            println!("  {} - speed: {}, status: {}", port.name, port.speed, port.admin_status);
        }

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn bgp_facts() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let facts = host.bgp_facts().await.expect("bgp_facts should parse");
        println!("BGP router_id: {}", facts.router_id);
        println!("BGP local_as:  {}", facts.local_as);
        println!("BGP neighbors:  {}", facts.neighbors.len());

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn config_facts() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let facts = host.config_facts().await.expect("config_facts");
        assert!(
            !facts.running_config.is_empty(),
            "running_config should not be empty"
        );
        println!(
            "Config tables: {:?}",
            facts.running_config.keys().collect::<Vec<_>>()
        );
        println!("Services: {}", facts.services.len());

        host.disconnect().await.ok();
    }
}

// ---------------------------------------------------------------------------
// SONiC Operations Tests (non-destructive)
// ---------------------------------------------------------------------------

mod operations {
    use super::*;

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn get_running_config() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let config = host
            .get_running_config()
            .await
            .expect("get_running_config should return valid JSON");

        assert!(config.is_object(), "running config should be a JSON object");

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn systemctl_list_services() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let result = host
            .execute("systemctl list-units --type=service --state=running --no-pager --plain --no-legend")
            .await
            .expect("systemctl list-units");

        assert_eq!(result.exit_code, 0);
        assert!(!result.stdout.is_empty(), "should list running services");
        println!("Running services:\n{}", result.stdout);

        host.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "requires testbed"]
    async fn show_platform_summary() {
        let mut host = SonicHost::new(test_target());
        host.connect().await.expect("connect");

        let result = host
            .execute("show platform summary")
            .await
            .expect("show platform summary");

        assert_eq!(result.exit_code, 0);
        println!("Platform summary:\n{}", result.stdout);

        host.disconnect().await.ok();
    }
}
