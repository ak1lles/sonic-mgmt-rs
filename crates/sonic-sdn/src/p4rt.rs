//! P4Runtime client for programmable forwarding-plane control.
//!
//! Implements connection setup with master arbitration, pipeline
//! configuration, table entry management, and counter reads. RPC dispatch
//! uses generated tonic stubs with conversions between the ergonomic public
//! Rust types and the proto wire format.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tracing::{debug, info, warn};

use sonic_core::{Result, SonicError};

use crate::proto::p4::v1 as pb;

// ---------------------------------------------------------------------------
// P4Runtime message types
// ---------------------------------------------------------------------------

/// A single match field in a table entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchField {
    /// Field ID (from the P4Info).
    pub field_id: u32,
    /// Match type.
    pub match_type: MatchType,
    /// Match value as bytes.
    pub value: Vec<u8>,
    /// Mask for ternary/optional matches.
    pub mask: Option<Vec<u8>>,
    /// Range bounds for range matches.
    pub range_low: Option<Vec<u8>>,
    pub range_high: Option<Vec<u8>>,
}

/// Match types supported by P4Runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    Exact,
    Lpm,
    Ternary,
    Range,
    Optional,
}

/// An action with parameters for a table entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// Action ID (from the P4Info).
    pub action_id: u32,
    /// Action parameters.
    pub params: Vec<ActionParam>,
}

/// A single action parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionParam {
    /// Parameter ID.
    pub param_id: u32,
    /// Parameter value as bytes.
    pub value: Vec<u8>,
}

/// A P4 table entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableEntry {
    /// Table ID (from the P4Info).
    pub table_id: u32,
    /// Table name (for human readability).
    pub table_name: String,
    /// Match fields that identify this entry.
    pub match_fields: Vec<MatchField>,
    /// Action to execute when the entry matches.
    pub action: Option<Action>,
    /// Priority for ternary/range tables (higher = more specific).
    pub priority: i32,
    /// Whether this entry is the default action.
    pub is_default_action: bool,
    /// Idle timeout in nanoseconds (0 = no timeout).
    pub idle_timeout_ns: i64,
    /// Arbitrary metadata.
    pub metadata: Vec<u8>,
}

impl TableEntry {
    /// Creates a new table entry for the given table.
    pub fn new(table_id: u32, table_name: impl Into<String>) -> Self {
        Self {
            table_id,
            table_name: table_name.into(),
            match_fields: Vec::new(),
            action: None,
            priority: 0,
            is_default_action: false,
            idle_timeout_ns: 0,
            metadata: Vec::new(),
        }
    }

    /// Adds an exact match field.
    pub fn with_exact_match(mut self, field_id: u32, value: Vec<u8>) -> Self {
        self.match_fields.push(MatchField {
            field_id,
            match_type: MatchType::Exact,
            value,
            mask: None,
            range_low: None,
            range_high: None,
        });
        self
    }

    /// Adds an LPM match field.
    pub fn with_lpm_match(
        mut self,
        field_id: u32,
        value: Vec<u8>,
        prefix_len: u32,
    ) -> Self {
        let mask = prefix_len.to_be_bytes().to_vec();
        self.match_fields.push(MatchField {
            field_id,
            match_type: MatchType::Lpm,
            value,
            mask: Some(mask),
            range_low: None,
            range_high: None,
        });
        self
    }

    /// Adds a ternary match field.
    pub fn with_ternary_match(
        mut self,
        field_id: u32,
        value: Vec<u8>,
        mask: Vec<u8>,
    ) -> Self {
        self.match_fields.push(MatchField {
            field_id,
            match_type: MatchType::Ternary,
            value,
            mask: Some(mask),
            range_low: None,
            range_high: None,
        });
        self
    }

    /// Sets the action for this entry.
    pub fn with_action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }

    /// Sets the priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

/// A counter entry read from the device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterEntry {
    /// Counter ID.
    pub counter_id: u32,
    /// Counter name.
    pub counter_name: String,
    /// Index within the counter array.
    pub index: i64,
    /// Byte count.
    pub byte_count: i64,
    /// Packet count.
    pub packet_count: i64,
}

// ---------------------------------------------------------------------------
// Proto <-> public type conversions
// ---------------------------------------------------------------------------

fn match_field_to_proto(mf: &MatchField) -> pb::FieldMatch {
    let field_match_type = match mf.match_type {
        MatchType::Exact => Some(pb::field_match::FieldMatchType::Exact(
            pb::field_match::Exact { value: mf.value.clone() },
        )),
        MatchType::Lpm => {
            let prefix_len = mf.mask.as_ref().map(|m| {
                if m.len() >= 4 {
                    i32::from_be_bytes([m[0], m[1], m[2], m[3]])
                } else {
                    0
                }
            }).unwrap_or(0);
            Some(pb::field_match::FieldMatchType::Lpm(
                pb::field_match::Lpm { value: mf.value.clone(), prefix_len },
            ))
        }
        MatchType::Ternary => Some(pb::field_match::FieldMatchType::Ternary(
            pb::field_match::Ternary {
                value: mf.value.clone(),
                mask: mf.mask.clone().unwrap_or_default(),
            },
        )),
        MatchType::Range => Some(pb::field_match::FieldMatchType::Range(
            pb::field_match::Range {
                low: mf.range_low.clone().unwrap_or_default(),
                high: mf.range_high.clone().unwrap_or_default(),
            },
        )),
        MatchType::Optional => Some(pb::field_match::FieldMatchType::Optional(
            pb::field_match::Optional { value: mf.value.clone() },
        )),
    };

    pb::FieldMatch {
        field_id: mf.field_id,
        field_match_type,
    }
}

fn match_field_from_proto(fm: &pb::FieldMatch) -> MatchField {
    match &fm.field_match_type {
        Some(pb::field_match::FieldMatchType::Exact(e)) => MatchField {
            field_id: fm.field_id,
            match_type: MatchType::Exact,
            value: e.value.clone(),
            mask: None,
            range_low: None,
            range_high: None,
        },
        Some(pb::field_match::FieldMatchType::Lpm(l)) => MatchField {
            field_id: fm.field_id,
            match_type: MatchType::Lpm,
            value: l.value.clone(),
            mask: Some(l.prefix_len.to_be_bytes().to_vec()),
            range_low: None,
            range_high: None,
        },
        Some(pb::field_match::FieldMatchType::Ternary(t)) => MatchField {
            field_id: fm.field_id,
            match_type: MatchType::Ternary,
            value: t.value.clone(),
            mask: Some(t.mask.clone()),
            range_low: None,
            range_high: None,
        },
        Some(pb::field_match::FieldMatchType::Range(r)) => MatchField {
            field_id: fm.field_id,
            match_type: MatchType::Range,
            value: Vec::new(),
            mask: None,
            range_low: Some(r.low.clone()),
            range_high: Some(r.high.clone()),
        },
        Some(pb::field_match::FieldMatchType::Optional(o)) => MatchField {
            field_id: fm.field_id,
            match_type: MatchType::Optional,
            value: o.value.clone(),
            mask: None,
            range_low: None,
            range_high: None,
        },
        None => MatchField {
            field_id: fm.field_id,
            match_type: MatchType::Exact,
            value: Vec::new(),
            mask: None,
            range_low: None,
            range_high: None,
        },
    }
}

fn table_entry_to_proto(entry: &TableEntry) -> pb::TableEntry {
    let action = entry.action.as_ref().map(|a| {
        pb::TableAction {
            r#type: Some(pb::table_action::Type::Action(pb::Action {
                action_id: a.action_id,
                params: a.params.iter().map(|p| pb::action::Param {
                    param_id: p.param_id,
                    value: p.value.clone(),
                }).collect(),
            })),
        }
    });

    pb::TableEntry {
        table_id: entry.table_id,
        r#match: entry.match_fields.iter().map(match_field_to_proto).collect(),
        action,
        priority: entry.priority,
        controller_metadata: 0,
        is_default_action: entry.is_default_action,
        idle_timeout_ns: entry.idle_timeout_ns,
        counter_data: None,
    }
}

fn table_entry_from_proto(te: &pb::TableEntry) -> TableEntry {
    let action = te.action.as_ref().and_then(|ta| {
        match &ta.r#type {
            Some(pb::table_action::Type::Action(a)) => Some(Action {
                action_id: a.action_id,
                params: a.params.iter().map(|p| ActionParam {
                    param_id: p.param_id,
                    value: p.value.clone(),
                }).collect(),
            }),
            None => None,
        }
    });

    TableEntry {
        table_id: te.table_id,
        table_name: String::new(),
        match_fields: te.r#match.iter().map(match_field_from_proto).collect(),
        action,
        priority: te.priority,
        is_default_action: te.is_default_action,
        idle_timeout_ns: te.idle_timeout_ns,
        metadata: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// P4RuntimeClient
// ---------------------------------------------------------------------------

/// P4Runtime client for managing the forwarding pipeline and table entries.
pub struct P4RuntimeClient {
    /// Target endpoint.
    endpoint: String,

    /// Device ID (as configured on the switch's P4Runtime agent).
    device_id: u64,

    /// Established gRPC channel.
    channel: Option<Channel>,

    /// Whether master arbitration has been completed.
    is_master: bool,

    /// Request timeout.
    timeout: Duration,

    /// Election ID used for master arbitration.
    election_id: (u64, u64),
}

impl std::fmt::Debug for P4RuntimeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P4RuntimeClient")
            .field("endpoint", &self.endpoint)
            .field("device_id", &self.device_id)
            .field("connected", &self.channel.is_some())
            .field("is_master", &self.is_master)
            .finish()
    }
}

impl P4RuntimeClient {
    /// Creates a new P4Runtime client.
    pub fn new(endpoint: impl Into<String>, device_id: u64) -> Self {
        Self {
            endpoint: endpoint.into(),
            device_id,
            channel: None,
            is_master: false,
            timeout: Duration::from_secs(30),
            election_id: (0, 1),
        }
    }

    /// Sets the election ID for master arbitration.
    pub fn with_election_id(mut self, high: u64, low: u64) -> Self {
        self.election_id = (high, low);
        self
    }

    /// Sets the request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Returns a reference to the channel or an error if not connected.
    fn channel(&self) -> Result<&Channel> {
        self.channel.as_ref().ok_or_else(|| SonicError::Connection {
            host: self.endpoint.clone(),
            reason: "not connected".to_owned(),
        })
    }

    /// Builds a generated P4Runtime client from the current channel.
    fn grpc_client(&self) -> Result<pb::p4_runtime_client::P4RuntimeClient<Channel>> {
        let channel = self.channel()?.clone();
        Ok(pb::p4_runtime_client::P4RuntimeClient::new(channel))
    }

    /// Returns the election ID as a proto Uint128.
    fn election_id_proto(&self) -> pb::Uint128 {
        pb::Uint128 {
            high: self.election_id.0,
            low: self.election_id.1,
        }
    }

    /// Establishes a connection and performs master arbitration.
    pub async fn connect(&mut self) -> Result<()> {
        info!(
            endpoint = %self.endpoint,
            device_id = self.device_id,
            "connecting to P4Runtime target"
        );

        let mut ep = Endpoint::from_shared(self.endpoint.clone()).map_err(|e| {
            SonicError::Connection {
                host: self.endpoint.clone(),
                reason: format!("invalid endpoint: {e}"),
            }
        })?;

        ep = ep
            .timeout(self.timeout)
            .connect_timeout(Duration::from_secs(10));

        if self.endpoint.starts_with("https") {
            let tls = ClientTlsConfig::new();
            ep = ep.tls_config(tls).map_err(|e| {
                SonicError::Connection {
                    host: self.endpoint.clone(),
                    reason: format!("TLS config error: {e}"),
                }
            })?;
        }

        let channel = ep.connect().await.map_err(|e| {
            SonicError::Connection {
                host: self.endpoint.clone(),
                reason: format!("gRPC connect failed: {e}"),
            }
        })?;

        self.channel = Some(channel);
        info!(endpoint = %self.endpoint, "P4Runtime connection established");

        // Attempt master arbitration.
        self.master_arbitration().await?;

        Ok(())
    }

    /// Performs the master arbitration handshake via the StreamChannel
    /// bidirectional streaming RPC.
    async fn master_arbitration(&mut self) -> Result<()> {
        info!(
            device_id = self.device_id,
            election_id = ?(self.election_id),
            "performing master arbitration"
        );

        let arb_request = pb::StreamMessageRequest {
            update: Some(pb::stream_message_request::Update::Arbitration(
                pb::MasterArbitrationUpdate {
                    device_id: self.device_id,
                    election_id: Some(self.election_id_proto()),
                    status: None,
                },
            )),
        };

        let mut client = self.grpc_client()?;

        // Send a single arbitration request through the bidirectional stream.
        let (req_tx, req_rx) = mpsc::channel(1);
        if req_tx.send(arb_request).await.is_err() {
            return Err(SonicError::Grpc(
                "failed to send arbitration request".to_owned(),
            ));
        }
        drop(req_tx);

        let stream = tokio_stream::wrappers::ReceiverStream::new(req_rx);
        let result = client.stream_channel(stream).await;

        match result {
            Ok(response) => {
                let mut resp_stream = response.into_inner();
                match resp_stream.message().await {
                    Ok(Some(msg)) => {
                        if let Some(pb::stream_message_response::Update::Arbitration(arb)) =
                            msg.update
                        {
                            let status_code = arb
                                .status
                                .as_ref()
                                .map(|s| s.code)
                                .unwrap_or(0);

                            if status_code == 0 {
                                self.is_master = true;
                                info!("master arbitration succeeded");
                            } else {
                                self.is_master = false;
                                warn!(status_code, "not elected as master");
                            }
                        } else {
                            // Response received but no arbitration update; treat as success.
                            self.is_master = true;
                            debug!("arbitration response had no arbitration update, assuming master");
                        }
                    }
                    Ok(None) => {
                        // Empty stream; some implementations close immediately on success.
                        self.is_master = true;
                        debug!("arbitration stream closed immediately, assuming master");
                    }
                    Err(e) => {
                        warn!(error = %e, "arbitration stream error");
                        self.is_master = false;
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "StreamChannel RPC failed during arbitration");
                self.is_master = false;
            }
        }

        Ok(())
    }

    /// Pushes a P4 forwarding pipeline configuration to the device.
    pub async fn set_forwarding_pipeline(
        &self,
        p4info: &[u8],
        device_config: &[u8],
    ) -> Result<()> {
        if !self.is_master {
            return Err(SonicError::Grpc(
                "cannot set pipeline: not the master controller".to_owned(),
            ));
        }

        info!(
            device_id = self.device_id,
            p4info_size = p4info.len(),
            config_size = device_config.len(),
            "setting forwarding pipeline"
        );

        // Deserialize the P4Info protobuf bytes into the generated type.
        let p4info_msg: Option<crate::proto::p4::config::v1::P4Info> =
            if p4info.is_empty() {
                None
            } else {
                Some(prost::Message::decode(p4info).map_err(|e| {
                    SonicError::Grpc(format!("failed to decode P4Info: {e}"))
                })?)
            };

        let request = pb::SetForwardingPipelineConfigRequest {
            device_id: self.device_id,
            election_id: Some(self.election_id_proto()),
            action: pb::set_forwarding_pipeline_config_request::Action::VerifyAndCommit
                as i32,
            config: Some(pb::ForwardingPipelineConfig {
                p4info: p4info_msg,
                p4_device_config: device_config.to_vec(),
                cookie: Vec::new(),
            }),
        };

        let mut client = self.grpc_client()?;
        client
            .set_forwarding_pipeline_config(request)
            .await
            .map_err(|e| {
                SonicError::Grpc(format!("SetForwardingPipelineConfig failed: {e}"))
            })?;

        info!("forwarding pipeline set successfully");
        Ok(())
    }

    /// Writes a table entry (install a forwarding rule).
    pub async fn write_table_entry(
        &self,
        table_id: u32,
        match_fields: Vec<MatchField>,
        action_id: u32,
        action_params: Vec<ActionParam>,
    ) -> Result<()> {
        if !self.is_master {
            return Err(SonicError::Grpc(
                "cannot write: not the master controller".to_owned(),
            ));
        }

        let entry = TableEntry {
            table_id,
            table_name: String::new(),
            match_fields,
            action: Some(Action {
                action_id,
                params: action_params,
            }),
            priority: 0,
            is_default_action: false,
            idle_timeout_ns: 0,
            metadata: Vec::new(),
        };

        info!(
            table_id,
            action_id,
            match_count = entry.match_fields.len(),
            "writing table entry"
        );

        let proto_entry = table_entry_to_proto(&entry);

        let request = pb::WriteRequest {
            device_id: self.device_id,
            election_id: Some(self.election_id_proto()),
            updates: vec![pb::Update {
                r#type: pb::update::Type::Insert as i32,
                entity: Some(pb::Entity {
                    entity: Some(pb::entity::Entity::TableEntry(proto_entry)),
                }),
            }],
        };

        let mut client = self.grpc_client()?;
        client.write(request).await.map_err(|e| {
            SonicError::Grpc(format!("P4Runtime Write failed: {e}"))
        })?;

        debug!(table_id, "table entry written");
        Ok(())
    }

    /// Reads all table entries for the given table.
    pub async fn read_table_entries(
        &self,
        table_id: u32,
    ) -> Result<Vec<TableEntry>> {
        debug!(table_id, "reading table entries");

        let request = pb::ReadRequest {
            device_id: self.device_id,
            entities: vec![pb::Entity {
                entity: Some(pb::entity::Entity::TableEntry(pb::TableEntry {
                    table_id,
                    r#match: Vec::new(),
                    action: None,
                    priority: 0,
                    controller_metadata: 0,
                    is_default_action: false,
                    idle_timeout_ns: 0,
                    counter_data: None,
                })),
            }],
        };

        let mut client = self.grpc_client()?;
        let response = client.read(request).await.map_err(|e| {
            SonicError::Grpc(format!("P4Runtime Read failed: {e}"))
        })?;

        // Read is a server-streaming RPC.
        let mut stream = response.into_inner();
        let mut entries = Vec::new();

        while let Some(msg) = stream.message().await.map_err(|e| {
            SonicError::Grpc(format!("read stream error: {e}"))
        })? {
            for entity in &msg.entities {
                if let Some(pb::entity::Entity::TableEntry(te)) = &entity.entity {
                    entries.push(table_entry_from_proto(te));
                }
            }
        }

        debug!(table_id, count = entries.len(), "read table entries");
        Ok(entries)
    }

    /// Reads counter entries for the given counter.
    pub async fn read_counters(
        &self,
        counter_id: u32,
    ) -> Result<Vec<CounterEntry>> {
        debug!(counter_id, "reading counters");

        let request = pb::ReadRequest {
            device_id: self.device_id,
            entities: vec![pb::Entity {
                entity: Some(pb::entity::Entity::CounterEntry(pb::CounterEntry {
                    counter_id,
                    index: None,
                    data: None,
                })),
            }],
        };

        let mut client = self.grpc_client()?;
        let response = client.read(request).await.map_err(|e| {
            SonicError::Grpc(format!("P4Runtime Read (counters) failed: {e}"))
        })?;

        let mut stream = response.into_inner();
        let mut counters = Vec::new();

        while let Some(msg) = stream.message().await.map_err(|e| {
            SonicError::Grpc(format!("counter read stream error: {e}"))
        })? {
            for entity in &msg.entities {
                if let Some(pb::entity::Entity::CounterEntry(ce)) = &entity.entity {
                    counters.push(CounterEntry {
                        counter_id: ce.counter_id,
                        counter_name: String::new(),
                        index: ce.index.map(|i| i.index).unwrap_or(0),
                        byte_count: ce.data.map(|d| d.byte_count).unwrap_or(0),
                        packet_count: ce.data.map(|d| d.packet_count).unwrap_or(0),
                    });
                }
            }
        }

        debug!(counter_id, count = counters.len(), "read counter entries");
        Ok(counters)
    }

    /// Returns whether the client is connected.
    pub fn is_connected(&self) -> bool {
        self.channel.is_some()
    }

    /// Returns whether this client is the master controller.
    pub fn is_master(&self) -> bool {
        self.is_master
    }

    /// Returns the device ID.
    pub fn device_id(&self) -> u64 {
        self.device_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_entry_builder() {
        let entry = TableEntry::new(1, "ipv4_lpm")
            .with_exact_match(1, vec![10, 0, 0, 1])
            .with_action(Action {
                action_id: 1,
                params: vec![ActionParam {
                    param_id: 1,
                    value: vec![0, 0, 0, 1],
                }],
            })
            .with_priority(100);

        assert_eq!(entry.table_id, 1);
        assert_eq!(entry.table_name, "ipv4_lpm");
        assert_eq!(entry.match_fields.len(), 1);
        assert_eq!(entry.match_fields[0].match_type, MatchType::Exact);
        assert!(entry.action.is_some());
        assert_eq!(entry.priority, 100);
    }

    #[test]
    fn table_entry_lpm_match() {
        let entry =
            TableEntry::new(2, "ipv4_lpm").with_lpm_match(1, vec![10, 0, 0, 0], 24);
        assert_eq!(entry.match_fields[0].match_type, MatchType::Lpm);
        assert!(entry.match_fields[0].mask.is_some());
    }

    #[test]
    fn table_entry_ternary_match() {
        let entry = TableEntry::new(3, "acl")
            .with_ternary_match(1, vec![0xFF, 0x00], vec![0xFF, 0x00]);
        assert_eq!(entry.match_fields[0].match_type, MatchType::Ternary);
    }

    #[test]
    fn counter_entry_structure() {
        let counter = CounterEntry {
            counter_id: 1,
            counter_name: "ingress_counter".into(),
            index: 42,
            byte_count: 1024,
            packet_count: 10,
        };
        assert_eq!(counter.byte_count, 1024);
        assert_eq!(counter.packet_count, 10);
    }

    #[test]
    fn client_not_connected() {
        let client = P4RuntimeClient::new("https://10.0.0.1:9559", 1);
        assert!(!client.is_connected());
        assert!(!client.is_master());
        assert_eq!(client.device_id(), 1);
    }

    #[test]
    fn match_type_serialization() {
        for mt in [
            MatchType::Exact,
            MatchType::Lpm,
            MatchType::Ternary,
            MatchType::Range,
            MatchType::Optional,
        ] {
            let json = serde_json::to_string(&mt).unwrap();
            let deserialized: MatchType = serde_json::from_str(&json).unwrap();
            assert_eq!(mt, deserialized);
        }
    }

    #[test]
    fn table_entry_serialization() {
        let entry = TableEntry::new(1, "test_table")
            .with_exact_match(1, vec![10, 0, 0, 1])
            .with_action(Action {
                action_id: 2,
                params: vec![ActionParam {
                    param_id: 1,
                    value: vec![1],
                }],
            });

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: TableEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.table_id, 1);
        assert_eq!(deserialized.match_fields.len(), 1);
    }

    #[test]
    fn table_entry_proto_roundtrip() {
        let entry = TableEntry::new(1, "test")
            .with_exact_match(1, vec![10, 0, 0, 1])
            .with_lpm_match(2, vec![192, 168, 0, 0], 16)
            .with_ternary_match(3, vec![0xFF], vec![0xFF])
            .with_action(Action {
                action_id: 5,
                params: vec![ActionParam { param_id: 1, value: vec![42] }],
            })
            .with_priority(100);

        let proto = table_entry_to_proto(&entry);
        assert_eq!(proto.table_id, 1);
        assert_eq!(proto.r#match.len(), 3);
        assert_eq!(proto.priority, 100);

        let back = table_entry_from_proto(&proto);
        assert_eq!(back.table_id, 1);
        assert_eq!(back.match_fields.len(), 3);
        assert_eq!(back.match_fields[0].match_type, MatchType::Exact);
        assert_eq!(back.match_fields[1].match_type, MatchType::Lpm);
        assert_eq!(back.match_fields[2].match_type, MatchType::Ternary);
        assert_eq!(back.action.as_ref().unwrap().action_id, 5);
        assert_eq!(back.priority, 100);
    }

    #[test]
    fn match_field_exact_roundtrip() {
        let mf = MatchField {
            field_id: 1,
            match_type: MatchType::Exact,
            value: vec![10, 0, 0, 1],
            mask: None,
            range_low: None,
            range_high: None,
        };
        let proto = match_field_to_proto(&mf);
        let back = match_field_from_proto(&proto);
        assert_eq!(back.field_id, 1);
        assert_eq!(back.match_type, MatchType::Exact);
        assert_eq!(back.value, vec![10, 0, 0, 1]);
    }
}
