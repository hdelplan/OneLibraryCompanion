// SPDX-License-Identifier: GPL-3.0-only
//! Bounded connection diagnostics. Never stores media payloads.
use std::{
    collections::VecDeque,
    sync::{LazyLock, Mutex},
    time::Instant,
};
const LIMIT: usize = 512;
static LOG: LazyLock<Mutex<(Instant, VecDeque<(u64, String)>)>> =
    LazyLock::new(|| Mutex::new((Instant::now(), VecDeque::new())));
/// Append a timestamped event, retaining at most 512 short messages.
pub fn record(message: impl Into<String>) {
    let mut log = LOG.lock().unwrap();
    let elapsed = log.0.elapsed().as_millis() as u64;
    if log.1.len() == LIMIT {
        log.1.pop_front();
    }
    log.1
        .push_back((elapsed, message.into().chars().take(512).collect()));
}
/// Copy the bounded log in monotonic-time order.
pub fn snapshot() -> Vec<(u64, String)> {
    LOG.lock().unwrap().1.iter().cloned().collect()
}
