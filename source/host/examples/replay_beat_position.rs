//! Replay captured UDP events through the production tracker. No hardware needed.
//! Grid anchors come from the capture's published coarse positions; no interpolation
//! or inferred grid entries are used. This measures consistency, not physical latency.
use pioneer_companion_host::beat_position::{BeatPosition, Position};
use prolink::monitor::PlayerStatus;
use prolink_proto::{beat, status};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Deck {
    tracker: BeatPosition,
    grid: Vec<f64>,
    key: String,
    status: Option<(PlayerStatus, Instant)>,
    pulse: Option<(beat::Beat, Instant)>,
    last: Option<Position>,
    corrections: Vec<f64>,
    accepted: usize,
    pulses: usize,
}
fn main() {
    let path = std::env::args().nth(1).expect("capture JSON path");
    let data: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let events = data["host"]["samples"].as_array().unwrap();
    let mut decks: BTreeMap<String, Deck> = BTreeMap::new();
    for event in events.iter().filter(|e| e["kind"] == "published") {
        let p = &event["deck"];
        // Builds 14/15 publish an exact grid anchor on a new beat (motion
        // timestamp differs from status timestamp). Later jitter-filtered
        // positions are not exact anchors and must never reconstruct a grid.
        let old_exact_anchor = data["host"]["positionTracker"].is_null()
            && p["positionSource"] == "beat-motion"
            && p["observationId"].as_str().is_some_and(|id| {
                let parts: Vec<_> = id.split(':').collect();
                parts.len() == 4 && parts[2] != parts[3]
            });
        if p["positionSource"] != "status-beat-estimate" && !old_exact_anchor {
            continue;
        }
        let beat_field = if old_exact_anchor {
            "beatAnchorNumber"
        } else {
            "beatNumber"
        };
        let (Some(ip), Some(n), Some(seconds), Some(key)) = (
            p["ip"].as_str(),
            p[beat_field].as_u64(),
            p["position"].as_f64(),
            p["trackKey"].as_str(),
        ) else {
            continue;
        };
        if n == 0 {
            continue;
        }
        let deck = decks.entry(ip.into()).or_default();
        if !deck.key.is_empty() {
            assert_eq!(deck.key, key, "one track per deck required");
        }
        deck.key = key.into();
        deck.grid.resize(deck.grid.len().max(n as usize), f64::NAN);
        deck.grid[n as usize - 1] = seconds;
    }
    // Optional reproduction of a wrong mapping learned before capture began.
    let seed_relation = std::env::args()
        .nth(2)
        .map(|s| s.parse::<u32>().expect("initial relation 0..3"));
    let start = Instant::now() + Duration::from_secs(1);
    let mut samples = Vec::new();
    for event in events.iter().filter(|e| e["kind"] == "udp") {
        let Some(deck) = event["ip"].as_str().and_then(|ip| decks.get_mut(ip)) else {
            continue;
        };
        let hex = event["hex"].as_str().unwrap();
        let bytes: Vec<_> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let ms = event["hostMs"].as_f64().unwrap();
        let at = start + Duration::from_secs_f64(ms / 1000.);
        match event["port"].as_u64() {
            Some(50002) => {
                if let Ok(status::Packet::CdjStatus(s)) = status::decode(&bytes) {
                    let decoded = PlayerStatus::from_packet(&s);
                    if deck.status.is_none()
                        && let Some(relation) = seed_relation
                    {
                        assert!(relation < 4);
                        let mut seed = decoded;
                        let number = seed.beat_number.unwrap();
                        seed.beat_in_bar = Some(((number + 3 - relation) % 4 + 1) as u8);
                        for back in [2, 1] {
                            let seed_at = start - Duration::from_millis(back);
                            deck.tracker
                                .observe(&deck.key, seed, seed_at, None, &deck.grid, seed_at);
                        }
                    }
                    deck.status = Some((decoded, at));
                } else {
                    continue;
                }
            }
            Some(50001) => {
                if let Ok(beat::Packet::Beat(b)) = beat::decode(&bytes) {
                    deck.pulse = Some((b, at));
                    deck.pulses += 1;
                } else {
                    continue;
                }
            }
            _ => continue,
        }
        let Some((status, status_at)) = deck.status else {
            continue;
        };
        if let Some(p) = deck
            .tracker
            .observe(&deck.key, status, status_at, deck.pulse, &deck.grid, at)
        {
            if deck.last.is_none_or(|old| p.beat_at != old.beat_at) {
                deck.accepted += 1;
                if let Some(old) = deck.last {
                    let predicted = old.seconds
                        + p.at.saturating_duration_since(old.at).as_secs_f64() * old.rate;
                    deck.corrections.push((p.seconds - predicted) * 1000.);
                }
            }
            samples.push(json!({"ip":event["ip"],"hostMs":ms,"positionMs":p.at.duration_since(start).as_secs_f64()*1000.,"position":p.seconds,"rate":p.rate,"beatNumber":p.beat_number,"correctionMs":p.correction_seconds*1000.,"arrivalResidualMs":p.arrival_residual_seconds*1000.}));
            deck.last = Some(p);
        }
    }
    let summary: BTreeMap<_, _> = decks.iter().map(|(ip,d)| {
        let n = d.corrections.len().max(1) as f64;
        (ip, json!({"receivedBeats":d.pulses,"acceptedAnchors":d.accepted,"meanAbsoluteCorrectionMs":d.corrections.iter().map(|v|v.abs()).sum::<f64>()/n,"maxAbsoluteCorrectionMs":d.corrections.iter().map(|v|v.abs()).fold(0.,f64::max),"correctionsMs":d.corrections}))
    }).collect();
    println!("{}",serde_json::to_string_pretty(&json!({"note":"Source milliseconds; UDP timing consistency only, not physical accuracy.","decks":summary,"samples":samples})).unwrap());
}
