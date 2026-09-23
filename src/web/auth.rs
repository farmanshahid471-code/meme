//! API authentication.
//!
//! TraderTony's REST API can start/stop trading, change strategies and read the
//! wallet, so it must never be reachable by "anyone who knows the URL". When
//! `API_KEY` is set, every request except the liveness probe has to carry the
//! key — as `X-API-Key`, `Authorization: Bearer …`, or `?api_key=` for the
//! WebSocket, which browsers cannot attach headers to.
//!
//! Leaving `API_KEY` empty keeps the historical behaviour (open API) so
//! existing deployments do not break, but it logs a loud warning at boot.

use axum::{
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use tracing::warn;

use super::AppState;

/// Endpoints that must answer without credentials (health probes, load balancers).
const PUBLIC_PATHS: &[&str] = &["/", "/api/health"];

#[derive(Serialize)]
struct AuthErrorBody {
    error: String,
}

/// Axum middleware: reject unauthenticated requests when `API_KEY` is configured.
pub async fn require_api_key(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // CORS preflights never carry custom headers; the CORS layer owns them.
    if req.method() == Method::OPTIONS {
        return next.run(req).await;
    }

    let Some(expected) = state.config.api_key.as_deref().filter(|k| !k.is_empty()) else {
        // Authentication disabled by configuration.
        return next.run(req).await;
    };

    let path = req.uri().path().to_string();
    if PUBLIC_PATHS.contains(&path.as_str()) {
        return next.run(req).await;
    }

    match extract_key(&req) {
        Some(provided) if constant_time_eq(provided.as_bytes(), expected.as_bytes()) => {
            next.run(req).await
        }
        Some(_) => {
            warn!("🚫 API request to {} rejected: invalid API key", path);
            unauthorized()
        }
        None => {
            warn!("🚫 API request to {} rejected: missing API key", path);
            unauthorized()
        }
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(AuthErrorBody {
            error: "Unauthorized: supply your API key via the X-API-Key header, \
                    an Authorization: Bearer token, or ?api_key= for the WebSocket"
                .to_string(),
        }),
    )
        .into_response()
}

/// Extract the presented key from the three supported carriers.
fn extract_key(req: &Request) -> Option<String> {
    let headers = req.headers();

    if let Some(value) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let value = value.trim();
        let token = value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "))
            .map(str::trim);
        if let Some(token) = token.filter(|t| !t.is_empty()) {
            return Some(token.to_string());
        }
    }

    req.uri()
        .query()
        .and_then(|query| {
            query.split('&').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                (key == "api_key").then(|| percent_decode(value))
            })
        })
        .filter(|value| !value.is_empty())
}

/// Minimal percent-decoding, enough for URL-safe API keys in a query string.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Length-independent byte comparison, so a wrong key cannot be discovered one
/// byte at a time from response timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u8;
    let max = a.len().max(b.len());
    for i in 0..max {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn request(uri: &str, headers: &[(&str, &str)]) -> Request {
        let mut builder = Request::builder().uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn constant_time_eq_matches_only_identical_keys() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn key_is_read_from_x_api_key_header() {
        let req = request("/api/wallet", &[("x-api-key", "abc123")]);
        assert_eq!(extract_key(&req).as_deref(), Some("abc123"));
    }

    #[test]
    fn key_is_read_from_bearer_token() {
        let req = request("/api/wallet", &[("authorization", "Bearer abc123")]);
        assert_eq!(extract_key(&req).as_deref(), Some("abc123"));
    }

    #[test]
    fn key_is_read_from_query_parameter_for_websockets() {
        let req = request("/ws?api_key=abc123&other=1", &[]);
        assert_eq!(extract_key(&req).as_deref(), Some("abc123"));
    }

    #[test]
    fn query_parameter_is_percent_decoded() {
        let req = request("/ws?api_key=a%2Bb%20c", &[]);
        assert_eq!(extract_key(&req).as_deref(), Some("a+b c"));
    }

    #[test]
    fn missing_credentials_yield_none() {
        assert!(extract_key(&request("/api/wallet", &[])).is_none());
        assert!(extract_key(&request("/api/wallet", &[("x-api-key", "  ")])).is_none());
        assert!(extract_key(&request("/api/wallet", &[("authorization", "Basic abc")])).is_none());
    }

    #[test]
    fn health_endpoint_is_public() {
        assert!(PUBLIC_PATHS.contains(&"/api/health"));
    }
}
