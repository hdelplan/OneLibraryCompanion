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
// Separate from the event ring: sustained playback must not evict load evidence.
static READS: LazyLock<Mutex<VecDeque<ReadProgress>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

#[derive(Default)]
struct ReadProgress {
    peer: String,
    path: String,
    requests: u64,
    bytes: u64,
    errors: u64,
    highest_end: u64,
    last_offset: u32,
    first_reads: Vec<(u32, u32, Option<usize>)>,
}

/// Retain bounded per-player/file read evidence without retaining audio data.
pub fn record_read(peer: &str, path: &str, offset: u32, requested: u32, returned: Option<usize>) {
    let mut reads = READS.lock().unwrap();
    let index = reads.iter().position(|r| r.peer == peer && r.path == path);
    let mut progress = index
        .and_then(|i| reads.remove(i))
        .unwrap_or_else(|| ReadProgress {
            peer: peer.into(),
            path: path.chars().take(256).collect(),
            ..Default::default()
        });
    progress.requests += 1;
    progress.bytes += returned.unwrap_or(0) as u64;
    progress.errors += u64::from(returned.is_none());
    if let Some(count) = returned {
        progress.highest_end = progress.highest_end.max(u64::from(offset) + count as u64);
    }
    progress.last_offset = offset;
    if progress.first_reads.len() < 16 {
        progress.first_reads.push((offset, requested, returned));
    }
    reads.push_back(progress);
    if reads.len() > 64 {
        reads.pop_front();
    }
}
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
    let log = LOG.lock().unwrap();
    let mut events: Vec<_> = log.1.iter().cloned().collect();
    let now = log.0.elapsed().as_millis() as u64;
    drop(log);
    for read in READS.lock().unwrap().iter() {
        events.push((now, format!("nfs_read_summary peer={} path={} requests={} bytes={} errors={} highest_end={} last_offset={} first_reads={:?}",
            read.peer, read.path, read.requests, read.bytes, read.errors, read.highest_end, read.last_offset, read.first_reads)));
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_progress_separates_players_and_keeps_initial_reads_bounded() {
        for offset in 0..20 {
            record_read(
                "test-player-1",
                "/test-progress.wav",
                offset * 2048,
                2048,
                Some(2048),
            );
        }
        record_read("test-player-1", "/test-progress.wav", 0, 2048, None);
        record_read("test-player-2", "/test-progress.wav", 0, 2048, Some(44));
        let reads = READS.lock().unwrap();
        let first = reads.iter().find(|r| r.peer == "test-player-1").unwrap();
        assert_eq!(first.requests, 21);
        assert_eq!(first.bytes, 40960);
        assert_eq!(first.errors, 1);
        assert_eq!(first.highest_end, 40960);
        assert_eq!(first.last_offset, 0);
        assert_eq!(first.first_reads.len(), 16);
        let second = reads.iter().find(|r| r.peer == "test-player-2").unwrap();
        assert_eq!(second.bytes, 44);
        assert_eq!(second.errors, 0);
        drop(reads);
        assert!(
            snapshot()
                .iter()
                .any(|(_, s)| s.contains("nfs_read_summary peer=test-player-1"))
        );
    }
}
