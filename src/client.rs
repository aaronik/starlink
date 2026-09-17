//! Read-only gRPC-Web client for Starlink's local diagnostic endpoint.
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Url, header, redirect::Policy};

pub use crate::proto::Query;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const HANDLE_PATH: &str = "SpaceX.API.Device.Device/Handle";

/// A local diagnostic client. It has no API for arbitrary or mutating RPCs.
pub struct Client {
    endpoint: Url,
    http: reqwest::Client,
}

impl Client {
    /// Construct a client for an explicitly selected local API endpoint.
    pub fn new(endpoint: &str, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            bail!("timeout must be greater than zero");
        }
        let mut endpoint = Url::parse(endpoint).context("invalid endpoint URL")?;
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none() {
            bail!("endpoint must be an absolute http or https URL");
        }
        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            bail!("endpoint must not contain credentials, query, or fragment");
        }
        if endpoint.path().contains("..") {
            bail!("endpoint path must not contain '..'");
        }
        if !endpoint.path().ends_with('/') {
            endpoint.set_path(&format!("{}/", endpoint.path()));
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(timeout)
            .redirect(Policy::none())
            .build()
            .context("building HTTP client")?;
        Ok(Self { endpoint, http })
    }

    /// Execute one allowlisted read RPC and return only its inner response JSON.
    pub async fn query(&self, query: Query) -> Result<serde_json::Value> {
        let url = self
            .endpoint
            .join(HANDLE_PATH)
            .context("constructing diagnostic RPC URL")?;
        let response = self
            .http
            .post(url)
            .header(header::CONTENT_TYPE, "application/grpc-web+proto")
            .header(header::ACCEPT, "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .body(grpc_frame(&query.encode_request(), false))
            .send()
            .await
            .context("sending local diagnostic request")?;
        let status = response.status();
        if !status.is_success() {
            bail!("diagnostic endpoint returned HTTP {status}");
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !content_type.split(';').next().is_some_and(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("application/grpc-web+proto")
                || value.trim().eq_ignore_ascii_case("application/grpc-web")
        }) {
            bail!("diagnostic endpoint returned unexpected content type {content_type:?}");
        }
        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .context("reading diagnostic response")?
        {
            let remaining = MAX_RESPONSE_BYTES.saturating_sub(bytes.len());
            if chunk.len() > remaining {
                bail!("diagnostic response exceeds {MAX_RESPONSE_BYTES} byte limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        let payload = parse_grpc_web(&bytes)?;
        crate::proto::decode_response(query, &payload)
    }
}

fn grpc_frame(payload: &[u8], trailer: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.push(if trailer { 0x80 } else { 0 });
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Decode a unary binary gRPC-Web response and insist on an OK final trailer.
fn parse_grpc_web(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut pos = 0;
    let mut message = None;
    let mut trailers = None;
    while pos < bytes.len() {
        if bytes.len() - pos < 5 {
            bail!("truncated gRPC-Web frame header");
        }
        let flags = bytes[pos];
        let length =
            u32::from_be_bytes(bytes[pos + 1..pos + 5].try_into().expect("four bytes")) as usize;
        pos += 5;
        if length > MAX_RESPONSE_BYTES || length > bytes.len() - pos {
            bail!("invalid or truncated gRPC-Web frame length");
        }
        let frame = &bytes[pos..pos + length];
        pos += length;
        match flags {
            0 if trailers.is_none() => {
                if message.replace(frame.to_vec()).is_some() {
                    bail!("multiple messages in unary gRPC-Web response");
                }
            }
            0x80 if trailers.is_none() => trailers = Some(parse_trailers(frame)?),
            0 | 0x80 => bail!("gRPC-Web trailers must be final"),
            _ => bail!("unsupported gRPC-Web frame flags 0x{flags:02x}"),
        }
    }
    let trailers = trailers.ok_or_else(|| anyhow::anyhow!("missing gRPC-Web trailers"))?;
    let code = trailers
        .get("grpc-status")
        .ok_or_else(|| anyhow::anyhow!("missing grpc-status trailer"))?;
    let code: u32 = code.parse().context("invalid grpc-status trailer")?;
    if code != 0 {
        bail!(
            "gRPC request failed with status {code}: {}",
            trailers
                .get("grpc-message")
                .map(String::as_str)
                .unwrap_or("")
        );
    }
    message.ok_or_else(|| anyhow::anyhow!("gRPC-Web response contains no message"))
}

fn parse_trailers(bytes: &[u8]) -> Result<std::collections::BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).context("gRPC-Web trailers are not UTF-8")?;
    let mut result = std::collections::BTreeMap::new();
    for line in text.split("\r\n").filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("malformed gRPC-Web trailer"))?;
        if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            bail!("invalid gRPC-Web trailer name");
        }
        let name = name.to_ascii_lowercase();
        if result.insert(name, value.trim_start().to_owned()).is_some() {
            bail!("duplicate gRPC-Web trailer name");
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grpc_web_accepts_unary_message_and_ok_trailer() {
        let mut bytes = grpc_frame(&[1, 2], false);
        bytes.extend(grpc_frame(b"grpc-status: 0\r\n", true));
        assert_eq!(parse_grpc_web(&bytes).unwrap(), [1, 2]);
    }
    #[test]
    fn grpc_web_rejects_bad_frame_order_and_duplicate_trailers() {
        let mut bytes = grpc_frame(b"grpc-status: 0\r\n", true);
        bytes.extend(grpc_frame(&[1], false));
        assert!(parse_grpc_web(&bytes).is_err());
        assert!(
            parse_grpc_web(&grpc_frame(b"grpc-status: 0\r\nGrpc-Status: 0\r\n", true)).is_err()
        );
    }
}
