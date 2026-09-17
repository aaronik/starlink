//! Minimal protobuf messages used by the local, read-only diagnostic API.
//!
//! Field numbers below were checked against the locally served Starlink web UI.
use anyhow::{Result, bail};
use prost::Message;
use serde::Serialize;

/// The only requests this crate can encode.  There is deliberately no generic RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    Status,
    History,
    Obstructions,
    DeviceInfo,
    RouterStatus,
    Clients,
}

impl Query {
    pub(crate) fn request_tag(self) -> u32 {
        match self {
            Self::Status | Self::RouterStatus => 1004, // get_status
            Self::History => 1007,                     // get_history
            Self::DeviceInfo => 1008,                  // get_device_info
            Self::Obstructions => 2008,                // dish_get_obstruction_map
            Self::Clients => 3002,                     // wifi_get_clients
        }
    }

    pub(crate) fn response_tag(self) -> u32 {
        match self {
            Self::Status => 2004, // dish_get_status
            Self::History => 2006,
            Self::DeviceInfo => 1004,
            Self::Obstructions => 2008,
            Self::RouterStatus => 3004, // wifi_get_status
            Self::Clients => 3002,
        }
    }

    /// A Device.Request protobuf. All supported read requests are empty.
    /// The explicitly selected endpoint determines whether get_status targets a
    /// dish or router; this client never guesses another route.
    pub(crate) fn encode_request(self) -> Vec<u8> {
        let body: Vec<u8> = Vec::new();
        let mut request = Vec::new();
        put_varint(&mut request, u64::from(self.request_tag()) << 3 | 2);
        put_varint(&mut request, body.len() as u64);
        request.extend(body);
        request
    }
}

#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AppStatus {
    #[prost(int32, tag = "1")]
    pub code: i32,
    #[prost(string, tag = "2")]
    pub message: String,
}

#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceInfo {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub hardware_version: String,
    #[prost(string, tag = "3")]
    pub software_version: String,
    #[prost(string, tag = "4")]
    pub country_code: String,
    #[prost(int32, tag = "5")]
    pub utc_offset_s: i32,
    #[prost(bool, tag = "6")]
    pub software_partitions_equal: bool,
    #[prost(bool, tag = "7")]
    pub is_dev: bool,
    #[prost(int32, tag = "8")]
    pub bootcount: i32,
    #[prost(int32, tag = "9")]
    pub anti_rollback_version: i32,
    #[prost(bool, tag = "10")]
    pub is_hitl: bool,
    #[prost(string, tag = "11")]
    pub manufactured_version: String,
    #[prost(int64, tag = "12")]
    pub generation_number: i64,
    #[prost(bool, tag = "13")]
    pub dish_cohoused: bool,
    #[prost(int32, tag = "14")]
    pub board_rev: i32,
    #[prost(string, tag = "15")]
    pub build_id: String,
    #[prost(int32, tag = "16")]
    pub hardware_index: i32,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceState {
    #[prost(uint64, tag = "1")]
    pub uptime_s: u64,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Alerts {
    #[prost(bool, tag = "1")]
    pub motors_stuck: bool,
    #[prost(bool, tag = "2")]
    pub thermal_shutdown: bool,
    #[prost(bool, tag = "3")]
    pub thermal_throttle: bool,
    #[prost(bool, tag = "4")]
    pub unexpected_location: bool,
    #[prost(bool, tag = "5")]
    pub mast_not_near_vertical: bool,
    #[prost(bool, tag = "6")]
    pub slow_ethernet_speeds: bool,
    #[prost(bool, tag = "7")]
    pub roaming: bool,
    #[prost(bool, tag = "8")]
    pub install_pending: bool,
    #[prost(bool, tag = "9")]
    pub is_heating: bool,
    #[prost(bool, tag = "23")]
    pub no_ethernet_link: bool,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ObstructionStats {
    #[prost(float, tag = "1")]
    pub fraction_obstructed: f32,
    #[prost(float, tag = "4")]
    pub valid_s: f32,
    #[prost(bool, tag = "5")]
    pub currently_obstructed: bool,
    #[prost(float, tag = "6")]
    pub avg_prolonged_obstruction_duration_s: f32,
    #[prost(float, tag = "7")]
    pub avg_prolonged_obstruction_interval_s: f32,
    #[prost(bool, tag = "8")]
    pub avg_prolonged_obstruction_valid: bool,
    #[prost(float, tag = "9")]
    pub time_obstructed: f32,
    #[prost(uint32, tag = "10")]
    pub patches_valid: u32,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct GpsStats {
    #[prost(bool, tag = "1")]
    pub gps_valid: bool,
    #[prost(uint32, tag = "2")]
    pub gps_sats: u32,
    #[prost(bool, tag = "3")]
    pub no_sats_after_ttff: bool,
    #[prost(bool, tag = "4")]
    pub inhibit_gps: bool,
    #[prost(int32, tag = "5")]
    pub pnt_filter_convergence_state: i32,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Outage {
    #[prost(int32, tag = "1")]
    pub cause: i32,
    #[prost(int64, tag = "2")]
    pub start_timestamp_ns: i64,
    #[prost(uint64, tag = "3")]
    pub duration_ns: u64,
    #[prost(bool, tag = "4")]
    pub did_switch: bool,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DishStatus {
    #[prost(message, optional, tag = "1")]
    pub device_info: Option<DeviceInfo>,
    #[prost(message, optional, tag = "2")]
    pub device_state: Option<DeviceState>,
    #[prost(message, optional, tag = "1004")]
    pub obstruction_stats: Option<ObstructionStats>,
    #[prost(message, optional, tag = "1005")]
    pub alerts: Option<Alerts>,
    #[prost(float, optional, tag = "1003")]
    pub pop_ping_drop_rate: Option<f32>,
    #[prost(float, optional, tag = "1007")]
    pub downlink_throughput_bps: Option<f32>,
    #[prost(float, optional, tag = "1008")]
    pub uplink_throughput_bps: Option<f32>,
    #[prost(float, optional, tag = "1009")]
    pub pop_ping_latency_ms: Option<f32>,
    #[prost(bool, optional, tag = "1010")]
    pub stow_requested: Option<bool>,
    #[prost(message, optional, tag = "1014")]
    pub outage: Option<Outage>,
    #[prost(message, optional, tag = "1015")]
    pub gps_stats: Option<GpsStats>,
    #[prost(uint32, tag = "1016")]
    pub eth_speed_mbps: u32,
    #[prost(bool, tag = "1018")]
    pub is_snr_above_noise_floor: bool,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct History {
    #[prost(uint64, tag = "1")]
    pub current: u64,
    #[prost(float, repeated, tag = "1001")]
    pub pop_ping_drop_rate: Vec<f32>,
    #[prost(float, repeated, tag = "1002")]
    pub pop_ping_latency_ms: Vec<f32>,
    #[prost(float, repeated, tag = "1003")]
    pub downlink_throughput_bps: Vec<f32>,
    #[prost(float, repeated, tag = "1004")]
    pub uplink_throughput_bps: Vec<f32>,
    #[prost(message, repeated, tag = "1009")]
    pub outages: Vec<Outage>,
    #[prost(float, repeated, tag = "1010")]
    pub power_in: Vec<f32>,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ObstructionMap {
    #[prost(uint32, tag = "1")]
    pub num_rows: u32,
    #[prost(uint32, tag = "2")]
    pub num_cols: u32,
    #[prost(float, repeated, tag = "3")]
    pub snr: Vec<f32>,
    #[prost(float, tag = "4")]
    pub min_elevation_deg: f32,
    #[prost(float, tag = "5")]
    pub max_theta_deg: f32,
    #[prost(int32, tag = "6")]
    pub map_reference_frame: i32,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct GetDeviceInfo {
    #[prost(message, optional, tag = "1")]
    pub device_info: Option<DeviceInfo>,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RouterStatus {
    #[prost(message, optional, tag = "3")]
    pub device_info: Option<DeviceInfo>,
    #[prost(message, optional, tag = "4")]
    pub device_state: Option<DeviceState>,
    #[prost(bool, optional, tag = "1")]
    pub captive_portal_enabled: Option<bool>,
    #[prost(float, optional, tag = "1004")]
    pub ping_drop_rate: Option<f32>,
    #[prost(float, optional, tag = "1005")]
    pub ping_latency_ms: Option<f32>,
    #[prost(float, optional, tag = "1012")]
    pub dish_ping_drop_rate: Option<f32>,
    #[prost(float, optional, tag = "1013")]
    pub dish_ping_latency_ms: Option<f32>,
    #[prost(float, optional, tag = "1014")]
    pub pop_ping_drop_rate: Option<f32>,
    #[prost(float, optional, tag = "1015")]
    pub pop_ping_latency_ms: Option<f32>,
    #[prost(uint32, optional, tag = "1034")]
    pub hops_from_controller: Option<u32>,
    #[prost(bool, optional, tag = "1035")]
    pub no_wan_link: Option<bool>,
}
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Clients {
    #[prost(message, repeated, tag = "1")]
    pub clients: Vec<ClientEntry>,
    #[prost(bool, tag = "2")]
    pub has_client_index: bool,
    #[prost(int32, tag = "3")]
    pub client_index: i32,
}
/// Minimal, verified WifiClient fields. Other generation-specific fields are skipped.
#[derive(Clone, PartialEq, Message, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientEntry {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub mac_address: String,
    #[prost(string, tag = "3")]
    pub ip_address: String,
    #[prost(float, optional, tag = "4")]
    pub signal_strength: Option<f32>,
    #[prost(uint32, optional, tag = "43")]
    pub client_id: Option<u32>,
    #[prost(bool, optional, tag = "58")]
    pub active: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
struct Response {
    #[prost(message, optional, tag = "2")]
    status: Option<AppStatus>,
    #[prost(bytes = "vec", optional, tag = "1004")]
    get_device_info: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2004")]
    dish_status: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2006")]
    history: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2008")]
    obstructions: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3002")]
    clients: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3004")]
    router_status: Option<Vec<u8>>,
}

pub(crate) fn decode_response(query: Query, bytes: &[u8]) -> Result<serde_json::Value> {
    let response = Response::decode(bytes)
        .map_err(|e| anyhow::anyhow!("invalid Device.Response protobuf: {e}"))?;
    if let Some(status) = response.status
        && status.code != 0
    {
        bail!(
            "device returned application status {}: {}",
            status.code,
            status.message
        );
    }
    let body = match query {
        Query::Status => response.dish_status,
        Query::History => response.history,
        Query::Obstructions => response.obstructions,
        Query::DeviceInfo => response.get_device_info,
        Query::RouterStatus => response.router_status,
        Query::Clients => response.clients,
    }
    .ok_or_else(|| {
        anyhow::anyhow!(
            "response did not contain expected read-only variant tag {}",
            query.response_tag()
        )
    })?;
    macro_rules! json {
        ($t:ty) => {{
            let message = <$t>::decode(body.as_slice())
                .map_err(|e| anyhow::anyhow!("invalid response payload: {e}"))?;
            serde_json::to_value(message).map_err(Into::into)
        }};
    }
    match query {
        Query::Status => json!(DishStatus),
        Query::History => json!(History),
        Query::Obstructions => json!(ObstructionMap),
        Query::DeviceInfo => json!(GetDeviceInfo),
        Query::RouterStatus => json!(RouterStatus),
        Query::Clients => json!(Clients),
    }
}
fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_are_only_verified_tags() {
        assert_eq!(Query::Status.encode_request(), vec![0xe2, 0x3e, 0]);
        assert_eq!(Query::History.encode_request(), vec![0xfa, 0x3e, 0]);
        assert_eq!(Query::Obstructions.encode_request(), vec![0xc2, 0x7d, 0]);
        assert_eq!(Query::DeviceInfo.encode_request(), vec![0x82, 0x3f, 0]);
        assert_eq!(Query::Clients.encode_request(), vec![0xd2, 0xbb, 0x01, 0]);
    }
    #[test]
    fn malformed_response_fails() {
        assert!(decode_response(Query::Status, &[0x12]).is_err());
    }
}
