//! Explicit, opt-in controls for a packaged systemd user service.
use axum::{
    Json,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::process::Command;

pub fn available() -> bool {
    cfg!(target_os = "linux")
        && crate::distribution::env("HOST_CONTROLS").as_deref()
            == Some(std::ffi::OsStr::new("systemd-user"))
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Restart,
    Shutdown,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    action: Action,
    confirmed: bool,
}

pub(crate) fn validate(headers: &HeaderMap, confirmed: bool) -> Result<(), &'static str> {
    if !confirmed {
        return Err("Confirm the action first");
    }
    let origin = headers.get("origin").and_then(|h| h.to_str().ok());
    let host = headers.get("host").and_then(|h| h.to_str().ok());
    if !matches!((origin, host), (Some(origin), Some(host)) if origin == format!("http://{host}") || origin == format!("https://{host}"))
    {
        return Err("Host controls require a request from this OLC page");
    }
    Ok(())
}

/// Fail closed for a loaded OLC USB track unless a fresh status confirms it stopped.
pub(crate) fn playback_guard(decks: &[Value]) -> Result<(), String> {
    let blocked: Vec<_> = decks
        .iter()
        .filter(|d| {
            d["sourcePlayer"].as_u64() == Some(u64::from(crate::local_serving::NUMBER))
                && d["sourceSlot"] == "usb"
                && d["trackId"].as_u64().is_some_and(|id| id > 0)
        })
        .filter(|d| {
            d["connection"] != "connected"
                || d["statusAgeMs"].as_f64().is_none_or(|age| age > 1000.)
                || d["playing"] != false
                || !matches!(d["playState"].as_str(), Some("paused" | "cued" | "ended"))
        })
        .map(|d| format!("CDJ{}", d["number"]))
        .collect();
    if blocked.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Stop playback on {} before restarting or shutting down OLC. Players using OLC USB tracks may enter an emergency loop and require the track to be reloaded. Wait for a fresh stopped-player status.",
            blocked.join(", ")
        ))
    }
}

pub async fn control(
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if !available() {
        return Err((
            StatusCode::FORBIDDEN,
            "Host controls are unavailable on this installation".into(),
        ));
    }
    validate(&headers, request.confirmed).map_err(|e| (StatusCode::FORBIDDEN, e.into()))?;
    let output = tokio::task::spawn_blocking(move || {
        if matches!(request.action, Action::Shutdown) {
            let allowed = Command::new("/usr/bin/sudo")
                .args(["-n", "-l", "/usr/bin/systemctl", "poweroff"])
                .output()?;
            if !allowed.status.success() {
                return Err(std::io::Error::other(
                    "Shutdown permission is not configured",
                ));
            }
        }
        let mut command = Command::new("/usr/bin/systemd-run");
        command.args([
            "--user",
            "--collect",
            "--on-active=2s",
            "--timer-property=AccuracySec=1ms",
            "--timer-property=RemainAfterElapse=no",
        ]);
        match request.action {
            Action::Restart => {
                command.args([
                    "--unit=olc-request-restart",
                    "/usr/bin/systemctl",
                    "--user",
                    "restart",
                    "olc-host.service",
                ]);
            }
            Action::Shutdown => {
                command.args([
                    "--unit=olc-request-shutdown",
                    "/usr/bin/sudo",
                    "-n",
                    "/usr/bin/systemctl",
                    "poweroff",
                ]);
            }
        }
        command.output()
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
    Ok(Json(json!({"scheduled":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unconfirmed_and_cross_origin_actions() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "olc.local:8787".parse().unwrap());
        headers.insert("origin", "http://olc.local:8787".parse().unwrap());
        assert!(validate(&headers, true).is_ok());
        assert!(validate(&headers, false).is_err());
        headers.insert("origin", "http://other.local".parse().unwrap());
        assert!(validate(&headers, true).is_err());
        headers.remove("origin");
        assert!(validate(&headers, true).is_err());
    }
    #[test]
    fn permits_only_fixed_actions() {
        assert!(
            serde_json::from_value::<Request>(json!({"action":"restart","confirmed":true})).is_ok()
        );
        assert!(
            serde_json::from_value::<Request>(json!({"action":"shell","confirmed":true})).is_err()
        );
        assert!(
            serde_json::from_value::<Request>(
                json!({"action":"shutdown","confirmed":true,"command":"anything"})
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod playback_tests {
    use super::*;
    #[test]
    fn playing_and_stale_olc_usb_tracks_block_but_other_sources_do_not() {
        let mut d = json!({"number":1,"sourcePlayer":4,"sourceSlot":"usb","trackId":1,"connection":"connected","statusAgeMs":20.,"playing":true,"playState":"playing"});
        assert!(playback_guard(&[d.clone()]).is_err());
        d["playing"] = json!(false);
        d["playState"] = json!("paused");
        assert!(playback_guard(&[d.clone()]).is_ok());
        d["statusAgeMs"] = json!(2000.);
        assert!(playback_guard(&[d.clone()]).is_err());
        d["sourcePlayer"] = json!(1);
        d["playing"] = json!(true);
        assert!(playback_guard(&[d]).is_ok());
        assert!(playback_guard(&[]).is_ok());
    }
}
