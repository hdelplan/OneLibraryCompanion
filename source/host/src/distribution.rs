//! Desktop packaging configuration and application identity.
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use network_interface::{NetworkInterface, NetworkInterfaceConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

pub static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn env(name: &str) -> Option<OsString> {
    std::env::var_os(format!("OLC_{name}"))
        .or_else(|| std::env::var_os(format!("PIONEER_COMPANION_{name}")))
}
pub fn instance_id() -> String {
    std::env::var("OLC_INSTANCE").unwrap_or_else(|_| format!("host-{}", std::process::id()))
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    interface: Option<String>,
}
struct Desktop {
    config: PathBuf,
    interface: Option<String>,
    bind: SocketAddr,
}
fn interfaces() -> Vec<Value> {
    NetworkInterface::show()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|interface| {
            interface.addr.into_iter().filter_map(move |address| {
                let ip = address.ip();
                if !ip.is_ipv4() || ip.is_loopback() {
                    return None;
                }
                Some(json!({"name":interface.name,"ip":ip.to_string()}))
            })
        })
        .collect()
}
async fn info(State(state): State<Arc<Desktop>>) -> Json<Value> {
    let networks = interfaces();
    let addresses: Vec<_> = networks
        .iter()
        .filter_map(|n| {
            let ip = n["ip"].as_str()?;
            (state.bind.ip().is_unspecified() || state.bind.ip().to_string() == ip)
                .then(|| format!("http://{ip}:{}", state.bind.port()))
        })
        .collect();
    Json(
        json!({"product":"OneLibraryCompanion","version":env!("CARGO_PKG_VERSION"),
        "experiments":crate::EXPERIMENTS,"addresses":addresses,"networks":networks,
        "interface":state.interface,"hostControls":crate::host_controls::available(),"authentication":"none","configPath":state.config}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsbRequest {
    device: String,
    confirmed: bool,
}
async fn usb_info() -> Result<Json<Value>, (StatusCode, String)> {
    if !crate::host_controls::available() {
        return Err((
            StatusCode::FORBIDDEN,
            "USB unmounting is unavailable on this installation".into(),
        ));
    }
    tokio::task::spawn_blocking(|| {
        let output = std::process::Command::new("/usr/bin/python3")
            .args(["/usr/lib/onelibrarycompanion/olc-usb.py", "list"])
            .output()
            .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
        if !output.status.success() {
            return Err((
                StatusCode::CONFLICT,
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        serde_json::from_slice(&output.stdout)
            .map(Json)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}
async fn usb_unmount(
    State(state): State<Arc<Desktop>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<UsbRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    crate::host_controls::validate(&headers, request.confirmed)
        .map_err(|e| (StatusCode::FORBIDDEN, e.into()))?;
    let Json(inventory) = usb_info().await?;
    if !inventory["volumes"].as_array().is_some_and(|volumes| {
        volumes
            .iter()
            .any(|v| v["device"].as_str() == Some(&request.device))
    }) {
        return Err((
            StatusCode::CONFLICT,
            "Choose a currently mounted external USB volume".into(),
        ));
    }
    let job = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();
    let ip = if state.bind.ip().is_unspecified() {
        if state.bind.is_ipv4() {
            "127.0.0.1".to_string()
        } else {
            "[::1]".to_string()
        }
    } else if state.bind.is_ipv6() {
        format!("[{}]", state.bind.ip())
    } else {
        state.bind.ip().to_string()
    };
    let url = format!("http://{ip}:{}/api/live", state.bind.port());
    let next_job = job.clone();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("/usr/bin/systemd-run")
            .args([
                "--user",
                "--collect",
                "--on-active=2s",
                "--timer-property=AccuracySec=1ms",
                "--timer-property=RemainAfterElapse=no",
                "--unit=olc-request-usb",
                "/usr/bin/python3",
                "/usr/lib/onelibrarycompanion/olc-usb.py",
                "unmount",
                &request.device,
                &url,
                &next_job,
            ])
            .output()
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
    if !output.status.success() {
        return Err((
            StatusCode::CONFLICT,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(Json(json!({"job":job})))
}
fn validate(settings: &Settings, networks: &[Value]) -> Result<(), String> {
    if let Some(name) = &settings.interface
        && name != "auto"
        && !networks.iter().any(|n| n["name"].as_str() == Some(name))
    {
        return Err("Choose an available network interface".into());
    }
    Ok(())
}
async fn save(
    State(state): State<Arc<Desktop>>,
    Json(settings): Json<Settings>,
) -> Result<Json<Value>, (StatusCode, String)> {
    validate(&settings, &interfaces()).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let temp = state.config.with_extension("tmp");
    // Serialize requests so concurrent remote clients cannot race the temporary file.
    static WRITER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _lock = WRITER.lock().await;
    let bytes = serde_json::to_vec_pretty(&settings)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(&temp, bytes)
        .and_then(|_| std::fs::rename(&temp, &state.config))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({"restartRequired":true})))
}
fn data_directory(root: &Path, packaged: bool) -> PathBuf {
    if let Some(path) = env("DATA") {
        return path.into();
    }
    if !packaged {
        return crate::set_history::default_directory(root);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_owned());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/OneLibraryCompanion")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("onelibrarycompanion")
    }
}
fn bind_listener(address: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    // Probe ownership with bind/listen, never outbound connections: network
    // filters can accept connect() on unused ports and cause false conflicts.
    // Darwin permits wildcard/specific overlap with SO_REUSEADDR, so check
    // each local address as well. Reuse permits immediate TIME_WAIT restart.
    #[cfg(target_os = "macos")]
    if address.port() != 0 {
        let mut ips = vec![std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)];
        ips.push(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
        ips.extend(
            NetworkInterface::show()
                .unwrap_or_default()
                .into_iter()
                .flat_map(|n| n.addr.into_iter().map(|a| a.ip())),
        );
        ips.push(address.ip());
        ips.sort();
        ips.dedup();
        for ip in ips
            .into_iter()
            .filter(|ip| ip.is_ipv4() == address.is_ipv4())
        {
            let probe = if ip.is_ipv4() {
                tokio::net::TcpSocket::new_v4()?
            } else {
                tokio::net::TcpSocket::new_v6()?
            };
            probe.set_reuseaddr(true)?;
            let result = probe
                .bind(SocketAddr::new(ip, address.port()))
                .and_then(|()| probe.listen(1));
            if let Err(error) = result
                && error.kind() == std::io::ErrorKind::AddrInUse
            {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!(
                        "Cannot reserve local port {} on {ip}: {error}",
                        address.port()
                    ),
                ));
            }
            // Some enumerated virtual/link-local addresses cannot be bound.
            // The requested address is always validated by the real bind below.
        }
    }
    let socket = if address.is_ipv4() {
        tokio::net::TcpSocket::new_v4()?
    } else {
        tokio::net::TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;
    socket.bind(address)?;
    socket.listen(1024)
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = env("ROOT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let packaged = env("UI_ROOT").is_some();
    let ui = env("UI_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("source/ui/dist"));
    if !ui.join("index.html").is_file() {
        return Err("OLC interface is missing. Build or reinstall the application.".into());
    }
    let data = data_directory(&root, packaged);
    std::fs::create_dir_all(&data)?;
    let config = data.join("desktop.json");
    let saved: Settings = match std::fs::read(&config) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
        Err(e) => return Err(e.into()),
    };
    let interface = env("INTERFACE")
        .map(|v| v.to_string_lossy().into_owned())
        .or(saved.interface)
        .filter(|s| !s.is_empty());
    let bind: SocketAddr = env("BIND")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "0.0.0.0:8787".into())
        .parse()?;
    // Bind before starting any CDJ session, so a second process cannot compete for UDP.
    let listener = bind_listener(bind)?;
    let bind = listener.local_addr()?;
    let desktop = Router::new()
        .route("/api/app", get(info).post(save))
        .route("/api/app/usb", get(usb_info).post(usb_unmount))
        .with_state(Arc::new(Desktop {
            config,
            interface: interface.clone(),
            bind,
        }));
    let app = crate::app_router(
        ui,
        crate::offline::OfflineSource::from_environment()?,
        interface,
        env("LIBRARY").map(PathBuf::from),
        data,
    )
    .merge(desktop);
    println!(
        "OneLibraryCompanion {}: http://{bind}",
        env!("CARGO_PKG_VERSION")
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::Relaxed);
        })
        .await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn refuses_overlapping_local_and_lan_listeners() {
        let local = bind_listener("127.0.0.1:0".parse().unwrap()).unwrap();
        let port = local.local_addr().unwrap().port();
        assert!(bind_listener(format!("0.0.0.0:{port}").parse().unwrap()).is_err());
        // A port check must not create a connection to the existing service.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), local.accept())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn restarts_after_an_accepted_connection() {
        let listener = bind_listener("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let client = tokio::net::TcpStream::connect(address).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        drop(server);
        drop(client);
        drop(listener);
        let restarted = bind_listener(address).unwrap();
        assert_eq!(restarted.local_addr().unwrap(), address);
    }
    #[test]
    fn network_selection_accepts_direct_mode_and_rejects_unknown_interfaces() {
        let networks = vec![json!({"name":"eth0","ip":"192.168.1.4"})];
        assert!(validate(&Settings::default(), &networks).is_ok());
        assert!(
            validate(
                &Settings {
                    interface: Some("auto".into())
                },
                &[]
            )
            .is_ok()
        );
        assert!(
            validate(
                &Settings {
                    interface: Some("eth0".into())
                },
                &networks
            )
            .is_ok()
        );
        assert!(
            validate(
                &Settings {
                    interface: Some("missing".into())
                },
                &networks
            )
            .is_err()
        );
    }
}
