//! Explicit, read-only unicast feasibility probe. No discovery or observer.
use axum::{Json, http::StatusCode};
use prolink::{
    Slot,
    consume::nfs::{NfsClient, NfsConfig, ReadSize},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};

static ACTIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Deserialize)]
pub struct Request {
    ip: String,
    #[serde(default)]
    large_reads: bool,
}

pub(crate) fn validate_ip(value: &str) -> Result<Ipv4Addr, String> {
    let ip: Ipv4Addr = value
        .trim()
        .parse()
        .map_err(|_| "Enter the CDJ's numeric IPv4 address")?;
    let last = ip.octets()[3];
    // Conservative diagnostic scope: typical private/link-local host addresses.
    if !(ip.is_private() || ip.is_link_local()) || last == 0 || last == 255 {
        return Err(
            "Use a private or 169.254.x.x host address ending in 1–254; no broadcast addresses"
                .into(),
        );
    }
    Ok(ip)
}

pub async fn run(Json(request): Json<Request>) -> Result<Json<Value>, (StatusCode, String)> {
    let ip = validate_ip(&request.ip).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let _guard = ACTIVE.try_lock().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "A diagnostic is already running".into(),
        )
    })?;
    let mut steps = Vec::<String>::new();
    let start = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(35),
        probe(ip, request.large_reads, &mut steps),
    )
    .await;
    let (ok, error) = match result {
        Ok(Ok(())) => (true, None),
        Ok(Err(e)) => (false, Some(e)),
        Err(_) => (
            false,
            Some(
                "35-second limit reached. A timeout alone does not prove an entitlement problem."
                    .into(),
            ),
        ),
    };
    Ok(Json(
        json!({"ok":ok,"ip":ip.to_string(),"elapsedMs":start.elapsed().as_millis(),"steps":steps,"error":error}),
    ))
}

async fn probe(ip: Ipv4Addr, large: bool, steps: &mut Vec<String>) -> Result<(), String> {
    steps.push(format!(
        "Target {ip}; unicast RPC/NFS only; read size {} bytes",
        if large { 8192 } else { 1280 }
    ));
    let route = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    route
        .connect((ip, 111))
        .map_err(|e| format!("Route selection: {e}"))?;
    steps.push(format!(
        "OS-selected source address: {} (check this is your Ethernet address)",
        route.local_addr().map_err(|e| e.to_string())?.ip()
    ));
    drop(route);
    steps.push("Querying player portmapper on UDP 111…".into());
    let config = NfsConfig {
        read_size: if large {
            ReadSize::CDJ
        } else {
            ReadSize::UNFRAGMENTED
        },
        ..NfsConfig::default()
    };
    let mut client = NfsClient::connect_with(ip, None, config)
        .await
        .map_err(|e| format!("Port discovery: {e}"))?;
    let ports = client.ports();
    steps.push(format!(
        "Portmapper replied: mount={}, NFS={}",
        ports.mount, ports.nfs
    ));
    let mount = client.mount_slot(Slot::USB).await.map_err(|e| format!("USB export access: {e}. Insert a rekordbox-exported USB into the CDJ. Some players require an observer announcement; this test does not send one."))?;
    steps.push("Player granted read access to its USB export".into());
    let file = client
        .open(&mount, "/PIONEER/rekordbox/export.pdb")
        .await
        .map_err(|e| format!("Open export.pdb: {e}"))?;
    if file.size() > 64 * 1024 * 1024 {
        return Err("Database exceeds this test's 64 MiB limit".into());
    }
    steps.push(format!("Reading export.pdb ({} bytes)…", file.size()));
    let bytes = client
        .read_file(&file)
        .await
        .map_err(|e| format!("Read export.pdb: {e}"))?;
    let length = bytes.len();
    let library = tokio::task::spawn_blocking(move || prolink_rekordbox::Library::parse(&bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Database parse: {e}"))?;
    steps.push(format!(
        "SUCCESS: read and parsed {length} bytes; {} tracks",
        library.tracks.len()
    ));
    steps.push("This proves direct USB-library access only. Broadcast discovery, live status and beats have not been tested.".into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_private_host_addresses_are_accepted() {
        for ip in ["169.254.1.2", "192.168.1.12", "10.0.0.2", "172.16.3.4"] {
            assert!(validate_ip(ip).is_ok());
        }
        for ip in [
            "localhost",
            "127.0.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "8.8.8.8",
            "192.168.1.255",
            "10.0.0.0",
            "::1",
        ] {
            assert!(validate_ip(ip).is_err(), "{ip}");
        }
    }
}
