//! Health-checking subsystem.
//!
//! [`HealthChecker`] probes every device in a testbed and aggregates the
//! results into a [`TestbedHealth`] snapshot.  Individual device probes are run
//! concurrently via `tokio::spawn` with configurable timeouts and retries.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use sonic_core::{Device, DeviceInfo, HealthStatus, SonicError};

use crate::device_mgr::DeviceManager;

// ---------------------------------------------------------------------------
// Per-device health
// ---------------------------------------------------------------------------

/// Health snapshot for a single device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceHealth {
    /// Device hostname.
    pub hostname: String,
    /// Whether the device responded to a management-plane probe (ping/SSH).
    pub reachable: bool,
    /// Round-trip latency in milliseconds (0.0 if unreachable).
    pub ping_latency_ms: f64,
    /// Map of critical service name -> running.
    pub critical_services: HashMap<String, bool>,
    /// Number of BGP sessions in `Established` state.
    pub bgp_sessions_up: u32,
    /// Total number of configured BGP sessions.
    pub bgp_sessions_total: u32,
    /// Disk utilisation percentage (0.0..100.0).
    pub disk_usage_pct: f32,
    /// Memory utilisation percentage (0.0..100.0).
    pub memory_usage_pct: f32,
}

impl DeviceHealth {
    /// Derives a [`HealthStatus`] for this single device.
    pub fn status(&self) -> HealthStatus {
        if !self.reachable {
            return HealthStatus::Unhealthy;
        }

        let all_services_ok = self.critical_services.values().all(|&ok| ok);
        let bgp_ok = self.bgp_sessions_total == 0
            || self.bgp_sessions_up == self.bgp_sessions_total;
        let resources_ok = self.disk_usage_pct < 90.0 && self.memory_usage_pct < 90.0;

        if all_services_ok && bgp_ok && resources_ok {
            HealthStatus::Healthy
        } else if self.bgp_sessions_up > 0 || all_services_ok {
            HealthStatus::Degraded
        } else {
            HealthStatus::Unhealthy
        }
    }

    /// Creates a health record for an unreachable device.
    fn unreachable(hostname: &str) -> Self {
        Self {
            hostname: hostname.to_string(),
            reachable: false,
            ping_latency_ms: 0.0,
            critical_services: HashMap::new(),
            bgp_sessions_up: 0,
            bgp_sessions_total: 0,
            disk_usage_pct: 0.0,
            memory_usage_pct: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Testbed-wide health
// ---------------------------------------------------------------------------

/// Aggregate health snapshot for an entire testbed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestbedHealth {
    /// Overall status (worst-of across all devices).
    pub overall: HealthStatus,
    /// Per-device results.
    pub devices: Vec<DeviceHealth>,
    /// When this check was performed.
    pub checked_at: DateTime<Utc>,
}

impl TestbedHealth {
    /// Computes the overall status from the individual device results.
    fn aggregate(devices: &[DeviceHealth]) -> HealthStatus {
        if devices.is_empty() {
            return HealthStatus::Unknown;
        }

        let mut worst = HealthStatus::Healthy;
        for d in devices {
            match d.status() {
                HealthStatus::Unhealthy => return HealthStatus::Unhealthy,
                HealthStatus::Degraded => worst = HealthStatus::Degraded,
                HealthStatus::Unknown if worst == HealthStatus::Healthy => {
                    worst = HealthStatus::Unknown;
                }
                _ => {}
            }
        }
        worst
    }
}

// ---------------------------------------------------------------------------
// HealthChecker
// ---------------------------------------------------------------------------

/// Configurable health-checking engine.
pub struct HealthChecker {
    /// Per-device probe timeout.
    pub timeout: Duration,
    /// Number of retries on transient failure.
    pub retries: u32,
    /// Delay between retries.
    pub retry_delay: Duration,
    /// Names of SONiC services that are considered critical.
    pub critical_services: Vec<String>,
}

impl HealthChecker {
    /// Creates a checker with sensible defaults.
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            retries: 2,
            retry_delay: Duration::from_secs(3),
            critical_services: vec![
                "bgp".to_string(),
                "database".to_string(),
                "lldp".to_string(),
                "pmon".to_string(),
                "snmp".to_string(),
                "swss".to_string(),
                "syncd".to_string(),
                "teamd".to_string(),
            ],
        }
    }

    /// Builder: override timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builder: override retry count.
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// Builder: override retry delay.
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// Checks the health of a single device.
    ///
    /// In a real deployment this would open an SSH session (or use gRPC) and
    /// run show commands.  The current implementation performs a TCP-connect
    /// probe against the device's management port to determine reachability,
    /// then returns a best-effort health record.
    pub async fn check_device(&self, device: &DeviceInfo) -> DeviceHealth {
        let hostname = &device.hostname;
        info!(hostname = %hostname, ip = %device.mgmt_ip, "checking device health");

        for attempt in 0..=self.retries {
            if attempt > 0 {
                debug!(hostname = %hostname, attempt, "retrying health check");
                tokio::time::sleep(self.retry_delay).await;
            }

            let addr = std::net::SocketAddr::new(device.mgmt_ip, device.port);
            let start = std::time::Instant::now();

            match tokio::time::timeout(
                self.timeout,
                tokio::net::TcpStream::connect(addr),
            )
            .await
            {
                Ok(Ok(_stream)) => {
                    let latency = start.elapsed().as_secs_f64() * 1000.0;
                    debug!(hostname = %hostname, latency_ms = latency, "device reachable");

                    // Build a health record.  In production the checker would
                    // run `show system status`, `show ip bgp summary`, etc.
                    // over the session to populate these fields.
                    let mut services = HashMap::new();
                    for svc in &self.critical_services {
                        // Assume healthy if we can reach the device.
                        services.insert(svc.clone(), true);
                    }

                    return DeviceHealth {
                        hostname: hostname.clone(),
                        reachable: true,
                        ping_latency_ms: latency,
                        critical_services: services,
                        bgp_sessions_up: 0,
                        bgp_sessions_total: 0,
                        disk_usage_pct: 0.0,
                        memory_usage_pct: 0.0,
                    };
                }
                Ok(Err(e)) => {
                    warn!(
                        hostname = %hostname,
                        attempt,
                        error = %e,
                        "TCP connect failed"
                    );
                }
                Err(_) => {
                    warn!(
                        hostname = %hostname,
                        attempt,
                        timeout_ms = self.timeout.as_millis() as u64,
                        "TCP connect timed out"
                    );
                }
            }
        }

        warn!(hostname = %hostname, "device unreachable after retries");
        DeviceHealth::unreachable(hostname)
    }

    /// Checks every device in the testbed concurrently and returns an
    /// aggregated [`TestbedHealth`] snapshot.
    pub async fn check_testbed(
        &self,
        devices: &[DeviceInfo],
    ) -> sonic_core::Result<TestbedHealth> {
        if devices.is_empty() {
            return Err(SonicError::testbed("no devices to check"));
        }

        info!(device_count = devices.len(), "starting testbed health check");

        // Spawn a task per device so probes run concurrently.
        let mut handles = Vec::with_capacity(devices.len());
        for device in devices {
            let dev = device.clone();
            let timeout = self.timeout;
            let retries = self.retries;
            let retry_delay = self.retry_delay;
            let critical = self.critical_services.clone();

            handles.push(tokio::spawn(async move {
                let checker = HealthChecker {
                    timeout,
                    retries,
                    retry_delay,
                    critical_services: critical,
                };
                checker.check_device(&dev).await
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(health) => results.push(health),
                Err(e) => {
                    warn!(error = %e, "health check task panicked");
                    results.push(DeviceHealth::unreachable("unknown"));
                }
            }
        }

        let overall = TestbedHealth::aggregate(&results);
        info!(overall = %overall, devices_checked = results.len(), "testbed health check complete");

        Ok(TestbedHealth {
            overall,
            devices: results,
            checked_at: Utc::now(),
        })
    }
}

impl HealthChecker {
    /// Checks device health over an active SSH connection.
    ///
    /// Runs `docker ps`, `show ip bgp summary`, `df`, and `free` to
    /// populate the health record with real data from the device.
    pub async fn check_device_connected(&self, device: &dyn Device) -> DeviceHealth {
        let hostname = device.hostname().to_string();
        info!(hostname = %hostname, "checking device health via live connection");

        let start = std::time::Instant::now();

        // Check if the connection is still alive.
        if !device.is_connected().await {
            warn!(hostname = %hostname, "device reports not connected");
            return DeviceHealth::unreachable(&hostname);
        }

        let latency = start.elapsed().as_secs_f64() * 1000.0;

        // Critical services via docker ps.
        let critical_services = self.check_services(device).await;

        // BGP session counts.
        let (bgp_up, bgp_total) = self.check_bgp(device).await;

        // Disk usage.
        let disk_pct = self.check_disk(device).await;

        // Memory usage.
        let mem_pct = self.check_memory(device).await;

        DeviceHealth {
            hostname,
            reachable: true,
            ping_latency_ms: latency,
            critical_services,
            bgp_sessions_up: bgp_up,
            bgp_sessions_total: bgp_total,
            disk_usage_pct: disk_pct,
            memory_usage_pct: mem_pct,
        }
    }

    /// Checks every device, using live connections from the device manager
    /// when available, falling back to TCP probes for disconnected devices.
    pub async fn check_testbed_connected(
        &self,
        devices: &[DeviceInfo],
        device_mgr: &DeviceManager,
    ) -> sonic_core::Result<TestbedHealth> {
        if devices.is_empty() {
            return Err(SonicError::testbed("no devices to check"));
        }

        info!(
            device_count = devices.len(),
            "starting testbed health check (connected mode)"
        );

        let mut results = Vec::with_capacity(devices.len());

        for device_info in devices {
            let health = if let Some(dev) = device_mgr.get_device(&device_info.hostname) {
                debug!(
                    hostname = %device_info.hostname,
                    "using live connection for health check"
                );
                self.check_device_connected(dev).await
            } else {
                debug!(
                    hostname = %device_info.hostname,
                    "no live connection, falling back to TCP probe"
                );
                self.check_device(device_info).await
            };
            results.push(health);
        }

        let overall = TestbedHealth::aggregate(&results);
        info!(
            overall = %overall,
            devices_checked = results.len(),
            "testbed health check complete"
        );

        Ok(TestbedHealth {
            overall,
            devices: results,
            checked_at: Utc::now(),
        })
    }

    // -- internal health probes -----------------------------------------------

    /// Runs `docker ps` on the device and checks which critical containers
    /// are running.
    async fn check_services(&self, device: &dyn Device) -> HashMap<String, bool> {
        let mut services = HashMap::new();

        let cmd = "docker ps --format '{{.Names}} {{.Status}}'";
        match device.execute(cmd).await {
            Ok(result) => {
                for svc in &self.critical_services {
                    let running = result.stdout.lines().any(|line| {
                        line.contains(svc) && line.contains("Up")
                    });
                    services.insert(svc.clone(), running);
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to check services via docker ps");
                // Mark all as unknown (false) since we could not determine.
                for svc in &self.critical_services {
                    services.insert(svc.clone(), false);
                }
            }
        }

        services
    }

    /// Runs `show ip bgp summary` and parses established session counts.
    async fn check_bgp(&self, device: &dyn Device) -> (u32, u32) {
        let cmd = "show ip bgp summary";
        match device.execute(cmd).await {
            Ok(result) => {
                let mut total = 0u32;
                let mut established = 0u32;

                for line in result.stdout.lines() {
                    let fields: Vec<&str> = line.split_whitespace().collect();
                    // BGP summary lines have the state/prefix-count as the last
                    // field. A numeric value means established; anything else
                    // (Idle, Active, Connect, OpenSent, etc.) means not up.
                    if fields.len() >= 10 {
                        total += 1;
                        if let Some(last) = fields.last() {
                            if last.parse::<u32>().is_ok() {
                                established += 1;
                            }
                        }
                    }
                }
                (established, total)
            }
            Err(e) => {
                warn!(error = %e, "failed to check BGP summary");
                (0, 0)
            }
        }
    }

    /// Runs `df -h /` and parses disk usage percentage.
    async fn check_disk(&self, device: &dyn Device) -> f32 {
        let cmd = "df -h /";
        match device.execute(cmd).await {
            Ok(result) => parse_df_usage(&result.stdout),
            Err(e) => {
                warn!(error = %e, "failed to check disk usage");
                0.0
            }
        }
    }

    /// Runs `free -m` and parses memory usage percentage.
    async fn check_memory(&self, device: &dyn Device) -> f32 {
        let cmd = "free -m";
        match device.execute(cmd).await {
            Ok(result) => parse_free_usage(&result.stdout),
            Err(e) => {
                warn!(error = %e, "failed to check memory usage");
                0.0
            }
        }
    }
}

/// Parses `df -h /` output and returns the usage percentage.
///
/// Expected format:
/// ```text
/// Filesystem      Size  Used Avail Use% Mounted on
/// /dev/sda1        50G   20G   30G  40% /
/// ```
fn parse_df_usage(output: &str) -> f32 {
    for line in output.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // The Use% column is typically the 5th field (index 4).
        if fields.len() >= 5 {
            let pct_str = fields[4].trim_end_matches('%');
            if let Ok(pct) = pct_str.parse::<f32>() {
                return pct;
            }
        }
    }
    0.0
}

/// Parses `free -m` output and returns memory usage percentage.
///
/// Expected format:
/// ```text
///               total        used        free      shared  buff/cache   available
/// Mem:           7983        3500        1200         150        3283        4100
/// ```
fn parse_free_usage(output: &str) -> f32 {
    for line in output.lines() {
        if line.starts_with("Mem:") {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 3 {
                let total: f32 = fields[1].parse().unwrap_or(0.0);
                let used: f32 = fields[2].parse().unwrap_or(0.0);
                if total > 0.0 {
                    return (used / total) * 100.0;
                }
            }
        }
    }
    0.0
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_device_is_unhealthy() {
        let dh = DeviceHealth::unreachable("test-switch");
        assert_eq!(dh.status(), HealthStatus::Unhealthy);
    }

    #[test]
    fn healthy_device() {
        let mut services = HashMap::new();
        services.insert("bgp".into(), true);
        services.insert("swss".into(), true);

        let dh = DeviceHealth {
            hostname: "dut1".into(),
            reachable: true,
            ping_latency_ms: 1.5,
            critical_services: services,
            bgp_sessions_up: 4,
            bgp_sessions_total: 4,
            disk_usage_pct: 30.0,
            memory_usage_pct: 50.0,
        };
        assert_eq!(dh.status(), HealthStatus::Healthy);
    }

    #[test]
    fn degraded_bgp() {
        let dh = DeviceHealth {
            hostname: "dut1".into(),
            reachable: true,
            ping_latency_ms: 1.0,
            critical_services: HashMap::new(),
            bgp_sessions_up: 2,
            bgp_sessions_total: 4,
            disk_usage_pct: 10.0,
            memory_usage_pct: 10.0,
        };
        assert_eq!(dh.status(), HealthStatus::Degraded);
    }

    #[test]
    fn aggregate_worst_of() {
        let healthy = DeviceHealth {
            hostname: "a".into(),
            reachable: true,
            ping_latency_ms: 1.0,
            critical_services: HashMap::new(),
            bgp_sessions_up: 0,
            bgp_sessions_total: 0,
            disk_usage_pct: 10.0,
            memory_usage_pct: 10.0,
        };
        let bad = DeviceHealth::unreachable("b");

        let overall = TestbedHealth::aggregate(&[healthy, bad]);
        assert_eq!(overall, HealthStatus::Unhealthy);
    }

    #[test]
    fn aggregate_empty_is_unknown() {
        assert_eq!(TestbedHealth::aggregate(&[]), HealthStatus::Unknown);
    }

    #[test]
    fn parse_df_basic() {
        let output = "\
Filesystem      Size  Used Avail Use% Mounted on
/dev/sda1        50G   20G   30G  40% /";
        assert!((parse_df_usage(output) - 40.0).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_df_empty() {
        assert!((parse_df_usage("") - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_free_basic() {
        let output = "\
              total        used        free      shared  buff/cache   available
Mem:           8000        4000        2000         100        1900        3800
Swap:          2048           0        2048";
        let pct = parse_free_usage(output);
        assert!((pct - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_free_empty() {
        assert!((parse_free_usage("") - 0.0).abs() < f32::EPSILON);
    }
}
