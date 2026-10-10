// SPDX-License-Identifier: GPL-3.0-only

//! What is playing, at what tempo, where in the bar — and who is master.
//!
//! Two ports answer that question and neither answers all of it.
//!
//! **UDP 50001** is broadcast. Beat packets arrive *on* the beat, so a passive
//! listener that has announced nothing still sees every playing deck's tempo,
//! its pitch fader and its position in the bar. [`Monitor::start`] is that, and
//! it transmits nothing.
//!
//! **UDP 50002** is unicast to peers that have announced themselves and to
//! nobody else (F21). It carries the loaded track, the play state and — at byte
//! `0x9e` — **who holds tempo master**, which is published there and nowhere
//! else. A device that never announces can therefore never know who the master
//! is, however long it listens. [`Monitor::with_status`] takes a
//! [`VirtualCdj`] by reference for exactly that reason: the requirement is in
//! the signature rather than in a comment somebody has to read.
//!
//! # Interpolating phase
//!
//! A beat packet is an event, not a report: its arrival *is* the deck saying "I
//! am starting a beat now". So phase is `elapsed_since_the_packet /
//! beat_interval`, clamped rather than wrapped, and re-anchored by the next
//! arrival. The interval comes from the **effective** tempo — the track's BPM
//! with the pitch fader applied — because the packet's own millisecond fields
//! are quoted as if the fader were centred (see [`prolink_proto::beat`]).
//!
//! Clamped, not wrapped, because a deck that has stopped should sit at the end
//! of its beat rather than spin: a wrapped estimate looks alive, and looking
//! alive is the one thing a stopped deck must not do.
//!
//! After [`BEAT_STALE_AFTER`] — three beats at 60 BPM, and seven at 145 — the
//! phase is not merely old, it is meaningless, and [`BeatObservation::beat_phase`]
//! returns `None`. The last tempo is still readable, because "it was playing at
//! 128 and stopped" is more useful than an empty row.
//!
//! Silence is evidence, but only about a CDJ. A nexus deck sends beat packets
//! only while playing and only for a rekordbox-analysed track: in this
//! project's corpus not one status packet reporting "nothing loaded" had a beat
//! from the same deck in the preceding second. A **mixer** sends them
//! continuously as a backup metronome, so no such inference holds for one.
//!
//! # One socket per port, and who else wants them
//!
//! [`Monitor::with_status`] binds UDP 50002, and a [`VirtualCdj`] built with
//! `emit_status` set binds it too, to answer media queries. Both sockets set
//! `SO_REUSEPORT`, so both binds succeed — but a **unicast** datagram is
//! delivered to only one of them: macOS gives it to the socket bound first,
//! Linux hashes. So a monitor and a serving virtual CDJ cannot both read status.
//!
//! That is not the constraint it looks like. What makes peers unicast their
//! status to us is the *keep-alive*, not our own status stream, so a virtual CDJ
//! configured with `emit_status: false` announces, receives everything, and
//! leaves 50002 free for the monitor. Emitting status is needed only to be
//! browsed. `prolink status --announce` does exactly that.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use prolink_proto::beat::{self, Beat, BeatInBar, Pitch};
use prolink_proto::status::{self, CdjStatus, StatusFlags};
use prolink_proto::{BEAT_PORT, DeviceName, DeviceNumber, STATUS_PORT, Slot};
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use crate::Result;
use crate::interface::Interface;
use crate::socket::{self, MAX_DATAGRAM};
use crate::virtual_cdj::VirtualCdj;

/// How many events a slow subscriber may fall behind before it starts missing
/// them.
///
/// Generous: four playing decks produce about sixteen beats a second between
/// them, so this is a minute of slack rather than the seconds device churn
/// needs.
const EVENT_CAPACITY: usize = 1024;

/// A device that has sent no beat packet for this long has stopped, and its
/// phase is stale rather than merely old.
///
/// Three beats at 60 BPM, so even an unusually slow track keeps its phase alive
/// between packets; at the 145 BPM common in this corpus it is seven beats.
/// Deliberately generous — a phase reported a beat late is wrong in a way a DJ
/// notices, and a deck declared stopped a beat early is wrong in a way that
/// makes the display flicker.
pub const BEAT_STALE_AFTER: Duration = Duration::from_secs(3);

/// A device that has said nothing on either port for this long is dropped.
///
/// Long enough to outlast a nudged cable or a switch reconverging, which the
/// device table treats the same way.
pub const FORGET_AFTER: Duration = Duration::from_secs(30);

/// How often staleness is noticed.
///
/// Fast enough that a stopped deck is reported within a fifth of a beat at any
/// plausible tempo.
const REAP_INTERVAL: Duration = Duration::from_millis(250);

// -- what a player is doing -----------------------------------------------

/// Byte `0x7b` of a status packet: what the platter is doing.
///
/// A newtype rather than an enum because a firmware or a model we have not seen
/// may use a value we have not seen, and a decoder that refused it would lose
/// the tempo and the master flag in the same packet.
///
/// Ten of the twelve documented values appear in this project's corpus. The two
/// that do not are marked below.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayState(pub u8);

impl PlayState {
    /// Nothing loaded.
    pub const NO_TRACK: Self = Self(0x00);
    /// A track is being loaded.
    pub const LOADING: Self = Self(0x02);
    /// Playing.
    pub const PLAYING: Self = Self(0x03);
    /// Playing inside a loop.
    pub const LOOPING: Self = Self(0x04);
    /// Paused, platter stopped.
    pub const PAUSED: Self = Self(0x05);
    /// Stopped at the cue point.
    pub const CUED: Self = Self(0x06);
    /// Playing from the cue point while the cue button is held.
    pub const CUE_PLAY: Self = Self(0x07);
    /// Scrubbing from the cue point. *Documented; never observed here.*
    pub const CUE_SCRATCH: Self = Self(0x08);
    /// Searching through the track.
    pub const SEARCHING: Self = Self(0x09);
    /// The CD has spun down. Observed only with nothing loaded.
    pub const SPUN_DOWN: Self = Self(0x0e);
    /// The end of the track has been reached. *Documented; never observed
    /// here.*
    pub const END_OF_TRACK: Self = Self(0x11);
    /// The emergency loop: the medium went away mid-play and the deck is
    /// looping what it has buffered.
    ///
    /// Confirmed rather than assumed — the 189 packets carrying this value all
    /// came from a deck whose slots had both just gone empty while a rekordbox
    /// track was loaded, and all set the emergency flag at byte `0xba`.
    pub const EMERGENCY_LOOP: Self = Self(0x12);

    /// A name for a display, or `None` for a value we have never seen.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::NO_TRACK => "no track",
            Self::LOADING => "loading",
            Self::PLAYING => "playing",
            Self::LOOPING => "looping",
            Self::PAUSED => "paused",
            Self::CUED => "cued",
            Self::CUE_PLAY => "cue play",
            Self::CUE_SCRATCH => "cue scratch",
            Self::SEARCHING => "searching",
            Self::SPUN_DOWN => "spun down",
            Self::END_OF_TRACK => "end of track",
            Self::EMERGENCY_LOOP => "emergency loop",
            _ => return None,
        })
    }

    /// Whether audio is coming out of this deck.
    ///
    /// The emergency loop counts, because it is audible — but note that it is
    /// the one playing state that produced **no** beat packets in this corpus,
    /// the medium having gone away. So "playing" and "sending beats" are two
    /// facts, not one.
    pub fn is_playing(self) -> bool {
        matches!(
            self,
            Self::PLAYING | Self::LOOPING | Self::CUE_PLAY | Self::EMERGENCY_LOOP
        )
    }
}

impl fmt::Debug for PlayState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "PlayState({:#04x})", self.0),
        }
    }
}

impl fmt::Display for PlayState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "{:#04x}", self.0),
        }
    }
}

/// Byte `0x2a` of a status packet: what kind of thing is loaded.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackKind(pub u8);

impl TrackKind {
    /// Nothing.
    pub const NONE: Self = Self(0x00);
    /// A rekordbox-analysed track — the only kind with a beat grid, and so the
    /// only kind a deck sends beat packets for.
    pub const REKORDBOX: Self = Self(0x01);
    /// A file on the medium that rekordbox has not analysed.
    pub const UNANALYSED: Self = Self(0x02);
    /// A track on an audio CD.
    pub const AUDIO_CD: Self = Self(0x05);
    /// A track from a streaming service.
    pub const STREAMING: Self = Self(0x06);

    /// A name for a display, or `None` for a value we have never seen.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::NONE => "none",
            Self::REKORDBOX => "rekordbox",
            Self::UNANALYSED => "unanalysed",
            Self::AUDIO_CD => "audio cd",
            Self::STREAMING => "streaming",
            _ => return None,
        })
    }
}

impl fmt::Debug for TrackKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "TrackKind({:#04x})", self.0),
        }
    }
}

impl fmt::Display for TrackKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// What a player has loaded, and where it came from.
///
/// A track is identified by *whose* slot it came from as well as by its id: two
/// decks playing row 182 of two different sticks are playing two different
/// tracks, and a metadata lookup addressed to the wrong player returns
/// somebody else's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadedTrack {
    /// The player whose medium the track came from — its own number when it is
    /// playing from its own slot.
    pub source_player: DeviceNumber,
    /// Which of that player's slots.
    pub slot: Slot,
    /// The rekordbox row id, or the track number on an audio CD.
    pub id: u32,
    /// What kind of thing it is.
    pub kind: TrackKind,
}

/// The fields of a status packet this crate models.
///
/// Everything here comes from UDP 50002 and is therefore unavailable without a
/// [`VirtualCdj`] announcing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStatus {
    /// Whether the local USB slot reports mounted media.
    pub usb_present: bool,
    /// Whether the local SD slot reports mounted media.
    pub sd_present: bool,
    /// Manual transport evidence; synced fader/effective-pitch differences are normal.
    pub jogging: bool,
    /// Optional high-resolution bar length and position; may remain stale while playing.
    pub bar_position: Option<(u16, u16)>,
    /// Reported active loop: start/end microseconds and whole-beat count (newer players).
    pub active_loop: Option<(u64, u64, u16)>,
    /// One-based beat within the bar, used to validate bar position.
    pub beat_in_bar: Option<u8>,
    /// Reverse transport flag from play-state byte 0x9d (Beat Link protocol research).
    pub reverse: bool,
    /// Current one-based beat-grid index, if provided by the player.
    pub beat_number: Option<u32>,
    /// What the platter is doing.
    pub play_state: PlayState,
    /// What is loaded, or `None` when nothing is.
    pub track: Option<LoadedTrack>,
    /// The track's own tempo in hundredths of a BPM, before the pitch fader.
    ///
    /// `None` when the deck has no track, rather than the `0xffff` the wire
    /// carries there — which reads as 655.35 BPM (F49).
    pub bpm_centi: Option<u16>,
    /// The pitch fader as a multiplier, from `0x8c`.
    ///
    /// While [`Self::is_synced`] this is not the DJ's fader position: the deck
    /// slews it to hold its effective tempo equal to the master's.
    pub pitch: Option<Pitch>,
    /// Unsigned motion-speed field at 0x98. Useful for forward nudges; does not
    /// encode the direction of paused scratching. Keep separate from tempo.
    pub motion_pitch: Option<Pitch>,
    /// Forward play/loop/cue-audition transport, excluding holds and reverse.
    /// Loop position tracking additionally requires validated endpoints.
    pub forward_transport: bool,
    /// Whether this player holds tempo master, from byte `0x9e`.
    ///
    /// **The only place mastership is published.** Byte `0x9e` and flag bit 5
    /// of byte `0x89` agreed in 46 011 of the 46 012 status packets in this
    /// corpus; the byte is used because it distinguishes a master on a
    /// rekordbox track (`1`) from one with no usable tempo (`2`).
    pub is_tempo_master: bool,
    /// Whether SYNC is lit on this deck, from flag bit 4 of byte `0x89`.
    pub is_synced: bool,
    /// Whether the deck is producing sound, from flag bit 6 of byte `0x89`.
    ///
    /// Not a restatement of [`Self::play_state`]: 289 of the 8 859 captured
    /// packets with play state `Playing` have this bit clear (F50).
    pub is_playing: bool,
    /// The device this master is handing mastership to, mid-handoff.
    ///
    /// `None` except for the one or two packets between a
    /// [`prolink_proto::beat::MasterResponse`] and the old master dropping its
    /// claim — the window in which **both** decks report `is_tempo_master`.
    pub yielding_to: Option<DeviceNumber>,
}

impl PlayerStatus {
    /// Position updates must stay direct when motion cannot be predicted safely.
    /// Nexus fractional loops can report P3=1 without running in reverse.
    pub fn position_requires_direct_updates(&self) -> bool {
        self.jogging
            || !self.is_playing
            || self.reverse
            || (self.play_state.0 == 4 && !self.forward_transport)
    }

    /// The tempo actually playing: the track's tempo with the fader applied.
    ///
    /// `None` when either half is missing. Unlike
    /// [`BeatObservation::effective_bpm`] this is available while the deck is
    /// paused, because status packets keep flowing when beats do not.
    pub fn effective_bpm(&self) -> Option<f64> {
        let bpm = f64::from(self.bpm_centi?) / 100.0;
        Some(bpm * self.pitch?.multiplier())
    }
}

/// A beat packet and how long ago it landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeatObservation {
    /// Monotonic receive timestamp for distinguishing new pulses from snapshots.
    pub received_at: Instant,
    /// The packet.
    pub beat: Beat,
    /// How long ago it arrived. Zero at the instant of the beat.
    pub age: Duration,
}

impl BeatObservation {
    /// Whether this observation is too old to place the player in its beat.
    ///
    /// See [`BEAT_STALE_AFTER`]. A stale observation still carries the last
    /// tempo, which is why it is kept rather than discarded.
    pub fn is_stale(self) -> bool {
        self.age > BEAT_STALE_AFTER
    }

    /// The tempo actually playing: the track's tempo with the pitch fader
    /// applied.
    pub fn effective_bpm(self) -> f64 {
        self.beat.effective_bpm()
    }

    /// Where in the current beat the player is: `0.0` on the beat, approaching
    /// `1.0` before the next.
    ///
    /// `None` when the observation is stale, or when the packet carries no
    /// usable tempo to divide by. Clamped rather than wrapped — a player that
    /// has stopped sits at the end of its beat instead of spinning.
    pub fn beat_phase(self) -> Option<f64> {
        if self.is_stale() {
            return None;
        }
        let interval = self.beat.beat_interval()?.as_secs_f64();
        if interval <= 0.0 {
            return None;
        }
        Some((self.age.as_secs_f64() / interval).clamp(0.0, 1.0))
    }

    /// Where in the four-beat bar the player is: `0.0` on the downbeat.
    ///
    /// `None` when there is no phase, and also when the player is not on a
    /// rekordbox track — byte `0x5c` is then `0`, which means "no bar to be in"
    /// rather than "beat zero".
    pub fn bar_phase(self) -> Option<f64> {
        let position = self.beat.beat_in_bar?;
        let within = self.beat_phase()?;
        Some((f64::from(position.index()) + within) / f64::from(BeatInBar::PER_BAR))
    }
}

impl PlayerStatus {
    /// Decode the same status fields for broadcast-discovered and direct-IP peers.
    pub fn from_packet(packet: &CdjStatus) -> Self {
        let flags = packet.flags();
        let raw = packet.as_bytes();
        let synced = flags.is_some_and(StatusFlags::is_synced);
        // Pitch2 follows the local fader, while Pitch1 can follow the sync
        // master. Their difference is not jog evidence on a synced player.
        // P2's stopped bit still detects a held platter while P1 says playing.
        let held = matches!(packet.play_state(), Some(3 | 4))
            && raw.get(0x8b).is_some_and(|state| state & 0x04 != 0);
        let jogging = held
            || (!synced
                && raw
                    .get(0x98..0x9c)
                    .zip(raw.get(0x8c..0x90))
                    .is_some_and(|(motion, fader)| {
                        u32::from_be_bytes(motion.try_into().unwrap())
                            .abs_diff(u32::from_be_bytes(fader.try_into().unwrap()))
                            > 4096
                    }));
        PlayerStatus {
            usb_present: packet.usb_state().has_media(),
            sd_present: packet.sd_state().has_media(),
            jogging,
            active_loop: raw.get(0..0x1ca).and_then(|b| {
                let start = u64::from(u32::from_be_bytes(b[0x1b6..0x1ba].try_into().ok()?)) * 65536;
                let end = u64::from(u32::from_be_bytes(b[0x1be..0x1c2].try_into().ok()?)) * 65536;
                (end > start).then_some((start, end, u16::from_be_bytes([b[0x1c8], b[0x1c9]])))
            }),
            bar_position: packet.as_bytes().get(0x114..0x11c).map(|b| {
                (
                    u16::from_be_bytes([b[2], b[3]]),
                    u16::from_be_bytes([b[6], b[7]]),
                )
            }),
            beat_in_bar: packet
                .as_bytes()
                .get(0xa6)
                .copied()
                .filter(|b| (1..=4).contains(b)),
            // P3=1 also occurs in forward fractional loops on original nexus.
            // Only ordinary playing has a supported reverse interpretation.
            reverse: packet.play_state() == Some(3)
                && flags.is_some_and(StatusFlags::is_playing)
                && !held
                && packet.as_bytes().get(0x9d) == Some(&1),
            beat_number: packet.beat_number(),
            play_state: PlayState(packet.play_state().unwrap_or(0)),
            track: loaded_track(packet),
            bpm_centi: packet.bpm_centi(),
            pitch: packet.pitch(),
            motion_pitch: raw
                .get(0x98..0x9c)
                .map(|b| Pitch(u32::from_be_bytes(b.try_into().unwrap()))),
            forward_transport: matches!(packet.play_state(), Some(3 | 4 | 7))
                && (flags.is_some_and(StatusFlags::is_playing) || packet.play_state() == Some(7))
                && raw.get(0x8b).is_some_and(|b| b & 4 == 0)
                && matches!(raw.get(0x9d), Some(9 | 13)),
            is_tempo_master: packet.is_tempo_master().unwrap_or(false),
            is_synced: synced,
            // Nexus cue audition advances audio with the playing flag clear.
            // Keep the flag authoritative for loops, which can also be paused.
            is_playing: packet.play_state() == Some(PlayState::CUE_PLAY.0)
                || flags.is_some_and(StatusFlags::is_playing),
            yielding_to: packet.yielding_to(),
        }
    }
}

/// Reject reordered status datagrams once a sender's counter advances.
/// Constant/missing counters remain supported; a receive gap permits a reboot.
#[derive(Clone, Debug, Default)]
pub struct StatusPacketOrder {
    last: Option<(u32, Instant)>,
    advancing: bool,
}
impl StatusPacketOrder {
    /// Accept forward counters (including wrap), with a fallback for fixed counters.
    pub fn accept(&mut self, counter: Option<u32>, now: Instant) -> bool {
        let Some(counter) = counter else {
            *self = Self::default();
            return true;
        };
        if let Some((previous, at)) = self.last
            && now.saturating_duration_since(at) < Duration::from_secs(1)
        {
            let delta = counter.wrapping_sub(previous);
            if delta == 0 {
                if self.advancing {
                    return false;
                }
            } else if delta < (1 << 31) {
                self.advancing = true;
            } else {
                return false;
            }
        } else {
            self.advancing = false;
        }
        self.last = Some((counter, now));
        true
    }
}

/// A status packet's modelled fields and how long ago it landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusObservation {
    /// Monotonic receive time of the actual accepted UDP packet.
    pub received_at: Instant,
    /// Sender counter, when the packet contains one.
    pub packet_counter: Option<u32>,
    /// The fields.
    pub status: PlayerStatus,
    /// How long ago the packet arrived. A deck sends one every ~200 ms.
    pub age: Duration,
}

/// Everything both ports say about one device, as of one instant.
///
/// Keyed by device number, because that is the only identifier a beat packet
/// carries: there is no MAC and no address in it, and the number at `0x21` is
/// the same one at `0x5f`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerState {
    /// The number the device holds.
    pub device: DeviceNumber,
    /// The name it puts in its packets.
    pub name: DeviceName,
    /// The last beat packet. `None` until one arrives — which for a CDJ means
    /// it has not played a rekordbox track since we started listening.
    pub beat: Option<BeatObservation>,
    /// The last status packet. `None` unless a [`VirtualCdj`] is announcing.
    pub status: Option<StatusObservation>,
}

impl PlayerState {
    /// The tempo actually playing, from the beat packet.
    ///
    /// `None` when no beat packet has arrived. A stale one still answers: a
    /// deck that stopped a minute ago stopped at *some* tempo, and reporting
    /// nothing would lose that.
    pub fn effective_bpm(&self) -> Option<f64> {
        self.beat.map(BeatObservation::effective_bpm)
    }

    /// Where in the current beat, `0.0`–`1.0`. `None` when stale or unknown.
    pub fn beat_phase(&self) -> Option<f64> {
        self.beat.and_then(BeatObservation::beat_phase)
    }

    /// Where in the bar, `0.0`–`1.0`. `None` when stale or off the grid.
    pub fn bar_phase(&self) -> Option<f64> {
        self.beat.and_then(BeatObservation::bar_phase)
    }

    /// Which beat of the bar the last packet announced, 1–4.
    pub fn beat_in_bar(&self) -> Option<BeatInBar> {
        self.beat.and_then(|observed| observed.beat.beat_in_bar)
    }

    /// Whether this device holds tempo master.
    ///
    /// `None` — not `false` — when no status packet has been seen, because a
    /// listener that has not announced cannot tell "not master" from "cannot
    /// know" and must not report the first when it means the second.
    pub fn is_tempo_master(&self) -> Option<bool> {
        self.status.map(|observed| observed.status.is_tempo_master)
    }

    /// Whether SYNC is lit on this deck. `None` without status.
    pub fn is_synced(&self) -> Option<bool> {
        self.status.map(|observed| observed.status.is_synced)
    }

    /// The device this one is handing mastership to, if a handoff is in
    /// progress. Almost always `None`; see [`PlayerStatus::yielding_to`].
    pub fn yielding_to(&self) -> Option<DeviceNumber> {
        self.status.and_then(|observed| observed.status.yielding_to)
    }

    /// What the deck says it is doing. `None` without status.
    pub fn play_state(&self) -> Option<PlayState> {
        self.status.map(|observed| observed.status.play_state)
    }

    /// What is loaded. `None` without status, and also when nothing is loaded.
    pub fn track(&self) -> Option<LoadedTrack> {
        self.status.and_then(|observed| observed.status.track)
    }

    /// Whether this device is sending beats right now.
    ///
    /// For a nexus CDJ this is "playing a rekordbox track"; for a mixer it is
    /// always true, since a mixer is a metronome whatever the decks are doing.
    pub fn is_beating(&self) -> bool {
        self.beat.is_some_and(|observed| !observed.is_stale())
    }
}

/// Something changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorEvent {
    /// A device started a beat. One per packet, so about four a second per
    /// playing deck.
    Beat(Box<PlayerState>),
    /// A device's status changed in a way this crate models. Repeats of an
    /// unchanged status — the overwhelming majority, at five packets a second —
    /// are not events.
    Status(Box<PlayerState>),
    /// Tempo master moved, or was lost.
    TempoMaster(Option<DeviceNumber>),
    /// A device pressed MASTER and is asking the holder to hand over.
    ///
    /// Addressed to whoever holds it, so receiving one means **we** hold it and
    /// are expected to answer with a [`prolink_proto::beat::MasterResponse`]
    /// and stand down. A deck that ignores it keeps claiming mastership the
    /// rest of the network has already moved on from.
    MasterRequested(DeviceNumber),
    /// A device stopped sending beats: it has stopped playing.
    Stopped(DeviceNumber),
    /// A device said nothing on either port for [`FORGET_AFTER`].
    Gone(DeviceNumber),
}

impl MonitorEvent {
    /// The device this event is about, where it is about one.
    pub fn device(&self) -> Option<DeviceNumber> {
        match self {
            Self::Beat(state) | Self::Status(state) => Some(state.device),
            Self::Stopped(device) | Self::Gone(device) | Self::MasterRequested(device) => {
                Some(*device)
            }
            Self::TempoMaster(device) => *device,
        }
    }
}

// -- the table ------------------------------------------------------------

#[derive(Clone, Debug)]
struct Entry {
    name: DeviceName,
    beat: Option<(Beat, Instant)>,
    status: Option<(PlayerStatus, Instant)>,
    status_order: StatusPacketOrder,
    /// Set while the device is beating, so that stopping is reported once
    /// rather than at every tick of the reaper.
    beating: bool,
}

impl Entry {
    fn last_heard(&self) -> Option<Instant> {
        match (self.beat, self.status) {
            (Some((_, beat)), Some((_, status))) => Some(beat.max(status)),
            (Some((_, at)), None) | (None, Some((_, at))) => Some(at),
            (None, None) => None,
        }
    }
}

/// Players keyed by device number, with the tempo master alongside.
#[derive(Debug, Default)]
struct PlayerTable {
    players: BTreeMap<DeviceNumber, Entry>,
    master: Option<DeviceNumber>,
}

impl PlayerTable {
    fn entry(&mut self, device: DeviceNumber, name: DeviceName) -> &mut Entry {
        let entry = self.players.entry(device).or_insert_with(|| Entry {
            name,
            beat: None,
            status: None,
            status_order: StatusPacketOrder::default(),
            beating: false,
        });
        entry.name = name;
        entry
    }

    fn observe_beat(&mut self, beat: &Beat, now: Instant) -> MonitorEvent {
        let entry = self.entry(beat.device, beat.name);
        entry.beat = Some((*beat, now));
        entry.beating = true;
        MonitorEvent::Beat(Box::new(self.state(beat.device, now).unwrap_or(
            PlayerState {
                device: beat.device,
                name: beat.name,
                beat: None,
                status: None,
            },
        )))
    }

    /// Fold in a status packet. Returns nothing when it says the same as the
    /// last one, which at five packets a second is nearly all of them.
    fn observe_status(&mut self, packet: &CdjStatus, now: Instant) -> Vec<MonitorEvent> {
        let Some(device) = packet.sender() else {
            return Vec::new();
        };
        let status = PlayerStatus::from_packet(packet);
        let entry = self.entry(device, packet.name());
        if !entry.status_order.accept(packet.packet_counter(), now) {
            return Vec::new();
        }
        let changed = entry.status.map(|(previous, _)| previous) != Some(status);
        entry.status = Some((status, now));

        let mut events = Vec::new();
        if changed && let Some(state) = self.state(device, now) {
            events.push(MonitorEvent::Status(Box::new(state)));
        }
        if let Some(event) = self.settle_master(device, status.is_tempo_master, status.yielding_to)
        {
            events.push(event);
        }
        events
    }

    /// Keep [`Self::master`] in step with what a device just claimed.
    ///
    /// A device asserting master takes it; a device that was master and is no
    /// longer gives it up. Only the claim is authoritative — there is no packet
    /// that says "nobody is master" — so losing it is inferred from the
    /// previous holder's own status and from nothing else.
    ///
    /// The exception is a handoff. For the one or two packets between agreeing
    /// to hand over and dropping its claim, the outgoing master reports itself
    /// master *and* names its successor in byte `0x9f`, so both decks claim it
    /// at once. Taking that byte as the answer makes the transition atomic:
    /// without it, whether a listener sees a spurious "nobody is master" in
    /// between depends on which of two packets 14 ms apart it happens to
    /// process first.
    fn settle_master(
        &mut self,
        device: DeviceNumber,
        claims: bool,
        yielding_to: Option<DeviceNumber>,
    ) -> Option<MonitorEvent> {
        let was = self.master;
        match yielding_to {
            // Only to a device that has actually been heard from, so a stray
            // byte cannot invent a master that is not on the network.
            Some(successor) if self.players.contains_key(&successor) => {
                self.master = Some(successor);
            }
            _ if claims => self.master = Some(device),
            _ if was == Some(device) => self.master = None,
            _ => {}
        }
        (self.master != was).then_some(MonitorEvent::TempoMaster(self.master))
    }

    fn state(&self, device: DeviceNumber, now: Instant) -> Option<PlayerState> {
        let entry = self.players.get(&device)?;
        Some(PlayerState {
            device,
            name: entry.name,
            beat: entry.beat.map(|(beat, at)| BeatObservation {
                received_at: at,
                beat,
                age: now.saturating_duration_since(at),
            }),
            status: entry.status.map(|(status, at)| StatusObservation {
                received_at: at,
                packet_counter: entry.status_order.last.map(|(counter, _)| counter),
                status,
                age: now.saturating_duration_since(at),
            }),
        })
    }

    fn snapshot(&self, now: Instant) -> Vec<PlayerState> {
        self.players
            .keys()
            .filter_map(|device| self.state(*device, now))
            .collect()
    }

    /// Report devices that have stopped beating, and drop ones that have gone
    /// silent altogether.
    fn reap(&mut self, now: Instant) -> Vec<MonitorEvent> {
        let mut events = Vec::new();
        let mut gone = Vec::new();
        for (device, entry) in &mut self.players {
            if entry.beating
                && entry
                    .beat
                    .is_none_or(|(_, at)| now.saturating_duration_since(at) > BEAT_STALE_AFTER)
            {
                entry.beating = false;
                events.push(MonitorEvent::Stopped(*device));
            }
            if entry
                .last_heard()
                .is_none_or(|at| now.saturating_duration_since(at) > FORGET_AFTER)
            {
                gone.push(*device);
            }
        }
        for device in gone {
            self.players.remove(&device);
            if self.master == Some(device) {
                self.master = None;
                events.push(MonitorEvent::TempoMaster(None));
            }
            events.push(MonitorEvent::Gone(device));
        }
        events
    }
}

/// The loaded track, or `None` when the packet says nothing is loaded.
///
/// "Nothing loaded" is a source player of `0`, which [`CdjStatus::source_player`]
/// already reports as `None`; a zero track id with a real source player is a
/// deck that has a medium selected and no track chosen, which is the same
/// answer.
fn loaded_track(packet: &CdjStatus) -> Option<LoadedTrack> {
    let source_player = packet.source_player()?;
    let id = packet.track_id();
    let kind = TrackKind(packet.track_type());
    if id == 0 && kind == TrackKind::NONE {
        return None;
    }
    Some(LoadedTrack {
        source_player,
        slot: packet.source_slot(),
        id,
        kind,
    })
}

// -- the monitor ----------------------------------------------------------

/// A live view of the players on the network.
///
/// Dropping it stops the listeners.
#[derive(Debug)]
pub struct Monitor {
    interface: Interface,
    players: Arc<Mutex<PlayerTable>>,
    events: broadcast::Sender<MonitorEvent>,
    watches_status: bool,
    status_sender: Option<Arc<UdpSocket>>,
    status_packets: broadcast::Sender<(SocketAddr, Arc<Vec<u8>>)>,
    tasks: Vec<JoinHandle<()>>,
}

impl Monitor {
    /// Watch beats on UDP 50001. **Transmits nothing.**
    ///
    /// Tempo, pitch and bar position for every playing deck, from a host that
    /// has announced nothing. What this cannot see is the loaded track, the
    /// play state and the tempo master — for those, see [`Self::with_status`].
    pub async fn start(interface: Interface) -> Result<Self> {
        Self::build(interface, None).await
    }

    /// Watch beats on 50001 *and* status on 50002.
    ///
    /// The [`VirtualCdj`] is not decoration: status is unicast to peers that
    /// have announced themselves (F21), so without one announcing, 50002 stays
    /// silent however long it is listened to. Taking it by reference puts that
    /// requirement in the type rather than in a comment.
    ///
    /// The virtual CDJ should be configured with `emit_status: false` — see the
    /// module documentation for why a serving virtual CDJ and a monitor cannot
    /// share port 50002.
    /// Watch beats and status alongside a virtual CDJ.
    ///
    /// When that CDJ emits status it already holds UDP 50002, and this reads
    /// its tap rather than binding a second socket — see
    /// [`VirtualCdj::status_datagrams`]. Binding anyway would split the
    /// unicast stream arbitrarily between the two sockets and silently lose
    /// half of every deck's status.
    pub async fn with_status(interface: Interface, announcing: &VirtualCdj) -> Result<Self> {
        match announcing.status_datagrams() {
            Some(tap) => Self::build_shared(interface, announcing.number(), tap).await,
            None => Self::build(interface, Some(announcing.number())).await,
        }
    }

    /// Like [`Monitor::build`], but reading status from someone else's socket.
    #[expect(
        clippy::unused_async,
        reason = "spawns tasks, so it needs a tokio runtime; matches build's signature"
    )]
    async fn build_shared(
        interface: Interface,
        ours: DeviceNumber,
        tap: broadcast::Receiver<Arc<Vec<u8>>>,
    ) -> Result<Self> {
        let mut monitor = Self::empty(interface, true);
        let beats = socket::bind(BEAT_PORT, Some(&monitor.interface))?;
        monitor.tasks.push(monitor.spawn_beats(beats));
        monitor.tasks.push(monitor.spawn_tapped_status(tap, ours));
        monitor.tasks.push(monitor.spawn_reaper());
        Ok(monitor)
    }

    #[expect(
        clippy::unused_async,
        reason = "spawns tasks, so it needs a tokio runtime; async is how that is documented \
                  at the call site, and keeps the signature stable if setup later awaits"
    )]
    async fn build(interface: Interface, ours: Option<DeviceNumber>) -> Result<Self> {
        let mut monitor = Self::empty(interface, ours.is_some());
        let beats = socket::bind(BEAT_PORT, Some(&monitor.interface))?;
        monitor.tasks.push(monitor.spawn_beats(beats));
        if let Some(ours) = ours {
            let status = Arc::new(socket::bind(STATUS_PORT, Some(&monitor.interface))?);
            monitor.status_sender = Some(status.clone());
            monitor.tasks.push(monitor.spawn_status(status, ours));
        }
        monitor.tasks.push(monitor.spawn_reaper());
        Ok(monitor)
    }

    /// The parts every constructor shares, with no tasks started yet.
    fn empty(interface: Interface, watches_status: bool) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            interface,
            players: Arc::new(Mutex::new(PlayerTable::default())),
            events,
            watches_status,
            status_sender: None,
            status_packets: broadcast::channel(256).0,
            tasks: Vec::new(),
        }
    }

    /// Clone the already-bound status socket for explicit application commands.
    /// The monitor remains its sole receiver. Unavailable when using a shared tap.
    pub fn status_sender(&self) -> Option<Arc<UdpSocket>> {
        self.status_sender.clone()
    }

    /// Bounded tap of the existing status receiver, including the source endpoint.
    /// Consumers must never receive directly from `status_sender`.
    pub fn status_packets(&self) -> broadcast::Receiver<(SocketAddr, Arc<Vec<u8>>)> {
        self.status_packets.subscribe()
    }

    /// The interface being listened on.
    pub fn interface(&self) -> &Interface {
        &self.interface
    }

    /// Whether the 50002 half is running.
    ///
    /// `false` means the loaded track, the play state and the tempo master are
    /// permanently unknown — not absent, unknown — and a display should say so
    /// rather than show blanks.
    pub fn watches_status(&self) -> bool {
        self.watches_status
    }

    /// Every device heard from, ordered by number.
    pub fn players(&self) -> Vec<PlayerState> {
        let now = Instant::now();
        self.with_table(|table| table.snapshot(now))
    }

    /// One device, or `None` if it has said nothing.
    pub fn player(&self, device: DeviceNumber) -> Option<PlayerState> {
        let now = Instant::now();
        self.with_table(|table| table.state(device, now))
    }

    /// Who holds tempo master.
    ///
    /// Always `None` without [`Self::watches_status`], because mastership is
    /// published only on 50002.
    pub fn tempo_master(&self) -> Option<DeviceNumber> {
        self.with_table(|table| table.master)
    }

    /// Subscribe to changes.
    pub fn subscribe(&self) -> broadcast::Receiver<MonitorEvent> {
        self.events.subscribe()
    }

    fn with_table<T>(&self, read: impl FnOnce(&PlayerTable) -> T) -> T {
        match self.players.lock() {
            Ok(table) => read(&table),
            // The table holds no invariant a panic mid-update could break —
            // every mutation is a single assignment — so recovering beats
            // propagating a panic into a DJ's set.
            Err(poisoned) => read(&poisoned.into_inner()),
        }
    }

    fn spawn_beats(&self, socket: UdpSocket) -> JoinHandle<()> {
        let players = Arc::clone(&self.players);
        let events = self.events.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; MAX_DATAGRAM];
            loop {
                let Some(datagram) = receive(&socket, &mut buffer, "beat").await else {
                    return;
                };
                let decoded = match beat::decode(&datagram) {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        trace!(%error, bytes = datagram.len(), "undecodable datagram on 50001");
                        continue;
                    }
                };
                let beat = match decoded {
                    beat::Packet::Beat(beat) => beat,
                    // Somebody pressed MASTER. Unicast at whoever holds it,
                    // which is why it is worth an event rather than a shrug:
                    // if that is us, we have to answer and stand down, and this
                    // socket is the only one that can hear it — the beat port
                    // is shared with `SO_REUSEPORT` and only one member of the
                    // group receives a given unicast datagram.
                    beat::Packet::MasterRequest(request) => {
                        let _ = events.send(MonitorEvent::MasterRequested(request.device));
                        continue;
                    }
                    other => {
                        // On-air and fader-start from a mixer, or a CDJ-3000's
                        // absolute position. Nothing here models them yet, and
                        // saying so at trace level beats a warning per packet.
                        trace!(kind = ?other.kind(), "unmodelled datagram on 50001");
                        continue;
                    }
                };
                let now = Instant::now();
                let event = with_table_mut(&players, |table| table.observe_beat(&beat, now));
                let _ = events.send(event);
            }
        })
    }

    fn spawn_status(&self, socket: Arc<UdpSocket>, ours: DeviceNumber) -> JoinHandle<()> {
        let players = Arc::clone(&self.players);
        let events = self.events.clone();
        let packets = self.status_packets.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; MAX_DATAGRAM];
            loop {
                let (len, from) = match socket.recv_from(&mut buffer).await {
                    Ok(received) => received,
                    Err(error) => {
                        warn!(%error, "status socket closed");
                        return;
                    }
                };
                if !from.is_ipv4() {
                    continue;
                }
                let datagram = buffer[..len].to_vec();
                record_diagnostic_datagram("status", from, &datagram);
                observe_status_datagram(&players, &events, &datagram, ours);
                let _ = packets.send((from, Arc::new(datagram)));
            }
        })
    }

    /// The status half, reading datagrams somebody else received.
    fn spawn_tapped_status(
        &self,
        mut tap: broadcast::Receiver<Arc<Vec<u8>>>,
        ours: DeviceNumber,
    ) -> JoinHandle<()> {
        let players = Arc::clone(&self.players);
        let events = self.events.clone();
        tokio::spawn(async move {
            loop {
                let datagram = match tap.recv().await {
                    Ok(datagram) => datagram,
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        // The picture is stale rather than wrong: the next
                        // status packet from each player is 200 ms away and
                        // carries the whole state, so nothing has to be
                        // reconstructed.
                        warn!(missed, "fell behind the status tap");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                observe_status_datagram(&players, &events, &datagram, ours);
            }
        })
    }

    fn spawn_reaper(&self) -> JoinHandle<()> {
        let players = Arc::clone(&self.players);
        let events = self.events.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(REAP_INTERVAL);
            loop {
                ticker.tick().await;
                let now = Instant::now();
                for event in with_table_mut(&players, |table| table.reap(now)) {
                    debug!(?event, "player went quiet");
                    let _ = events.send(event);
                }
            }
        })
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Fold one datagram from UDP 50002 into the player table.
///
/// Shared by the two status tasks, which differ only in where the bytes come
/// from — our own socket, or the tap of a virtual CDJ that holds the port.
fn observe_status_datagram(
    players: &Arc<Mutex<PlayerTable>>,
    events: &broadcast::Sender<MonitorEvent>,
    datagram: &[u8],
    ours: DeviceNumber,
) {
    let Ok(status::Packet::CdjStatus(packet)) = status::decode(datagram) else {
        return;
    };
    // Our own status, echoed back by a switch or reflected by a peer, would
    // show up as a player claiming our number.
    if packet.sender() == Some(ours) {
        return;
    }
    let now = Instant::now();
    for event in with_table_mut(players, |table| table.observe_status(&packet, now)) {
        debug!(?event, "player status changed");
        let _ = events.send(event);
    }
}

/// Receive one datagram, or `None` once the socket is closed.
async fn receive(socket: &UdpSocket, buffer: &mut [u8], port: &'static str) -> Option<Vec<u8>> {
    loop {
        let (len, from) = match socket.recv_from(buffer).await {
            Ok(received) => received,
            Err(error) => {
                warn!(%error, port, "socket closed");
                return None;
            }
        };
        if !matches!(from, SocketAddr::V4(_)) {
            continue;
        }
        if let Some(datagram) = buffer.get(..len) {
            record_diagnostic_datagram(port, from, datagram);
            return Some(datagram.to_vec());
        }
    }
}

// Opt-in research capture of packets this monitor already receives. No extra
// sockets or messages; bounded queue keeps disk writes off the receive task.
fn record_diagnostic_datagram(port: &str, from: SocketAddr, data: &[u8]) {
    use std::io::Write;
    use std::sync::{OnceLock, mpsc};
    static RECORDER: OnceLock<Option<mpsc::SyncSender<String>>> = OnceLock::new();
    let recorder = RECORDER.get_or_init(|| {
        let path = std::env::var_os("PIONEER_COMPANION_PACKET_LOG")?;
        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(file) => file,
            Err(error) => {
                eprintln!("Packet recorder could not open file: {error}");
                return None;
            }
        };
        let (sender, receiver) = mpsc::sync_channel::<String>(4096);
        std::thread::spawn(move || {
            while let Ok(line) = receiver.recv() {
                if writeln!(file, "{line}").is_err() {
                    break;
                }
            }
        });
        Some(sender)
    });
    if let Some(recorder) = recorder {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |t| t.as_micros());
        let hex: String = data.iter().map(|byte| format!("{byte:02x}")).collect();
        if let Err(error) = recorder.try_send(format!("{time} {port} {from} {hex}")) {
            eprintln!("Packet recorder dropped a packet: {error}");
        }
    }
}

fn with_table_mut<T>(table: &Mutex<PlayerTable>, write: impl FnOnce(&mut PlayerTable) -> T) -> T {
    match table.lock() {
        Ok(mut table) => write(&mut table),
        Err(poisoned) => write(&mut poisoned.into_inner()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prolink_proto::beat::{Pitch, Timings};

    #[tokio::test]
    async fn status_tap_preserves_endpoint_and_does_not_consume_monitor_packets() {
        let interface = Interface {
            name: "test".into(),
            index: 1,
            ip: [127, 0, 0, 1].into(),
            netmask: [255, 0, 0, 0].into(),
            mac: prolink_proto::MacAddress([1, 2, 3, 4, 5, 6]),
        };
        let mut monitor = Monitor::empty(interface, true);
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut tap = monitor.status_packets();
        let mut second = monitor.status_packets();
        monitor
            .tasks
            .push(monitor.spawn_status(socket.clone(), device(15)));
        let raw = status_from(1, 5, true, 182).into_bytes();
        peer.send_to(&raw, socket.local_addr().unwrap())
            .await
            .unwrap();
        let (from, received) = tokio::time::timeout(Duration::from_secs(1), tap.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(from, peer.local_addr().unwrap());
        assert_eq!(*received, raw);
        assert_eq!(*second.recv().await.unwrap().1, raw);
        assert!(monitor.player(device(1)).and_then(|p| p.status).is_some());
        // Non-status requests must reach the serving tap as well.
        peer.send_to(b"media-request", socket.local_addr().unwrap())
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), tap.recv())
                .await
                .unwrap()
                .unwrap()
                .1
                .as_slice(),
            b"media-request"
        );
    }

    #[test]
    fn cue_audition_advances_with_playing_flag_clear_then_stops_on_release() {
        let mut table = PlayerTable::default();
        let now = Instant::now();
        for (play, expected) in [(7, true), (6, false), (4, false), (5, false)] {
            let mut raw = status_from(1, play, true, 182).into_bytes();
            raw[0x89] &= !0x40;
            let packet = CdjStatus::parse(&raw).unwrap();
            table.observe_status(&packet, now);
            assert_eq!(
                table
                    .state(device(1), now)
                    .unwrap()
                    .status
                    .unwrap()
                    .status
                    .is_playing,
                expected
            );
        }
    }

    fn device(number: u8) -> DeviceNumber {
        DeviceNumber::new(number).expect("a non-zero device number")
    }

    /// A beat packet at 145.00 BPM with the fader centred: a 413.79 ms beat.
    fn beat_at(number: u8, position: u8) -> Beat {
        Beat {
            name: DeviceName::new("CDJ-2000nexus"),
            device: device(number),
            timings: Timings {
                next_beat: Some(Duration::from_millis(414)),
                second_beat: Some(Duration::from_millis(828)),
                next_bar: Some(Duration::from_millis(414)),
                fourth_beat: Some(Duration::from_millis(1655)),
                second_bar: Some(Duration::from_millis(2069)),
                eighth_beat: Some(Duration::from_millis(3310)),
            },
            pitch: Pitch::UNITY,
            bpm_centi: 14500,
            beat_in_bar: BeatInBar::new(position),
            scratching: false,
        }
    }

    /// A status packet with the fields this crate models substituted into a
    /// real deck's skeleton.
    fn status_from(number: u8, play: u8, master: bool, track_id: u32) -> CdjStatus {
        let mut raw = CdjStatus::builder()
            .device_number(device(number))
            .name(DeviceName::new("CDJ-2000nexus"))
            .play_state(play)
            .build()
            .into_bytes();
        raw[0x28] = number; // source player
        raw[0x29] = Slot::USB.0;
        raw[0x2a] = TrackKind::REKORDBOX.0;
        raw[0x2c..0x30].copy_from_slice(&track_id.to_be_bytes());
        raw[0x92..0x94].copy_from_slice(&14500u16.to_be_bytes());
        raw[0x9e] = u8::from(master);
        CdjStatus::parse(&raw).expect("a status packet")
    }

    #[test]
    fn a_tapped_datagram_lands_in_the_table_unless_it_is_our_own() {
        // The path a session takes when a virtual CDJ holds UDP 50002: the
        // bytes arrive from its tap rather than from a socket of ours, and
        // everything downstream has to behave as though they had not.
        let players = Arc::new(Mutex::new(PlayerTable::default()));
        let (events, mut heard) = broadcast::channel(16);
        let ours = device(4);

        let foreign = status_from(2, PlayState::PLAYING.0, true, 0x1234).into_bytes();
        observe_status_datagram(&players, &events, &foreign, ours);
        let now = Instant::now();
        let state = with_table_mut(&players, |table| table.state(device(2), now))
            .expect("the deck is in the table");
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|seen| seen.status.is_tempo_master),
            "device 2 said it was the tempo master"
        );
        assert!(heard.try_recv().is_ok(), "and that was worth an event");

        // Our own status, reflected back to us. A device claiming our number
        // is us, and adding ourselves would show Mixxx as a player.
        let echo = status_from(ours.get(), PlayState::PLAYING.0, false, 0).into_bytes();
        observe_status_datagram(&players, &events, &echo, ours);
        assert!(
            with_table_mut(&players, |table| table.state(ours, now)).is_none(),
            "our own echo must not become a player"
        );
    }

    #[test]
    fn a_beat_packet_anchors_the_phase_at_the_beat() {
        let mut table = PlayerTable::default();
        let now = Instant::now();
        table.observe_beat(&beat_at(2, 1), now);
        let state = table.state(device(2), now).expect("a player");
        let phase = state.beat_phase().expect("a phase");
        assert!(phase.abs() < 1e-9, "on the beat the phase is zero: {phase}");
        assert!(state.bar_phase().expect("a bar phase").abs() < 1e-9);
        assert_eq!(state.beat_in_bar().map(BeatInBar::get), Some(1));
    }

    #[test]
    fn the_phase_advances_with_the_clock_and_clamps_rather_than_wrapping() {
        // 145 BPM is a 413.79 ms beat, so half a beat is 207 ms.
        let mut table = PlayerTable::default();
        let start = Instant::now();
        table.observe_beat(&beat_at(2, 1), start);

        let half = table
            .state(device(2), start + Duration::from_millis(207))
            .and_then(|state| state.beat_phase())
            .expect("a phase");
        assert!((half - 0.5).abs() < 0.005, "half a beat in: {half}");

        // Past the next beat with no packet to re-anchor it, the estimate sits
        // at the end of the beat instead of spinning round again.
        let late = table
            .state(device(2), start + Duration::from_millis(1200))
            .and_then(|state| state.beat_phase())
            .expect("a phase");
        assert!(
            (late - 1.0).abs() < 1e-9,
            "clamped, not wrapped, so a stopped deck cannot look alive: {late}"
        );
    }

    #[test]
    fn the_bar_phase_walks_the_four_beats() {
        let mut table = PlayerTable::default();
        let now = Instant::now();
        for (position, expected) in [(1u8, 0.0), (2, 0.25), (3, 0.5), (4, 0.75)] {
            table.observe_beat(&beat_at(2, position), now);
            let phase = table
                .state(device(2), now)
                .and_then(|state| state.bar_phase())
                .expect("a bar phase");
            assert!(
                (phase - expected).abs() < 1e-9,
                "beat {position} of 4 is {expected} of the bar, got {phase}"
            );
        }
    }

    #[test]
    fn a_player_silent_for_three_beats_has_a_stale_phase_not_an_old_one() {
        let mut table = PlayerTable::default();
        let start = Instant::now();
        table.observe_beat(&beat_at(2, 1), start);
        let state = table
            .state(
                device(2),
                start + BEAT_STALE_AFTER + Duration::from_millis(1),
            )
            .expect("a player");
        assert!(state.beat_phase().is_none(), "the phase is meaningless");
        assert!(state.bar_phase().is_none());
        assert!(!state.is_beating());
        // ...but the tempo it stopped at is still worth showing.
        assert!(
            state
                .effective_bpm()
                .is_some_and(|bpm| (bpm - 145.0).abs() < 0.01)
        );
    }

    #[test]
    fn a_player_with_no_rekordbox_track_has_no_bar_to_be_in() {
        // Byte 0x5c is 0, which is not beat zero.
        let mut beat = beat_at(2, 1);
        beat.beat_in_bar = None;
        let mut table = PlayerTable::default();
        let now = Instant::now();
        table.observe_beat(&beat, now);
        let state = table.state(device(2), now).expect("a player");
        assert!(state.beat_phase().is_some(), "the beat is still placed");
        assert!(state.bar_phase().is_none(), "but the bar is not");
    }

    #[test]
    fn the_effective_tempo_follows_the_pitch_fader() {
        let mut beat = beat_at(2, 1);
        beat.pitch = Pitch(0x000f_8312); // −3.05%
        let mut table = PlayerTable::default();
        let now = Instant::now();
        table.observe_beat(&beat, now);
        let state = table.state(device(2), now).expect("a player");
        let bpm = state.effective_bpm().expect("a tempo");
        assert!(
            (bpm - 140.577).abs() < 0.01,
            "145.00 at −3.05% is 140.58, not 145.00: {bpm}"
        );
        // ...and the phase is paced by the effective tempo, not the track's.
        let phase = table
            .state(device(2), now + Duration::from_millis(213))
            .and_then(|state| state.beat_phase())
            .expect("a phase");
        assert!(
            (phase - 0.5).abs() < 0.01,
            "half of a 426.8 ms beat: {phase}"
        );
    }

    #[test]
    fn without_status_the_master_is_unknown_rather_than_absent() {
        // The distinction a passive listener must not blur: nobody being
        // reported master is not the same as nobody being master.
        let mut table = PlayerTable::default();
        let now = Instant::now();
        table.observe_beat(&beat_at(2, 1), now);
        let state = table.state(device(2), now).expect("a player");
        assert_eq!(state.is_tempo_master(), None);
        assert_eq!(state.play_state(), None);
        assert_eq!(state.track(), None);
        assert_eq!(table.master, None);
    }

    #[test]
    fn a_status_packet_names_the_track_and_the_master() {
        let mut table = PlayerTable::default();
        let now = Instant::now();
        let events = table.observe_status(&status_from(3, PlayState::PLAYING.0, true, 182), now);
        assert!(matches!(
            events.as_slice(),
            [MonitorEvent::Status(_), MonitorEvent::TempoMaster(Some(_))]
        ));
        let state = table.state(device(3), now).expect("a player");
        assert_eq!(state.is_tempo_master(), Some(true));
        assert_eq!(state.play_state(), Some(PlayState::PLAYING));
        assert!(state.play_state().is_some_and(PlayState::is_playing));
        let track = state.track().expect("a loaded track");
        assert_eq!(track.id, 182);
        assert_eq!(track.slot, Slot::USB);
        assert_eq!(track.kind, TrackKind::REKORDBOX);
        assert_eq!(track.source_player, device(3));
        assert_eq!(table.master, Some(device(3)));
    }

    #[test]
    fn an_unchanged_status_is_not_an_event() {
        // A deck sends five a second and almost never changes anything.
        let mut table = PlayerTable::default();
        let now = Instant::now();
        let packet = status_from(3, PlayState::PLAYING.0, false, 182);
        assert_eq!(table.observe_status(&packet, now).len(), 1);
        assert!(
            table
                .observe_status(&packet, now + Duration::from_millis(200))
                .is_empty()
        );
    }

    #[test]
    fn nothing_loaded_is_no_track_rather_than_track_zero() {
        let mut table = PlayerTable::default();
        let now = Instant::now();
        let mut raw = status_from(3, PlayState::NO_TRACK.0, false, 0).into_bytes();
        raw[0x28] = 0; // no source player
        raw[0x29] = Slot::NONE.0;
        raw[0x2a] = TrackKind::NONE.0;
        let packet = CdjStatus::parse(&raw).expect("a status packet");
        table.observe_status(&packet, now);
        assert_eq!(table.state(device(3), now).and_then(|s| s.track()), None);
    }

    #[test]
    fn the_master_moves_when_another_deck_claims_it() {
        let mut table = PlayerTable::default();
        let now = Instant::now();
        table.observe_status(&status_from(1, PlayState::PLAYING.0, true, 10), now);
        assert_eq!(table.master, Some(device(1)));

        let events = table.observe_status(&status_from(2, PlayState::PLAYING.0, true, 20), now);
        assert!(events.contains(&MonitorEvent::TempoMaster(Some(device(2)))));
        assert_eq!(table.master, Some(device(2)));

        // The old master dropping the flag must not take the new one with it.
        let events = table.observe_status(&status_from(1, PlayState::PLAYING.0, false, 10), now);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, MonitorEvent::TempoMaster(_))),
            "device 1 giving up a role it no longer holds changes nothing"
        );
        assert_eq!(table.master, Some(device(2)));
    }

    #[test]
    fn the_master_is_lost_when_the_holder_gives_it_up() {
        let mut table = PlayerTable::default();
        let now = Instant::now();
        table.observe_status(&status_from(1, PlayState::PLAYING.0, true, 10), now);
        let events = table.observe_status(&status_from(1, PlayState::PAUSED.0, false, 10), now);
        assert!(events.contains(&MonitorEvent::TempoMaster(None)));
        assert_eq!(table.master, None);
    }

    #[test]
    fn a_deck_that_stops_beating_is_reported_once() {
        let mut table = PlayerTable::default();
        let start = Instant::now();
        table.observe_beat(&beat_at(2, 1), start);
        assert!(
            table.reap(start + Duration::from_millis(500)).is_empty(),
            "still beating"
        );
        let stale = start + BEAT_STALE_AFTER + Duration::from_millis(1);
        assert_eq!(table.reap(stale), vec![MonitorEvent::Stopped(device(2))]);
        assert!(
            table.reap(stale + Duration::from_millis(250)).is_empty(),
            "and not again at every tick"
        );
    }

    #[test]
    fn a_device_that_says_nothing_at_all_is_forgotten_and_gives_up_the_master() {
        let mut table = PlayerTable::default();
        let start = Instant::now();
        table.observe_status(&status_from(4, PlayState::PLAYING.0, true, 7), start);
        assert_eq!(table.master, Some(device(4)));

        let events = table.reap(start + FORGET_AFTER + Duration::from_secs(1));
        assert!(events.contains(&MonitorEvent::Gone(device(4))));
        assert!(events.contains(&MonitorEvent::TempoMaster(None)));
        assert!(table.snapshot(start).is_empty());
        assert_eq!(table.master, None);
    }

    /// The outgoing master mid-handoff: still claiming, but naming its
    /// successor at `0x9f`.
    fn yielding_status(number: u8, to: u8) -> CdjStatus {
        let mut raw = status_from(number, PlayState::PLAYING.0, true, 182).into_bytes();
        raw[0x9f] = to;
        CdjStatus::parse(&raw).expect("a status packet")
    }

    #[test]
    fn a_master_handoff_moves_mastership_exactly_once() {
        // Both decks report themselves master for the packet or two between the
        // outgoing master agreeing to hand over and dropping its claim. Byte
        // 0x9f is what breaks the tie, and it has to break it the same way
        // whichever order those packets are processed in — on the wire they are
        // 14 ms apart (F52).
        for reversed in [false, true] {
            let mut table = PlayerTable::default();
            let now = Instant::now();
            table.observe_status(&status_from(2, PlayState::PLAYING.0, true, 182), now);
            table.observe_status(&status_from(1, PlayState::PLAYING.0, false, 7), now);
            assert_eq!(table.master, Some(device(2)), "device 2 starts as master");

            let claiming = status_from(1, PlayState::PLAYING.0, true, 7);
            let dropped = status_from(2, PlayState::PLAYING.0, false, 182);
            let mut events = table.observe_status(&yielding_status(2, 1), now);
            let rest = if reversed {
                [dropped, claiming]
            } else {
                [claiming, dropped]
            };
            for packet in &rest {
                events.extend(table.observe_status(packet, now));
            }

            let handovers: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    MonitorEvent::TempoMaster(master) => Some(*master),
                    _ => None,
                })
                .collect();
            assert_eq!(
                handovers,
                vec![Some(device(1))],
                "mastership moved straight from 2 to 1 (reversed: {reversed})"
            );
            assert_eq!(table.master, Some(device(1)));
        }
    }

    #[test]
    fn a_deck_yielding_to_a_device_that_is_not_here_is_ignored() {
        // The successor has to be a device we have heard from, so a stray byte
        // cannot invent a master that is not on the network.
        let mut table = PlayerTable::default();
        let now = Instant::now();
        table.observe_status(&status_from(2, PlayState::PLAYING.0, true, 182), now);
        table.observe_status(&yielding_status(2, 4), now);
        assert_eq!(table.master, Some(device(2)));
    }

    #[test]
    fn the_two_ports_describe_one_player() {
        let mut table = PlayerTable::default();
        let now = Instant::now();
        table.observe_beat(&beat_at(2, 3), now);
        table.observe_status(&status_from(2, PlayState::PLAYING.0, true, 182), now);
        let state = table.state(device(2), now).expect("a player");
        assert_eq!(state.name.as_str(), "CDJ-2000nexus");
        assert!(state.effective_bpm().is_some(), "tempo from 50001");
        assert_eq!(state.is_tempo_master(), Some(true), "master from 50002");
        assert_eq!(state.track().map(|track| track.id), Some(182));
        assert_eq!(state.beat_in_bar().map(BeatInBar::get), Some(3));
        assert_eq!(table.snapshot(now).len(), 1, "one player, not two");
    }

    #[test]
    fn play_states_are_named_where_we_have_seen_them() {
        for (state, name) in [
            (PlayState::NO_TRACK, "no track"),
            (PlayState::PLAYING, "playing"),
            (PlayState::PAUSED, "paused"),
            (PlayState::EMERGENCY_LOOP, "emergency loop"),
        ] {
            assert_eq!(state.name(), Some(name));
        }
        assert_eq!(PlayState(0x7f).name(), None);
        assert_eq!(format!("{}", PlayState(0x7f)), "0x7f");
        assert!(!PlayState::PAUSED.is_playing());
        assert!(PlayState::LOOPING.is_playing());
    }
}
