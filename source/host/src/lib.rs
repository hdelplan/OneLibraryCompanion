//! HTTP serves the companion UI, never a CDJ protocol service.
pub const EXPERIMENTS: bool = cfg!(any(target_os = "ios", feature = "experiments"));
mod artwork;
mod audio_header;
mod bar_position;
pub mod beat_position;
mod bridge_probe;
mod cdj_usb_load;
mod cue_window;
mod cues;
mod direct_ip;
mod direct_status;
mod distribution;
mod handoff_capture;
mod jog_trace;
mod library;
mod library_mood;
mod library_preview;
mod live;
mod loading;
mod local_media;
mod local_serving;
pub mod local_usb_probe;
mod loop_region;
mod musical_key;
mod offline;
mod onelibrary;
mod phrases;
pub mod set_history;
mod set_history_import;
mod subnet_search;
mod sync_tap;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use offline::OfflineSource;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
pub mod embedded;
use tower_http::services::ServeDir;
type ApiResult = Result<Json<Value>, (StatusCode, String)>;
struct App {
    offline: Arc<OfflineSource>,
    live: live::Shared,
    library: library::Shared,
    history: set_history::Shared,
}
async fn load_track(State(app): State<Arc<App>>, Json(body): Json<Value>) -> Json<Value> {
    Json(live::load(&app.live, body).await)
}
async fn library_artwork(
    State(app): State<Arc<App>>,
    Path((id, track)): Path<(String, u32)>,
    Query(query): Query<library::Query>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match artwork::get(&app.library, &id, track, &query).await {
        Some((mime, bytes)) => (
            [
                ("content-type", mime),
                ("cache-control", "private, max-age=3600"),
                ("x-content-type-options", "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn manual_library(State(app): State<Arc<App>>, Json(body): Json<Value>) -> ApiResult {
    if app.live.lock().unwrap().enabled {
        return Err((
            StatusCode::CONFLICT,
            "Disconnect live before changing the library connection".into(),
        ));
    }
    let ip = direct_ip::validate_ip(body["ip"].as_str().unwrap_or(""))
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    library::connect_direct(&app.library, ip).map_err(|e| (StatusCode::CONFLICT, e))?;
    Ok(Json(library::sources(&app.library)))
}
async fn usb_probe_command(State(app): State<Arc<App>>, Json(body): Json<Value>) -> ApiResult {
    if body["stop"] == true {
        local_usb_probe::stop();
    } else {
        if app.live.lock().unwrap().enabled {
            return Err((
                StatusCode::CONFLICT,
                "Disconnect live mode before starting the USB playback test".into(),
            ));
        }
        let ip = direct_ip::validate_ip(body["ip"].as_str().unwrap_or(""))
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        let number = body["number"]
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .unwrap_or(0);
        live::direct::release_idle_transport(&app.live)
            .await
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        local_usb_probe::start(ip, number, body["isolated"] == true)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    }
    Ok(Json(local_usb_probe::report()))
}
async fn cue_window_command(State(app): State<Arc<App>>, Json(body): Json<Value>) -> ApiResult {
    let action = body["action"].as_str().unwrap_or("");
    let _ = app; // Preferences also work before connecting players.
    cue_window::control(
        action,
        body["durationSeconds"].as_u64(),
        body["enabled"].as_bool(),
    )
    .map_err(|e| (StatusCode::CONFLICT, e))?;
    Ok(Json(cue_window::state()))
}
async fn jog_command(State(app): State<Arc<App>>, Json(body): Json<Value>) -> ApiResult {
    if cue_window::occupied() {
        return Err((
            StatusCode::CONFLICT,
            "Disable cue windows and wait for restoration before using other captures".into(),
        ));
    }

    if body["start"] == true && jog_trace::state()["active"] == true {
        return Err((
            StatusCode::CONFLICT,
            "Stop the current capture before starting another capture".into(),
        ));
    }
    if body["start"] == true && body["bridge"] == true {
        live::direct::start_bridge(&app.live)
            .await
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        Ok(Json(jog_trace::state()))
    } else if body["start"] == true && body["handoff"] == true {
        live::direct::start_handoff(&app.live)
            .await
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        Ok(Json(jog_trace::state()))
    } else if body["start"] == true && body["syncTap"] == true {
        live::direct::start_sync_tap(&app.live)
            .await
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        Ok(Json(jog_trace::state()))
    } else {
        Ok(Json(jog_trace::command(body["start"] == true)))
    }
}
async fn direct_live_connect(State(app): State<Arc<App>>, Json(body): Json<Value>) -> ApiResult {
    if local_usb_probe::active() {
        return Err((
            StatusCode::CONFLICT,
            "Stop the USB playback test before connecting live".into(),
        ));
    }
    if body["stop"] == true {
        live::direct::stop(&app.live, &app.library).map_err(|e| (StatusCode::CONFLICT, e))?;
    } else {
        let number = body["number"]
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .unwrap_or(1);
        let ip = if body["disconnect"] == true {
            None
        } else {
            Some(
                direct_ip::validate_ip(body["ip"].as_str().unwrap_or(""))
                    .map_err(|e| (StatusCode::BAD_REQUEST, e))?,
            )
        };
        live::direct::configure(&app.live, &app.library, number, ip)
            .await
            .map_err(|e| (StatusCode::CONFLICT, e))?;
    }
    Ok(Json(live::snapshot(&app.live)))
}
async fn library_mood(
    State(app): State<Arc<App>>,
    Path((id, track)): Path<(String, u32)>,
    Query(query): Query<library::Query>,
) -> ApiResult {
    library_mood::get(&app.library, &id, track, &query)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::CONFLICT, e))
}
async fn library_preview(
    State(app): State<Arc<App>>,
    Path((id, track)): Path<(String, u32)>,
    Query(query): Query<library::Query>,
) -> ApiResult {
    library_preview::get(&app.library, &id, track, &query)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::CONFLICT, e))
}
async fn library_sources(State(app): State<Arc<App>>) -> Json<Value> {
    Json(library::sources(&app.library))
}
async fn library_refresh(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult {
    library::refresh(&app.library, &id).map_err(|e| (StatusCode::CONFLICT, e))?;
    Ok(Json(library::sources(&app.library)))
}
async fn library_catalog(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(query): Query<library::Query>,
) -> ApiResult {
    let (generation, catalog) =
        library::catalog(&app.library, &id, &query).map_err(|e| (StatusCode::CONFLICT, e))?;
    tokio::task::spawn_blocking(move || Json(catalog.description(generation)))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
async fn saved_set_tracks(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(query): Query<library::Query>,
) -> ApiResult {
    let tracks = app
        .history
        .lock()
        .unwrap()
        .ordered_tracks(&id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    tokio::task::spawn_blocking(move || {
        library::saved_tracks(&query, &tracks)
            .map(Json)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

async fn library_tracks(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(query): Query<library::Query>,
) -> ApiResult {
    let (generation, catalog) =
        library::catalog(&app.library, &id, &query).map_err(|e| (StatusCode::CONFLICT, e))?;
    let set_tracks = query
        .get("set")
        .filter(|id| !id.is_empty())
        .map(|id| app.history.lock().unwrap().ordered_tracks(id))
        .transpose()
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    tokio::task::spawn_blocking(move || {
        let result = if let Some(tracks) = &set_tracks {
            catalog.tracks_for_set(generation, &query, Some(tracks))
        } else {
            catalog.tracks(generation, &query)
        };
        result.map(Json).map_err(|e| (StatusCode::BAD_REQUEST, e))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}
async fn health(State(app): State<Arc<App>>) -> Json<Value> {
    let state = app.live.lock().unwrap();
    Json(
        json!({"product":"OneLibraryCompanion","version":env!("CARGO_PKG_VERSION"),"experiments":EXPERIMENTS,"instance":distribution::instance_id(),"mode":if state.enabled {"live-monitor"} else {"offline-read-only"},"live":state.enabled,"error":state.error}),
    )
}
async fn live_status(State(app): State<Arc<App>>) -> Json<Value> {
    Json(live::snapshot(&app.live))
}
async fn live_events(
    State(app): State<Arc<App>>,
) -> axum::response::Sse<
    impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    let receiver = live::subscribe(&app.live);
    let initial = live::snapshot(&app.live);
    let stream = futures_util::stream::unfold(
        (Some(initial), receiver),
        |(initial, mut receiver)| async move {
            if distribution::SHUTTING_DOWN.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            let mut value = if let Some(value) = initial {
                value
            } else {
                tokio::select! { result = receiver.changed() => { result.ok()?; }, _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {} }
                receiver.borrow_and_update().clone()
            };
            value["jogCapture"] = jog_trace::state();
            Some((
                Ok(axum::response::sse::Event::default().data(value.to_string())),
                (None, receiver),
            ))
        },
    );
    axum::response::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}
async fn live_analysis(State(app): State<Arc<App>>, Path(number): Path<u8>) -> ApiResult {
    live::analysis(&app.live, number)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "Live analysis is not ready".into()))
}
async fn live_artwork(
    State(app): State<Arc<App>>,
    Path(number): Path<u8>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match query
        .get("key")
        .and_then(|key| live::artwork(&app.live, number, key))
    {
        Some((mime, bytes)) => (
            [
                ("content-type", mime),
                ("cache-control", "private, max-age=3600"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn analysis(State(app): State<Arc<App>>, bytes: Bytes) -> ApiResult {
    let source = app.offline.clone();
    tokio::task::spawn_blocking(move || {
        source
            .decode(&bytes)
            .map(Json)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}
async fn capture(State(app): State<Arc<App>>) -> ApiResult {
    let source = app.offline.clone();
    tokio::task::spawn_blocking(move || {
        let path=source.capture.as_ref().ok_or((StatusCode::NOT_FOUND,
            "No saved capture configured. Open an analysis file, or set OLC_CAPTURE before starting the host.".into()))?;
        let bytes=std::fs::read(path).map_err(|e|(StatusCode::NOT_FOUND,e.to_string()))?;
        source.decode(&bytes).map(Json).map_err(|e|(StatusCode::BAD_REQUEST,e))
    }).await.map_err(|e|(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?
}

fn app_router(
    ui_root: PathBuf,
    source: OfflineSource,
    interface: Option<String>,
    library_path: Option<PathBuf>,
    data_dir: PathBuf,
) -> Router {
    let library = library::with_local_path(interface.clone(), library_path);
    local_media::start(library.clone());
    let live = live::start(interface, library.clone());
    let history = set_history::start(data_dir, live.clone());
    let router = Router::new();
    let router = if EXPERIMENTS {
        router
            .route(
                "/diagnostics/local-usb",
                get(|| async { axum::response::Html(include_str!("local_usb_probe.html")) }),
            )
            .route(
                "/api/diagnostics/local-usb",
                get(|| async { Json(local_usb_probe::report()) }).post(usb_probe_command),
            )
            .route(
                "/api/diagnostics/cue-window",
                get(|| async { Json(cue_window::state()) }).post(cue_window_command),
            )
            .route(
                "/api/diagnostics/cue-window/reports",
                get(|| async {
                    if cue_window::occupied() {
                        Err((
                            StatusCode::CONFLICT,
                            "Wait for the current cue window to finish restoring before exporting"
                                .to_owned(),
                        ))
                    } else {
                        Ok(Json(cue_window::reports()))
                    }
                }),
            )
            .route(
                "/api/diagnostics/jog",
                get(|| async { Json(jog_trace::report()) }).post(jog_command),
            )
            .route(
                "/api/diagnostics/jog/state",
                get(|| async { Json(jog_trace::state()) }),
            )
    } else {
        router
    };
    router
        .route("/api/health", get(health))
        .route(
            "/api/network/search",
            get(subnet_search::list).post(subnet_search::search),
        )
        .route("/api/sets", get(set_list).post(set_command))
        .route("/api/sets/import", post(set_import))
        .route("/api/sets/{id}/tracks", get(saved_set_tracks))
        .route("/api/sets/artwork/{name}", get(set_artwork))
        .route("/api/library/sources", get(library_sources))
        .route(
            "/api/library/local",
            get(|| async { Json(local_media::status()) }),
        )
        .route("/api/library/manual", post(manual_library))
        .route("/api/live/direct", post(direct_live_connect))
        .route("/api/library/{id}/preview/{track}", get(library_preview))
        .route("/api/library/{id}/mood/{track}", get(library_mood))
        .route("/api/library/{id}/refresh", post(library_refresh))
        .route("/api/library/{id}", get(library_catalog))
        .route("/api/library/{id}/tracks", get(library_tracks))
        .route("/api/library/{id}/artwork/{track}", get(library_artwork))
        .route("/api/analysis", post(analysis))
        .route("/api/capture", get(capture))
        .route("/api/live", get(live_status))
        .route("/api/live/load", post(load_track))
        .route("/api/live/events", get(live_events))
        .route("/api/live/analysis/{number}", get(live_analysis))
        .route("/api/live/artwork/{number}", get(live_artwork))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .fallback_service(ServeDir::new(ui_root))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                // The entry page points at versioned assets; never reuse an old
                // entry page after either a native-app or web-host update.
                if response
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("text/html"))
                {
                    response.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-store"),
                    );
                }
                response
            },
        ))
        .with_state(Arc::new(App {
            offline: Arc::new(source),
            live,
            library,
            history,
        }))
}

pub async fn run_desktop() -> Result<(), Box<dyn std::error::Error>> {
    distribution::run().await
}

async fn set_list(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.history.lock().unwrap().snapshot())
}
async fn set_command(State(app): State<Arc<App>>, Json(body): Json<Value>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let mut store = app.history.lock().unwrap();
        store
            .command(&body)
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        Ok(Json(store.snapshot()))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}
async fn set_import(State(app): State<Arc<App>>, Json(body): Json<Value>) -> ApiResult {
    let source = body["source"]
        .as_str()
        .ok_or((StatusCode::BAD_REQUEST, "Choose a library source".into()))?
        .to_owned();
    let generation = body["generation"]
        .as_u64()
        .ok_or((StatusCode::BAD_REQUEST, "Missing source generation".into()))?;
    let query = library::Query::from([("generation".into(), generation.to_string())]);
    let (_, catalog) =
        library::catalog(&app.library, &source, &query).map_err(|e| (StatusCode::CONFLICT, e))?;
    let state = app.history.clone();
    let (result, pending) = tokio::task::spawn_blocking(move || {
        let mut store = state.lock().unwrap();
        let pending = store.import(&catalog.library)?;
        Ok::<_, String>((store.snapshot(), pending))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::CONFLICT, e))?;
    tokio::spawn(async move {
        for (set, event, track) in pending {
            if let Some((mime, bytes)) = artwork::get(&app.library, &source, track, &query).await {
                let state = app.history.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let mut store = state.lock().unwrap();
                    let _ = store.save_artwork(&set, &event, mime, &bytes);
                    let _ = store.flush();
                })
                .await;
            }
        }
    });
    Ok(Json(result))
}
async fn set_artwork(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = app.history.lock().unwrap().artwork_path(&name);
    if let Some(path) = path
        && let Ok(Ok(bytes)) = tokio::task::spawn_blocking(move || std::fs::read(path)).await
    {
        let mime = if name.ends_with(".png") {
            "image/png"
        } else {
            "image/jpeg"
        };
        return (
            [
                ("content-type", mime),
                ("cache-control", "private, max-age=86400"),
                ("x-content-type-options", "nosniff"),
            ],
            bytes,
        )
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}
