//! Bounded load evidence; no audio payloads and no changes to player commands.
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};
const KEEP: usize = 20;
const MAX_BYTES: usize = 2 * 1024 * 1024;
static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[derive(Clone)]
pub struct Recorder(Arc<Inner>);
struct Inner {
    writer: mpsc::SyncSender<Value>,
    states: Mutex<VecDeque<Value>>,
    last_request: Mutex<Value>,
}
fn now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
impl Recorder {
    pub fn start(root: &Path, live: crate::live::Shared) -> Self {
        let directory = root.join("load-incidents");
        let (writer, rx) = mpsc::sync_channel::<Value>(4);
        std::thread::spawn(move || {
            for report in rx {
                if let Err(error) = persist(&directory, &report) {
                    eprintln!("Load incident save failed: {error}");
                }
            }
        });
        let recorder = Self(Arc::new(Inner {
            writer,
            states: Mutex::new(VecDeque::new()),
            last_request: Mutex::new(Value::Null),
        }));
        // Do not keep an abandoned app/runtime alive through diagnostic tasks.
        let weak = Arc::downgrade(&recorder.0);
        tokio::spawn(async move {
            let mut failures = prolink::serve::diagnostics::failure_count();
            let mut last_capture = std::time::Instant::now() - std::time::Duration::from_secs(30);
            loop {
                let Some(inner) = weak.upgrade() else { break };
                {
                    let mut states = inner.states.lock().unwrap();
                    states.push_back(json!({"unixMs":now(),"live":crate::live::snapshot(&live)}));
                    if states.len() > 120 {
                        states.pop_front();
                    }
                }
                let current = prolink::serve::diagnostics::failure_count();
                if current != failures
                    && last_capture.elapsed() >= std::time::Duration::from_secs(30)
                {
                    failures = current;
                    last_capture = std::time::Instant::now();
                    let body = inner.last_request.lock().unwrap().clone();
                    Self(inner.clone()).finished(
                        &body,
                        &json!({"outcome":"serving-error","failureCount":current}),
                    );
                }
                drop(inner);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
        recorder
    }
    pub fn begin(&self, body: &Value) -> Load {
        let enabled = !crate::cdj_config::is_command(body);
        let request = json!({"target":body["target"],"source":body["source"],"trackId":body["trackId"],"generation":body["generation"]});
        if enabled {
            *self.0.last_request.lock().unwrap() = request.clone();
            prolink::serve::diagnostics::record(format!("load_request {request}"));
        }
        Load {
            recorder: self.clone(),
            request,
            completed: !enabled,
        }
    }
    fn finished(&self, body: &Value, result: &Value) {
        prolink::serve::diagnostics::record(format!(
            "load_result target={} track={} result={result}",
            body["target"], body["trackId"]
        ));
        if matches!(result["outcome"].as_str(), Some("confirmed" | "reported")) {
            return;
        }
        let states = self.0.states.lock().unwrap().clone();
        let report = report(
            body,
            result,
            &states,
            prolink::serve::diagnostics::snapshot(),
        );
        if let Err(error) = self.0.writer.try_send(report) {
            eprintln!("Load incident queue unavailable: {error}");
        }
    }
}
// Preserve evidence if the HTTP request disappears while a load is in flight.
// This is an unknown outcome, never a reason to resend a player command.
pub struct Load {
    recorder: Recorder,
    request: Value,
    completed: bool,
}
impl Load {
    pub fn finish(mut self, result: &Value) {
        if !self.completed {
            self.recorder.finished(&self.request, result);
        }
        self.completed = true;
    }
}
impl Drop for Load {
    fn drop(&mut self) {
        if !self.completed {
            self.recorder.finished(&self.request, &json!({"outcome":"unknown","message":"HTTP load request ended before a result was returned"}));
        }
    }
}
fn report(
    body: &Value,
    result: &Value,
    states: &VecDeque<Value>,
    serving: Vec<(u64, String)>,
) -> Value {
    json!({"version":1,"unixMs":now(),"applicationVersion":env!("CARGO_PKG_VERSION"),
        "request":{"target":body["target"],"source":body["source"],"trackId":body["trackId"],"generation":body["generation"]},
        "result":result,"playerHistory":states,"serving":serving})
}
fn persist(directory: &Path, report: &Value) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let name = format!(
        "load-{}-{}-{:020}.json",
        report["unixMs"],
        report["request"]["target"],
        SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let path = directory.join(name);
    let temporary = path.with_extension("tmp");
    let bytes = serde_json::to_vec(report)?;
    if bytes.len() > MAX_BYTES {
        return Err(std::io::Error::other("Load incident exceeds 2 MiB limit"));
    }
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, path)?;
    let mut files: Vec<_> = std::fs::read_dir(directory)?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("load-") && n.ends_with(".json"))
        })
        .collect();
    files.sort_by_key(|e| e.file_name());
    let excess = files.len().saturating_sub(KEEP);
    for file in files.into_iter().take(excess) {
        std::fs::remove_file(file.path())?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn test_recorder() -> (Recorder, mpsc::Receiver<Value>) {
        let (writer, rx) = mpsc::sync_channel(4);
        (
            Recorder(Arc::new(Inner {
                writer,
                states: Mutex::new(VecDeque::new()),
                last_request: Mutex::new(Value::Null),
            })),
            rx,
        )
    }
    #[test]
    fn canceled_requests_save_unknown_and_success_does_not_save() {
        let (recorder, rx) = test_recorder();
        let body = json!({"target":1,"source":"local-usb:test","trackId":42,"generation":3});
        recorder
            .begin(&body)
            .finish(&json!({"outcome":"confirmed"}));
        assert!(rx.try_recv().is_err());
        drop(recorder.begin(&body));
        assert_eq!(rx.try_recv().unwrap()["result"]["outcome"], "unknown");
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn recorders_do_not_share_history_or_keep_the_writer_alive() {
        let (a, rx) = test_recorder();
        let (b, _) = test_recorder();
        a.0.states.lock().unwrap().push_back(json!({"unixMs":1}));
        assert!(b.0.states.lock().unwrap().is_empty());
        let weak = Arc::downgrade(&a.0);
        drop(a);
        assert!(weak.upgrade().is_none());
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
    #[test]
    fn failure_report_preserves_prior_player_state_and_request_identity() {
        let states = VecDeque::from([
            json!({"unixMs":1,"live":{"decks":[{"number":1,"playState":"loading"}]}}),
        ]);
        let body = json!({"target":1,"source":"local-usb:test","trackId":42,"generation":3});
        let report = report(
            &body,
            &json!({"outcome":"unknown","message":"Timed out"}),
            &states,
            vec![(1, "load_request".into())],
        );
        assert_eq!(report["request"]["trackId"], 42);
        assert_eq!(
            report["playerHistory"][0]["live"]["decks"][0]["playState"],
            "loading"
        );
        assert_eq!(report["result"]["outcome"], "unknown");
        assert_eq!(report["serving"][0][1], "load_request");
    }
    #[test]
    fn keeps_only_twenty_incidents_and_preserves_other_files() {
        let dir = std::env::temp_dir().join(format!(
            "olc-load-incidents-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("unrelated.json"), "keep").unwrap();
        for i in 100..125 {
            persist(
                &dir,
                &json!({"unixMs":i,"request":{"target":1},"result":{"outcome":"unknown"}}),
            )
            .unwrap();
        }
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.starts_with("load-100-")));
        assert!(names.iter().any(|n| n.starts_with("load-124-")));
        assert!(dir.join("unrelated.json").exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 21);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
