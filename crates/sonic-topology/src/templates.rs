//! Template-based topology rendering.
//!
//! [`TopologyRenderer`] uses [Tera](https://docs.rs/tera) to transform a
//! [`TopologyDefinition`] into the configuration artifacts that the testbed
//! consumes: Ansible-compatible inventory, SONiC minigraph XML, and
//! `config_db.json`.

use serde::Serialize;
use tera::{Context, Tera};
use tracing::{debug, info};

use sonic_core::{SonicError, TopologyDefinition};

// ---------------------------------------------------------------------------
// Built-in template strings
// ---------------------------------------------------------------------------

const INVENTORY_TEMPLATE: &str = r#"# Auto-generated testbed inventory
# Topology: {{ topo_type }}
# VMs: {{ vms | length }}

[sonic]
{% for vm in vms %}
{{ vm.name }} ansible_host={{ vm.mgmt_ip }} vm_type={{ vm.vm_type }} vm_offset={{ vm.vm_offset }}
{% endfor %}

[ptf]
ptf_host ansible_host=ptf-{{ topo_type }}

[server:children]
sonic
ptf

[all:vars]
topology_type={{ topo_type }}
num_vms={{ vms | length }}
num_vlans={{ vlans | length }}
"#;

const MINIGRAPH_TEMPLATE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<DeviceMiniGraph xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <DpgDec>
    <DeviceDataPlaneInfo>
      <IPSecurityPolicyList />
      <PortChannelInterfaces>
{% for lag in lag_links %}
        <PortChannel>
          <Name>{{ lag.channel_name }}</Name>
          <Members>
{% for member in lag.members %}
            <Member>{{ member }}</Member>
{% endfor %}
          </Members>
          <MinimumLinks>1</MinimumLinks>
          <Fallback>true</Fallback>
        </PortChannel>
{% endfor %}
      </PortChannelInterfaces>
      <VlanInterfaces>
{% for vlan in vlans %}
        <VlanInterface>
          <VlanID>{{ vlan.id }}</VlanID>
          <Name>{{ vlan.name }}</Name>
{% if vlan.prefix %}
          <Prefix>{{ vlan.prefix }}</Prefix>
{% endif %}
          <Members>
{% for intf in vlan.intfs %}
            <Member>{{ intf }}</Member>
{% endfor %}
          </Members>
        </VlanInterface>
{% endfor %}
      </VlanInterfaces>
      <IPInterfaces>
{% for pair in ip_pairs %}
{% if pair.dut_ipv4 %}
        <IPInterface>
          <Prefix>{{ pair.dut_ipv4_prefix }}</Prefix>
          <Address>{{ pair.dut_ipv4 }}</Address>
          <AttachTo>{{ pair.attach_to }}</AttachTo>
        </IPInterface>
{% endif %}
{% if pair.dut_ipv6 %}
        <IPInterface>
          <Prefix>{{ pair.dut_ipv6_prefix }}</Prefix>
          <Address>{{ pair.dut_ipv6 }}</Address>
          <AttachTo>{{ pair.attach_to }}</AttachTo>
        </IPInterface>
{% endif %}
{% endfor %}
      </IPInterfaces>
    </DeviceDataPlaneInfo>
  </DpgDec>
  <PeerInfo>
{% for vm in vms %}
    <Device>
      <Name>{{ vm.name }}</Name>
      <ManagementAddress>{{ vm.mgmt_ip }}</ManagementAddress>
    </Device>
{% endfor %}
  </PeerInfo>
  <MetadataDeclaration>
    <TopologyType>{{ topo_type }}</TopologyType>
    <NumVMs>{{ vms | length }}</NumVMs>
  </MetadataDeclaration>
</DeviceMiniGraph>
"#;

const CONFIG_DB_TEMPLATE: &str = r#"{
  "DEVICE_METADATA": {
    "localhost": {
      "hostname": "sonic-dut",
      "type": "{{ device_role }}",
      "topology": "{{ topo_type }}"
    }
  },
{% if vlans | length > 0 %}
  "VLAN": {
{% for vlan in vlans %}
    "{{ vlan.name }}": {
      "vlanid": "{{ vlan.id }}"
    }{% if not loop.last %},{% endif %}

{% endfor %}
  },
  "VLAN_MEMBER": {
{% for member in vlan_members %}
    "{{ member.key }}": {
      "tagging_mode": "untagged"
    }{% if not loop.last %},{% endif %}

{% endfor %}
  },
{% endif %}
{% if lag_links | length > 0 %}
  "PORTCHANNEL": {
{% for lag in lag_links %}
    "{{ lag.channel_name }}": {
      "admin_status": "up",
      "min_links": "1",
      "lacp_key": "auto"
    }{% if not loop.last %},{% endif %}

{% endfor %}
  },
  "PORTCHANNEL_MEMBER": {
{% for pm in pc_members %}
    "{{ pm.key }}": {}{% if not loop.last %},{% endif %}

{% endfor %}
  },
{% endif %}
  "INTERFACE": {
{% for intf in interface_entries %}
    "{{ intf.key }}": {}{% if not loop.last %},{% endif %}

{% endfor %}
  },
  "BGP_NEIGHBOR": {
{% for nbr in bgp_neighbors %}
    "{{ nbr.address }}": {
      "rrclient": "0",
      "name": "{{ nbr.vm_name }}",
      "local_addr": "{{ nbr.local_addr }}",
      "asn": "{{ nbr.asn }}"
    }{% if not loop.last %},{% endif %}

{% endfor %}
  }
}
"#;

// ---------------------------------------------------------------------------
// Pre-computed Tera context views
// ---------------------------------------------------------------------------

/// VM serializable for templates.
#[derive(Debug, Serialize)]
struct VmView {
    name: String,
    vm_type: String,
    vm_offset: u32,
    mgmt_ip: String,
    peer_ports: Vec<String>,
}

impl From<&sonic_core::VmDefinition> for VmView {
    fn from(v: &sonic_core::VmDefinition) -> Self {
        Self {
            name: v.name.clone(),
            vm_type: v.vm_type.to_string(),
            vm_offset: v.vm_offset,
            mgmt_ip: v.mgmt_ip.to_string(),
            peer_ports: v.peer_ports.clone(),
        }
    }
}

/// LAG link with a pre-formatted PortChannel name.
#[derive(Debug, Serialize)]
struct LagLinkView {
    lag_id: u32,
    channel_name: String,
    members: Vec<String>,
    vm_index: u32,
}

/// Flattened IP pair for templates (no nested Option gymnastics).
#[derive(Debug, Serialize)]
struct IpPairView {
    vm_index: u32,
    attach_to: String,
    dut_ipv4: Option<String>,
    dut_ipv6: Option<String>,
    dut_ipv4_prefix: Option<String>,
    dut_ipv6_prefix: Option<String>,
    nbr_ipv4: Option<String>,
    nbr_ipv6: Option<String>,
}

/// A VLAN member entry: `"Vlan1000|Ethernet48"`.
#[derive(Debug, Serialize)]
struct VlanMemberEntry {
    key: String,
}

/// A PortChannel member entry: `"PortChannel0000|Ethernet0"`.
#[derive(Debug, Serialize)]
struct PcMemberEntry {
    key: String,
}

/// An INTERFACE entry: `"Ethernet0|10.0.0.0/31"`.
#[derive(Debug, Serialize)]
struct InterfaceEntry {
    key: String,
}

/// A BGP_NEIGHBOR entry.
#[derive(Debug, Serialize)]
struct BgpNeighborEntry {
    address: String,
    vm_name: String,
    local_addr: String,
    asn: u32,
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// Renders topology definitions into configuration file contents.
pub struct TopologyRenderer {
    tera: Tera,
}

impl TopologyRenderer {
    /// Creates a renderer with the built-in templates compiled.
    pub fn new() -> sonic_core::Result<Self> {
        let mut tera = Tera::default();
        tera.add_raw_template("inventory", INVENTORY_TEMPLATE)
            .map_err(|e| SonicError::topology(format!("inventory template: {e}")))?;
        tera.add_raw_template("minigraph", MINIGRAPH_TEMPLATE)
            .map_err(|e| SonicError::topology(format!("minigraph template: {e}")))?;
        tera.add_raw_template("config_db", CONFIG_DB_TEMPLATE)
            .map_err(|e| SonicError::topology(format!("config_db template: {e}")))?;

        Ok(Self { tera })
    }

    /// Renders an Ansible-compatible inventory (INI-like format).
    pub fn render_inventory(&self, def: &TopologyDefinition) -> sonic_core::Result<String> {
        let ctx = self.build_context(def)?;
        info!(topo = %def.topo_type, "rendering inventory");
        self.tera
            .render("inventory", &ctx)
            .map_err(|e| SonicError::topology(format!("render inventory: {e}")))
    }

    /// Renders a SONiC minigraph XML document.
    pub fn render_minigraph(&self, def: &TopologyDefinition) -> sonic_core::Result<String> {
        let ctx = self.build_context(def)?;
        info!(topo = %def.topo_type, "rendering minigraph");
        self.tera
            .render("minigraph", &ctx)
            .map_err(|e| SonicError::topology(format!("render minigraph: {e}")))
    }

    /// Renders a `config_db.json` fragment.
    pub fn render_config_db(&self, def: &TopologyDefinition) -> sonic_core::Result<String> {
        let ctx = self.build_context(def)?;
        info!(topo = %def.topo_type, "rendering config_db.json");
        self.tera
            .render("config_db", &ctx)
            .map_err(|e| SonicError::topology(format!("render config_db: {e}")))
    }

    // -- internal -----------------------------------------------------------

    fn build_context(&self, def: &TopologyDefinition) -> sonic_core::Result<Context> {
        let vms: Vec<VmView> = def.vms.iter().map(VmView::from).collect();

        // Pre-compute LAG views with formatted channel names.
        let lag_links: Vec<LagLinkView> = def
            .lag_links
            .iter()
            .map(|l| LagLinkView {
                lag_id: l.lag_id,
                channel_name: format!("PortChannel{:04}", l.lag_id),
                members: l.members.clone(),
                vm_index: l.vm_index,
            })
            .collect();

        // Pre-compute IP pair views with all formatting done.
        let ip_pairs: Vec<IpPairView> = def
            .ip_pairs
            .iter()
            .map(|a| IpPairView {
                vm_index: a.vm_index,
                attach_to: format!("Ethernet{}", a.vm_index * 4),
                dut_ipv4: a.dut_ip.ipv4.map(|v| v.to_string()),
                dut_ipv6: a.dut_ip.ipv6.map(|v| v.to_string()),
                dut_ipv4_prefix: a.dut_ip.ipv4_prefix.map(|v| v.to_string()),
                dut_ipv6_prefix: a.dut_ip.ipv6_prefix.map(|v| v.to_string()),
                nbr_ipv4: a.neighbor_ip.ipv4.map(|v| v.to_string()),
                nbr_ipv6: a.neighbor_ip.ipv6.map(|v| v.to_string()),
            })
            .collect();

        // Pre-compute VLAN member entries.
        let vlan_members: Vec<VlanMemberEntry> = def
            .vlans
            .iter()
            .flat_map(|vlan| {
                vlan.intfs.iter().map(move |intf| VlanMemberEntry {
                    key: format!("{}|{}", vlan.name, intf),
                })
            })
            .collect();

        // Pre-compute PortChannel member entries.
        let pc_members: Vec<PcMemberEntry> = lag_links
            .iter()
            .flat_map(|lag| {
                lag.members.iter().map(move |member| PcMemberEntry {
                    key: format!("{}|{}", lag.channel_name, member),
                })
            })
            .collect();

        // Pre-compute INTERFACE entries.
        let mut interface_entries: Vec<InterfaceEntry> = Vec::new();
        for pair in &ip_pairs {
            if let Some(ref v4) = pair.dut_ipv4 {
                interface_entries.push(InterfaceEntry {
                    key: format!("{}|{}/31", pair.attach_to, v4),
                });
            }
            if let Some(ref v6) = pair.dut_ipv6 {
                interface_entries.push(InterfaceEntry {
                    key: format!("{}|{}/127", pair.attach_to, v6),
                });
            }
        }

        // Pre-compute BGP neighbor entries.
        let mut bgp_neighbors: Vec<BgpNeighborEntry> = Vec::new();
        for (pair, ip_view) in def.ip_pairs.iter().zip(ip_pairs.iter()) {
            let vm_name = vms
                .get(pair.vm_index as usize)
                .map(|v| v.name.as_str())
                .unwrap_or("VM");
            let asn = 64600 + pair.vm_index;

            if let Some(ref nbr_v4) = ip_view.nbr_ipv4 {
                bgp_neighbors.push(BgpNeighborEntry {
                    address: nbr_v4.clone(),
                    vm_name: vm_name.to_string(),
                    local_addr: ip_view.dut_ipv4.clone().unwrap_or_default(),
                    asn,
                });
            }
            if let Some(ref nbr_v6) = ip_view.nbr_ipv6 {
                bgp_neighbors.push(BgpNeighborEntry {
                    address: nbr_v6.clone(),
                    vm_name: vm_name.to_string(),
                    local_addr: ip_view.dut_ipv6.clone().unwrap_or_default(),
                    asn,
                });
            }
        }

        let device_role = match def.topo_type {
            sonic_core::TopologyType::T0
            | sonic_core::TopologyType::T064
            | sonic_core::TopologyType::T0116
            | sonic_core::TopologyType::Dualtor
            | sonic_core::TopologyType::MgmtTor
            | sonic_core::TopologyType::M0Vlan => "ToRRouter",
            sonic_core::TopologyType::T1
            | sonic_core::TopologyType::T164
            | sonic_core::TopologyType::T1Lag => "LeafRouter",
            sonic_core::TopologyType::T2 => "SpineRouter",
            _ => "Server",
        };

        let mut ctx = Context::new();
        ctx.insert("topo_type", &def.topo_type.to_string());
        ctx.insert("vms", &vms);
        ctx.insert("vlans", &def.vlans);
        ctx.insert("host_interfaces", &def.host_interfaces);
        ctx.insert("lag_links", &lag_links);
        ctx.insert("ip_pairs", &ip_pairs);
        ctx.insert("vlan_members", &vlan_members);
        ctx.insert("pc_members", &pc_members);
        ctx.insert("interface_entries", &interface_entries);
        ctx.insert("bgp_neighbors", &bgp_neighbors);
        ctx.insert("device_role", device_role);

        debug!(
            vms = vms.len(),
            vlans = def.vlans.len(),
            ip_pairs = ip_pairs.len(),
            bgp_neighbors = bgp_neighbors.len(),
            "template context built"
        );

        Ok(ctx)
    }
}

impl Default for TopologyRenderer {
    fn default() -> Self {
        Self::new().expect("built-in templates must compile")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::DefaultTopologyGenerator;
    use sonic_core::{TopologyGenerator, TopologyType};

    fn sample_def() -> TopologyDefinition {
        DefaultTopologyGenerator::veos()
            .generate(TopologyType::T0)
            .unwrap()
    }

    #[test]
    fn inventory_contains_vms() {
        let renderer = TopologyRenderer::default();
        let output = renderer.render_inventory(&sample_def()).unwrap();
        assert!(output.contains("[sonic]"));
        assert!(output.contains("ARISTA00"));
        assert!(output.contains("[ptf]"));
    }

    #[test]
    fn minigraph_is_xml() {
        let renderer = TopologyRenderer::default();
        let output = renderer.render_minigraph(&sample_def()).unwrap();
        assert!(output.contains("<?xml"));
        assert!(output.contains("<DeviceMiniGraph"));
        assert!(output.contains("</DeviceMiniGraph>"));
    }

    #[test]
    fn config_db_is_json_ish() {
        let renderer = TopologyRenderer::default();
        let output = renderer.render_config_db(&sample_def()).unwrap();
        assert!(output.contains("DEVICE_METADATA"));
        assert!(output.contains("\"topology\""));
    }

    #[test]
    fn t1_config_db_has_portchannels() {
        let def = DefaultTopologyGenerator::veos()
            .generate(TopologyType::T1)
            .unwrap();
        let renderer = TopologyRenderer::default();
        let output = renderer.render_config_db(&def).unwrap();
        assert!(output.contains("PORTCHANNEL"));
        assert!(output.contains("PortChannel0000"));
    }

    #[test]
    fn ptf_only_renders() {
        let def = DefaultTopologyGenerator::veos()
            .generate(TopologyType::Ptf32)
            .unwrap();
        let renderer = TopologyRenderer::default();
        // Should not panic even with zero VMs/IPs.
        let _inv = renderer.render_inventory(&def).unwrap();
        let _mg = renderer.render_minigraph(&def).unwrap();
        let _cdb = renderer.render_config_db(&def).unwrap();
    }
}
