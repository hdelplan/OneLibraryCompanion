//! Optional diagnostic capture, enabled only on explicitly configured hosts.
use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{LazyLock, Mutex, Once},
    time::Duration,
};
type Result<T> = std::result::Result<T, (StatusCode, String)>;
static CLEANUP: Once = Once::new();
static SAVED: LazyLock<Mutex<Value>> = LazyLock::new(|| Mutex::new(Value::Null));
pub fn init() {
    let Ok(dir) = directory() else {
        return;
    };
    CLEANUP.call_once(|| {
        std::thread::spawn(move || {
            let mut restored = false;
            loop {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    let mut reports: Vec<_> = entries
                        .flatten()
                        .filter_map(|entry| {
                            let name = entry.file_name();
                            let name = name.to_str()?;
                            if !name.starts_with("bug-") || !name.ends_with(".json") {
                                return None;
                            }
                            let metadata = entry.metadata().ok()?;
                            if !metadata.is_file() {
                                return None;
                            }
                            Some((metadata.modified().ok()?, entry.path()))
                        })
                        .collect();
                    reports.sort_by_key(|(time, _)| *time);
                    {
                        let mut saved = SAVED.lock().unwrap();
                        if !restored
                            && saved.is_null()
                            && let Some((time, path)) = reports.last()
                            && time
                                .elapsed()
                                .is_ok_and(|age| age <= Duration::from_secs(86400))
                            && let Ok(bytes) = std::fs::read(path)
                            && let Ok(value) = serde_json::from_slice(&bytes)
                        {
                            *saved = value;
                        }
                        restored = true;
                    }
                    let excess = reports.len().saturating_sub(20);
                    for (index, (time, path)) in reports.into_iter().enumerate() {
                        if (index < excess
                            || time
                                .elapsed()
                                .is_ok_and(|age| age > Duration::from_secs(86400)))
                            && let Err(error) = std::fs::remove_file(path)
                        {
                            eprintln!("Bug report cleanup: {error}");
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(3600));
            }
        });
    });
}
fn directory() -> Result<PathBuf> {
    std::env::var_os("OLC_BUG_REPORT_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or((
            StatusCode::NOT_FOUND,
            "Bug reports are not enabled on this host".into(),
        ))
}
pub fn enabled() -> bool {
    directory().is_ok()
}
fn save(value: &Value) -> Result<()> {
    let dir = directory()?;
    std::fs::create_dir_all(&dir).map_err(io_error)?;
    let session = value["session"].as_str().unwrap_or("");
    if session.is_empty() || !session.bytes().all(|b| b.is_ascii_digit()) {
        return Err((StatusCode::BAD_REQUEST, "Invalid report session".into()));
    }
    let path = dir.join(format!("bug-{session}.json"));
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_vec(value).map_err(io_error)?).map_err(io_error)?;
    std::fs::rename(temp, path).map_err(io_error)
}
fn io_error(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
pub async fn status() -> Json<Value> {
    let saved = SAVED.lock().unwrap();
    Json(
        json!({"enabled":directory().is_ok(),"capture":crate::jog_trace::state(),
        "savedSession":saved["session"],"saveError":saved["saveError"]}),
    )
}
pub fn start() -> Result<Value> {
    let dir = directory()?;
    std::fs::create_dir_all(&dir).map_err(io_error)?;
    let state = crate::jog_trace::command_for(true, 60);
    crate::jog_trace::experiment(json!({"profile":"bug-report","durationMs":60000}));
    *SAVED.lock().unwrap() = Value::Null;
    let session = state["session"].clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        if crate::jog_trace::state()["session"] != session {
            return;
        }
        let report = json!({"version":1,"session":session,"createdAtUnixSeconds":
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
            "applicationVersion":env!("CARGO_PKG_VERSION"),"host":crate::jog_trace::report(),
            "serving":prolink::serve::diagnostics::snapshot(),"browser":null});
        let mut saved = SAVED.lock().unwrap();
        *saved = report;
        if let Err((_, error)) = save(&saved) {
            saved["saveError"] = json!(error);
        }
    });
    Ok(state)
}
pub async fn browser(Json(body): Json<Value>) -> Result<Json<Value>> {
    directory()?;
    let mut saved = SAVED.lock().unwrap();
    if saved.is_null() || saved["session"] != body["session"] {
        return Err((
            StatusCode::CONFLICT,
            "Host report is not ready for this session".into(),
        ));
    }
    saved["browser"] = body["browser"].clone();
    save(&saved)?;
    saved.as_object_mut().unwrap().remove("saveError");
    Ok(Json(json!({"saved":true,"session":saved["session"]})))
}
pub async fn latest() -> Result<Json<Value>> {
    directory()?;
    let saved = SAVED.lock().unwrap();
    if saved.is_null() {
        return Err((StatusCode::NOT_FOUND, "No report is ready".into()));
    }
    Ok(Json(saved.clone()))
}
