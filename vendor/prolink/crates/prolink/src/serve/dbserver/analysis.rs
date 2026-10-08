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
/// ours. The host supplies these facts when publishing the USB.
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
/// `[request type, 0, record bytes, records, record size, hot count, memory count,
/// time bytes, times]` — fixed-size cue records, then one `(time, loop time)` pair each.
/// Both blobs contain memories in stored order, then hot slots A-C in letter order.
fn cue_points(transaction: u32, medium: Option<&Medium>, track_id: u32) -> Message {
    let entries = legacy_cues(&cues(medium, track_id));
    super::super::diagnostics::record(format!(
        "cue_reply track={track_id} hot_slots_times_ms={:?} memory={}",
        entries
            .iter()
            .filter(|c| c.hot_cue != 0)
            .map(|c| (c.hot_cue, c.time_ms, c.loop_time_ms))
            .collect::<Vec<_>>(),
        entries.iter().filter(|c| c.hot_cue == 0).count(),
    ));
    cue_reply(transaction, &entries)
}

pub(super) fn cue_reply(transaction: u32, cues: &[Cue]) -> Message {
    let cues = legacy_cues(cues);
    let blobs = analysis::cue_points(&cues);
    let hot_count =
        u32::try_from(cues.iter().filter(|c| c.hot_cue != 0).count()).unwrap_or(u32::MAX);
    let memory_count =
        u32::try_from(cues.iter().filter(|c| c.hot_cue == 0).count()).unwrap_or(u32::MAX);
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
            Field::U32(hot_count),
            Field::U32(memory_count),
            Field::U32(time_bytes),
            Field::Blob(blobs.times),
        ]),
    )
}

/// Native 0x4702 replies contain a memory section followed by hot slots A-C.
/// Keep memory order and all overlapping entries. The two binary blocks use
/// this same ordering. D-H belong to the extended protocol, not this reply.
/// Confirmed byte-for-byte against CDJ-2000nexus 1.30 Russia and Danz captures.
fn legacy_cues(cues: &[Cue]) -> Vec<Cue> {
    let mut entries: Vec<_> = cues.iter().copied().filter(|c| c.hot_cue <= 3).collect();
    entries.sort_by_key(|c| c.hot_cue);
    entries
}

/// A track's memory points and hot cues, from both `PCOB` lists.
///
/// The first two wire bytes are loop/cue flags, not the ANLZ sort order.
/// A point is [0, 1]; a loop is [1, 1].
/// See Deep Symmetry's CueList.parseNexusEntries and track_metadata reference.
///
/// The loop time travels exactly as the file records it, which for a cue that
/// is not a loop is `0xffffffff` and not zero.
fn cues(medium: Option<&Medium>, track_id: u32) -> Vec<Cue> {
    let Some(medium) = medium else {
        return Vec::new();
    };
    let parsed = medium.analysis(track_id);
    independent_cues(&parsed)
}

/// Keep the selected USB lists intact and in their stored entry/tag order.
/// Extended lists remain authoritative per category, even when empty; basic
/// lists supply only categories without an extended representation. Choosing
/// between redundant formats must not sort or deduplicate their cue contents.
pub(super) fn independent_cues(parsed: &crate::serve::medium::Analysis) -> Vec<Cue> {
    let mut result = Vec::new();
    let mut append = |kind: u32, hot: u32, cue_type: u8, time_ms, loop_time_ms| {
        let valid_slot = match kind {
            0 => hot == 0,
            1 => (1..=8).contains(&hot),
            _ => false,
        };
        if valid_slot && matches!(cue_type, 1 | 2) {
            result.push(Cue {
                order: if cue_type == 2 { 0x101 } else { 0x100 },
                hot_cue: hot as u16,
                time_ms,
                loop_time_ms,
            });
        }
    };
    let mut extended = [false; 2];
    for file in [parsed.ext.as_ref(), parsed.dat.as_ref()]
        .into_iter()
        .flatten()
    {
        let prior = extended;
        for list in file.extended_cue_lists() {
            let kind = list.list_type.0 as usize;
            if kind >= extended.len() || prior[kind] {
                continue;
            }
            extended[kind] = true;
            for cue in &list.cues {
                append(
                    list.list_type.0,
                    cue.hot_cue,
                    cue.cue_type.0,
                    cue.time,
                    cue.loop_time,
                );
            }
        }
    }
    for file in [parsed.dat.as_ref(), parsed.ext.as_ref()]
        .into_iter()
        .flatten()
    {
        for list in file.cue_lists() {
            let kind = list.list_type.0 as usize;
            if kind >= extended.len() || extended[kind] {
                continue;
            }
            for cue in &list.cues {
                append(
                    list.list_type.0,
                    cue.hot_cue,
                    cue.cue_type.0,
                    cue.time,
                    cue.loop_time,
                );
            }
        }
    }
    result
}

#[cfg(test)]
pub(super) mod cue_reply_tests {
    use super::*;
    #[test]
    fn native_cdj_russia_and_danz_replies_match_in_full() {
        // CDJ-2000nexus firmware 1.30, DJ RV, captured by build 51.
        // Input tags and expected replies are independent hardware evidence.
        for (dat, ext, expected) in [
            (
                include_bytes!("fixtures/native-russia.dat").as_slice(),
                include_bytes!("fixtures/native-russia.ext").as_slice(),
                include_bytes!("fixtures/native-russia.reply").as_slice(),
            ),
            (
                include_bytes!("fixtures/native-danz.dat").as_slice(),
                include_bytes!("fixtures/native-danz.ext").as_slice(),
                include_bytes!("fixtures/native-danz.reply").as_slice(),
            ),
        ] {
            let parsed = crate::serve::Analysis {
                dat: Some(prolink_rekordbox::AnlzFile::parse(dat).unwrap()),
                ext: Some(prolink_rekordbox::AnlzFile::parse(ext).unwrap()),
            };
            let (native, used) = Message::decode(expected).unwrap();
            assert_eq!(used, expected.len());
            let actual = cue_reply(native.transaction_id, &independent_cues(&parsed));
            assert_eq!(actual.encode(), expected);
        }
    }
    #[test]
    fn empty_cue_wire_omits_both_blobs_without_consuming_next_reply() {
        // Independent framing expectation: 32 header bytes and seven tagged
        // u32 fields. Neither zero-length blob has an on-wire field header.
        let empty = cue_reply(73, &[]);
        let bytes = empty.encode();
        assert_eq!(bytes.len(), 67);
        let next = cue_reply(74, &[]);
        let mut stream = bytes.clone();
        stream.extend(next.encode());
        let (decoded, consumed) = Message::decode(&stream).unwrap();
        assert_eq!(consumed, 67);
        assert_eq!(decoded, empty);
        assert_eq!(decoded.number(2), Some(0));
        assert_eq!(decoded.number(4), Some(36));
        assert_eq!(decoded.number(7), Some(0));
        assert_eq!(Message::decode(&stream[consumed..]).unwrap().0, next);
    }

    pub(in crate::serve::dbserver) fn fixture(
        extended: bool,
        kind: u32,
        entries: &[(u32, u32, u32)],
    ) -> prolink_rekordbox::AnlzFile {
        prolink_rekordbox::AnlzFile::parse(&fixture_bytes(extended, kind, entries)).unwrap()
    }

    fn fixture_bytes(extended: bool, kind: u32, entries: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut body = kind.to_be_bytes().to_vec();
        if !extended {
            body.extend(0u16.to_be_bytes());
        }
        body.extend((entries.len() as u16).to_be_bytes());
        if extended {
            body.extend(0u16.to_be_bytes());
        } else {
            body.extend(0u32.to_be_bytes());
        }
        for &(slot, time, end) in entries {
            let mut entry = if extended {
                b"PCP2".to_vec()
            } else {
                b"PCPT".to_vec()
            };
            entry.extend(12u32.to_be_bytes());
            entry.extend(40u32.to_be_bytes());
            entry.extend(slot.to_be_bytes());
            if !extended {
                entry.extend(1u32.to_be_bytes());
                entry.extend([0; 8]);
            }
            entry.push(if end == u32::MAX { 1 } else { 2 });
            entry.extend([0, 3, 232]);
            entry.extend(time.to_be_bytes());
            entry.extend(end.to_be_bytes());
            if extended {
                entry.extend([0; 12]);
            }
            assert_eq!(entry.len(), 40);
            body.extend(entry);
        }
        let mut tag = if extended {
            b"PCO2".to_vec()
        } else {
            b"PCOB".to_vec()
        };
        tag.extend(12u32.to_be_bytes());
        tag.extend((12 + body.len() as u32).to_be_bytes());
        tag.extend(body);
        let mut file = b"PMAI".to_vec();
        file.extend(12u32.to_be_bytes());
        file.extend((12 + tag.len() as u32).to_be_bytes());
        file.extend(tag);
        file
    }

    pub(in crate::serve::dbserver) fn category_fixture(
        entries: &[(u32, u32, u32)],
    ) -> prolink_rekordbox::AnlzFile {
        let mut tags = Vec::new();
        for kind in [1, 0] {
            let selected: Vec<_> = entries
                .iter()
                .copied()
                .filter(|e| u32::from(e.0 != 0) == kind)
                .collect();
            tags.extend_from_slice(&fixture_bytes(false, kind, &selected)[12..]);
        }
        let mut file = b"PMAI".to_vec();
        file.extend(12u32.to_be_bytes());
        file.extend((12 + tags.len() as u32).to_be_bytes());
        file.extend(tags);
        prolink_rekordbox::AnlzFile::parse(&file).unwrap()
    }

    #[test]
    fn empty_extended_category_does_not_resurrect_deleted_basic_cues() {
        let parsed = crate::serve::Analysis {
            dat: Some(fixture(false, 0, &[(0, 0, u32::MAX), (0, 1000, u32::MAX)])),
            ext: Some(fixture(true, 0, &[])),
        };
        let entries = independent_cues(&parsed);
        assert!(entries.is_empty());
        let reply = cue_reply(1, &entries);
        let (decoded, used) = Message::decode(&reply.encode()).unwrap();
        assert_eq!(used, reply.encode().len());
        assert_eq!(decoded.number(5), Some(0));
        assert_eq!(decoded.number(6), Some(0));
    }

    #[test]
    fn recovery_falls_back_to_extended_cues_when_basic_is_missing() {
        let parsed = crate::serve::Analysis {
            dat: None,
            ext: Some(fixture(true, 1, &[(1, 0, u32::MAX), (2, 1000, u32::MAX)])),
        };
        assert_eq!(
            independent_cues(&parsed)
                .iter()
                .map(|c| c.hot_cue)
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn usb_hot_cues_survive_at_the_same_position_as_memory_cues() {
        // Values from the saved Russia and Monologue exports. Test the reply
        // records themselves: preserving a UI cue list would not be sufficient.
        for (times, extra) in [
            (vec![105, 192105, 208105], 272104),
            (vec![2, 30723, 46083], 61443),
        ] {
            let mut basic = Vec::new();
            let mut extended = Vec::new();
            for (index, time) in times.iter().copied().enumerate() {
                basic.push((index as u32 + 1, time, u32::MAX));
                basic.push((0, time, u32::MAX));
                extended.push((index as u32 + 1, time, u32::MAX));
            }
            extended.push((4, extra, extra + 2000));
            let parsed = crate::serve::Analysis {
                dat: Some(category_fixture(&basic)),
                ext: Some(fixture(true, 1, &extended)),
            };
            let entries = independent_cues(&parsed);
            let reply = cue_reply(19, &entries);
            let (decoded, _) = Message::decode(&reply.encode()).unwrap();
            assert_eq!(decoded.number(5), Some(3));
            assert_eq!(decoded.number(6), Some(3));
            let records = decoded.blob(3).unwrap();
            let timestamps = decoded.blob(8).unwrap();
            let wire: Vec<_> = records
                .chunks_exact(36)
                .zip(timestamps.chunks_exact(8))
                .map(|(record, time)| {
                    (
                        u16::from_le_bytes(record[2..4].try_into().unwrap()),
                        u32::from_le_bytes(time[0..4].try_into().unwrap()),
                        u16::from_le_bytes(record[0..2].try_into().unwrap()),
                        u32::from_le_bytes(time[4..8].try_into().unwrap()),
                    )
                })
                .collect();
            for (index, time) in times.into_iter().enumerate() {
                assert!(wire.contains(&(index as u16 + 1, time, 0x100, u32::MAX)));
                assert!(wire.contains(&(0, time, 0x100, u32::MAX)));
            }
            assert!(entries.iter().any(|c| c.hot_cue == 4 && c.time_ms == extra));
            assert!(!wire.iter().any(|c| c.0 > 3));
        }
    }

    #[test]
    fn edited_hot_slot_replaces_legacy_representation_without_changing_memory() {
        let parsed = crate::serve::Analysis {
            dat: Some(category_fixture(&[(1, 100, u32::MAX), (0, 100, u32::MAX)])),
            ext: Some(fixture(true, 1, &[(1, 250, u32::MAX)])),
        };
        let entries = independent_cues(&parsed);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|c| c.hot_cue == 1 && c.time_ms == 250));
        assert!(entries.iter().any(|c| c.hot_cue == 0 && c.time_ms == 100));
    }

    fn wire_cues(entries: &[Cue]) -> Vec<(u16, u32, u32)> {
        let reply = cue_reply(91, entries);
        let (decoded, _) = Message::decode(&reply.encode()).unwrap();
        decoded
            .blob(3)
            .unwrap()
            .chunks_exact(36)
            .zip(decoded.blob(8).unwrap().chunks_exact(8))
            .map(|(record, time)| {
                let millis = u32::from_le_bytes(time[..4].try_into().unwrap());
                assert_eq!(
                    u32::from_le_bytes(record[12..16].try_into().unwrap()),
                    millis * 150 / 1000
                );
                (
                    u16::from_le_bytes(record[2..4].try_into().unwrap()),
                    millis,
                    u32::from_le_bytes(time[4..8].try_into().unwrap()),
                )
            })
            .collect()
    }

    #[test]
    fn original_russia_usb_tags_use_native_legacy_sections() {
        // Actual PCO2 tags copied verbatim from DJTT's Russia EXT; only the
        // surrounding PMAI length changes to exclude unrelated waveform tags.
        let ext = prolink_rekordbox::AnlzFile::parse(include_bytes!("fixtures/russia-cues.anlz"))
            .unwrap();
        let parsed = crate::serve::Analysis {
            dat: None,
            ext: Some(ext),
        };
        let entries = independent_cues(&parsed);
        assert_eq!(
            wire_cues(&entries),
            vec![
                (0, 272104, u32::MAX),
                (0, 208105, u32::MAX),
                (0, 192105, u32::MAX),
                (0, 105, u32::MAX),
                (1, 105, u32::MAX),
                (2, 192105, u32::MAX),
                (3, 208105, u32::MAX),
            ]
        );
    }

    #[test]
    fn memory_order_and_repeated_entries_survive_hot_slot_ordering() {
        for extended in [false, true] {
            let memory = fixture_bytes(
                extended,
                0,
                &[(0, 2000, 4000), (0, 1000, u32::MAX), (0, 1000, u32::MAX)],
            );
            let hot = fixture_bytes(
                extended,
                1,
                &[
                    (3, 500, u32::MAX),
                    (1, 1000, u32::MAX),
                    (2, 1000, u32::MAX),
                    (2, 1000, u32::MAX),
                ],
            );
            let mut raw = memory;
            raw.extend_from_slice(&hot[12..]);
            let len = raw.len() as u32;
            raw[8..12].copy_from_slice(&len.to_be_bytes());
            let file = prolink_rekordbox::AnlzFile::parse(&raw).unwrap();
            let parsed = if extended {
                crate::serve::Analysis {
                    dat: None,
                    ext: Some(file),
                }
            } else {
                crate::serve::Analysis {
                    dat: Some(file),
                    ext: None,
                }
            };
            let entries = independent_cues(&parsed);
            assert_eq!(
                wire_cues(&entries),
                vec![
                    (0, 2000, 4000),
                    (0, 1000, u32::MAX),
                    (0, 1000, u32::MAX),
                    (1, 1000, u32::MAX),
                    (2, 1000, u32::MAX),
                    (2, 1000, u32::MAX),
                    (3, 500, u32::MAX),
                ]
            );
            assert_eq!(cue_reply(91, &entries).number(5), Some(4));
            assert_eq!(cue_reply(91, &entries).number(6), Some(3));
        }
    }

    #[test]
    fn memory_and_hot_cues_have_separate_counts() {
        let cues = [
            Cue {
                order: 0x100,
                hot_cue: 0,
                time_ms: 1000,
                loop_time_ms: u32::MAX,
            },
            Cue {
                order: 0x100,
                hot_cue: 1,
                time_ms: 2000,
                loop_time_ms: u32::MAX,
            },
            Cue {
                order: 0x101,
                hot_cue: 3,
                time_ms: 3000,
                loop_time_ms: 5000,
            },
        ];
        let reply = cue_reply(1, &cues);
        assert_eq!(reply.number(5), Some(2));
        assert_eq!(reply.number(6), Some(1));
        let empty = cue_reply(2, &[]);
        assert_eq!(empty.number(5), Some(0));
        assert_eq!(empty.number(6), Some(0));
    }
}
