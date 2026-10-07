// SPDX-License-Identifier: GPL-3.0-only

//! Beats, tempo and mixer control — UDP 50001, broadcast.
//!
//! The half of Pro DJ Link that a passive listener *can* see. Unlike UDP 50002,
//! everything here is broadcast, so a host that has never announced itself
//! learns every playing deck's tempo, its pitch and its position in the bar.
//! What it cannot learn from this port is **who is tempo master**: that is
//! published only in a status packet, and status is unicast to announced peers
//! (F21). The two halves are complementary and neither is sufficient — see
//! [`crate::status`].
//!
//! # A packet is an event, not a report
//!
//! A beat packet arrives **on** the beat. Its arrival *is* the player saying "I
//! am starting a beat now", which is why phase between packets is extrapolated
//! from the tempo and re-anchored by the next arrival, and why there is no
//! cadence to speak of: at 145 BPM a deck sends one every 414 ms and at a
//! standstill it sends none at all.
//!
//! A CDJ transmits only **while playing and only for a rekordbox-analysed
//! track**. In this project's corpus that is exact: of 35 016 status packets,
//! not one reporting play state `0x00` (nothing loaded), `0x0e` (spun down) or
//! `0x12` (end of track) had a beat packet from the same deck in the preceding
//! second, while 2299 of 2559 reporting `0x03` (playing) did. A mixer, by
//! contrast, sends them continuously and acts as a backup metronome — so
//! silence means "not playing" for a nexus CDJ and means nothing at all in
//! general.
//!
//! # The timings are quoted at 0% pitch
//!
//! The six millisecond fields describe the track's own grid, **not** what the
//! platter is doing. Confirmed on hardware here rather than merely inherited
//! from the literature: across all 1110 captured beat packets the next-beat
//! field equals `60000 / bpm` to within 0.8 ms, and in one run it held 414 ms
//! unchanged while the pitch field swung between 0.669 and 1.094 as the DJ
//! nudged the jog wheel. Meanwhile the observed interval between consecutive
//! beat packets tracks `60000 / (bpm × pitch)` with a median error of −0.11 ms,
//! against +2.12 ms for the unscaled tempo.
//!
//! So the tempo a deck is *playing* is [`Beat::effective_bpm`], and a consumer
//! that reports [`Beat::bpm`] is wrong by exactly the pitch fader.
//!
//! # The header is the 50002 one
//!
//! Name at `0x0b`–`0x1e`, structural `0x01` at `0x1f` (C14) — not the discovery
//! port's layout. This module shares [`crate::status`]'s header code rather
//! than keeping a second copy; only the byte at `0x0a` differs, and it is
//! port-specific, so `0x06` here is neither a keep-alive nor a media response.
//!
//! ```text
//! 0x00-0x09  magic "Qspt1WmJOL"
//! 0x0a       packet kind
//! 0x0b-0x1e  device name        20 bytes ASCII, NUL-padded
//! 0x1f       structural 0x01
//! 0x20       subtype
//! 0x21       sender device number
//! 0x22-0x23  body length        bytes following 0x24
//! 0x24…      body
//! ```
//!
//! # What is modelled and what is not
//!
//! [`Beat`] is an ordinary struct with an exact encoder, not a captured
//! skeleton, because the beat packet is almost entirely understood: of its 96
//! bytes the only ones we cannot name are 24 bytes of `0xff` filler and two
//! two-byte scratch flags, and all 1110 captured packets re-encode byte for
//! byte from the decoded fields.
//!
//! [`MasterRequest`] and [`MasterResponse`] are the tempo-master handoff, and
//! are modelled the same way for the same reason: 40 and 44 bytes of which only
//! the subtype is unaccounted for. Both are **unicast**, so they reach a
//! capture only through a mirror port or a hub — which is why they appear in
//! one session of this corpus and not in the thirty-two before it (F48).
//!
//! [`ChannelsOnAir`] and [`FaderStart`] are decoded from the pre-hardware
//! literature and **have never been observed**: no mixer took part in any
//! capture in this project's corpus, where every datagram addressed to 50001 is
//! either a beat packet or a master handoff. They are decode-only for that
//! reason. The sync-control kind (`0x2a`) and the CDJ-3000's absolute position
//! packet (`0x0b`) are named in [`BeatKind`] so a log can say what it saw, and
//! are otherwise left alone — `0x2a` has never been seen here because SYNC is
//! toggled on a deck's own front panel rather than over the network.

use std::fmt;
use std::time::Duration;

use crate::device::{DeviceName, DeviceNumber};
use crate::status::{
    OFF_BODY_LEN, OFF_NAME, OFF_SENDER, be_u16_at, be_u32_at, byte_at, check_header,
    write_shared_header,
};
use crate::{Error, MAGIC, Result};

/// Byte `0x0a`, the discriminator on this port.
///
/// A newtype rather than an enum because the type byte is shared with the other
/// two UDP ports while the layouts behind it are not, and because a decoder that
/// refused an unknown value would stop reporting the beats it does understand.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BeatKind(pub u8);

impl BeatKind {
    /// A device starting a beat. The only kind ever seen in this corpus.
    pub const BEAT: Self = Self(0x28);
    /// A mixer's fader-start command. *Literature only; never observed.*
    pub const FADER_START: Self = Self(0x02);
    /// A mixer saying which channels are audible. *Literature only; never
    /// observed.*
    pub const CHANNELS_ON_AIR: Self = Self(0x03);
    /// A CDJ-3000's absolute playhead position, sent every 30 ms whenever a
    /// track is loaded. Named so a log can identify it; not modelled, because
    /// no CDJ-3000 has been on the wire here.
    pub const ABSOLUTE_POSITION: Self = Self(0x0b);
    /// "Turn sync on", "turn sync off" or "become tempo master", unicast at a
    /// target's 50001. Named, not modelled.
    pub const SYNC_CONTROL: Self = Self(0x2a);
    /// A challenger asking the current master to hand over. See
    /// [`MasterRequest`].
    pub const MASTER_REQUEST: Self = Self(0x26);
    /// The outgoing master's answer to a [`Self::MASTER_REQUEST`]. See
    /// [`MasterResponse`].
    pub const MASTER_RESPONSE: Self = Self(0x27);

    /// A name for logs, or `None` for a kind we have no name for.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::BEAT => "beat",
            Self::FADER_START => "fader_start",
            Self::CHANNELS_ON_AIR => "channels_on_air",
            Self::ABSOLUTE_POSITION => "absolute_position",
            Self::SYNC_CONTROL => "sync_control",
            Self::MASTER_REQUEST => "master_request",
            Self::MASTER_RESPONSE => "master_response",
            _ => return None,
        })
    }
}

impl fmt::Debug for BeatKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "BeatKind({:#04x})", self.0),
        }
    }
}

/// One decoded UDP-50001 datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet {
    /// A device starting a beat.
    Beat(Beat),
    /// A mixer publishing which channels are audible.
    ChannelsOnAir(ChannelsOnAir),
    /// A mixer's fader-start command.
    FaderStart(FaderStart),
    /// A player asking the current tempo master to hand over.
    MasterRequest(MasterRequest),
    /// The outgoing master agreeing to hand over.
    MasterResponse(MasterResponse),
    /// A well-formed datagram of a kind this crate does not model.
    Other {
        /// The kind byte at `0x0a`.
        kind: BeatKind,
        /// The whole datagram.
        raw: Vec<u8>,
    },
}

impl Packet {
    /// The kind byte this packet carries.
    pub fn kind(&self) -> BeatKind {
        match self {
            Self::Beat(_) => BeatKind::BEAT,
            Self::ChannelsOnAir(_) => BeatKind::CHANNELS_ON_AIR,
            Self::FaderStart(_) => BeatKind::FADER_START,
            Self::MasterRequest(_) => BeatKind::MASTER_REQUEST,
            Self::MasterResponse(_) => BeatKind::MASTER_RESPONSE,
            Self::Other { kind, .. } => *kind,
        }
    }
}

/// Decode one UDP-50001 datagram.
///
/// A kind we do not model, or one too short for the layout its kind declares,
/// comes back as [`Packet::Other`] rather than as an error: a listener that gave
/// up on the first surprise would stop following the decks it does understand.
pub fn decode(data: &[u8]) -> Result<Packet> {
    if data.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
        let got = data.get(..MAGIC.len()).unwrap_or(data);
        return Err(Error::BadMagic {
            expected: MAGIC.as_slice().into(),
            got: got.into(),
        });
    }
    let Some(&raw_kind) = data.get(MAGIC.len()) else {
        return Err(Error::Truncated {
            need: MAGIC.len() + 1,
            at: 0,
            have: data.len(),
        });
    };
    let kind = BeatKind(raw_kind);
    let other = || Packet::Other {
        kind,
        raw: data.to_vec(),
    };

    Ok(match kind {
        BeatKind::BEAT => Beat::parse(data).map_or_else(|_| other(), Packet::Beat),
        BeatKind::CHANNELS_ON_AIR => {
            ChannelsOnAir::parse(data).map_or_else(|_| other(), Packet::ChannelsOnAir)
        }
        BeatKind::FADER_START => {
            FaderStart::parse(data).map_or_else(|_| other(), Packet::FaderStart)
        }
        BeatKind::MASTER_REQUEST => {
            MasterRequest::parse(data).map_or_else(|_| other(), Packet::MasterRequest)
        }
        BeatKind::MASTER_RESPONSE => {
            MasterResponse::parse(data).map_or_else(|_| other(), Packet::MasterResponse)
        }
        _ => other(),
    })
}

// -- pitch ----------------------------------------------------------------

/// The pitch fader, as the wire encodes it: a 32-bit fixed-point multiplier.
///
/// `0x0010_0000` is 0% — a multiplier of exactly one — so the value is a
/// twentieth-bit fixed point and not a percentage. `0x0000_0000` would be
/// −100% and `0x0020_0000` +100%; the widest excursion in this corpus is
/// `0x000a_b4a2` (−33.1%), from a jog-wheel nudge rather than the fader.
///
/// The same encoding appears in the status packet at `0x8c` and `0x98`, which
/// is why this type lives beside the packet that made it necessary rather than
/// inside it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pitch(pub u32);

impl Pitch {
    /// 0% — the fader at centre.
    pub const UNITY: Self = Self(0x0010_0000);

    /// The multiplier to apply to a tempo. `1.0` at 0%.
    pub fn multiplier(self) -> f64 {
        f64::from(self.0) / f64::from(Self::UNITY.0)
    }

    /// The fader reading a DJ would recognise, in percent. `0.0` at centre.
    pub fn percent(self) -> f64 {
        (self.multiplier() - 1.0) * 100.0
    }
}

impl Default for Pitch {
    fn default() -> Self {
        Self::UNITY
    }
}

impl fmt::Debug for Pitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:+.2}%", self.percent())
    }
}

impl fmt::Display for Pitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

// -- beat in bar ----------------------------------------------------------

/// Which of the four beats of a bar a player is on: 1, 2, 3 or 4.
///
/// Constructed only from 1–4, because the wire's fifth value — `0` — does not
/// mean a fifth beat. It means the player has no rekordbox track and therefore
/// no bar to be in, which is why [`Beat::beat_in_bar`] is an [`Option`] rather
/// than a number that a caller has to remember to test.
///
/// The count is only meaningful as a downbeat when it comes from the tempo
/// master; a follower's `1` is its own idea of the bar.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BeatInBar(u8);

impl BeatInBar {
    /// Beats to a bar. Four, on every device this protocol runs on.
    pub const PER_BAR: u8 = 4;
    /// The first beat of the bar.
    pub const DOWNBEAT: Self = Self(1);

    /// Parse a beat number, rejecting `0` and anything past the fourth beat.
    pub const fn new(raw: u8) -> Option<Self> {
        if raw >= 1 && raw <= Self::PER_BAR {
            Some(Self(raw))
        } else {
            None
        }
    }

    /// The number as it goes on the wire, 1–4.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// The same thing zero-based, 0–3, for arithmetic on bar position.
    pub const fn index(self) -> u8 {
        self.0.saturating_sub(1)
    }

    /// How many beats until the next downbeat: 4 on beat 1, 1 on beat 4.
    ///
    /// Confirmed against the wire rather than assumed: in all 1110 captured
    /// packets the next-bar field equals the next-beat field plus this many
    /// fewer than four beat intervals, to within the packet's own rounding.
    pub const fn beats_to_next_bar(self) -> u8 {
        Self::PER_BAR.saturating_add(1).saturating_sub(self.0)
    }
}

impl fmt::Debug for BeatInBar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "beat {} of {}", self.0, Self::PER_BAR)
    }
}

impl fmt::Display for BeatInBar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// -- beat (0x28) ----------------------------------------------------------

/// How far off six upcoming grid points are, **as if the pitch fader were at
/// 0%**.
///
/// A field is `None` where the wire holds `0xffff_ffff`, which means the track
/// ends before that point is reached.
///
/// The six are not independent: `second_beat` is two beat intervals out,
/// `fourth_beat` four, `eighth_beat` eight, `next_bar` however many beats are
/// left of the current bar and `second_bar` four beats after that. They are
/// carried separately anyway, because that is what the deck sends and because
/// deriving one from another would be a formula standing in for a measurement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    /// To the next beat.
    pub next_beat: Option<Duration>,
    /// To the beat after that.
    pub second_beat: Option<Duration>,
    /// To the next bar line — one to four beats away.
    pub next_bar: Option<Duration>,
    /// To the fourth upcoming beat.
    pub fourth_beat: Option<Duration>,
    /// To the bar line after next — five to eight beats away.
    pub second_bar: Option<Duration>,
    /// To the eighth upcoming beat.
    pub eighth_beat: Option<Duration>,
}

/// A device starting a beat.
///
/// The whole packet, decoded. 96 bytes on the wire in every one of the 1110
/// captured examples, all from CDJ-2000nexus decks on firmware 1.44.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Beat {
    /// `0x0b`–`0x1e`, the sending device's name.
    pub name: DeviceName,
    /// Byte `0x21`, repeated at `0x5f`. A beat from device 0 is refused, since
    /// nothing can be attributed to it.
    pub device: DeviceNumber,
    /// The six grid distances, at 0% pitch.
    pub timings: Timings,
    /// `0x54`–`0x57`, the pitch fader as a multiplier.
    pub pitch: Pitch,
    /// `0x5a`–`0x5b`, the track's own tempo in hundredths of a BPM, before
    /// pitch. Use [`Self::effective_bpm`] for what is actually playing.
    pub bpm_centi: u16,
    /// Byte `0x5c`. `None` when the player has no rekordbox track and so no bar
    /// to be in.
    pub beat_in_bar: Option<BeatInBar>,
    /// `0x58`–`0x59` and `0x5d`–`0x5e`, which hold `0000` at rest and `ffff`
    /// while the platter is being scratched.
    ///
    /// The two words are written as a pair. No scratching packet appears in
    /// this corpus — all 1110 hold `0000` in both — so the `ffff` form is
    /// **untested against hardware**, and a packet in which the two disagreed
    /// would not re-encode byte for byte. None has been observed.
    pub scratching: bool,
}

/// What a timing field holds when the track ends before that grid point.
const NEVER: u32 = 0xffff_ffff;

impl Beat {
    /// Bytes on the wire. Fixed: `len_r` is `0x003c` in all 1110 captured
    /// packets and the layout has no variable part.
    pub const LEN: usize = 0x60;

    const OFF_NEXT_BEAT: usize = 0x24;
    const OFF_SECOND_BEAT: usize = 0x28;
    const OFF_NEXT_BAR: usize = 0x2c;
    const OFF_FOURTH_BEAT: usize = 0x30;
    const OFF_SECOND_BAR: usize = 0x34;
    const OFF_EIGHTH_BEAT: usize = 0x38;
    /// 24 bytes of `0xff` between the timings and the pitch. Exactly that in
    /// all 1110 captured packets, so it is reproduced rather than zeroed —
    /// substituting a plausible zero has broken playback twice elsewhere (F33,
    /// F35).
    const OFF_FILLER: usize = 0x3c;
    const LEN_FILLER: usize = 24;
    const OFF_PITCH: usize = 0x54;
    const OFF_SCRATCH_1: usize = 0x58;
    const OFF_BPM: usize = 0x5a;
    const OFF_BEAT_IN_BAR: usize = 0x5c;
    const OFF_SCRATCH_2: usize = 0x5d;
    /// The device number again. A quirk of subtype-`00` packets; it matched
    /// byte `0x21` in all 1110 captured packets, so it is written rather than
    /// carried as a field that could disagree with the one at `0x21`.
    const OFF_DEVICE_2: usize = 0x5f;

    /// Parse a beat packet, or fail if it is not one.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check_header(data, BeatKind::BEAT.0, Self::LEN)?;
        let device = DeviceNumber::new(byte_at(data, OFF_SENDER).unwrap_or(0))
            .ok_or_else(|| Error::malformed(OFF_SENDER, "beat packet from device 0"))?;
        let interval = |offset: usize| {
            be_u32_at(data, offset)
                .and_then(|raw| (raw != NEVER).then(|| Duration::from_millis(u64::from(raw))))
        };
        Ok(Self {
            name: name_at(data),
            device,
            timings: Timings {
                next_beat: interval(Self::OFF_NEXT_BEAT),
                second_beat: interval(Self::OFF_SECOND_BEAT),
                next_bar: interval(Self::OFF_NEXT_BAR),
                fourth_beat: interval(Self::OFF_FOURTH_BEAT),
                second_bar: interval(Self::OFF_SECOND_BAR),
                eighth_beat: interval(Self::OFF_EIGHTH_BEAT),
            },
            pitch: Pitch(be_u32_at(data, Self::OFF_PITCH).unwrap_or(Pitch::UNITY.0)),
            bpm_centi: be_u16_at(data, Self::OFF_BPM).unwrap_or(0),
            beat_in_bar: BeatInBar::new(byte_at(data, Self::OFF_BEAT_IN_BAR).unwrap_or(0)),
            scratching: be_u16_at(data, Self::OFF_SCRATCH_1).unwrap_or(0) != 0,
        })
    }

    /// Encode this beat as the 96 bytes a deck puts on the wire.
    ///
    /// Exact for anything that came off the wire: all 1110 captured packets
    /// re-encode byte for byte. A [`Timings`] field longer than `u32::MAX`
    /// milliseconds — 49 days, unreachable from the wire — collapses to the
    /// "track ends first" sentinel.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut raw = [0u8; Self::LEN];
        write_shared_header(&mut raw, BeatKind::BEAT.0, self.name, self.device.get());
        {
            let mut put_u32 = |offset: usize, value: u32| {
                if let Some(field) = raw.get_mut(offset..offset.saturating_add(4)) {
                    field.copy_from_slice(&value.to_be_bytes());
                }
            };
            let mut put_interval = |offset: usize, value: Option<Duration>| {
                let millis = value.map_or(NEVER, |duration| {
                    u32::try_from(duration.as_millis()).unwrap_or(NEVER)
                });
                put_u32(offset, millis);
            };
            put_interval(Self::OFF_NEXT_BEAT, self.timings.next_beat);
            put_interval(Self::OFF_SECOND_BEAT, self.timings.second_beat);
            put_interval(Self::OFF_NEXT_BAR, self.timings.next_bar);
            put_interval(Self::OFF_FOURTH_BEAT, self.timings.fourth_beat);
            put_interval(Self::OFF_SECOND_BAR, self.timings.second_bar);
            put_interval(Self::OFF_EIGHTH_BEAT, self.timings.eighth_beat);
            put_u32(Self::OFF_PITCH, self.pitch.0);
        }
        if let Some(field) = raw.get_mut(Self::OFF_FILLER..Self::OFF_FILLER + Self::LEN_FILLER) {
            field.fill(0xff);
        }
        let scratch = if self.scratching { 0xffffu16 } else { 0 };
        for offset in [Self::OFF_SCRATCH_1, Self::OFF_SCRATCH_2] {
            if let Some(field) = raw.get_mut(offset..offset + 2) {
                field.copy_from_slice(&scratch.to_be_bytes());
            }
        }
        if let Some(field) = raw.get_mut(Self::OFF_BPM..Self::OFF_BPM + 2) {
            field.copy_from_slice(&self.bpm_centi.to_be_bytes());
        }
        if let Some(slot) = raw.get_mut(Self::OFF_BEAT_IN_BAR) {
            *slot = self.beat_in_bar.map_or(0, BeatInBar::get);
        }
        if let Some(slot) = raw.get_mut(Self::OFF_DEVICE_2) {
            *slot = self.device.get();
        }
        raw
    }

    /// The track's own tempo, before the pitch fader.
    ///
    /// This is the number printed on the waveform, and it is **not** what is
    /// coming out of the speakers unless the fader is centred.
    pub fn bpm(&self) -> f64 {
        f64::from(self.bpm_centi) / 100.0
    }

    /// The tempo actually playing: the track's tempo with the fader applied.
    pub fn effective_bpm(&self) -> f64 {
        self.bpm() * self.pitch.multiplier()
    }

    /// How long one beat lasts at the effective tempo.
    ///
    /// `None` when the tempo is zero or the packet carries no usable BPM, which
    /// is the case a phase estimate has to refuse rather than divide by.
    pub fn beat_interval(&self) -> Option<Duration> {
        let bpm = self.effective_bpm();
        if bpm <= 0.0 {
            return None;
        }
        Duration::try_from_secs_f64(60.0 / bpm).ok()
    }
}

impl fmt::Debug for Beat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Beat")
            .field("device", &self.device)
            .field("name", &self.name)
            .field("bpm", &self.bpm())
            .field("pitch", &self.pitch)
            .field("effective_bpm", &self.effective_bpm())
            .field("beat_in_bar", &self.beat_in_bar)
            .field("next_beat", &self.timings.next_beat)
            .finish_non_exhaustive()
    }
}

// -- channels on air (0x03) -----------------------------------------------

/// Which of a mixer's channels are audible.
///
/// **Never observed.** No mixer took part in any capture in this project's
/// corpus — all 1110 datagrams addressed to 50001 there are beat packets — so
/// this layout is the pre-hardware literature's and is untested against
/// hardware. Decode-only for the same reason: a CDJ believes it is on air
/// because a mixer said so, and synthesising that claim is a thing to do
/// deliberately, not a thing a codec should make easy.
///
/// A channel is off air when the crossfader, the channel fader, the trim, a
/// filter, the source switch or the master level has silenced it — so "off air"
/// is a statement about audibility and not about playback.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChannelsOnAir {
    /// The mixer's name.
    pub name: DeviceName,
    /// Byte `0x21`. A mixer announces itself as `0x21`, which is a legitimate
    /// device number.
    pub sender: Option<DeviceNumber>,
    on_air: [bool; Self::MAX_CHANNELS],
    channels: usize,
}

impl ChannelsOnAir {
    /// The most channels the six-channel form describes.
    pub const MAX_CHANNELS: usize = 6;
    /// Bytes on the wire in the four-channel form, as a DJM-2000nexus sends it:
    /// subtype `0x00`, body length `0x0009`.
    pub const FOUR_CHANNEL_LEN: usize = 0x2d;
    /// Bytes on the wire in the six-channel form, as a DJM-V10 sends it:
    /// subtype `0x03`, body length `0x0011`.
    pub const SIX_CHANNEL_LEN: usize = 0x35;

    const FOUR_CHANNEL_BODY: u16 = 0x0009;
    const SIX_CHANNEL_BODY: u16 = 0x0011;
    const OFF_FIRST: usize = 0x24;
    /// Channels 5 and 6 sit past five pad bytes in the six-channel form.
    const OFF_FIFTH: usize = 0x2d;

    /// Parse an on-air packet in either form.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check_header(data, BeatKind::CHANNELS_ON_AIR.0, Self::FOUR_CHANNEL_LEN)?;
        // The two forms are told apart by the body length they declare, which
        // is the field that actually says how many bytes follow; the subtype
        // (`0x00` against `0x03`) agrees with it in the literature. A third
        // length is a form nobody has described, so it is refused here and
        // survives as [`Packet::Other`] with its bytes intact rather than being
        // read as one of the two we know.
        let body = be_u16_at(data, OFF_BODY_LEN).unwrap_or(0);
        let six = match body {
            Self::FOUR_CHANNEL_BODY => false,
            Self::SIX_CHANNEL_BODY => true,
            other => {
                return Err(Error::malformed(
                    OFF_BODY_LEN,
                    format!(
                        "on-air body length {other:#06x} is neither the four- nor the six-channel form"
                    ),
                ));
            }
        };
        if six && data.len() < Self::SIX_CHANNEL_LEN {
            return Err(Error::Truncated {
                need: Self::SIX_CHANNEL_LEN,
                at: 0,
                have: data.len(),
            });
        }
        let mut on_air = [false; Self::MAX_CHANNELS];
        for (index, slot) in on_air.iter_mut().take(4).enumerate() {
            *slot = byte_at(data, Self::OFF_FIRST.saturating_add(index)).unwrap_or(0) != 0;
        }
        if six {
            for (index, slot) in on_air.iter_mut().skip(4).enumerate() {
                *slot = byte_at(data, Self::OFF_FIFTH.saturating_add(index)).unwrap_or(0) != 0;
            }
        }
        Ok(Self {
            name: name_at(data),
            sender: DeviceNumber::new(byte_at(data, OFF_SENDER).unwrap_or(0)),
            on_air,
            channels: if six { 6 } else { 4 },
        })
    }

    /// One flag per channel the packet describes: four of them, or six.
    ///
    /// The length is the packet's, not a fixed six, because a four-channel
    /// mixer says nothing at all about channels 5 and 6 and reporting them as
    /// off air would be inventing a fact.
    pub fn channels(&self) -> &[bool] {
        self.on_air.get(..self.channels).unwrap_or(&self.on_air)
    }

    /// Whether channel `number` (1-based, as the mixer labels it) is audible,
    /// or `None` if this packet does not describe it.
    pub fn is_on_air(&self, number: usize) -> Option<bool> {
        self.channels().get(number.checked_sub(1)?).copied()
    }
}

impl fmt::Debug for ChannelsOnAir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelsOnAir")
            .field("sender", &self.sender)
            .field("channels", &self.channels())
            .finish_non_exhaustive()
    }
}

// -- fader start (0x02) ---------------------------------------------------

/// What a fader-start packet asks one channel's player to do.
///
/// *Literature only; never observed.* Not supported by an XDJ-XZ or a CDJ-3000
/// even in principle.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaderAction(pub u8);

impl FaderAction {
    /// Start playing, if the player is sitting at its cue point.
    pub const START: Self = Self(0x00);
    /// Stop and return to the cue point.
    pub const STOP: Self = Self(0x01);
    /// Leave this channel's player alone.
    pub const UNCHANGED: Self = Self(0x02);

    /// A name for logs, or `None` for a value we have no name for.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::START => "start",
            Self::STOP => "stop",
            Self::UNCHANGED => "unchanged",
            _ => return None,
        })
    }
}

impl fmt::Debug for FaderAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "FaderAction({:#04x})", self.0),
        }
    }
}

/// A mixer starting or stopping players from its channel faders.
///
/// **Never observed** — see [`ChannelsOnAir`] for why. Decode-only.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FaderStart {
    /// The mixer's name.
    pub name: DeviceName,
    /// Byte `0x21`.
    pub sender: Option<DeviceNumber>,
    /// One action per channel, `0x24`–`0x27`.
    pub channels: [FaderAction; Self::CHANNELS],
}

impl FaderStart {
    /// Channels a fader-start packet addresses.
    pub const CHANNELS: usize = 4;
    /// Bytes on the wire: subtype `0x00`, body length `0x0004`.
    pub const LEN: usize = 0x28;

    const OFF_FIRST: usize = 0x24;

    /// Parse a fader-start packet.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check_header(data, BeatKind::FADER_START.0, Self::LEN)?;
        let mut channels = [FaderAction::UNCHANGED; Self::CHANNELS];
        for (index, slot) in channels.iter_mut().enumerate() {
            *slot = FaderAction(byte_at(data, Self::OFF_FIRST.saturating_add(index)).unwrap_or(0));
        }
        Ok(Self {
            name: name_at(data),
            sender: DeviceNumber::new(byte_at(data, OFF_SENDER).unwrap_or(0)),
            channels,
        })
    }
}

impl fmt::Debug for FaderStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FaderStart")
            .field("sender", &self.sender)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

// -- master handoff (0x26, 0x27) ------------------------------------------

/// A player asking the current tempo master to hand mastership over.
///
/// Sent when a DJ presses MASTER on a deck that is not already master. It is
/// **unicast at the current master**, not broadcast, so a capture taken on a
/// switch that is not mirroring will not contain it (F48).
///
/// Observed five times, in `captures/S28-master-beat-sync-taglist`, always
/// answered within 5 ms by a [`MasterResponse`] from the deck that held master,
/// after which the requester's status byte `0x9e` goes to `1` within ~70 ms.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MasterRequest {
    /// `0x0b`–`0x1e`, the requesting device's name.
    pub name: DeviceName,
    /// Byte `0x21`, repeated as the body word at `0x24`. The device that wants
    /// to become master.
    pub device: DeviceNumber,
}

impl MasterRequest {
    /// Bytes on the wire. Fixed: `0x0004` of body in all five captured packets.
    pub const LEN: usize = 0x28;

    /// The device number again, as a big-endian word. It matched byte `0x21` in
    /// all five captured packets, so it is written rather than carried as a
    /// field that could disagree with the one at `0x21` — the same redundancy
    /// [`Beat::OFF_DEVICE_2`] has.
    const OFF_DEVICE_2: usize = 0x24;

    /// Parse a master request, or fail if it is not one.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check_header(data, BeatKind::MASTER_REQUEST.0, Self::LEN)?;
        let device = DeviceNumber::new(byte_at(data, OFF_SENDER).unwrap_or(0))
            .ok_or_else(|| Error::malformed(OFF_SENDER, "master request from device 0"))?;
        Ok(Self {
            name: name_at(data),
            device,
        })
    }

    /// Encode this request as the 40 bytes a deck puts on the wire.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut raw = [0u8; Self::LEN];
        write_shared_header(
            &mut raw,
            BeatKind::MASTER_REQUEST.0,
            self.name,
            self.device.get(),
        );
        if let Some(field) = raw.get_mut(Self::OFF_DEVICE_2..Self::OFF_DEVICE_2 + 4) {
            field.copy_from_slice(&u32::from(self.device.get()).to_be_bytes());
        }
        raw
    }
}

impl fmt::Debug for MasterRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MasterRequest")
            .field("device", &self.device)
            .field("name", &self.name)
            .finish()
    }
}

/// The outgoing master's answer to a [`MasterRequest`].
///
/// Unicast back at the requester. The master keeps publishing `0x9e = 1` in its
/// status for one or two more packets while byte `0x9f` names the device it is
/// yielding to, then drops both — so mastership is briefly claimed by *both*
/// decks, and a follower that treats master as exclusive will see it flicker.
/// [`crate::status::CdjStatus::yielding_to`] is what disambiguates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MasterResponse {
    /// `0x0b`–`0x1e`, the answering device's name.
    pub name: DeviceName,
    /// Byte `0x21`, repeated as the body word at `0x24`. The device that *held*
    /// master and is giving it up — not the one that asked.
    pub device: DeviceNumber,
    /// The second body word at `0x28`: `1` in all five captured packets, each
    /// time followed by the handoff actually happening.
    ///
    /// Modelled as a boolean on the reading that `0` refuses, which is what a
    /// one-word acknowledgement in this position conventionally means. **A
    /// refusal has never been observed**, so encoding one is untested against
    /// hardware; nothing in this library sends it.
    pub granted: bool,
}

impl MasterResponse {
    /// Bytes on the wire. Fixed: `0x0008` of body in all five captured packets.
    pub const LEN: usize = 0x2c;

    /// The answering device's number again, as a big-endian word. See
    /// [`MasterRequest::OFF_DEVICE_2`].
    const OFF_DEVICE_2: usize = 0x24;
    const OFF_GRANTED: usize = 0x28;

    /// Parse a master response, or fail if it is not one.
    pub fn parse(data: &[u8]) -> Result<Self> {
        check_header(data, BeatKind::MASTER_RESPONSE.0, Self::LEN)?;
        let device = DeviceNumber::new(byte_at(data, OFF_SENDER).unwrap_or(0))
            .ok_or_else(|| Error::malformed(OFF_SENDER, "master response from device 0"))?;
        Ok(Self {
            name: name_at(data),
            device,
            granted: be_u32_at(data, Self::OFF_GRANTED).unwrap_or(0) != 0,
        })
    }

    /// Encode this response as the 44 bytes a deck puts on the wire.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut raw = [0u8; Self::LEN];
        write_shared_header(
            &mut raw,
            BeatKind::MASTER_RESPONSE.0,
            self.name,
            self.device.get(),
        );
        if let Some(field) = raw.get_mut(Self::OFF_DEVICE_2..Self::OFF_DEVICE_2 + 4) {
            field.copy_from_slice(&u32::from(self.device.get()).to_be_bytes());
        }
        if let Some(field) = raw.get_mut(Self::OFF_GRANTED..Self::OFF_GRANTED + 4) {
            field.copy_from_slice(&u32::from(self.granted).to_be_bytes());
        }
        raw
    }
}

impl fmt::Debug for MasterResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MasterResponse")
            .field("device", &self.device)
            .field("granted", &self.granted)
            .field("name", &self.name)
            .finish()
    }
}

/// The name field, `0x0b`–`0x1e`. Empty on a datagram too short to hold it,
/// which the constructors have already ruled out.
fn name_at(raw: &[u8]) -> DeviceName {
    let mut field = [0u8; DeviceName::LEN];
    if let Some(bytes) = raw.get(OFF_NAME..OFF_NAME + DeviceName::LEN) {
        field.copy_from_slice(bytes);
    }
    DeviceName(field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{OFF_CONST_ONE, OFF_SUBTYPE};

    /// The first beat packet device 1 sent in `captures/S06-load-and-play`.
    ///
    /// A CDJ-2000nexus playing a 132.01 BPM track with the fader at −3.05%, on
    /// the downbeat. Pinned as bytes because a round trip between our own
    /// encoder and our own decoder proves only that they agree with each other.
    const REAL_BEAT: [u8; Beat::LEN] = [
        0x51, 0x73, 0x70, 0x74, 0x31, 0x57, 0x6d, 0x4a, 0x4f, 0x4c, 0x28, 0x43, 0x44, 0x4a, 0x2d,
        0x32, 0x30, 0x30, 0x30, 0x6e, 0x65, 0x78, 0x75, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x01, 0x00, 0x3c, 0x00, 0x00, 0x01, 0xc6, 0x00, 0x00, 0x03, 0x8d, 0x00,
        0x00, 0x07, 0x1a, 0x00, 0x00, 0x07, 0x1a, 0x00, 0x00, 0x0e, 0x34, 0x00, 0x00, 0x0e, 0x34,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x0f, 0x83, 0x12, 0x00, 0x00,
        0x33, 0x91, 0x01, 0x00, 0x00, 0x01,
    ];

    fn real() -> Beat {
        Beat::parse(&REAL_BEAT).expect("a captured beat packet")
    }

    #[test]
    fn a_captured_beat_packet_says_what_the_deck_was_playing() {
        let beat = real();
        assert_eq!(beat.device.get(), 1);
        assert_eq!(beat.name.as_str(), "CDJ-2000nexus");
        assert_eq!(beat.bpm_centi, 13201);
        assert_eq!(beat.beat_in_bar, Some(BeatInBar::DOWNBEAT));
        assert!(!beat.scratching);
        assert_eq!(beat.timings.next_beat, Some(Duration::from_millis(454)));
        assert_eq!(beat.timings.second_beat, Some(Duration::from_millis(909)));
        assert_eq!(beat.timings.next_bar, Some(Duration::from_millis(1818)));
        assert_eq!(beat.timings.fourth_beat, Some(Duration::from_millis(1818)));
        assert_eq!(beat.timings.second_bar, Some(Duration::from_millis(3636)));
        assert_eq!(beat.timings.eighth_beat, Some(Duration::from_millis(3636)));
    }

    #[test]
    fn a_captured_beat_packet_re_encodes_byte_for_byte() {
        assert_eq!(real().encode(), REAL_BEAT);
    }

    #[test]
    fn the_effective_tempo_is_the_one_the_fader_leaves() {
        let beat = real();
        // 132.01 BPM at −3.05% is 127.98, and a consumer reporting 132.01 is
        // wrong by exactly the fader.
        assert!(
            (beat.bpm() - 132.01).abs() < 0.005,
            "raw tempo: {}",
            beat.bpm()
        );
        assert!(
            (beat.pitch.percent() + 3.0500).abs() < 0.001,
            "pitch: {}",
            beat.pitch.percent()
        );
        assert!(
            (beat.effective_bpm() - 127.9836).abs() < 0.001,
            "effective tempo: {}",
            beat.effective_bpm()
        );
        let interval = beat.beat_interval().expect("a usable tempo");
        assert!(
            (interval.as_secs_f64() - 0.46881).abs() < 0.0001,
            "beat interval: {interval:?}"
        );
    }

    #[test]
    fn the_timing_fields_are_quoted_at_zero_pitch() {
        // The packet's own next-beat field is 60000/132.01 = 454.5 ms, the
        // track's grid — not 469 ms, which is what the platter was doing. A
        // consumer that used the field as a wall-clock interval would drift by
        // the pitch fader every beat.
        let beat = real();
        let next = beat.timings.next_beat.expect("a next beat");
        let at_zero_pitch = 60_000.0 / beat.bpm();
        assert!(
            (next.as_secs_f64() * 1000.0 - at_zero_pitch).abs() < 1.0,
            "next beat {next:?} against {at_zero_pitch} ms at 0% pitch"
        );
        assert!(
            beat.beat_interval().expect("a tempo").as_secs_f64() * 1000.0 - at_zero_pitch > 10.0,
            "and the two are far enough apart to matter"
        );
    }

    #[test]
    fn the_next_bar_is_however_many_beats_are_left_of_this_one() {
        let beat = real();
        let bar = beat.timings.next_bar.expect("a next bar");
        let next = beat.timings.next_beat.expect("a next beat");
        let second = beat.timings.second_beat.expect("a second beat");
        let step = second.saturating_sub(next);
        let beats_left = beat
            .beat_in_bar
            .expect("a bar position")
            .beats_to_next_bar();
        let expected = next + step * u32::from(beats_left - 1);
        assert!(
            bar.abs_diff(expected) <= Duration::from_millis(3),
            "next bar {bar:?} against {expected:?}"
        );
    }

    #[test]
    fn a_beat_from_device_zero_is_not_a_beat() {
        // Nothing can be attributed to device 0, so a table keyed by number
        // would grow an entry that no packet ever updates.
        let mut mangled = REAL_BEAT;
        mangled[OFF_SENDER] = 0;
        assert!(Beat::parse(&mangled).is_err());
        assert!(matches!(
            decode(&mangled),
            Ok(Packet::Other {
                kind: BeatKind::BEAT,
                ..
            })
        ));
    }

    #[test]
    fn a_beat_in_bar_of_zero_means_no_bar_rather_than_beat_zero() {
        let mut mangled = REAL_BEAT;
        mangled[Beat::OFF_BEAT_IN_BAR] = 0;
        let beat = Beat::parse(&mangled).expect("still a beat packet");
        assert_eq!(beat.beat_in_bar, None);
        assert_eq!(beat.encode(), mangled, "and zero goes back out as zero");
        assert_eq!(BeatInBar::new(0), None);
        assert_eq!(BeatInBar::new(5), None);
    }

    #[test]
    fn a_track_ending_before_a_grid_point_reports_no_distance() {
        let mut mangled = REAL_BEAT;
        mangled[Beat::OFF_EIGHTH_BEAT..Beat::OFF_EIGHTH_BEAT + 4].fill(0xff);
        let beat = Beat::parse(&mangled).expect("still a beat packet");
        assert_eq!(
            beat.timings.eighth_beat, None,
            "0xffffffff is 'the track ends first', not a 49-day wait"
        );
        assert_eq!(beat.encode(), mangled);
    }

    #[test]
    fn scratching_sets_both_flag_words_together() {
        let mut beat = real();
        beat.scratching = true;
        let raw = beat.encode();
        assert_eq!(
            &raw[Beat::OFF_SCRATCH_1..Beat::OFF_SCRATCH_1 + 2],
            [0xff, 0xff]
        );
        assert_eq!(
            &raw[Beat::OFF_SCRATCH_2..Beat::OFF_SCRATCH_2 + 2],
            [0xff, 0xff]
        );
        assert!(Beat::parse(&raw).expect("a beat").scratching);
    }

    #[test]
    fn the_pitch_encoding_is_fixed_point_and_not_a_percentage() {
        assert!((Pitch::UNITY.multiplier() - 1.0).abs() < f64::EPSILON);
        assert!(Pitch::UNITY.percent().abs() < f64::EPSILON);
        assert!((Pitch(0x0000_0000).percent() + 100.0).abs() < f64::EPSILON);
        assert!((Pitch(0x0020_0000).percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn beats_to_the_next_bar_counts_down_to_one() {
        let expected = [(1u8, 4u8), (2, 3), (3, 2), (4, 1)];
        for (beat, left) in expected {
            let position = BeatInBar::new(beat).expect("a beat in 1-4");
            assert_eq!(position.beats_to_next_bar(), left, "on beat {beat}");
            assert_eq!(position.index(), beat - 1);
        }
    }

    #[test]
    fn a_keep_alive_is_not_a_beat() {
        // The type byte is shared across the three UDP ports and the layouts
        // behind it are not. A keep-alive is far too short for this layout, so
        // it fails rather than decoding into confident nonsense.
        let keep_alive = crate::djl::Packet::new(
            DeviceName::default(),
            crate::DeviceKind::CDJ,
            crate::djl::Body::KeepAlive {
                device_number: 2,
                was_first_on_network: 1,
                mac: crate::MacAddress::default(),
                ip: std::net::Ipv4Addr::UNSPECIFIED,
                peer_count: 1,
                pad_31: [0; 3],
                flags: 1,
                trailing: 0,
            },
        )
        .encode();
        assert!(Beat::parse(&keep_alive).is_err());
        assert!(matches!(decode(&keep_alive), Ok(Packet::Other { .. })));
    }

    #[test]
    fn something_that_is_not_pro_dj_link_is_rejected_before_anything_else() {
        assert!(matches!(
            decode(b"not a datagram"),
            Err(Error::BadMagic { .. })
        ));
        assert!(decode(&[]).is_err());
    }

    /// Build the header of a 50001 packet the corpus has no example of.
    fn synthetic(kind: BeatKind, subtype: u8, len: usize) -> Vec<u8> {
        let mut raw = vec![0u8; len];
        write_shared_header(&mut raw, kind.0, DeviceName::new("DJM-2000nexus"), 0x21);
        raw[OFF_SUBTYPE] = subtype;
        raw
    }

    #[test]
    fn the_four_channel_on_air_form_says_nothing_about_channels_five_and_six() {
        // Literature only: no mixer appears in this corpus. Reporting six
        // channels here would invent two facts the packet does not carry.
        let mut raw = synthetic(
            BeatKind::CHANNELS_ON_AIR,
            0x00,
            ChannelsOnAir::FOUR_CHANNEL_LEN,
        );
        raw[0x24] = 1;
        raw[0x26] = 1;
        let Ok(Packet::ChannelsOnAir(on_air)) = decode(&raw) else {
            panic!("a four-channel on-air packet");
        };
        assert_eq!(on_air.channels(), [true, false, true, false]);
        assert_eq!(on_air.is_on_air(1), Some(true));
        assert_eq!(on_air.is_on_air(2), Some(false));
        assert_eq!(on_air.is_on_air(5), None);
        assert_eq!(on_air.is_on_air(0), None, "channels are 1-based");
        assert_eq!(on_air.sender.map(DeviceNumber::get), Some(0x21));
        assert_eq!(
            u16::from_be_bytes([raw[OFF_BODY_LEN], raw[OFF_BODY_LEN + 1]]),
            ChannelsOnAir::FOUR_CHANNEL_BODY
        );
    }

    #[test]
    fn the_six_channel_on_air_form_carries_two_more_past_the_padding() {
        let mut raw = synthetic(
            BeatKind::CHANNELS_ON_AIR,
            0x03,
            ChannelsOnAir::SIX_CHANNEL_LEN,
        );
        raw[0x24] = 1;
        raw[ChannelsOnAir::OFF_FIFTH + 1] = 1;
        let on_air = ChannelsOnAir::parse(&raw).expect("a six-channel on-air packet");
        assert_eq!(
            on_air.channels(),
            [true, false, false, false, false, true],
            "channels 5 and 6 sit past five pad bytes"
        );
    }

    #[test]
    fn a_six_channel_on_air_packet_cut_short_is_truncated_not_four_channel() {
        let mut raw = synthetic(
            BeatKind::CHANNELS_ON_AIR,
            0x03,
            ChannelsOnAir::SIX_CHANNEL_LEN,
        );
        raw.truncate(ChannelsOnAir::FOUR_CHANNEL_LEN);
        let error = ChannelsOnAir::parse(&raw).expect_err("a truncated packet");
        assert!(error.is_truncated(), "{error}");
    }

    #[test]
    fn fader_start_names_one_action_per_channel() {
        let mut raw = synthetic(BeatKind::FADER_START, 0x00, FaderStart::LEN);
        raw[0x24] = FaderAction::START.0;
        raw[0x25] = FaderAction::STOP.0;
        raw[0x26] = FaderAction::UNCHANGED.0;
        raw[0x27] = 0x7f;
        let Ok(Packet::FaderStart(fader)) = decode(&raw) else {
            panic!("a fader-start packet");
        };
        assert_eq!(
            fader.channels,
            [
                FaderAction::START,
                FaderAction::STOP,
                FaderAction::UNCHANGED,
                FaderAction(0x7f),
            ]
        );
        assert_eq!(
            fader.channels[3].name(),
            None,
            "and an unknown one stays a byte"
        );
    }

    #[test]
    fn an_unmodelled_kind_survives_as_bytes() {
        // A CDJ-3000 puts an absolute-position packet on this port every 30 ms.
        // Dropping it would be fine; failing to decode the beats around it
        // would not be.
        let raw = synthetic(BeatKind::ABSOLUTE_POSITION, 0x02, 0x3c);
        let decoded = decode(&raw).expect("a well-formed datagram");
        assert_eq!(decoded.kind(), BeatKind::ABSOLUTE_POSITION);
        assert!(matches!(decoded, Packet::Other { ref raw, .. } if raw.len() == 0x3c));
        assert_eq!(BeatKind(0x77).name(), None);
        assert_eq!(format!("{:?}", BeatKind(0x77)), "BeatKind(0x77)");
    }

    // -- master handoff ---------------------------------------------------

    /// Device 1 asking device 2 for master, 740.338 s into
    /// `captures/S28-master-beat-sync-taglist`.
    const REAL_MASTER_REQUEST: [u8; MasterRequest::LEN] = [
        0x51, 0x73, 0x70, 0x74, 0x31, 0x57, 0x6d, 0x4a, 0x4f, 0x4c, 0x26, 0x43, 0x44, 0x4a, 0x2d,
        0x32, 0x30, 0x30, 0x30, 0x6e, 0x65, 0x78, 0x75, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x01, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01,
    ];

    /// Device 2 agreeing, 5 ms later in the same capture.
    const REAL_MASTER_RESPONSE: [u8; MasterResponse::LEN] = [
        0x51, 0x73, 0x70, 0x74, 0x31, 0x57, 0x6d, 0x4a, 0x4f, 0x4c, 0x27, 0x43, 0x44, 0x4a, 0x2d,
        0x32, 0x30, 0x30, 0x30, 0x6e, 0x65, 0x78, 0x75, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x02, 0x00, 0x08, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01,
    ];

    #[test]
    fn a_master_request_names_the_deck_that_wants_it() {
        let Packet::MasterRequest(request) = decode(&REAL_MASTER_REQUEST).expect("a datagram")
        else {
            panic!("not decoded as a master request");
        };
        assert_eq!(request.device.get(), 1);
        assert_eq!(request.name.to_string(), "CDJ-2000nexus");
        assert_eq!(request.encode(), REAL_MASTER_REQUEST);
    }

    #[test]
    fn a_master_response_names_the_deck_giving_it_up_not_the_one_asking() {
        let Packet::MasterResponse(response) = decode(&REAL_MASTER_RESPONSE).expect("a datagram")
        else {
            panic!("not decoded as a master response");
        };
        // Device 1 asked; device 2 answers, and it is 2 that appears in both
        // the header and the first body word.
        assert_eq!(response.device.get(), 2);
        assert!(response.granted);
        assert_eq!(response.encode(), REAL_MASTER_RESPONSE);
    }

    #[test]
    fn the_body_length_field_matches_the_two_handoff_layouts() {
        // 0x0004 for the request's one word, 0x0008 for the response's two.
        // Derived from the buffer size rather than written by hand, so a wrong
        // LEN would show up here rather than as a deck ignoring the packet.
        let request = MasterRequest {
            name: DeviceName::new("CDJ-2000nexus"),
            device: DeviceNumber::new(1).expect("1 is a device number"),
        };
        assert_eq!(be_u16_at(&request.encode(), OFF_BODY_LEN), Some(0x0004));
        assert_eq!(byte_at(&request.encode(), OFF_SUBTYPE), Some(0x00));

        let response = MasterResponse {
            name: DeviceName::new("CDJ-2000nexus"),
            device: DeviceNumber::new(2).expect("2 is a device number"),
            granted: true,
        };
        assert_eq!(be_u16_at(&response.encode(), OFF_BODY_LEN), Some(0x0008));
        assert_eq!(byte_at(&response.encode(), OFF_CONST_ONE), Some(0x01));
    }

    #[test]
    fn a_handoff_packet_from_device_zero_is_refused() {
        let mut raw = REAL_MASTER_REQUEST;
        raw[OFF_SENDER] = 0;
        // Refused as a request, but still surfaced rather than dropped: a
        // listener that gave up on the first surprise would stop following the
        // decks it does understand.
        assert!(MasterRequest::parse(&raw).is_err());
        assert!(matches!(
            decode(&raw).expect("still a datagram"),
            Packet::Other { kind, .. } if kind == BeatKind::MASTER_REQUEST
        ));
    }

    // -- the capture corpus -----------------------------------------------

    /// Every beat packet in the corpus, or an empty vector on a machine with no
    /// captures.
    ///
    /// The tests below therefore always run; the pinned [`REAL_BEAT`] above is
    /// the committed floor, so a coverage regression cannot hide behind an
    /// empty corpus.
    fn corpus_beats() -> Vec<Vec<u8>> {
        let Some(corpus) = prolink_capture::Corpus::locate() else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for path in corpus.captures() {
            let Ok(capture) = prolink_capture::Capture::open(&path) else {
                continue;
            };
            for packet in capture.udp_to(prolink_capture::BEAT_PORT).flatten() {
                found.push(packet.payload.clone());
            }
        }
        found
    }

    #[test]
    fn every_beat_packet_in_the_corpus_re_encodes_byte_for_byte() {
        let datagrams = corpus_beats();
        if datagrams.is_empty() {
            return;
        }
        assert!(
            datagrams.len() >= 1000,
            "the corpus holds 1110 datagrams addressed to 50001; found {}",
            datagrams.len()
        );
        let mut beats = 0usize;
        let mut handoff = 0usize;
        for raw in &datagrams {
            // The corpus holds more than beats: alternating tempo master
            // between two decks puts the handoff request and its response on
            // this port too, and they are neither 96 bytes nor subtype 0x00.
            let encoded = match decode(raw).expect("a Pro DJ Link datagram") {
                Packet::Beat(beat) => {
                    assert_eq!(raw.len(), Beat::LEN, "beat packets are a fixed 96 bytes");
                    beats += 1;
                    beat.encode().to_vec()
                }
                Packet::MasterRequest(request) => {
                    handoff += 1;
                    request.encode().to_vec()
                }
                Packet::MasterResponse(response) => {
                    handoff += 1;
                    response.encode().to_vec()
                }
                other => panic!("unexpected 50001 datagram: {:?}", other.kind()),
            };
            assert_eq!(
                encoded.as_slice(),
                raw.as_slice(),
                "re-encoding a {:?} changed bytes",
                decode(raw).map(|packet| packet.kind())
            );
        }
        // Both counts are asserted so a change in either shows up: a decoder
        // that stopped recognising the handoff would otherwise just move those
        // packets into the other bucket.
        assert!(beats >= 4400, "only {beats} beat packets re-encoded");
        assert!(
            handoff >= 10,
            "the corpus holds five master handoffs — ten datagrams; found {handoff}"
        );
        assert_eq!(
            beats + handoff,
            datagrams.len(),
            "every datagram on 50001 is either a beat or a master handoff"
        );
    }

    #[test]
    fn every_beat_packet_in_the_corpus_is_internally_consistent() {
        let datagrams = corpus_beats();
        if datagrams.is_empty() {
            return;
        }
        let mut checked = 0usize;
        for raw in &datagrams {
            let Ok(Packet::Beat(beat)) = decode(raw) else {
                continue;
            };
            let next = beat.timings.next_beat.expect("a next beat");
            let second = beat.timings.second_beat.expect("a second beat");
            let step = second.saturating_sub(next);

            // The next-beat field is the track's grid, so it is 60000/BPM
            // whatever the fader is doing. The observed spread over the whole
            // corpus is −0.79 to +0.56 ms, which is the deck's own rounding.
            let at_zero_pitch = 60_000.0 / beat.bpm();
            let millis = next.as_secs_f64() * 1000.0;
            assert!(
                (millis - at_zero_pitch).abs() <= 1.0,
                "next beat {millis} ms against {at_zero_pitch} ms at 0% pitch, in {beat:?}"
            );

            // ...and the rest of the grid follows from it.
            let position = beat.beat_in_bar.expect("a bar position");
            let expect = |beats: u32| next + step * beats.saturating_sub(1);
            for (label, field, beats) in [
                ("fourth beat", beat.timings.fourth_beat, 4),
                ("eighth beat", beat.timings.eighth_beat, 8),
                (
                    "next bar",
                    beat.timings.next_bar,
                    u32::from(position.beats_to_next_bar()),
                ),
                (
                    "second bar",
                    beat.timings.second_bar,
                    u32::from(position.beats_to_next_bar()) + 4,
                ),
            ] {
                let actual = field.expect(label);
                assert!(
                    actual.abs_diff(expect(beats)) <= Duration::from_millis(7),
                    "{label} {actual:?} against {:?} in {beat:?}",
                    expect(beats)
                );
            }
            checked += 1;
        }
        assert!(checked >= 1000, "checked only {checked} beat packets");
    }

    #[test]
    fn the_corpus_holds_only_shapes_this_codec_expects() {
        let datagrams = corpus_beats();
        if datagrams.is_empty() {
            return;
        }
        for raw in &datagrams {
            // Beat packets only: the master handoff shares this port and has
            // its own shape.
            if byte_at(raw, MAGIC.len()) != Some(BeatKind::BEAT.0) {
                continue;
            }
            assert_eq!(
                byte_at(raw, OFF_SUBTYPE),
                Some(0x00),
                "every captured beat packet is subtype 0x00"
            );
            assert_eq!(
                be_u16_at(raw, OFF_BODY_LEN),
                Some(0x003c),
                "and declares 0x3c bytes of body"
            );
            assert_eq!(
                byte_at(raw, OFF_CONST_ONE),
                Some(0x01),
                "byte 0x1f is structural here too, not the last byte of the name (C14)"
            );
            assert_eq!(
                raw.get(Beat::OFF_FILLER..Beat::OFF_FILLER + Beat::LEN_FILLER),
                Some([0xffu8; Beat::LEN_FILLER].as_slice()),
                "the filler is 24 bytes of 0xff, never anything else"
            );
            assert_eq!(
                byte_at(raw, OFF_SENDER),
                byte_at(raw, Beat::OFF_DEVICE_2),
                "the device number is repeated at 0x5f and always agrees"
            );
        }
    }
}
