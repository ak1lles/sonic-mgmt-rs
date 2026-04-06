//! CLI output parsers for SONiC device commands.
//!
//! Each parser handles the most common output format variations and is
//! designed to be tolerant of extra whitespace, missing fields, and minor
//! format differences across SONiC versions.

use std::net::IpAddr;

use regex::Regex;
use sonic_core::{
    AclStage, AclTable, AclTableType, BasicFacts, BgpFacts, BgpNeighbor,
    BgpState, LacpMode, LagInfo, PortInfo, PortStatus, Result, TaggingMode,
    VlanInfo, VlanMember,
};
use tracing::instrument;

// =========================================================================
// show version
// =========================================================================

/// Parse the combined output of `show version` and `show platform summary`
/// into a [`BasicFacts`].
#[instrument(skip(output))]
pub fn parse_show_version(output: &str) -> BasicFacts {
    let mut facts = BasicFacts::default();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Key-value lines use `:` as a separator.
        if let Some((key, value)) = split_kv(line) {
            let key_lower = key.to_lowercase();
            let val = value.trim().to_string();

            if key_lower.contains("sonic software version")
                || key_lower.contains("software version")
                || key_lower == "version"
            {
                facts.os_version = val;
            } else if key_lower.contains("hwsku") || key_lower == "hardware sku" {
                facts.hwsku = val;
            } else if key_lower.contains("platform") {
                facts.platform = val;
            } else if key_lower.contains("hostname") {
                facts.hostname = val;
            } else if key_lower.contains("serial") || key_lower.contains("serial number") {
                facts.serial_number = val;
            } else if key_lower.contains("model") || key_lower.contains("hardware") {
                if facts.model.is_empty() {
                    facts.model = val;
                }
            } else if key_lower.contains("mac") || key_lower.contains("base mac") {
                facts.mac_address = val;
            } else if key_lower.contains("asic") {
                facts.asic_type = val;
            } else if key_lower.contains("kernel") {
                facts.kernel_version = val;
            } else if key_lower.contains("uptime") {
                facts.uptime = parse_uptime_seconds(&val);
            }
        }
    }

    facts
}

/// Parse a rough uptime string ("3 days, 12:34:56") into total seconds.
fn parse_uptime_seconds(s: &str) -> u64 {
    let mut total: u64 = 0;

    // Days component ("3 days" or "3 day").
    let day_re = Regex::new(r"(\d+)\s*day").unwrap();
    if let Some(cap) = day_re.captures(s) {
        total += cap[1].parse::<u64>().unwrap_or(0) * 86400;
    }

    // H:M:S component.
    let hms_re = Regex::new(r"(\d+):(\d+):(\d+)").unwrap();
    if let Some(cap) = hms_re.captures(s) {
        let h: u64 = cap[1].parse().unwrap_or(0);
        let m: u64 = cap[2].parse().unwrap_or(0);
        let sec: u64 = cap[3].parse().unwrap_or(0);
        total += h * 3600 + m * 60 + sec;
    }

    total
}

// =========================================================================
// show ip bgp summary  (plain text)
// =========================================================================

/// Parse `show ip bgp summary` plain-text output.
///
/// Example format:
/// ```text
/// BGP router identifier 10.1.0.32, local AS number 65100 vrf-id 0
/// ...
/// Neighbor        V  AS   MsgRcvd MsgSent TblVer InQ OutQ Up/Down  State/PfxRcd
/// 10.0.0.57       4  64600  12345   12345  0      0   0    01:02:03 6402
/// ```
#[instrument(skip(output))]
pub fn parse_bgp_summary(output: &str) -> BgpFacts {
    let mut facts = BgpFacts::default();
    let mut in_table = false;

    let header_re = Regex::new(r"(?i)^Neighbor").unwrap();

    for line in output.lines() {
        let line = line.trim();

        // Router identifier line.
        if line.starts_with("BGP router identifier") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(id) = parts.get(3) {
                facts.router_id = id.trim_end_matches(',').to_string();
            }
            // "local AS number XXXXX"
            for (i, p) in parts.iter().enumerate() {
                if *p == "number" {
                    if let Some(asn) = parts.get(i + 1) {
                        facts.local_as = asn.trim_end_matches(|c: char| !c.is_ascii_digit())
                            .parse()
                            .unwrap_or(0);
                    }
                }
            }
            continue;
        }

        // Detect header row.
        if header_re.is_match(line) {
            in_table = true;
            continue;
        }

        // Skip separator lines.
        if line.starts_with("---") || line.starts_with("Total number") {
            continue;
        }

        if in_table && !line.is_empty() {
            if let Some(nbr) = parse_bgp_neighbor_line(line, facts.local_as) {
                facts.neighbors.push(nbr);
            }
        }
    }

    facts
}

/// Parse a single neighbor row from `show ip bgp summary`.
fn parse_bgp_neighbor_line(line: &str, local_as: u32) -> Option<BgpNeighbor> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 9 {
        return None;
    }

    let address: IpAddr = parts[0].parse().ok()?;
    let remote_as: u32 = parts[2].parse().ok()?;
    let state_or_pfx = parts[parts.len() - 1];

    let (state, prefixes_received) = if let Ok(n) = state_or_pfx.parse::<u64>() {
        (BgpState::Established, n)
    } else {
        let s = match state_or_pfx.to_lowercase().as_str() {
            "idle" | "idle(admin)" => BgpState::Idle,
            "connect" => BgpState::Connect,
            "active" => BgpState::Active,
            "opensent" => BgpState::OpenSent,
            "openconfirm" => BgpState::OpenConfirm,
            "established" => BgpState::Established,
            _ => BgpState::Idle,
        };
        (s, 0)
    };

    Some(BgpNeighbor {
        address,
        remote_as,
        local_as,
        state,
        description: None,
        hold_time: 180,
        keepalive: 60,
        prefixes_received,
        prefixes_sent: 0,
        up_since: None,
    })
}

// =========================================================================
// show bgp summary json  (vtysh JSON output)
// =========================================================================

/// Parse the JSON output of `vtysh -c 'show bgp summary json'`.
#[instrument(skip(json_str))]
pub fn parse_bgp_summary_json(json_str: &str) -> Result<BgpFacts> {
    let root: serde_json::Value = serde_json::from_str(json_str)?;
    let mut facts = BgpFacts::default();

    // The JSON has a top-level key per address-family, e.g. "ipv4Unicast".
    // Each contains "routerId", "as", and "peers".
    for (_af_key, af_val) in root.as_object().into_iter().flatten() {
        if facts.router_id.is_empty() {
            if let Some(rid) = af_val.get("routerId").and_then(|v| v.as_str()) {
                facts.router_id = rid.to_string();
            }
        }
        if facts.local_as == 0 {
            if let Some(asn) = af_val.get("as").and_then(|v| v.as_u64()) {
                facts.local_as = asn as u32;
            }
        }

        if let Some(peers) = af_val.get("peers").and_then(|v| v.as_object()) {
            for (peer_ip, peer_val) in peers {
                let address: IpAddr = match peer_ip.parse() {
                    Ok(a) => a,
                    Err(_) => continue,
                };
                let remote_as = peer_val
                    .get("remoteAs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                let state_str = peer_val
                    .get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Idle");
                let state = match state_str.to_lowercase().as_str() {
                    "established" => BgpState::Established,
                    "connect" => BgpState::Connect,
                    "active" => BgpState::Active,
                    "opensent" => BgpState::OpenSent,
                    "openconfirm" => BgpState::OpenConfirm,
                    _ => BgpState::Idle,
                };

                let pfx_rcvd = peer_val
                    .get("pfxRcd")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let pfx_sent = peer_val
                    .get("pfxSnt")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                facts.neighbors.push(BgpNeighbor {
                    address,
                    remote_as,
                    local_as: facts.local_as,
                    state,
                    description: peer_val
                        .get("desc")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    hold_time: peer_val
                        .get("holdtime")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(180) as u32,
                    keepalive: peer_val
                        .get("keepalive")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(60) as u32,
                    prefixes_received: pfx_rcvd,
                    prefixes_sent: pfx_sent,
                    up_since: None,
                });
            }
        }
    }

    Ok(facts)
}

// =========================================================================
// show interfaces status
// =========================================================================

/// Parse `show interfaces status` output.
///
/// Example:
/// ```text
///   Interface        Lanes    Speed    MTU    FEC        Alias    Vlan    Oper    Admin    Type    Asym PFC
/// -----------  -----------  -------  -----  -----  -----------  ------  ------  -------  ------  ----------
///   Ethernet0          0,1     50G   9100     rs  Eth1/1(1)  routed      up       up     QSFP28    N/A
/// ```
#[instrument(skip(output))]
pub fn parse_interface_status(output: &str) -> Vec<PortInfo> {
    let mut ports = Vec::new();
    let mut header_indices: Option<Vec<(String, usize)>> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("---") {
            continue;
        }

        // Detect the header row.
        if trimmed.starts_with("Interface") {
            // Record column names but we use whitespace splitting because
            // SONiC's tabular output isn't perfectly fixed-width across
            // versions.
            header_indices = Some(Vec::new()); // sentinel
            continue;
        }

        if header_indices.is_some() {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            let name = parts[0].to_string();
            if !name.starts_with("Ethernet")
                && !name.starts_with("PortChannel")
                && !name.starts_with("Loopback")
            {
                continue;
            }

            // Best-effort extraction: exact column positions vary, so we
            // search for known tokens.
            let mut speed: u64 = 0;
            let mut mtu: u16 = 9100;
            let mut fec: Option<String> = None;
            let mut alias: Option<String> = None;
            let mut oper = PortStatus::Down;
            let mut admin = PortStatus::Down;
            let mut lanes = Vec::new();

            for (_i, p) in parts.iter().enumerate().skip(1) {
                let lower = p.to_lowercase();
                // Speed: "100G", "25G", "10G", "1G", "100M"
                if lower.ends_with('g') || lower.ends_with('m') {
                    if let Some(s) = parse_speed(p) {
                        speed = s;
                        continue;
                    }
                }
                // MTU is always an integer in the 1500..9999 range.
                if let Ok(m) = p.parse::<u16>() {
                    if (1000..=9999).contains(&m) {
                        mtu = m;
                        continue;
                    }
                }
                // FEC
                if lower == "rs" || lower == "fc" || lower == "none" || lower == "n/a" {
                    if fec.is_none() {
                        fec = Some(p.to_string());
                    }
                    continue;
                }
                // Oper / admin status
                if lower == "up" {
                    if oper == PortStatus::Down {
                        oper = PortStatus::Up;
                    } else {
                        admin = PortStatus::Up;
                    }
                    continue;
                }
                if lower == "down" {
                    // Already default Down; skip.
                    continue;
                }
                // Lanes: "0,1,2,3"
                if p.contains(',') && p.chars().all(|c| c.is_ascii_digit() || c == ',') {
                    lanes = p
                        .split(',')
                        .filter_map(|s| s.parse::<u32>().ok())
                        .collect();
                    continue;
                }
                // Alias: anything containing a `/`.
                if p.contains('/') && alias.is_none() {
                    alias = Some(p.to_string());
                }
            }

            let index = extract_port_index(&name).unwrap_or(0);

            ports.push(PortInfo {
                name,
                alias,
                index,
                speed,
                lanes,
                mtu,
                admin_status: admin,
                oper_status: oper,
                fec,
                autoneg: None,
            });
        }
    }

    ports
}

/// Extract the numeric index from a port name like "Ethernet48".
fn extract_port_index(name: &str) -> Option<u32> {
    let digits: String = name.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let reversed: String = digits.chars().rev().collect();
    reversed.parse().ok()
}

/// Parse a speed token like "100G" / "25G" / "10G" into bits/sec.
fn parse_speed(s: &str) -> Option<u64> {
    let upper = s.to_uppercase();
    if upper.ends_with('G') {
        let num: u64 = upper.trim_end_matches('G').parse().ok()?;
        Some(num * 1_000_000_000)
    } else if upper.ends_with('M') {
        let num: u64 = upper.trim_end_matches('M').parse().ok()?;
        Some(num * 1_000_000)
    } else {
        None
    }
}

// =========================================================================
// show vlan brief
// =========================================================================

/// Parse `show vlan brief` output.
///
/// Example:
/// ```text
/// +-----------+-----------------+-----------+----------------+-----------+
/// |   VLAN ID | IP Address      | Ports     | Port Tagging   | Proxy ARP |
/// +===========+=================+===========+================+===========+
/// |      1000 | 192.168.0.1/21  | Ethernet4 | tagged         | disabled  |
/// |           |                 | Ethernet8 | untagged       |           |
/// +-----------+-----------------+-----------+----------------+-----------+
/// ```
#[instrument(skip(output))]
pub fn parse_vlan_brief(output: &str) -> Vec<VlanInfo> {
    let mut vlans: Vec<VlanInfo> = Vec::new();

    // Lines that matter start with `|`.  We skip the grid/header lines.
    let data_re = Regex::new(r"^\|\s*(\d+)?\s*\|\s*(.*?)\s*\|\s*(.*?)\s*\|\s*(.*?)\s*\|")
        .unwrap();

    for line in output.lines() {
        let line = line.trim();
        if !line.starts_with('|') || line.contains("VLAN ID") || line.starts_with("+") {
            continue;
        }

        if let Some(caps) = data_re.captures(line) {
            let vlan_id_str = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let _ip_str = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            let port_str = caps.get(3).map(|m| m.as_str().trim()).unwrap_or("");
            let tagging_str = caps.get(4).map(|m| m.as_str().trim()).unwrap_or("");

            if !vlan_id_str.is_empty() {
                // New VLAN row.
                let id: u16 = vlan_id_str.parse().unwrap_or(0);
                let mut vlan = VlanInfo {
                    id,
                    name: format!("Vlan{}", id),
                    members: Vec::new(),
                    ip_addresses: Vec::new(),
                    dhcp_servers: Vec::new(),
                };
                if !port_str.is_empty() {
                    vlan.members.push(VlanMember {
                        port: port_str.to_string(),
                        tagging_mode: parse_tagging(tagging_str),
                    });
                }
                vlans.push(vlan);
            } else if !port_str.is_empty() {
                // Continuation row (port belonging to the previous VLAN).
                if let Some(last) = vlans.last_mut() {
                    last.members.push(VlanMember {
                        port: port_str.to_string(),
                        tagging_mode: parse_tagging(tagging_str),
                    });
                }
            }
        }
    }

    vlans
}

fn parse_tagging(s: &str) -> TaggingMode {
    if s.to_lowercase().contains("untag") {
        TaggingMode::Untagged
    } else {
        TaggingMode::Tagged
    }
}

// =========================================================================
// show acl table / show acl rule
// =========================================================================

/// Parse `show acl table` output into [`AclTable`] entries.
///
/// Example:
/// ```text
/// Name    Type    Binding    Description    Stage    Status
/// ------  ------  ---------  -----------    -------  --------
/// DATAACL L3      Ethernet0  DATAACL        ingress  Active
/// ```
#[instrument(skip(output))]
pub fn parse_acl_table(output: &str) -> Vec<AclTable> {
    let mut tables: Vec<AclTable> = Vec::new();
    let mut in_table = false;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("---") {
            continue;
        }
        if trimmed.starts_with("Name") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let name = parts[0].to_string();
        let table_type = match parts[1].to_uppercase().as_str() {
            "L3" => AclTableType::L3,
            "L3V6" => AclTableType::L3V6,
            "MIRROR" => AclTableType::Mirror,
            "MIRROR_DSCP" => AclTableType::MirrorDscp,
            "PFCWD" => AclTableType::Pfcwd,
            "CTRLPLANE" => AclTableType::Ctrlplane,
            _ => AclTableType::L3,
        };
        let binding = parts[2].to_string();

        // Stage may not always be present.
        let stage = parts
            .iter()
            .find_map(|p| match p.to_lowercase().as_str() {
                "ingress" => Some(AclStage::Ingress),
                "egress" => Some(AclStage::Egress),
                _ => None,
            })
            .unwrap_or(AclStage::Ingress);

        tables.push(AclTable {
            name,
            table_type,
            stage,
            ports: vec![binding],
            rules: Vec::new(),
        });
    }

    tables
}

// =========================================================================
// show interfaces portchannel  (LAG brief)
// =========================================================================

/// Parse `show interfaces portchannel` output.
///
/// Example:
/// ```text
/// Flags: A - active, I - inactive, Up - up, Dw - down, N/A
///   No.  Team Dev       Protocol     Ports
/// -----  ----------     ---------    --------
///     1  PortChannel1   LACP(A)(Up)  Ethernet0(S)  Ethernet4(S)
///     2  PortChannel2   LACP(A)(Up)  Ethernet8(S)
/// ```
#[instrument(skip(output))]
pub fn parse_lag_brief(output: &str) -> Vec<LagInfo> {
    let mut lags = Vec::new();
    let mut in_table = false;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("No.") || trimmed.starts_with("---") {
            in_table = true;
            continue;
        }
        if !in_table || trimmed.is_empty() {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        // parts[0] = number, parts[1] = PortChannelN, parts[2] = protocol string
        let name = parts[1].to_string();
        let protocol_str = parts[2].to_uppercase();
        let lacp_mode = if protocol_str.contains("LACP") {
            if protocol_str.contains("(A)") {
                LacpMode::Active
            } else {
                LacpMode::Passive
            }
        } else {
            LacpMode::On
        };

        let admin_status = if protocol_str.contains("Up") {
            PortStatus::Up
        } else {
            PortStatus::Down
        };

        // Member ports start at parts[3..]; strip "(S)" / "(D)" suffix.
        let member_re = Regex::new(r"^(\S+?)(\(\w+\))?$").unwrap();
        let members: Vec<String> = parts[3..]
            .iter()
            .filter_map(|p| {
                member_re
                    .captures(p)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_string())
            })
            .collect();

        lags.push(LagInfo {
            name,
            members,
            min_links: 1,
            lacp_mode,
            admin_status,
        });
    }

    lags
}

// =========================================================================
// Helpers
// =========================================================================

/// Split a line on the first `:` into (key, value).
fn split_kv(line: &str) -> Option<(&str, &str)> {
    let idx = line.find(':')?;
    let key = line[..idx].trim();
    let value = line[idx + 1..].trim();
    if key.is_empty() {
        return None;
    }
    Some((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_show_version_basic() {
        let output = "\
SONiC Software Version: SONiC.20220531.01
Platform: x86_64-mlnx_msn2700-r0
HwSKU: ACS-MSN2700
Hostname: sonic-dut-01
Serial Number: MT1234567890
Base MAC: 98:03:9b:01:02:03
Uptime: 3 days, 4:05:06
Kernel Version: 5.10.0-18-amd64
ASIC: Mellanox-Spectrum
";
        let facts = parse_show_version(output);
        assert_eq!(facts.os_version, "SONiC.20220531.01");
        assert_eq!(facts.platform, "x86_64-mlnx_msn2700-r0");
        assert_eq!(facts.hwsku, "ACS-MSN2700");
        assert_eq!(facts.hostname, "sonic-dut-01");
        assert_eq!(facts.serial_number, "MT1234567890");
        assert_eq!(facts.mac_address, "98:03:9b:01:02:03");
        assert_eq!(facts.uptime, 3 * 86400 + 4 * 3600 + 5 * 60 + 6);
        assert_eq!(facts.kernel_version, "5.10.0-18-amd64");
    }

    #[test]
    fn test_parse_bgp_summary() {
        let output = "\
BGP router identifier 10.1.0.32, local AS number 65100 vrf-id 0
BGP table version 8972
RIB entries 12745, using 2342K of memory

Neighbor        V  AS    MsgRcvd MsgSent TblVer InQ OutQ Up/Down  State/PfxRcd
10.0.0.57       4  64600   12345   12345  0      0   0    01:02:03 6402
10.0.0.59       4  64601   11111   11111  0      0   0    00:30:00 Idle
";
        let facts = parse_bgp_summary(output);
        assert_eq!(facts.router_id, "10.1.0.32");
        assert_eq!(facts.local_as, 65100);
        assert_eq!(facts.neighbors.len(), 2);
        assert_eq!(facts.neighbors[0].state, BgpState::Established);
        assert_eq!(facts.neighbors[0].prefixes_received, 6402);
        assert_eq!(facts.neighbors[1].state, BgpState::Idle);
    }

    #[test]
    fn test_parse_interface_status() {
        let output = "\
  Interface        Lanes    Speed    MTU    FEC    Alias        Vlan    Oper    Admin    Type        Asym PFC
-----------  -----------  -------  -----  -----  -----------  ------  ------  -------  ----------  ----------
  Ethernet0          0,1     50G   9100     rs   Eth1/1       routed    up       up     QSFP28       N/A
  Ethernet4        4,5,6,7  100G   9100     rs   Eth2/1       trunk     up       up     QSFP28       N/A
  Ethernet8          8,9     25G   9100     none  Eth3/1       routed   down     down    SFP28       N/A
";
        let ports = parse_interface_status(output);
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0].name, "Ethernet0");
        assert_eq!(ports[0].speed, 50_000_000_000);
        assert_eq!(ports[0].mtu, 9100);
        assert_eq!(ports[0].oper_status, PortStatus::Up);
        assert_eq!(ports[1].speed, 100_000_000_000);
        assert_eq!(ports[2].oper_status, PortStatus::Down);
    }

    #[test]
    fn test_parse_vlan_brief() {
        let output = "\
+-----------+-----------------+-----------+----------------+-----------+
|   VLAN ID | IP Address      | Ports     | Port Tagging   | Proxy ARP |
+===========+=================+===========+================+===========+
|      1000 | 192.168.0.1/21  | Ethernet4 | tagged         | disabled  |
|           |                 | Ethernet8 | untagged       |           |
+-----------+-----------------+-----------+----------------+-----------+
|      2000 | 10.0.0.1/24     | Ethernet0 | tagged         | disabled  |
+-----------+-----------------+-----------+----------------+-----------+
";
        let vlans = parse_vlan_brief(output);
        assert_eq!(vlans.len(), 2);
        assert_eq!(vlans[0].id, 1000);
        assert_eq!(vlans[0].members.len(), 2);
        assert_eq!(vlans[0].members[0].tagging_mode, TaggingMode::Tagged);
        assert_eq!(vlans[0].members[1].tagging_mode, TaggingMode::Untagged);
        assert_eq!(vlans[1].id, 2000);
    }

    #[test]
    fn test_parse_lag_brief() {
        let output = "\
Flags: A - active, I - inactive, Up - up, Dw - down, N/A
  No.  Team Dev       Protocol     Ports
-----  ----------     ---------    --------
    1  PortChannel1   LACP(A)(Up)  Ethernet0(S)  Ethernet4(S)
    2  PortChannel2   LACP(A)(Dw)  Ethernet8(S)
";
        let lags = parse_lag_brief(output);
        assert_eq!(lags.len(), 2);
        assert_eq!(lags[0].name, "PortChannel1");
        assert_eq!(lags[0].members, vec!["Ethernet0", "Ethernet4"]);
        assert_eq!(lags[0].lacp_mode, LacpMode::Active);
        assert_eq!(lags[0].admin_status, PortStatus::Up);
        assert_eq!(lags[1].admin_status, PortStatus::Down);
    }

    #[test]
    fn test_parse_bgp_summary_json() {
        let json = r#"{
  "ipv4Unicast": {
    "routerId": "10.1.0.32",
    "as": 65100,
    "peers": {
      "10.0.0.57": {
        "remoteAs": 64600,
        "state": "Established",
        "pfxRcd": 6402,
        "pfxSnt": 100,
        "holdtime": 180,
        "keepalive": 60
      }
    }
  }
}"#;
        let facts = parse_bgp_summary_json(json).unwrap();
        assert_eq!(facts.router_id, "10.1.0.32");
        assert_eq!(facts.local_as, 65100);
        assert_eq!(facts.neighbors.len(), 1);
        assert_eq!(facts.neighbors[0].state, BgpState::Established);
        assert_eq!(facts.neighbors[0].prefixes_received, 6402);
        assert_eq!(facts.neighbors[0].prefixes_sent, 100);
    }

    #[test]
    fn test_parse_acl_table() {
        let output = "\
Name      Type    Binding      Description    Stage    Status
------    ------  ---------    -----------    -------  --------
DATAACL   L3      Ethernet0    DATAACL        ingress  Active
SNMP_ACL  CTRLPLANE  all       SNMP_ACL       ingress  Active
";
        let tables = parse_acl_table(output);
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].name, "DATAACL");
        assert_eq!(tables[0].table_type, AclTableType::L3);
        assert_eq!(tables[0].stage, AclStage::Ingress);
        assert_eq!(tables[1].table_type, AclTableType::Ctrlplane);
    }
}
