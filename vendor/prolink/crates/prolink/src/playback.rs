// SPDX-License-Identifier: GPL-3.0-only

//! What this host is playing, in the terms the network understands.
//!
//! Everything else in this crate is about reading the network. This is the one
//! thing that is about *being read*: a tempo, a place on a beat grid, and
//! whether sound is coming out. Without it a virtual CDJ is a device that
//! browses and serves and is, as far as every other player is concerned,
//! silent — it appears in the device list with no tempo, so nothing can
//! beat-match to it and no deck will draw its phase.
//!
//! # Why a position and not a beat pulse
//!
//! The obvious interface is "tell me when a beat happens". It is the wrong one.
//! The caller is an audio application whose beat notifications arrive on
//! whatever schedule its UI thread runs at — 30 Hz here — and a beat packet
//! that is 30 ms late is 7% of a beat at 145 BPM, which is exactly the error a
//! DJ is trying to eliminate. Worse, a notification that is *missed* silently
//! drops a beat from the network's picture of us.
//!
//! So the caller states a **position** — which beat, and how far through it —
//! and this crate projects forward from the moment that position was sampled.
//! Between updates the projection is arithmetic on a monotonic clock, so beats
//! land where the tempo says they should rather than where the last poll
//! happened to fall. A late update corrects the position; it does not stutter
//! the output.
//!
//! # Bars are ours, not the track's
//!
//! [`BeatPosition::number`] counts beats from the start of the track and the
//! bar is `((number - 1) % 4) + 1`, so beat 1 is a downbeat by construction.
//! That is a **convention, not a measurement**: nothing in a Mixxx beat grid
//! names a downbeat, and rekordbox's own numbering is not carried through. Beat
//! alignment against another player is therefore trustworthy and bar alignment
//! is a coin flip — which is worth saying out loud, because a CDJ following us
//! will happily line its bars up to ours.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use prolink_proto::beat::{Beat, BeatInBar, Pitch, Timings};
use prolink_proto::{DeviceName, DeviceNumber};

/// How long a sample stays usable.
///
/// The caller is expected to refresh several times a second. Nothing arriving
/// for this long means it has stopped telling us — the application quit, the
/// UI thread wedged — and continuing to project from an old sample would put a
/// tempo on the network that nothing is producing.
pub const STALE_AFTER: Duration = Duration::from_millis(1500);

/// A float rounded to a whole number, clamped into `0..=max` first.
///
/// The one float-to-integer conversion in this crate, in one place. `as` on a
/// float saturates rather than wrapping in modern Rust, so it is not unsound —
/// but it is silent about it, and the workspace warns on `as` for exactly that
/// reason. Clamping first means the saturation can never be reached, which
/// turns "this happens to be safe" into "this cannot arise".
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn rounded(value: f64, max: u64) -> u64 {
    if !value.is_finite() {
        return 0;
    }
    #[allow(clippy::cast_precision_loss)]
    let ceiling = max as f64;
    value.round().clamp(0.0, ceiling) as u64
}

/// Where the playhead is on the beat grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatPosition {
    /// Beats since the start of the track, counting the first beat as 1.
    pub number: u32,
    /// How far through that beat, `0.0` on the beat and approaching `1.0`.
    pub fraction: f64,
}

impl BeatPosition {
    /// The position as a single number of beats, which is what advances.
    fn as_beats(self) -> f64 {
        f64::from(self.number) + self.fraction.clamp(0.0, 1.0)
    }

    fn from_beats(beats: f64) -> Self {
        let whole = beats.floor();
        // Beat zero does not exist and four billion beats is 500 days of
        // music, so both ends are clamped rather than checked.
        let number = u32::try_from(rounded(whole, u64::from(u32::MAX)))
            .unwrap_or(u32::MAX)
            .max(1);
        Self {
            number,
            fraction: (beats - whole).clamp(0.0, 1.0),
        }
    }

    /// Which beat of the bar this is, under the convention above.
    pub fn in_bar(self) -> BeatInBar {
        let index = (self.number.saturating_sub(1)) % u32::from(BeatInBar::PER_BAR);
        // 1..=4 by construction, so the None arm is unreachable.
        BeatInBar::new(u8::try_from(index + 1).unwrap_or(1)).unwrap_or(BeatInBar::DOWNBEAT)
    }
}

/// A snapshot of what this host is playing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playback {
    /// The track's own tempo before the fader, in hundredths of a BPM.
    ///
    /// `None` when nothing is loaded, or when the track has no tempo we can
    /// state. A zero would be a measurement of 0.00 BPM rather than an absence.
    pub bpm_centi: Option<u16>,
    /// The pitch fader as a multiplier.
    pub pitch: Pitch,
    /// Whether sound is coming out.
    pub playing: bool,
    /// Where the playhead is, or `None` off the grid.
    pub beat: Option<BeatPosition>,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            bpm_centi: None,
            pitch: Pitch::UNITY,
            playing: false,
            beat: None,
        }
    }
}

impl Playback {
    /// The track's own tempo, before the fader.
    pub fn bpm(&self) -> Option<f64> {
        self.bpm_centi
            .filter(|&centi| centi > 0)
            .map(|centi| f64::from(centi) / 100.0)
    }

    /// The tempo actually coming out.
    pub fn effective_bpm(&self) -> Option<f64> {
        Some(self.bpm()? * self.pitch.multiplier())
    }

    /// How long one beat lasts at the tempo actually coming out.
    ///
    /// `None` rather than an infinity when there is no tempo, because every
    /// caller divides by this.
    pub fn beat_interval(&self) -> Option<Duration> {
        let bpm = self.effective_bpm()?;
        if bpm <= 0.0 {
            return None;
        }
        Duration::try_from_secs_f64(60.0 / bpm).ok()
    }

    /// One beat **as quoted in a beat packet**: at 0% pitch.
    ///
    /// Not the same as [`Self::beat_interval`], and the difference is the whole
    /// reason both exist. A deck quotes its grid distances as though the fader
    /// were centred and states the fader separately, so a follower applies the
    /// pitch itself. Quoting the real interval here would apply the pitch twice.
    fn quoted_beat_interval(&self) -> Option<Duration> {
        let bpm = self.bpm()?;
        if bpm <= 0.0 {
            return None;
        }
        Duration::try_from_secs_f64(60.0 / bpm).ok()
    }

    /// This playback as it will be *age* later, with the playhead advanced.
    ///
    /// Only the beat position moves; a tempo change is news the caller has to
    /// bring, not something to guess at.
    #[must_use]
    pub fn projected(&self, age: Duration) -> Self {
        let advanced = if let (Some(beat), Some(interval), true) =
            (self.beat, self.beat_interval(), self.playing)
        {
            let beats = age.as_secs_f64() / interval.as_secs_f64();
            Some(BeatPosition::from_beats(beat.as_beats() + beats))
        } else {
            self.beat
        };
        Self {
            beat: advanced,
            ..*self
        }
    }

    /// The beat packet to broadcast for the beat at *position*.
    ///
    /// Returns `None` without a tempo, because every field in it is a distance
    /// measured in beats.
    pub fn beat_packet(
        &self,
        device: DeviceNumber,
        name: DeviceName,
        position: BeatPosition,
    ) -> Option<Beat> {
        let interval = self.quoted_beat_interval()?.as_secs_f64() * 1000.0;
        // The packet leaves *on* the beat, so the first grid point ahead is one
        // whole interval away and everything else is a multiple of it. The bar
        // lines are where the count is not a fixed multiple: from beat 3 of a
        // bar the next bar line is two beats out, not four.
        //
        // Each one is rounded from the exact multiple rather than accumulated
        // from a rounded interval, because the field is whole milliseconds and
        // the error would otherwise compound: at 145 BPM the eighth beat is
        // 3310 ms, and eight rounded 414s make 3312.
        let beats_out = |n: u32| {
            Some(Duration::from_millis(rounded(
                interval * f64::from(n),
                u64::MAX,
            )))
        };
        let to_next_bar = u32::from(position.in_bar().beats_to_next_bar());
        Some(Beat {
            name,
            device,
            timings: Timings {
                next_beat: beats_out(1),
                second_beat: beats_out(2),
                next_bar: beats_out(to_next_bar),
                fourth_beat: beats_out(4),
                second_bar: beats_out(to_next_bar + u32::from(BeatInBar::PER_BAR)),
                eighth_beat: beats_out(8),
            },
            pitch: self.pitch,
            bpm_centi: self.bpm_centi.unwrap_or(0),
            beat_in_bar: Some(position.in_bar()),
            // We have no platter to scratch.
            scratching: false,
        })
    }
}

/// The latest [`Playback`] the caller stated, and when.
///
/// Shared between the caller's thread and the emitting tasks, which is why it
/// is a lock rather than a channel: every reader wants the *current* value and
/// none of them wants a backlog of the ones it missed.
#[derive(Debug, Default)]
pub struct PlaybackCell {
    latest: Mutex<Option<(Playback, Instant)>>,
}

impl PlaybackCell {
    /// State what is playing now.
    pub fn set(&self, playback: Playback) {
        let stamped = Some((playback, Instant::now()));
        match self.latest.lock() {
            Ok(mut latest) => *latest = stamped,
            Err(poisoned) => *poisoned.into_inner() = stamped,
        }
    }

    /// Forget everything: nothing is playing and nothing is loaded.
    pub fn clear(&self) {
        match self.latest.lock() {
            Ok(mut latest) => *latest = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
    }

    /// What is playing right now, with the playhead projected forward.
    ///
    /// A default [`Playback`] — no tempo, not playing — once the last sample
    /// has gone stale, so a caller that stops talking to us stops being
    /// announced as a tempo rather than freezing at its last one.
    pub fn now(&self) -> Playback {
        let sample = match self.latest.lock() {
            Ok(latest) => *latest,
            Err(poisoned) => *poisoned.into_inner(),
        };
        match sample {
            Some((playback, at)) => {
                let age = at.elapsed();
                if age > STALE_AFTER {
                    Playback::default()
                } else {
                    playback.projected(age)
                }
            }
            None => Playback::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(number: u32, fraction: f64) -> BeatPosition {
        BeatPosition { number, fraction }
    }

    fn playing_at(bpm_centi: u16, position: BeatPosition) -> Playback {
        Playback {
            bpm_centi: Some(bpm_centi),
            pitch: Pitch::UNITY,
            playing: true,
            beat: Some(position),
        }
    }

    #[test]
    fn beat_one_is_a_downbeat_and_the_bar_repeats_every_four() {
        let bars: Vec<u8> = (1..=9).map(|n| at(n, 0.0).in_bar().get()).collect();
        assert_eq!(bars, vec![1, 2, 3, 4, 1, 2, 3, 4, 1]);
    }

    #[test]
    fn the_playhead_advances_by_the_clock_not_by_the_poll() {
        // 120 BPM: half a second is exactly one beat. The point of projecting
        // rather than counting notifications is that this holds for any age,
        // including ages no caller would ever poll at.
        let playback = playing_at(12_000, at(4, 0.0));
        let later = playback.projected(Duration::from_millis(500));
        assert_eq!(later.beat.expect("a beat").number, 5);
        let much_later = playback.projected(Duration::from_millis(1750));
        let beat = much_later.beat.expect("a beat");
        assert_eq!(beat.number, 7);
        assert!(
            (beat.fraction - 0.5).abs() < 1e-9,
            "1.75 s at 120 BPM is three and a half beats, not {beat:?}"
        );
    }

    #[test]
    fn a_paused_deck_stays_where_it_is() {
        let paused = Playback {
            playing: false,
            ..playing_at(12_000, at(4, 0.25))
        };
        assert_eq!(
            paused.projected(Duration::from_secs(10)).beat,
            Some(at(4, 0.25)),
            "a deck that is not playing does not travel"
        );
    }

    #[test]
    fn the_pitch_fader_moves_the_playhead_but_not_the_quoted_grid() {
        // The distinction the wire draws and the one it is easiest to get
        // wrong: the packet quotes distances at 0% and states the fader beside
        // them, so a follower applies the pitch once. Applying it here as well
        // would double it.
        let fast = Playback {
            pitch: Pitch(0x0020_0000), // twice unity
            ..playing_at(12_000, at(1, 0.0))
        };
        assert_eq!(
            fast.beat_interval(),
            Some(Duration::from_millis(250)),
            "240 effective BPM is a beat every quarter second"
        );
        assert_eq!(
            fast.quoted_beat_interval(),
            Some(Duration::from_millis(500)),
            "but the packet quotes the track's own 120 BPM"
        );
        let packet = fast
            .beat_packet(DeviceNumber::ONE, DeviceName::default(), at(1, 0.0))
            .expect("a beat packet");
        assert_eq!(packet.timings.next_beat, Some(Duration::from_millis(500)));
        // And a reader that applies the fader gets the truth back.
        assert_eq!(packet.beat_interval(), Some(Duration::from_millis(250)));
    }

    #[test]
    fn the_bar_line_is_however_many_beats_are_left_of_the_bar() {
        // Not four. From beat 3 of a bar the next bar line is two beats away,
        // and the one after it six -- a fixed four here would tell every
        // follower to drop its downbeat in the middle of ours.
        let playback = playing_at(12_000, at(3, 0.0));
        let packet = playback
            .beat_packet(DeviceNumber::ONE, DeviceName::default(), at(3, 0.0))
            .expect("a beat packet");
        assert_eq!(packet.beat_in_bar, BeatInBar::new(3));
        assert_eq!(packet.timings.next_bar, Some(Duration::from_secs(1)));
        assert_eq!(packet.timings.second_bar, Some(Duration::from_secs(3)));
        assert_eq!(packet.timings.next_beat, Some(Duration::from_millis(500)));
        assert_eq!(packet.timings.eighth_beat, Some(Duration::from_secs(4)));
    }

    #[test]
    fn a_beat_packet_we_emit_parses_back_as_a_beat_packet() {
        let playback = playing_at(14_500, at(9, 0.0));
        let packet = playback
            .beat_packet(DeviceNumber::ONE, DeviceName::default(), at(9, 0.0))
            .expect("a beat packet");
        let raw = packet.encode();
        let parsed = Beat::parse(&raw).expect("our own beat packet");
        assert_eq!(parsed, packet);
        assert_eq!(parsed.beat_in_bar, BeatInBar::new(1));
        assert!((parsed.effective_bpm() - 145.0).abs() < 0.01);
    }

    #[test]
    fn a_caller_that_stops_talking_stops_being_a_tempo() {
        // The failure this prevents is not a wrong number, it is a plausible
        // one: a frozen sample keeps announcing a tempo nothing is producing,
        // and a follower synced to it stays synced to a ghost.
        let cell = PlaybackCell::default();
        assert_eq!(cell.now().bpm_centi, None, "nothing stated yet");
        cell.set(playing_at(14_500, at(1, 0.0)));
        assert_eq!(cell.now().bpm_centi, Some(14_500));
        cell.clear();
        assert_eq!(cell.now().bpm_centi, None);
        assert!(!cell.now().playing);
    }

    #[test]
    fn a_track_with_no_tempo_produces_no_beat_packet() {
        let silent = Playback::default();
        assert!(
            silent
                .beat_packet(DeviceNumber::ONE, DeviceName::default(), at(1, 0.0))
                .is_none(),
            "every field in a beat packet is a distance in beats"
        );
        assert_eq!(silent.beat_interval(), None);
    }
}
