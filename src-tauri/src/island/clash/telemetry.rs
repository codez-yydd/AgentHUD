use super::{controller, IpLocation, Traffic};
use serde::Deserialize;
use std::io::Read;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

struct Sample<T> {
    value: Option<T>,
    key: u64,
    requested_at: Option<Instant>,
    pending: bool,
}

impl<T> Default for Sample<T> {
    fn default() -> Self {
        Self {
            value: None,
            key: 0,
            requested_at: None,
            pending: false,
        }
    }
}

static SPEED: LazyLock<Mutex<Sample<Traffic>>> = LazyLock::new(Mutex::default);
static EGRESS: LazyLock<Mutex<Sample<IpLocation>>> = LazyLock::new(Mutex::default);

fn sample<T: Clone + Send + 'static>(
    cache: &'static Mutex<Sample<T>>,
    key: u64,
    interval: Duration,
    read: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    let mut state = cache.lock().ok()?;
    if state.key != key {
        *state = Sample {
            key,
            ..Default::default()
        };
    }
    let now = Instant::now();
    if !state.pending
        && state
            .requested_at
            .is_none_or(|last| now.duration_since(last) >= interval)
    {
        state.pending = true;
        state.requested_at = Some(now);
        tauri::async_runtime::spawn_blocking(move || {
            let value = read();
            if let Ok(mut state) = cache.lock() {
                if state.key == key && state.requested_at == Some(now) {
                    state.value = value;
                    state.pending = false;
                }
            }
        });
    }
    state.value.clone()
}

pub(super) fn clear() {
    if let Ok(mut speed) = SPEED.lock() {
        *speed = Sample::default();
    }
    if let Ok(mut egress) = EGRESS.lock() {
        *egress = Sample::default();
    }
}

pub(super) fn traffic(controller: &controller::Controller) -> Option<Traffic> {
    let key = controller.identity();
    let endpoint = controller.clone();
    sample(&SPEED, key, Duration::from_millis(900), move || {
        let data = controller::request(&endpoint, "GET", "/traffic", None).ok()?;
        Some(Traffic {
            upload_speed: data["up"].as_u64()?,
            download_speed: data["down"].as_u64()?,
        })
    })
}

pub(super) fn egress(port: u16, route_key: u64) -> Option<IpLocation> {
    sample(&EGRESS, route_key, Duration::from_secs(120), move || {
        fetch_egress(port)
    })
}

#[derive(Deserialize)]
struct GeoResponse {
    ip: IpAddr,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
}

fn parse_egress(body: impl Read) -> Option<IpLocation> {
    let response: GeoResponse = serde_json::from_reader(body.take(16 * 1024)).ok()?;
    if response.ip.is_loopback() || response.ip.is_unspecified() {
        return None;
    }
    let mut geography = Vec::new();
    let labels = [response.country, response.region, response.city];
    for label in labels.iter().flatten() {
        let label = label.trim();
        if !label.is_empty() && !geography.contains(&label) {
            geography.push(label);
        }
    }
    Some(IpLocation {
        ip: response.ip.to_string(),
        region: geography.join(" · "),
    })
}

fn fetch_egress(port: u16) -> Option<IpLocation> {
    let proxy = ureq::Proxy::new(format!("http://127.0.0.1:{port}")).ok()?;
    let response = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout(Duration::from_secs(5))
        .redirects(0)
        .build()
        .get("https://api.ip.sb/geoip")
        .call()
        .ok()?;
    parse_egress(response.into_reader())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_public_location_without_repeating_regions() {
        let input = br#"{"ip":"203.0.113.5","country":"Example","region":"Metro","city":"Metro"}"#;
        let location = parse_egress(&input[..]).unwrap();
        assert_eq!(location.ip, "203.0.113.5");
        assert_eq!(location.region, "Example · Metro");
        assert!(parse_egress(&br#"{"ip":"127.0.0.1"}"#[..]).is_none());
    }
}
