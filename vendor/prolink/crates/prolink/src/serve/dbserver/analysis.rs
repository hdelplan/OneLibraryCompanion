// SPDX-License-Identifier: GPL-3.0-only

//! The six binary replies: artwork, and the five transformed analysis blobs.
//!
//! **A server cannot hand a player the bytes rekordbox wrote.** Every blob is
//! converted — the file is big-endian and the wire little-endian, and three of
//! the five change layout too (F30). The conversions themselves live in
//! `prolink_proto::analysis`, which takes raw tag payloads and knows nothing
//! about files; this module is the join between them and a [`Medium`], plus the
//! envelope each reply travels in.
//!
//! # The envelope, and the two things easy to get wrong
//!
//! Every binary reply is `[request type, 0, byte length, blob, *trailing]`, and
//! **argument 0 echoes the request's message type** rather than the track id.
//! A zero-length binary argument is omitted from the wire entirely, so "no
//! artwork" and "here is the artwork" are one shape and need no special case.
//!
//! **`GET_WAVEFORM_PREVIEW` carries the track id at argument 2**, not argument
//! 1 like its siblings — its arguments are `[descriptor, 3, track id, 0, b""]`.
//! Reading argument 1 asks for the analysis of track 3, finds nothing, and
//! answers with an empty blob, which is what happened.
//!
//! # The prefix word, and why it needs a clock
//!
//! The fifth word of the beat-grid and detail-waveform prefixes cannot be
//! derived: the two observed values are for the same track in the same load, so
//! it is per reply, and it advances about 40,000 a second. It **must be
//! non-zero** — with zero the main waveform does not draw (F33) — so
//! [`PrefixWord`] is non-zero by construction and [`PrefixWord::from_elapsed`]
//! wants a monotonic clock, which is why every call here takes the server's
//! uptime.

use std::time::Duration;

use prolink_proto::analysis::{self, Cue, PrefixWord};
use prolink_proto::dbserver::{Arguments, Field, MediaInfo, Message, MessageKind};
use prolink_rekordbox::FourCc;

use crate::serve::Medium;

/// Build the reply to a binary request, or `None` if it is not one.
///
/// A request naming a slot we do not serve still gets a reply, with an empty
/// blob: a deck waiting on a waveform it will never receive is worse than one
/// told there is no waveform.
pub(super) fn reply(
    message: &Message,
    medium: Option<&Medium>,
    uptime: Duration,
) -> Option<Message> {
    let kind = message.kind;
    let transaction = message.transaction_id;
    // Every sibling carries the track id at argument 1. This one does not.
    let track_id = match kind {
        MessageKind::GET_WAVEFORM_PREVIEW => message.number(2),
        _ => message.number(1),
    }
    .unwrap_or(0);

    if kind == MessageKind::GET_ARTWORK {
        let image = medium
            .map(|medium| medium.artwork(track_id))
            .unwrap_or_default();
        return Message::binary_reply(transaction, MessageKind::ARTWORK, kind, image, &[]);
    }
    if kind == MessageKind::GET_CUE_POINTS {
        return Some(cue_points(transaction, medium, track_id));
    }
    if kind == MessageKind::GET_MEDIA_INFO {
        return Some(media_info(transaction, medium));
    }

    let (response, payload, trailing) = match kind {
        MessageKind::GET_VBR_INDEX => (
            MessageKind::VBR_INDEX,
            with_payload(medium, track_id, FourCc::PVBR, analysis::vbr_index),
            [].as_slice(),
        ),
        MessageKind::GET_BEAT_GRID => {
            let prefix = PrefixWord::from_elapsed(uptime);
            (
                MessageKind::BEAT_GRID,
                with_payload(medium, track_id, FourCc::PQTZ, |payload| {
                    analysis::beat_grid(payload, prefix)
                }),
                // One trailing zero, which a real deck sends and no other
                // binary reply carries.
                [0u32].as_slice(),
            )
        }
        MessageKind::GET_WAVEFORM_PREVIEW => (
            MessageKind::WAVEFORM_PREVIEW,
            medium
                .map(|medium| {
                    let parsed = medium.analysis(track_id);
                    let packed = parsed.payload(FourCc::PWAV);
                    if packed.is_empty() {
                        Vec::new()
                    } else {
                        analysis::waveform_preview(packed, parsed.payload(FourCc::PWV2))
                    }
                })
                .unwrap_or_default(),
            [].as_slice(),
        ),
        MessageKind::GET_WAVEFORM_DETAIL => {
            let prefix = PrefixWord::from_elapsed(uptime);
            (
                MessageKind::WAVEFORM_DETAIL,
                medium
                    .map(|medium| {
                        let parsed = medium.analysis(track_id);
                        let payload = parsed.payload(FourCc::PWV3);
                        if payload.is_empty() {
                            Vec::new()
                        } else {
                            analysis::waveform_detail(
                                payload,
                                entry_width(medium, track_id),
                                prefix,
                            )
                        }
                    })
                    .unwrap_or_default(),
                [].as_slice(),
            )
        }
        _ => return None,
    };
    Message::binary_reply(transaction, response, kind, payload, trailing)
}

/// Describe the medium, in answer to `0x3903`.
///
/// **Answering this as an unknown request costs the whole browse session.**
/// A deck asks it during a load and expects `0x4902` with a 148-byte body; give
/// it a bare `SUCCESS` and it loses its menus, drops the track title back to the
/// medium's own name — `USB@PLAYER4` on the screen — and stops drawing the
/// scrolling waveform, until the DJ leaves LINK and comes back. Observed on
/// hardware; see [`MediaInfo`] for how the body was decoded.
///
/// The counts are the true ones, as everywhere (F24). The creation date and the
/// two sizes are the values our UDP media response also sends, so a deck that
/// asks both ways is told the same thing twice; they are the medium's, not
/// ours, and we do not yet read them off the volume.
fn media_info(transaction: u32, medium: Option<&Medium>) -> Message {
    let description = medium.map(Medium::description).unwrap_or_default();
    let body = MediaInfo {
        volume_name: description.volume_name,
        created: description.created,
        track_count: description.track_count,
        playlist_count: description.playlist_count,
        total_bytes: description.total_bytes.unwrap_or(0),
        free_bytes: description.free_bytes.unwrap_or(0),
    }
    .encode();
    Message::new(
        transaction,
        MessageKind::MEDIA_INFO,
        [
            Field::U32(MessageKind::GET_MEDIA_INFO.0.into()),
            Field::U32(0),
            Field::U32(u32::try_from(body.len()).unwrap_or(0)),
            Field::Blob(body),
        ],
    )
}

/// Whether `kind` is one of the requests [`reply`] answers.
pub(super) fn is_binary_request(kind: MessageKind) -> bool {
    matches!(
        kind,
        MessageKind::GET_ARTWORK
            | MessageKind::GET_MEDIA_INFO
            | MessageKind::GET_CUE_POINTS
            | MessageKind::GET_VBR_INDEX
            | MessageKind::GET_BEAT_GRID
            | MessageKind::GET_WAVEFORM_PREVIEW
            | MessageKind::GET_WAVEFORM_DETAIL
    )
}

/// Transform one tag's payload, or produce nothing when the medium has no
/// analysis for the track.
///
/// A track analysed by an older rekordbox legitimately lacks the newer tags,
/// and a missing waveform should cost the waveform rather than the load.
fn with_payload(
    medium: Option<&Medium>,
    track_id: u32,
    fourcc: FourCc,
    transform: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Vec<u8> {
    let Some(medium) = medium else {
        return Vec::new();
    };
    let parsed = medium.analysis(track_id);
    let payload = parsed.payload(fourcc);
    if payload.is_empty() {
        return Vec::new();
    }
    transform(payload)
}

/// The `PWV3` tag's own first header word, which the detail waveform's prefix
/// repeats.
///
/// Always 1 in every file seen, and `analysis::waveform_detail` floors it at 1,
/// so reading it is belt and braces rather than a guess.
fn entry_width(medium: &Medium, track_id: u32) -> u32 {
    let parsed = medium.analysis(track_id);
    parsed
        .ext
        .as_ref()
        .and_then(|file| file.tag(FourCc::PWV3))
        .map(prolink_rekordbox::Tag::header_extra)
        .and_then(|extra| extra.get(..4))
        .and_then(|word| <[u8; 4]>::try_from(word).ok())
        .map_or(1, u32::from_be_bytes)
}

/// `GET_CUE_POINTS` → `0x4702`: the one reply carrying **two** blobs.
///
/// `[request type, 0, record bytes, records, record size, count, 0, time bytes,
/// times]` — fixed-size cue records, then one `(time, loop time)` pair each.
/// Cues go out sorted by time, not in the order the file stores them.
fn cue_points(transaction: u32, medium: Option<&Medium>, track_id: u32) -> Message {
    let blobs = analysis::cue_points(&cues(medium, track_id));
    let record_bytes = u32::try_from(blobs.records.len()).unwrap_or(u32::MAX);
    let time_bytes = u32::try_from(blobs.times.len()).unwrap_or(u32::MAX);
    let entry_size = u32::try_from(analysis::CUE_ENTRY_LEN).unwrap_or(u32::MAX);
    Message::new(
        transaction,
        MessageKind::CUE_POINTS,
        Arguments::from([
            Field::U32(u32::from(MessageKind::GET_CUE_POINTS.0)),
            Field::U32(0),
            Field::U32(record_bytes),
            Field::Blob(blobs.records),
            Field::U32(entry_size),
            Field::U32(blobs.count),
            Field::U32(0),
            Field::U32(time_bytes),
            Field::Blob(blobs.times),
        ]),
    )
}

/// A track's memory points and hot cues, from both `PCOB` lists.
///
/// The order field is the one part of a cue record nobody has decoded. A real
/// deck wrote `00 01` there for all three cues of the reference load, which is
/// the entry's `cue_type` byte and the zero beside it read as a big-endian
/// pair; that is reproduced rather than explained *(unknown)*.
///
/// The loop time travels exactly as the file records it, which for a cue that
/// is not a loop is `0xffffffff` and not zero.
fn cues(medium: Option<&Medium>, track_id: u32) -> Vec<Cue> {
    let Some(medium) = medium else {
        return Vec::new();
    };
    let parsed = medium.analysis(track_id);
    let from_dat: Vec<Cue> = parsed
        .dat
        .as_ref()
        .into_iter()
        .flat_map(prolink_rekordbox::AnlzFile::cue_lists)
        .flat_map(|list| list.cues.iter())
        .map(|cue| Cue {
            order: u16::from(cue.cue_type.0).saturating_mul(0x100),
            hot_cue: u16::try_from(cue.hot_cue).unwrap_or(u16::MAX),
            time_ms: cue.time,
            loop_time_ms: cue.loop_time,
        })
        .collect();
    if !from_dat.is_empty() {
        return from_dat;
    }

    // **Fall back to the `.EXT`'s extended lists.** The `.DAT`'s `PCOB` is the
    // only place the reference implementation looked, and on hardware that
    // produced a `0x4702` reply carrying two *empty* blobs where a real deck
    // sends two full ones — caught by diffing our replies against a deck's for
    // the same requests. A track whose cues rekordbox wrote only as nxs2
    // `PCO2` entries has nothing in `PCOB` at all, and an empty reply is not an
    // error, so nothing else would ever have reported it.
    parsed
        .ext
        .as_ref()
        .into_iter()
        .flat_map(prolink_rekordbox::AnlzFile::extended_cue_lists)
        .flat_map(|list| list.cues.iter())
        .map(|cue| Cue {
            order: u16::from(cue.cue_type.0).saturating_mul(0x100),
            hot_cue: u16::try_from(cue.hot_cue).unwrap_or(u16::MAX),
            time_ms: cue.time,
            loop_time_ms: cue.loop_time,
        })
        .collect()
}
