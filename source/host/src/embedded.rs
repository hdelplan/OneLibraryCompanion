//! In-process offline host for the iPad shell. Never reads desktop environment
//! configuration or starts a Pro DJ Link session.
use std::{net::TcpListener, path::PathBuf};

pub struct Server {
    runtime: Option<tokio::runtime::Runtime>,
    pub port: u16,
}

impl Server {
    pub fn start(ui_root: PathBuf, port: u16) -> Result<Self, String> {
        if !ui_root.join("index.html").is_file() {
            return Err("The bundled interface is missing. Rebuild the iPad app.".into());
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .map_err(|e| format!("Cannot start the local app service: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        {
            let _entered = runtime.enter();
            let listener =
                tokio::net::TcpListener::from_std(listener).map_err(|e| e.to_string())?;
            #[cfg(target_os = "ios")]
            let data_dir = PathBuf::from(
                std::env::var_os("HOME").ok_or("Missing application home directory")?,
            )
            .join("Library/Application Support/PioneerCompanion");
            #[cfg(not(target_os = "ios"))]
            let data_dir = ui_root.join(".local/app-data");
            let app = super::app_router(
                ui_root,
                super::OfflineSource::default(),
                None,
                None,
                data_dir,
            );
            let app = if super::EXPERIMENTS {
                app.route(
                    "/diagnostics/direct-ip",
                    axum::routing::get(|| async {
                        axum::response::Html(include_str!("direct_ip.html"))
                    }),
                )
                .route(
                    "/api/diagnostics/direct-status",
                    axum::routing::post(super::direct_status::run),
                )
                .route(
                    "/api/diagnostics/direct-ip",
                    axum::routing::post(super::direct_ip::run),
                )
            } else {
                app
            };
            runtime.spawn(async move {
                if let Err(error) = axum::serve(listener, app).await {
                    eprintln!("Embedded OLC service stopped: {error}");
                }
            });
        }
        Ok(Self {
            runtime: Some(runtime),
            port,
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            // Abort network tasks before returning to the native lifecycle.
            runtime.shutdown_timeout(std::time::Duration::from_millis(250));
            super::local_usb_probe::reset();
            super::local_media::reset();
        }
    }
}

#[cfg(target_os = "ios")]
mod ffi {
    use super::*;
    use std::{
        ffi::{CStr, c_char},
        sync::Mutex,
    };
    static SERVER: Mutex<Option<Server>> = Mutex::new(None);

    /// Starts the offline service on a stable origin so WebView preferences persist.
    /// Returns the port or zero, with a UTF-8 error in the supplied buffer.
    ///
    /// # Safety
    /// `root` must be a valid NUL-terminated UTF-8 string. `error` must point to
    /// `capacity` writable bytes if capacity is nonzero. No pointers are retained.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn pc_start(
        root: *const c_char,
        error: *mut c_char,
        capacity: usize,
    ) -> u16 {
        let result = (|| {
            if root.is_null() {
                return Err("Missing interface directory".to_owned());
            }
            let root = unsafe { CStr::from_ptr(root) }
                .to_str()
                .map_err(|e| e.to_string())?;
            let mut server = SERVER
                .lock()
                .map_err(|_| "App service lock failed".to_owned())?;
            if server.is_none() {
                *server = Some(Server::start(root.into(), 8787)?);
            }
            Ok::<u16, String>(server.as_ref().unwrap().port)
        })();
        match result {
            Ok(port) => port,
            Err(message) => {
                if capacity > 0 && !error.is_null() {
                    let bytes = message.as_bytes();
                    let mut count = bytes.len().min(capacity - 1);
                    while !message.is_char_boundary(count) {
                        count -= 1;
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), error.cast::<u8>(), count);
                        *error.add(count) = 0;
                    }
                }
                0
            }
        }
    }

    /// Register only a native user-selected, security-scoped file. Returns zero on success.
    /// # Safety
    /// `path` is a valid NUL-terminated string; `error` has `capacity` writable bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn pc_usb_file(
        path: *const c_char,
        error: *mut c_char,
        capacity: usize,
    ) -> i32 {
        let result = if path.is_null() {
            Err("Missing file".into())
        } else {
            unsafe { CStr::from_ptr(path) }
                .to_str()
                .map_err(|e| e.to_string())
                .and_then(|p| crate::local_usb_probe::select_file(p.into()))
        };
        match result {
            Ok(()) => 0,
            Err(message) => {
                if capacity > 0 && !error.is_null() {
                    let mut count = message.len().min(capacity - 1);
                    while !message.is_char_boundary(count) {
                        count -= 1;
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(message.as_ptr(), error.cast::<u8>(), count);
                        *error.add(count) = 0;
                    }
                }
                1
            }
        }
    }

    /// Register a native-authorized USB root, or forget it when path is null.
    /// # Safety
    /// Strings must be NUL-terminated; error must have capacity writable bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn pc_local_usb(
        id: *const c_char,
        path: *const c_char,
        label: *const c_char,
        error: *mut c_char,
        capacity: usize,
    ) -> i32 {
        let result = (|| {
            if id.is_null() {
                return Err("Missing USB identity".to_owned());
            }
            let id = unsafe { CStr::from_ptr(id) }
                .to_str()
                .map_err(|e| e.to_string())?;
            if path.is_null() {
                crate::local_media::forget(id);
                return Ok(());
            }
            let path = unsafe { CStr::from_ptr(path) }
                .to_str()
                .map_err(|e| e.to_string())?;
            let label = if label.is_null() {
                "USB"
            } else {
                unsafe { CStr::from_ptr(label) }
                    .to_str()
                    .map_err(|e| e.to_string())?
            };
            crate::local_media::register(id, std::path::Path::new(path), label)
        })();
        if let Err(message) = result {
            if !error.is_null() && capacity > 0 {
                let mut count = message.len().min(capacity - 1);
                while !message.is_char_boundary(count) {
                    count -= 1;
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(message.as_ptr(), error.cast::<u8>(), count);
                    *error.add(count) = 0;
                }
            }
            1
        } else {
            0
        }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn pc_stop() {
        if let Ok(mut server) = SERVER.lock() {
            *server = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn offline_server_serves_ui_rejects_loads_and_releases_port() {
        let root = std::env::temp_dir().join(format!("pc-embedded-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), "embedded interface").unwrap();
        let server = Server::start(root.clone(), 0).unwrap();
        let request = |text: &str| {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", server.port)).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            stream.write_all(text.as_bytes()).unwrap();
            let mut result = String::new();
            stream.read_to_string(&mut result).unwrap();
            result
        };
        let interface = request("GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        assert!(interface.contains("embedded interface"));
        assert!(interface.to_lowercase().contains("cache-control: no-store"));
        let health =
            request("GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        assert!(health.contains("offline-read-only"));
        assert!(health.contains("\"live\":false"));
        for path in [
            "/diagnostics/local-usb",
            "/api/diagnostics/jog/state",
            "/api/diagnostics/cue-window",
            "/diagnostics/direct-ip",
        ] {
            let response = request(&format!(
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            ));
            assert_eq!(
                response.contains("200 OK"),
                crate::EXPERIMENTS,
                "{path}: {response}"
            );
            if !crate::EXPERIMENTS {
                assert!(response.contains("404 Not Found"));
            }
        }
        if !crate::EXPERIMENTS {
            assert!(crate::cue_window::control("arm", None, None).is_err());
            assert!(crate::cue_window::tick([None, None], true).is_empty());
            assert!(crate::local_usb_probe::start("127.0.0.1".parse().unwrap(), 3, true).is_err());
        }

        assert!(request("POST /api/live/load HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").contains("Live CDJ connection is not active"));
        assert!(request("POST /api/analysis HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad").contains("400 Bad Request"));
        let port = server.port;
        drop(server);
        let restarted = Server::start(root.clone(), port).unwrap();
        drop(restarted);
        std::fs::remove_dir_all(root).unwrap();
    }
}
