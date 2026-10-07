// SPDX-License-Identifier: GPL-3.0-only

//! What a real CDJ-2000NXS asks for, and what a real one answers.
//!
//! Two kinds of evidence, and they do different work.
//!
//! **Captured requests.** [`CAPTURED_REQUESTS`] is one real message of every
//! type a deck was observed to send across five sessions — 44 of them, from
//! `INTRODUCE` to the four undocumented types nobody has decoded. Replaying
//! them through a session is the closest thing to hardware available without
//! hardware, and it is what proves the property F25 is about: **not one of them
//! draws an error**.
//!
//! **Captured replies.** [`REAL_ROOT_MENU`] and [`REAL_SORT_MENU`] are the
//! twenty-four menu items one CDJ-2000NXS sent another in
//! `S20-browse-ground-truth`. Our sort menu is asserted to re-encode *byte for
//! byte* against them, which a round trip between our own encoder and our own
//! decoder could never establish.
//!
//! The library under test is the real 651-track `testdata/export.pdb`, so the
//! menus have real content: 329 artists of which 290 are referenced, 24 Camelot
//! keys, seven history playlists and a 40-track playlist.

use std::time::Duration;

use prolink_proto::dbserver::{
    Arguments, Descriptor, Drill, FILTER_ALL, Field, ItemType, METADATA_ITEMS, MediaInfo,
    MenuTarget, ROOT_CATEGORIES, SORT_MENU, SortOrder, TRACK_INFO_ITEMS, TrackType, drill_kind,
    menu_label,
};
use prolink_rekordbox::Library;
use tokio::net::TcpStream;

use super::*;

// -- fixtures -------------------------------------------------------------

/// One real request of each type a CDJ-2000NXS was observed to send, from
/// `S20-browse-ground-truth`, `S22-sorting`, `S23-search-and-keys`,
/// `S06-load-and-play` and `S18-two-slots`.
///
/// Every one is a byte-exact message off the wire. Four have never been
/// decoded — `0x3001`, `0x3401`, `0x3903`, `0x3b03` — and they are in the list
/// precisely because they must not draw a refusal.
const CAPTURED_REQUESTS: &[(&str, &str)] = &[
    (
        "introduce",
        "11872349ae11fffffffe1000000f01140000000c060000000000000000000000\
                   1100000002",
    ),
    (
        "menu_close",
        "11872349ae11038005841000010f00140000000c000000000000000000000000",
    ),
    (
        "menu_root",
        "11872349ae11038005ca1010000f03140000000c060606000000000000000000\
                   110201030111000000001100ffffff",
    ),
    (
        "menu_genre",
        "11872349ae110380057c1010010f02140000000c060600000000000000000000\
                    11020203011100000000",
    ),
    (
        "menu_artist",
        "11872349ae11038005d01010020f02140000000c060600000000000000000000\
                     11020203011100000000",
    ),
    (
        "menu_album",
        "11872349ae110380057a1010030f02140000000c060600000000000000000000\
                    11020203011100000000",
    ),
    (
        "menu_track",
        "11872349ae11038006e51010040f02140000000c060600000000000000000000\
                    11020203011100000000",
    ),
    (
        "menu_label",
        "11872349ae11038005ce10100a0f02140000000c060600000000000000000000\
                    11020203011100000000",
    ),
    (
        "menu_bitrate",
        "11872349ae11038006851010110f02140000000c060600000000000000000000\
                      11020203011100000000",
    ),
    (
        "menu_history",
        "11872349ae11038006e81010120f02140000000c060600000000000000000000\
                      11020203011100000000",
    ),
    (
        "menu_key",
        "11872349ae11038007521010140f02140000000c060600000000000000000000\
                  11020203011100000000",
    ),
    (
        "drill_1101",
        "11872349ae11038005801011010f03140000000c060606000000000000000000\
                    110202030111000000001100000004",
    ),
    (
        "drill_1102",
        "11872349ae11038005d41011020f03140000000c060606000000000000000000\
                    110202030111000000001100000128",
    ),
    (
        "drill_1103",
        "11872349ae11038006071011030f03140000000c060606000000000000000000\
                    1102020301110000000011000000b5",
    ),
    (
        "menu_playlist",
        "11872349ae11038008771011050f04140000000c060606060000000000000000\
                       1102020301110000000011000000001100000001",
    ),
    (
        "drill_110a",
        "11872349ae110380065310110a0f03140000000c060606000000000000000000\
                    11020203011100000000110000002b",
    ),
    (
        "drill_1111",
        "11872349ae110380068c1011110f03140000000c060606000000000000000000\
                    110202030111000000001100000844",
    ),
    (
        "drill_1112",
        "11872349ae11038006ec1011120f03140000000c060606000000000000000000\
                    110202030111000000001100000002",
    ),
    (
        "drill_1114",
        "11872349ae11038007561011140f03140000000c060606000000000000000000\
                    110202030111000000001100000001",
    ),
    (
        "drill_1201",
        "11872349ae110380059b1012010f04140000000c060606060000000000000000\
                    11020203011100000000110000000611ffffffff",
    ),
    (
        "drill_1202",
        "11872349ae11038005e21012020f04140000000c060606060000000000000000\
                    1102020301110000000011000000f011ffffffff",
    ),
    (
        "drill_120a",
        "11872349ae110380065f10120a0f04140000000c060606060000000000000000\
                    11020203011100000000110000002a110000010e",
    ),
    (
        "drill_1214",
        "11872349ae110380075a1012140f04140000000c060606060000000000000000\
                    1102020301110000000011000000011100000000",
    ),
    (
        "menu_search",
        "11872349ae11038008171013000f05140000000c060606020600000000000000\
                     1102010301110000000011000000042600000002004800001100000000",
    ),
    (
        "drill_1301",
        "11872349ae11038005a31013010f05140000000c060606060600000000000000\
                    110202030111000000001100000006110000002611ffffffff",
    ),
    (
        "drill_130a",
        "11872349ae110380066310130a0f05140000000c060606060600000000000000\
                    11020203011100000000110000002a110000010e11000000fc",
    ),
    (
        "menu_sort",
        "11872349ae11038008c71014000f03140000000c060606000000000000000000\
                   110205030111000000001100001105",
    ),
    (
        "get_metadata",
        "11872349ae11038005af1020020f02140000000c060600000000000000000000\
                      11020203011100000292",
    ),
    (
        "get_artwork",
        "11872349ae11038005b11020030f02140000000c060600000000000000000000\
                     11020803011100000269",
    ),
    (
        "get_waveform_preview",
        "11872349ae1103800f9e1020040f05140000000c060606060300000000000000\
                              1102080201110000000211000004681100000000",
    ),
    (
        "menu_folder",
        "11872349ae11038007481020060f04140000000c060606060000000000000000\
                     1102020302110000000011ffffffff1100000000",
    ),
    (
        "get_track_info",
        "11872349ae1103800f9c1021020f02140000000c060600000000000000000000\
                        11020802011100000468",
    ),
    (
        "get_cue_points",
        "11872349ae1103800fa11021040f02140000000c060600000000000000000000\
                        11020802011100000468",
    ),
    (
        "get_beat_grid",
        "11872349ae1103800fa81022040f02140000000c060600000000000000000000\
                       11020802011100000468",
    ),
    (
        "get_vbr_index",
        "11872349ae1103800fa71025040f02140000000c060600000000000000000000\
                       11020802011100000468",
    ),
    (
        "get_waveform_detail",
        "11872349ae1103800fae1029040f03140000000c060606000000000000000000\
                             110201020111000004681100000000",
    ),
    (
        "render_menu",
        "11872349ae110380057b1030000f06140000000c060606060606000000000000\
                     110202030111000000001100000006110000000011000001141100000000",
    ),
    (
        "unknown_3001",
        "11872349ae11038008ae1030010f02140000000c060600000000000000000000\
                      11020803011100000413",
    ),
    (
        "unknown_3100",
        "11872349ae11038007211031000f04140000000c060606060000000000000000\
                      1102010301110000000211000000001100000000",
    ),
    (
        "unknown_3401",
        "11872349ae11038008bd1034010f03140000000c060606000000000000000000\
                      110201030111000004131100000000",
    ),
    (
        "unknown_3903",
        "11872349ae11038008b11039030f01140000000c060000000000000000000000\
                      1102080301",
    ),
    (
        "unknown_3b03",
        "11872349ae11038008a6103b030f02140000000c060600000000000000000000\
                      11020103011100000413",
    ),
    (
        "unknown_3d03",
        "11872349ae1103800fb2103d030f02140000000c060600000000000000000000\
                      11020102011100000468",
    ),
    (
        "unknown_3e03",
        "11872349ae1103800d82103e030f01140000000c060000000000000000000000\
                      1102010301",
    ),
];

/// The twelve root-menu rows one CDJ-2000NXS sent another, in the order it sent
/// them, across two renders of six.
const REAL_ROOT_MENU: &[&str] = &[
    "11872349ae11038005cb1041010f0c140000000c060606020602060606060606\
     110000000011000000051100000016260000000bfffa0050004c00410059004c\
     004900530054fffb000011000000022600000001000011000000841100000000\
     1100000000110000000011000000001100000000",
    "11872349ae11038005cb1041010f0c140000000c060606020602060606060606\
     1100000000110000000311000000102600000008fffa0041004c00420055004d\
     fffb000011000000022600000001000011000000821100000000110000000011\
     0000000011000000001100000000",
    "11872349ae11038005cb1041010f0c140000000c060606020602060606060606\
     1100000000110000000111000000102600000008fffa00470045004e00520045\
     fffb000011000000022600000001000011000000801100000000110000000011\
     0000000011000000001100000000",
    "11872349ae11038005cb1041010f0c140000000c060606020602060606060606\
     1100000000110000000a11000000102600000008fffa004c004100420045004c\
     fffb000011000000022600000001000011000000891100000000110000000011\
     0000000011000000001100000000",
    "11872349ae11038005cb1041010f0c140000000c060606020602060606060606\
     1100000000110000000211000000122600000009fffa00410052005400490053\
     0054fffb00001100000002260000000100001100000081110000000011000000\
     00110000000011000000001100000000",
    "11872349ae11038005cb1041010f0c140000000c060606020602060606060606\
     110000000011000000141100000014260000000afffa00420049005400520041\
     00540045fffb0000110000000226000000010000110000009311000000001100\
     000000110000000011000000001100000000",
    "11872349ae11038007511041010f0c140000000c060606020602060606060606\
     1100000000110000001b110000001a260000000dfffa00440041005400450020\
     00410044004400450044fffb0000110000000226000000010000110000008c11\
     000000001100000000110000000011000000001100000000",
    "11872349ae11038007511041010f0c140000000c060606020602060606060606\
     1100000000110000000411000000102600000008fffa0054005200410043004b\
     fffb000011000000022600000001000011000000831100000000110000000011\
     0000000011000000001100000000",
    "11872349ae11038007511041010f0c140000000c060606020602060606060606\
     110000000011000000161100000014260000000afffa0048004900530054004f\
     00520059fffb0000110000000226000000010000110000009511000000001100\
     000000110000000011000000001100000000",
    "11872349ae11038007511041010f0c140000000c060606020602060606060606\
     1100000000110000001211000000122600000009fffa00530045004100520043\
     0048fffb00001100000002260000000100001100000091110000000011000000\
     00110000000011000000001100000000",
    "11872349ae11038007511041010f0c140000000c060606020602060606060606\
     1100000000110000001111000000122600000009fffa0046004f004c00440045\
     0052fffb00001100000002260000000100001100000090110000000011000000\
     00110000000011000000001100000000",
    "11872349ae11038007511041010f0c140000000c060606020602060606060606\
     1100000000110000000c110000000c2600000006fffa004b00450059fffb0000\
     110000000226000000010000110000008b110000000011000000001100000000\
     11000000001100000000",
];

/// The twelve SORT rows one CDJ-2000NXS sent another, in one render.
///
/// Transaction `0x38008c8`, which is what our own items are stamped with to
/// compare.
const REAL_SORT_TRANSACTION: u32 = 0x0380_08c8;

/// The twelve SORT rows, byte for byte.
const REAL_SORT_MENU: &[&str] = &[
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     110000000011000000001100000014260000000afffa00440045004600410055\
     004c0054fffb000011000000022600000001000011000000a111000000001100\
     000000110000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     110000000011000000011100000016260000000bfffa0041004c005000480041\
     004200450054fffb000011000000022600000001000011000000a21100000000\
     1100000000110000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     1100000000110000000211000000122600000009fffa00410052005400490053\
     0054fffb00001100000002260000000100001100000081110000000011000000\
     00110000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     1100000000110000000311000000102600000008fffa0041004c00420055004d\
     fffb000011000000022600000001000011000000821100000000110000000011\
     0000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     11000000001100000004110000000c2600000006fffa00420050004dfffb0000\
     1100000002260000000100001100000085110000000011000000001100000000\
     11000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     1100000000110000000511000000122600000009fffa0052004100540049004e\
     0047fffb00001100000002260000000100001100000086110000000011000000\
     00110000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     1100000000110000000c110000000c2600000006fffa004b00450059fffb0000\
     110000000226000000010000110000008b110000000011000000001100000000\
     11000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     1100000000110000000d1100000014260000000afffa00420049005400520041\
     00540045fffb0000110000000226000000010000110000009311000000001100\
     000000110000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     1100000000110000001011000000202600000010fffa0044004a00200050004c\
     0041005900200043004f0055004e0054fffb0000110000000226000000010000\
     110000009711000000001100000000110000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     1100000000110000000611000000102600000008fffa00470045004e00520045\
     fffb000011000000022600000001000011000000801100000000110000000011\
     0000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     11000000001100000011110000001a260000000dfffa00440041005400450020\
     00410044004400450044fffb0000110000000226000000010000110000008c11\
     000000001100000000110000000011000000001100000000",
    "11872349ae11038008c81041010f0c140000000c060606020602060606060606\
     1100000000110000000a11000000102600000008fffa004c004100420045004c\
     fffb000011000000022600000001000011000000891100000000110000000011\
     0000000011000000001100000000",
];

/// Decode a hex literal, ignoring the whitespace that keeps it readable.
fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        digits.len().is_multiple_of(2),
        "hex literal has an odd length"
    );
    digits
        .chunks_exact(2)
        .map(|pair| {
            let text: String = pair.iter().collect();
            u8::from_str_radix(&text, 16).expect("valid hex")
        })
        .collect()
}

fn decode(text: &str) -> Message {
    let raw = hex(text);
    let (message, consumed) = Message::decode(&raw).expect("a captured message decodes");
    assert_eq!(consumed, raw.len(), "the whole literal is one message");
    message
}

/// The real 651-track library the whole workspace tests against.
fn library() -> Library {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/export.pdb");
    let raw = std::fs::read(path).expect("testdata/export.pdb");
    Library::parse(&raw).expect("a real export.pdb parses")
}

fn usb() -> Arc<Medium> {
    Arc::new(Medium::synthetic(ServedSlot::USB, library(), "DJ"))
}

/// A one-track library, so a second slot is distinguishable from the first.
fn other_library() -> Library {
    let mut small = Library::default();
    let mut track = prolink_rekordbox::Track {
        id: 1,
        title: "Only Track".to_owned(),
        ..prolink_rekordbox::Track::default()
    };
    track.artist_id = 1;
    track.artist = "Only Artist".to_owned();
    small.tracks.insert(1, track);
    small.artists.insert(1, "Only Artist".to_owned());
    small
}

fn shared(media: impl IntoIterator<Item = Arc<Medium>>) -> Shared {
    Shared {
        tags: TagLists::default(),
        loaded: Arc::new(LoadedTracks::default()),
        device: BrowsableDeviceNumber::new(2).expect("device 2 is browsable"),
        media: Arc::new(MediaSet::new(media)),
        started: Instant::now(),
    }
}

fn descriptor(slot: Slot, menu: MenuTarget) -> Descriptor {
    Descriptor::new(
        BrowsableDeviceNumber::new(1).expect("device 1 is browsable"),
        slot,
        menu,
        TrackType::REKORDBOX,
    )
}

/// Run one request through a session and decode everything it wrote.
fn ask(session: &mut Session, shared: &Shared, request: &Message) -> Vec<Message> {
    let mut out = Vec::new();
    session.handle(shared, request, &mut out);
    let mut replies = Vec::new();
    let mut consumed = 0;
    while let Some(rest) = out.get(consumed..).filter(|rest| !rest.is_empty()) {
        let (message, used) = Message::decode(rest).expect("our own replies decode");
        consumed += used;
        replies.push(message);
    }
    replies
}

/// Everything one menu request produces: the `SUCCESS`, then every row, by
/// rendering the whole result set.
fn browse(session: &mut Session, shared: &Shared, request: &Message) -> Vec<MenuItem> {
    let replies = ask(session, shared, request);
    let [success] = replies.as_slice() else {
        panic!("a menu request draws exactly one reply, got {replies:?}");
    };
    assert_eq!(success.kind, MessageKind::SUCCESS);
    let total = success.number(1).expect("an item count");
    let descriptor = request.descriptor().expect("a descriptor");

    let mut items = Vec::new();
    let mut offset = 0;
    while offset < total {
        let render = Message::render_of(
            request.transaction_id + 1,
            descriptor,
            offset,
            MAX_RENDER_BATCH,
            total,
        );
        let page = ask(session, shared, &render);
        assert_eq!(page.first().map(|m| m.kind), Some(MessageKind::MENU_HEADER));
        assert_eq!(page.last().map(|m| m.kind), Some(MessageKind::MENU_FOOTER));
        items.extend(page.iter().filter_map(MenuItem::from_message));
        offset += MAX_RENDER_BATCH;
    }
    assert_eq!(u32::try_from(items.len()), Ok(total), "every row rendered");
    items
}

fn menu(kind: MessageKind, extra: &[u32]) -> Message {
    Message::menu_request(
        0x0380_0001,
        kind,
        descriptor(Slot::USB, MenuTarget::MAIN),
        extra,
    )
    .expect("at most twelve arguments")
}

// -- the captured request surface -----------------------------------------

#[test]
fn every_request_a_real_deck_sends_draws_a_reply_and_never_an_error() {
    // The property F25 is about. Answering `0x3e03` with `0x4003` made a deck
    // render every one of our root categories and then disconnect without
    // opening a single one, so an unrecognised request is acknowledged with an
    // empty result set and never refused.
    assert!(
        CAPTURED_REQUESTS.len() >= 44,
        "the fixture floor has shrunk: 44 request types were captured"
    );
    let shared = shared([usb()]);
    let mut session = Session::default();
    for (name, text) in CAPTURED_REQUESTS {
        let request = decode(text);
        let replies = ask(&mut session, &shared, &request);
        assert!(
            replies.iter().all(|reply| reply.kind != MessageKind::ERROR),
            "{name} drew a refusal: {replies:?}"
        );
        if matches!(
            request.kind,
            MessageKind::MENU_CLOSE | MessageKind::UNKNOWN_3001
        ) {
            // The two requests a real server answers with nothing at all. For
            // `MENU_CLOSE` no state may be discarded either (F16, F27); for
            // `0x3001` a reply would be read as the answer to the next request
            // and put every reply after it one behind.
            assert!(replies.is_empty(), "{name} should draw nothing");
        } else {
            assert!(!replies.is_empty(), "{name} drew nothing at all");
            assert!(
                replies
                    .iter()
                    .all(|reply| reply.transaction_id == request.transaction_id),
                "{name} answered under the wrong transaction id"
            );
        }
    }
}

#[test]
fn the_undecoded_request_types_are_acknowledged_rather_than_refused() {
    // `0x3001`, `0x3401` and `0x3b03` appear around a loaded track and nobody
    // has decoded any of them. A real deck answers them with a `SUCCESS`
    // carrying a count we cannot explain; we answer zero, which is the same
    // shape and is not a refusal.
    //
    // **`0x3903` used to be in this list and is not undecoded any more.** It is
    // "describe this medium", and answering it as an unknown request cost a
    // real deck its whole browse session — see
    // `a_medium_description_is_not_an_acknowledgement` below.
    let shared = shared([usb()]);
    let mut session = Session::default();
    for name in ["unknown_3401", "unknown_3b03"] {
        let text = CAPTURED_REQUESTS
            .iter()
            .find(|(fixture, _)| *fixture == name)
            .map(|(_, text)| *text)
            .expect("a captured fixture");
        let request = decode(text);
        let replies = ask(&mut session, &shared, &request);
        assert_eq!(replies.len(), 1, "{name}");
        assert_eq!(replies[0].kind, MessageKind::SUCCESS, "{name}");
        assert_eq!(
            replies[0].number(0),
            Some(u32::from(request.kind.0)),
            "{name} echoes its own type"
        );
    }
}

#[test]
fn nothing_is_sent_back_for_0x3001() {
    // A deck sends `0x3001` about a minute after a load, and across three
    // deck-to-deck captures a real server answers it **not once** — six
    // requests, zero replies. Answering it is what cost a real deck its browse
    // session: the reply nobody asked for is read as the answer to the
    // `GET_METADATA` that follows, so metadata comes back empty and every reply
    // after that is one behind. Menus blank, the title falls back to the
    // medium's name, the waveform stops. The stream stays perfectly framed the
    // whole time, which is why no framing check finds it.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let text = CAPTURED_REQUESTS
        .iter()
        .find(|(fixture, _)| *fixture == "unknown_3001")
        .map(|(_, text)| *text)
        .expect("a captured fixture");

    let replies = ask(&mut session, &shared, &decode(text));
    assert!(
        replies.is_empty(),
        "answered with {replies:?}; a real server says nothing"
    );
}

#[test]
fn a_medium_description_is_not_an_acknowledgement() {
    // `0x3903` is answered by a real deck with `0x4902` carrying a 148-byte
    // body describing the medium — the same volume name, creation date, counts
    // and sizes the UDP media query returns, little-endian. Answering it with a
    // bare `SUCCESS`, as an unknown request, is what left a deck with blank
    // menus, the track title replaced by the medium's own name and no scrolling
    // waveform, until the DJ left LINK and came back.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let text = CAPTURED_REQUESTS
        .iter()
        .find(|(fixture, _)| *fixture == "unknown_3903")
        .map(|(_, text)| *text)
        .expect("a captured fixture");

    let replies = ask(&mut session, &shared, &decode(text));
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].kind, MessageKind::MEDIA_INFO);
    assert_eq!(
        replies[0].number(0),
        Some(u32::from(MessageKind::GET_MEDIA_INFO.0))
    );

    let body = replies[0].blob(3).expect("the body");
    let info = MediaInfo::parse(body).expect("it parses");
    assert_eq!(info.track_count, 651, "the true count, as everywhere else");
}

#[test]
fn the_three_documented_undocumented_requests_get_the_replies_a_deck_sends() {
    let shared = shared([usb()]);
    let mut session = Session::default();

    let replies = ask(
        &mut session,
        &shared,
        &decode(
            CAPTURED_REQUESTS
                .iter()
                .find(|(n, _)| *n == "unknown_3e03")
                .expect("fixture")
                .1,
        ),
    );
    // Modelled byte for byte on a real reply between two players: `0x4b02`
    // with `[0x3e03, 0, our number, ""]`.
    assert_eq!(replies[0].kind, MessageKind::UNKNOWN_4B02);
    assert_eq!(
        replies[0].number(0),
        Some(u32::from(MessageKind::UNKNOWN_3E03.0))
    );
    assert_eq!(replies[0].number(2), Some(2), "our own device number");
    assert_eq!(replies[0].text(3), Some(""));

    for name in ["unknown_3100", "unknown_3d03"] {
        let text = CAPTURED_REQUESTS
            .iter()
            .find(|(n, _)| *n == name)
            .expect("fixture")
            .1;
        let replies = ask(&mut session, &shared, &decode(text));
        assert_eq!(replies[0].kind, MessageKind::SUCCESS, "{name}");
    }
}

#[test]
fn introduce_answers_with_our_own_player_number_not_a_count() {
    // The one `SUCCESS` whose argument 1 is not an item count (F7).
    let shared = shared([usb()]);
    let mut session = Session::default();
    let text = CAPTURED_REQUESTS
        .iter()
        .find(|(n, _)| *n == "introduce")
        .expect("fixture")
        .1;
    let replies = ask(&mut session, &shared, &decode(text));
    assert_eq!(replies[0].kind, MessageKind::SUCCESS);
    assert_eq!(replies[0].number(0), Some(0));
    assert_eq!(replies[0].number(1), Some(2));
    assert_eq!(replies[0].transaction_id, dbserver::SETUP_TRANSACTION_ID);
}

// -- the two fixed menus, against a real deck's bytes ----------------------

#[test]
fn the_root_menu_is_what_a_real_deck_sends_minus_the_category_we_cannot_answer() {
    let real: Vec<MenuItem> = REAL_ROOT_MENU
        .iter()
        .map(|text| MenuItem::from_message(&decode(text)).expect("a menu item"))
        .collect();
    assert_eq!(real.len(), 12, "a real root menu is all twelve");

    let ours = menu::build(MessageKind::MENU_ROOT, &Arguments::default(), None, &[])
        .expect("the root menu needs no medium");
    let expected: Vec<MenuItem> = real
        .into_iter()
        .filter(|item| item.label1 != menu_label("FOLDER"))
        .collect();
    assert_eq!(
        ours, expected,
        "every row we serve must match the real one exactly: id, item type and the \
         U+FFFA wrapping are each enough on their own to stop a deck opening it (F26)"
    );
}

#[test]
fn the_sort_menu_re_encodes_byte_for_byte_as_a_real_deck_sent_it() {
    // A round trip between our own encoder and our own decoder proves they
    // agree with each other, which is not the same as agreeing with a CDJ.
    let ours = menu::build(MessageKind::MENU_SORT, &Arguments::default(), None, &[])
        .expect("the sort menu needs no medium");
    assert_eq!(ours.len(), REAL_SORT_MENU.len());
    for (item, text) in ours.iter().zip(REAL_SORT_MENU) {
        let real = hex(text);
        assert_eq!(
            item.to_message(REAL_SORT_TRANSACTION).encode(),
            real,
            "our {:?} differs from the bytes a real deck sent",
            item.label1
        );
    }
}

#[test]
fn we_advertise_no_category_we_cannot_answer() {
    // An unimplemented category and an empty one are indistinguishable on a
    // deck's screen, so every row of our root menu must lead somewhere (F40).
    let root = menu::build(MessageKind::MENU_ROOT, &Arguments::default(), None, &[]).expect("root");
    let labels: Vec<&str> = ROOT_CATEGORIES
        .iter()
        .filter(|category| {
            root.iter()
                .any(|item| item.label1 == menu_label(category.label))
        })
        .map(|category| category.label)
        .collect();
    assert_eq!(
        labels,
        menu::SERVED,
        "the served set and the root menu must agree"
    );
    assert!(
        !labels.contains(&"FOLDER"),
        "FOLDER browses unanalysed files by directory, which a pdb does not describe"
    );
}

// -- concurrent menus, and the close that discards nothing -----------------

#[test]
fn a_metadata_menu_does_not_evict_a_track_list_of_the_same_size() {
    // F27 keyed result sets on the item count, which worked until a metadata
    // reply became thirteen items and collided with a thirteen-track album:
    // browsing that album then served metadata for every page (F41). The
    // descriptor's menu-target byte is what separates them.
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();

    // A thirteen-track album from the real library, and a metadata lookup,
    // which is also thirteen items.
    let album = thirteen_track_album(medium.library()).expect("a thirteen-track album");
    let list = Message::menu_request(
        1,
        drill_kind(1, 0x03),
        descriptor(Slot::USB, MenuTarget::MAIN),
        &[SortOrder::DEFAULT.0, album],
    )
    .expect("a drill request");
    let list_replies = ask(&mut session, &shared, &list);
    assert_eq!(
        list_replies[0].number(1),
        Some(13),
        "a thirteen-track album"
    );

    let track = list_replies[0].transaction_id;
    let metadata = Message::menu_request(
        track + 1,
        MessageKind::GET_METADATA,
        descriptor(Slot::USB, MenuTarget::SUB),
        &[first_track_of(medium.library(), album)],
    )
    .expect("a metadata request");
    let metadata_replies = ask(&mut session, &shared, &metadata);
    assert_eq!(
        metadata_replies[0].number(1),
        Some(13),
        "thirteen items (F32)"
    );

    // The deck now resumes the album at the next offset without re-issuing the
    // menu request. It must still be the album.
    let render = Message::render_of(
        3,
        descriptor(Slot::USB, MenuTarget::MAIN),
        0,
        MAX_RENDER_BATCH,
        13,
    );
    let page = ask(&mut session, &shared, &render);
    let rows: Vec<MenuItem> = page.iter().filter_map(MenuItem::from_message).collect();
    assert!(
        rows.iter()
            .all(|row| row.item_type == SortOrder::DEFAULT.track_item_type()),
        "the album's rows, not the metadata menu's: {rows:?}"
    );
}

#[test]
fn menu_close_draws_nothing_and_discards_nothing() {
    // A deck sends this while still scrolling the list it is supposedly
    // finished with, so honouring it destroys the result set mid-scroll
    // (F16, F27).
    let shared = shared([usb()]);
    let mut session = Session::default();
    let request = menu(MessageKind::MENU_ARTIST, &[SortOrder::DEFAULT.0]);
    let total = ask(&mut session, &shared, &request)[0]
        .number(1)
        .expect("a count");

    let close = Message::new(request.transaction_id, MessageKind::MENU_CLOSE, []);
    assert!(
        ask(&mut session, &shared, &close).is_empty(),
        "MENU_CLOSE draws no reply at all"
    );

    let render = Message::render_of(9, descriptor(Slot::USB, MenuTarget::MAIN), 0, 6, total);
    let rows: Vec<MenuItem> = ask(&mut session, &shared, &render)
        .iter()
        .filter_map(MenuItem::from_message)
        .collect();
    assert_eq!(rows.len(), 6, "the list survived the close");
}

#[test]
fn the_pending_menu_table_is_bounded() {
    // A connection that never forgets a result set grows for as long as a DJ
    // browses. Nothing a deck has been observed to do comes near the bound.
    let mut session = Session::default();
    for count in 0..u32::try_from(MAX_PENDING_MENUS * 2).unwrap_or(0) {
        session.remember(0x0102_0301, count, Vec::new());
    }
    assert_eq!(session.menus.len(), MAX_PENDING_MENUS);
    assert_eq!(session.order.len(), MAX_PENDING_MENUS);
}

// -- two media on one connection ------------------------------------------

#[test]
fn the_medium_is_resolved_from_every_message_not_cached_on_the_connection() {
    // A player browsing two media on one peer opens a single connection and
    // tells them apart purely by the descriptor's slot byte (F37).
    let sd = Arc::new(Medium::synthetic(ServedSlot::SD, other_library(), "SD"));
    let shared = shared([usb(), sd]);
    let mut session = Session::default();

    let count = |session: &mut Session, slot: Slot| {
        let request = Message::menu_request(
            1,
            MessageKind::MENU_ARTIST,
            descriptor(slot, MenuTarget::MAIN),
            &[SortOrder::DEFAULT.0],
        )
        .expect("a menu request");
        ask(session, &shared, &request)[0]
            .number(1)
            .expect("a count")
    };

    assert_eq!(count(&mut session, Slot::SD), 1, "the one-artist medium");
    assert_eq!(count(&mut session, Slot::USB), 290, "the real medium");
    // Interleaved, on the same session, which is what a deck actually does.
    assert_eq!(count(&mut session, Slot::SD), 1);
}

#[test]
fn a_slot_we_do_not_serve_is_an_empty_menu_and_not_a_refusal() {
    // With two media there is no falling back: the slot byte is the only thing
    // distinguishing them, so a third slot names nothing.
    let sd = Arc::new(Medium::synthetic(ServedSlot::SD, other_library(), "SD"));
    let shared = shared([usb(), sd]);
    let mut session = Session::default();
    let request = Message::menu_request(
        1,
        MessageKind::MENU_ARTIST,
        descriptor(Slot::CD, MenuTarget::MAIN),
        &[SortOrder::DEFAULT.0],
    )
    .expect("a menu request");
    let replies = ask(&mut session, &shared, &request);
    assert_eq!(replies[0].kind, MessageKind::SUCCESS);
    assert_eq!(replies[0].number(1), Some(0));
}

#[test]
fn one_medium_answers_a_request_that_names_any_slot() {
    // A deck asks about the slot it thinks it is browsing, and a unit with one
    // medium has nothing else to offer.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let request = Message::menu_request(
        1,
        MessageKind::MENU_ARTIST,
        descriptor(Slot::SD, MenuTarget::MAIN),
        &[SortOrder::DEFAULT.0],
    )
    .expect("a menu request");
    assert_eq!(ask(&mut session, &shared, &request)[0].number(1), Some(290));
}

// -- the browse surface ---------------------------------------------------

#[test]
fn a_category_lists_only_the_rows_a_track_references() {
    // A rekordbox medium carries artist rows nothing points at; they arrive
    // through remixer and composer references, which no track list browses by.
    // Confirmed against hardware: 329 rows in the table, 290 in the menu.
    let library = library();
    assert_eq!(library.artists.len(), 329, "the table");
    let shared = shared([usb()]);
    let mut session = Session::default();
    let rows = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_ARTIST, &[SortOrder::DEFAULT.0]),
    );
    assert_eq!(rows.len(), 290, "the menu");
    assert!(rows.iter().all(|row| row.item_type == ItemType::ARTIST));
    assert!(rows.iter().all(|row| row.flags == 0 && row.argument0 == 0));
}

#[test]
fn camelot_keys_come_out_in_wheel_order_and_not_as_text() {
    // The one place we deliberately differ from the hardware (§8): a CDJ sorts
    // key names as text, which interleaves the wheel positions and puts two
    // harmonically adjacent keys eleven screens apart.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let rows = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_KEY, &[SortOrder::DEFAULT.0]),
    );
    let labels: Vec<&str> = rows.iter().map(|row| row.label1.as_str()).collect();
    assert_eq!(
        labels,
        [
            "1A", "1B", "2A", "2B", "3A", "3B", "4A", "4B", "5A", "5B", "6A", "6B", "7A", "7B",
            "8A", "8B", "9A", "9B", "10A", "10B", "11A", "11B", "12A", "12B"
        ]
    );
}

#[test]
fn bitrates_are_listed_highest_first_with_no_label_at_all() {
    // A real deck sends 2116, 320, 256, 224, 192, 160 — descending — and leaves
    // both labels empty, because the value is its own label and the deck
    // formats it.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let rows = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_BITRATE, &[SortOrder::DEFAULT.0]),
    );
    assert!(rows.len() > 1);
    assert!(
        rows.iter()
            .all(|row| row.label1.is_empty() && row.label2.is_empty())
    );
    assert!(rows.iter().all(|row| row.item_type == ItemType::BITRATE));
    let rates: Vec<u32> = rows.iter().map(|row| row.id).collect();
    let mut descending = rates.clone();
    descending.sort_by(|a, b| b.cmp(a));
    assert_eq!(rates, descending);
}

#[test]
fn all_heads_a_filtered_list_only_when_there_is_more_than_one_entry() {
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();

    // A genre with several artists gets the ALL row...
    let (crowded, lonely) = genres_by_artist_count(medium.library());
    let rows = browse(
        &mut session,
        &shared,
        &menu(drill_kind(1, 0x01), &[SortOrder::DEFAULT.0, crowded]),
    );
    assert_eq!(rows.first().map(|row| row.id), Some(FILTER_ALL));
    assert_eq!(rows.first().map(|row| row.item_type), Some(ItemType::ALL));
    assert_eq!(
        rows.first().map(|row| row.label1.clone()),
        Some(menu_label("ALL"))
    );

    // ...and one with a single artist goes out bare.
    let rows = browse(
        &mut session,
        &shared,
        &menu(drill_kind(1, 0x01), &[SortOrder::DEFAULT.0, lonely]),
    );
    assert_eq!(rows.len(), 1, "a single-entry level has no choice to make");
    assert_ne!(rows[0].id, FILTER_ALL);
}

#[test]
fn choosing_all_does_not_narrow_that_level() {
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();
    let (genre, _) = genres_by_artist_count(medium.library());

    // GENRE narrows to an artist, then an album, then tracks. Taking ALL at
    // both intermediate levels must still reach every track of the genre.
    let everything = browse(
        &mut session,
        &shared,
        &menu(
            drill_kind(3, 0x01),
            &[SortOrder::DEFAULT.0, genre, FILTER_ALL, FILTER_ALL],
        ),
    );
    let expected = medium
        .library()
        .tracks
        .values()
        .filter(|track| track.genre_id == genre)
        .count();
    assert_eq!(
        everything.len(),
        expected,
        "ALL means do not narrow at that level"
    );

    // And picking one artist instead narrows to that artist alone.
    let artists = browse(
        &mut session,
        &shared,
        &menu(drill_kind(1, 0x01), &[SortOrder::DEFAULT.0, genre]),
    );
    let one = artists
        .iter()
        .find(|row| row.id != FILTER_ALL)
        .expect("an artist row")
        .id;
    let narrowed = browse(
        &mut session,
        &shared,
        &menu(
            drill_kind(3, 0x01),
            &[SortOrder::DEFAULT.0, genre, one, FILTER_ALL],
        ),
    );
    assert!(!narrowed.is_empty() && narrowed.len() < everything.len());
    assert!(
        narrowed
            .iter()
            .all(|row| medium.library().tracks[&row.id].artist_id == one)
    );
}

#[test]
fn drilling_into_a_key_offers_three_tolerances_before_any_track() {
    // Choosing a key does not list its tracks: a real player offers three
    // widening harmonic matches, all with the same key id and differing only in
    // argument 0, and `0x1214` then takes `(key id, tolerance)` (F44).
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();
    let key_id = *medium
        .library()
        .keys
        .iter()
        .find(|(_, name)| *name == "1A")
        .expect("the library holds 1A")
        .0;

    let tolerances = browse(
        &mut session,
        &shared,
        &menu(drill_kind(1, 0x14), &[SortOrder::DEFAULT.0, key_id]),
    );
    assert_eq!(tolerances.len(), 3);
    assert!(tolerances.iter().all(|row| row.id == key_id));
    assert_eq!(
        tolerances
            .iter()
            .map(|row| row.argument0)
            .collect::<Vec<_>>(),
        [0, 1, 2],
        "the tolerance travels in argument 0"
    );
    assert_eq!(tolerances[0].label1, "1A");
    assert_eq!(tolerances[1].label1, "1A, 1B", "plus the relative");
    assert_eq!(
        tolerances[2].label1, "1A, 1B, 12A, 2A",
        "plus the adjacent wheel positions"
    );

    // And the level below widens the track list as the tolerance widens.
    let narrow = browse(
        &mut session,
        &shared,
        &menu(drill_kind(2, 0x14), &[SortOrder::DEFAULT.0, key_id, 0]),
    );
    let wide = browse(
        &mut session,
        &shared,
        &menu(drill_kind(2, 0x14), &[SortOrder::DEFAULT.0, key_id, 2]),
    );
    assert!(!narrow.is_empty());
    assert!(
        wide.len() > narrow.len(),
        "a wider tolerance reaches more tracks"
    );
}

#[test]
fn every_drill_type_the_grid_generates_is_answered() {
    // All thirteen types seen in one exhaustive session come from
    // `0x1000 | depth << 8 | category` (F42). Implementing it as a grid is what
    // makes LABEL, BITRATE, HISTORY and KEY work at all.
    let shared = shared([usb()]);
    let mut session = Session::default();
    // ALL at every narrowing level, except the two categories whose first
    // filter identifies a thing rather than narrowing to one: a history
    // playlist and a key, where `0xffffffff` names nothing.
    let history = 1;
    let key = 1;
    let grid: [(u8, u8, &[u32]); 13] = [
        (1, 0x01, &[FILTER_ALL]),
        (1, 0x02, &[FILTER_ALL]),
        (1, 0x03, &[FILTER_ALL]),
        (1, 0x0a, &[FILTER_ALL]),
        (1, 0x11, &[FILTER_ALL]),
        (1, 0x12, &[history]),
        (1, 0x14, &[key]),
        (2, 0x01, &[FILTER_ALL, FILTER_ALL]),
        (2, 0x02, &[FILTER_ALL, FILTER_ALL]),
        (2, 0x0a, &[FILTER_ALL, FILTER_ALL]),
        (2, 0x14, &[key, 0]),
        (3, 0x01, &[FILTER_ALL, FILTER_ALL, FILTER_ALL]),
        (3, 0x0a, &[FILTER_ALL, FILTER_ALL, FILTER_ALL]),
    ];
    for (depth, category, filters) in grid {
        let kind = drill_kind(depth, category);
        assert_eq!(
            Drill::parse(kind),
            Some(Drill { depth, category }),
            "the grid formula must round-trip"
        );
        let mut extra = vec![SortOrder::DEFAULT.0];
        extra.extend_from_slice(filters);
        let replies = ask(&mut session, &shared, &menu(kind, &extra));
        assert_eq!(replies[0].kind, MessageKind::SUCCESS, "{kind:?}");
        assert!(
            replies[0].number(1).is_some_and(|count| count > 0),
            "{kind:?} came back empty, which is indistinguishable from a refusal"
        );
    }
}

#[test]
fn history_lists_newest_first_and_drills_to_the_order_the_tracks_were_played() {
    // A real deck answers HISTORY 002 before HISTORY 001.
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();
    let lists = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_HISTORY, &[SortOrder::DEFAULT.0]),
    );
    assert_eq!(lists.len(), 7);
    assert!(
        lists
            .iter()
            .all(|row| row.item_type == ItemType::HISTORY_PLAYLIST)
    );
    let ids: Vec<u32> = lists.iter().map(|row| row.id).collect();
    assert_eq!(ids, [7, 6, 5, 4, 3, 2, 1], "newest first");

    let first = lists.last().expect("a history list").id;
    let tracks = browse(
        &mut session,
        &shared,
        &menu(drill_kind(1, 0x12), &[SortOrder::DEFAULT.0, first]),
    );
    let played: Vec<u32> = medium
        .library()
        .history
        .get(&first)
        .expect("the list")
        .track_ids
        .clone();
    assert_eq!(
        tracks.iter().map(|row| row.id).collect::<Vec<_>>(),
        played,
        "DEFAULT keeps the order they were played in"
    );
    assert_eq!(
        tracks
            .iter()
            .map(|row| row.playlist_position)
            .collect::<Vec<_>>(),
        (1..=u32::try_from(played.len()).unwrap_or(0)).collect::<Vec<_>>(),
        "argument 9 is the 1-based position in the list"
    );
}

#[test]
fn a_playlist_keeps_its_curated_order_under_default_and_sorts_under_anything_else() {
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();
    let playlist = *medium
        .library()
        .playlists
        .keys()
        .next()
        .expect("the library has a playlist");
    let curated = medium.library().playlists[&playlist].track_ids.clone();

    let default = browse(
        &mut session,
        &shared,
        &menu(
            MessageKind::MENU_PLAYLIST,
            &[SortOrder::DEFAULT.0, playlist, 0],
        ),
    );
    assert_eq!(
        default.iter().map(|row| row.id).collect::<Vec<_>>(),
        curated,
        "DEFAULT inside a playlist is the curated order, not an alphabetical one"
    );

    let alphabetical = browse(
        &mut session,
        &shared,
        &menu(
            MessageKind::MENU_PLAYLIST,
            &[SortOrder::TITLE.0, playlist, 0],
        ),
    );
    assert_eq!(alphabetical.len(), default.len());
    assert_ne!(
        alphabetical.iter().map(|row| row.id).collect::<Vec<_>>(),
        curated,
        "and any other sort applies, because most browsing happens inside a playlist"
    );
}

#[test]
fn the_playlist_tree_lists_folders_first_and_carries_their_sort_order() {
    let shared = shared([usb()]);
    let mut session = Session::default();
    let rows = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_PLAYLIST, &[SortOrder::DEFAULT.0, 0, 1]),
    );
    assert!(!rows.is_empty());
    let folders_first = rows
        .iter()
        .map(|row| row.item_type == ItemType::FOLDER)
        .collect::<Vec<_>>();
    assert!(
        folders_first.windows(2).all(|pair| pair[0] >= pair[1]),
        "a real deck lists every folder before any playlist"
    );
    assert!(
        rows.iter()
            .all(|row| matches!(row.item_type, ItemType::FOLDER | ItemType::PLAYLIST))
    );
}

#[test]
fn search_returns_matching_artists_then_albums_then_tracks() {
    // A real deck's search result is not a track list: searching `H` came back
    // with artist rows carrying argument 0 = 1, and searching `HEL` with a
    // track row carrying 3. That is how the deck knows a click on the first
    // means an ARTIST drill and a click on the last means a load.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let request = Message::search(
        1,
        descriptor(Slot::USB, MenuTarget::MAIN),
        SortOrder::DEFAULT,
        "a",
    );
    let rows = browse(&mut session, &shared, &request);
    assert!(!rows.is_empty());

    let kinds: Vec<u32> = rows.iter().map(|row| row.argument0).collect();
    assert!(
        kinds.windows(2).all(|pair| pair[0] <= pair[1]),
        "artists, then albums, then tracks — never interleaved"
    );
    for row in &rows {
        match row.argument0 {
            1 => assert_eq!(row.item_type, ItemType::ARTIST),
            2 => assert_eq!(row.item_type, ItemType::ALBUM),
            3 => {
                assert_eq!(row.item_type, ItemType::TRACK_TITLE);
                assert_eq!(row.flags, MenuItem::TRACK_FLAGS);
            }
            other => panic!("a search row carrying {other}"),
        }
    }
    assert!(kinds.contains(&1) && kinds.contains(&3));
}

#[test]
fn a_search_row_can_be_drilled_into_the_way_a_deck_drills_into_one() {
    // The deck answered a click on a search result with `0x1102` carrying the
    // row's id, which is an ordinary ARTIST drill.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let request = Message::search(
        1,
        descriptor(Slot::USB, MenuTarget::MAIN),
        SortOrder::DEFAULT,
        "beau",
    );
    let rows = browse(&mut session, &shared, &request);
    let artist = rows
        .iter()
        .find(|row| row.argument0 == 1)
        .expect("a matching artist");
    let albums = browse(
        &mut session,
        &shared,
        &menu(drill_kind(1, 0x02), &[SortOrder::DEFAULT.0, artist.id]),
    );
    assert!(
        !albums.is_empty(),
        "the artist row must open onto something"
    );
}

// -- sorting --------------------------------------------------------------

#[test]
fn the_sort_selects_the_second_column_of_every_row() {
    // The feature that makes sorting useful rather than cosmetic. The item type
    // is `(column field type << 8) | 0x04`, so `0x0704` is not "title and
    // artist" but a track whose second column is the ARTIST field (F43).
    let shared = shared([usb()]);
    let mut session = Session::default();
    for option in SORT_MENU {
        let rows = browse(
            &mut session,
            &shared,
            &menu(MessageKind::MENU_TRACK, &[option.sort.0]),
        );
        assert_eq!(rows.len(), 651, "{:?}", option.sort);
        let expected = option.sort.track_item_type();
        assert!(
            rows.iter().all(|row| row.item_type == expected),
            "{:?} should type its rows {expected:?}",
            option.sort
        );
        if option.sort.column_is_numeric() {
            // A numeric column sends an empty label and puts the raw number in
            // argument 0; the deck formats it.
            assert!(
                rows.iter().all(|row| row.label2.is_empty()),
                "{:?} is a numeric column and must send no text",
                option.sort
            );
            assert!(
                rows.iter().any(|row| row.argument0 != 0),
                "{:?} must put its number in argument 0",
                option.sort
            );
        } else {
            assert!(
                rows.iter().any(|row| !row.label2.is_empty()),
                "{:?} is a text column and must send text",
                option.sort
            );
        }
        assert!(rows.iter().all(|row| row.flags == MenuItem::TRACK_FLAGS));
    }
}

#[test]
fn a_text_column_carries_the_referenced_rows_id_and_the_date_column_the_tracks_own() {
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();

    let by_artist = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_TRACK, &[SortOrder::ARTIST.0]),
    );
    for row in by_artist.iter().take(20) {
        let track = &medium.library().tracks[&row.id];
        assert_eq!(row.argument0, track.artist_id);
        assert_eq!(row.label2, track.artist);
    }

    let by_date = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_TRACK, &[SortOrder::DATE_ADDED.0]),
    );
    for row in by_date.iter().take(20) {
        let track = &medium.library().tracks[&row.id];
        assert_eq!(
            row.argument0, track.id,
            "the date column's id is the track's own"
        );
        assert_eq!(row.label2, track.date_added);
    }
}

#[test]
fn the_default_sort_of_a_track_list_is_by_title() {
    // A real deck answering MENU_TRACK with sort 0 returns titles in order
    // whatever their artists; the reference used the library's own
    // artist-then-title order.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let rows = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_TRACK, &[SortOrder::DEFAULT.0]),
    );
    let titles: Vec<String> = rows.iter().map(|row| row.label1.to_lowercase()).collect();
    let mut sorted = titles.clone();
    sorted.sort();
    assert_eq!(titles, sorted);
}

#[test]
fn a_numeric_sort_where_more_is_better_puts_the_largest_first() {
    let shared = shared([usb()]);
    let mut session = Session::default();
    for (sort, descending) in [
        (SortOrder::BPM, false),
        (SortOrder::BITRATE, false),
        (SortOrder::RATING, true),
        (SortOrder::PLAY_COUNT, true),
        (SortOrder::DATE_ADDED, true),
    ] {
        let rows = browse(
            &mut session,
            &shared,
            &menu(MessageKind::MENU_TRACK, &[sort.0]),
        );
        let values: Vec<u32> = rows.iter().map(|row| row.argument0).collect();
        let ordered = if descending {
            values.windows(2).all(|pair| pair[0] >= pair[1])
        } else {
            values.windows(2).all(|pair| pair[0] <= pair[1])
        };
        // DATE ADDED is a text column: its argument 0 is the track id, so only
        // the label is ordered.
        if sort == SortOrder::DATE_ADDED {
            let dates: Vec<&str> = rows.iter().map(|row| row.label2.as_str()).collect();
            assert!(
                dates.windows(2).all(|pair| pair[0] >= pair[1]),
                "newest first"
            );
        } else {
            assert!(ordered, "{sort:?}");
        }
    }
}

#[test]
fn a_track_row_in_a_plain_list_carries_its_number_within_its_album() {
    // Argument 9 is not always zero: `ACXD` came back from a real deck as 23.
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();
    let rows = browse(
        &mut session,
        &shared,
        &menu(MessageKind::MENU_TRACK, &[SortOrder::DEFAULT.0]),
    );
    for row in rows.iter().take(50) {
        assert_eq!(
            row.playlist_position,
            medium.library().tracks[&row.id].track_number
        );
    }
    assert!(rows.iter().any(|row| row.playlist_position > 1));
}

// -- one track ------------------------------------------------------------

#[test]
fn metadata_is_thirteen_items_each_carrying_the_row_it_references() {
    // Thirteen, not nine: a player renders whatever it is given and looks
    // entirely correct with colour, date added, bitrate and label missing (F32).
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();
    let track_id = 91;
    let track = &medium.library().tracks[&track_id];

    let rows = browse(
        &mut session,
        &shared,
        &menu(MessageKind::GET_METADATA, &[track_id]),
    );
    assert_eq!(rows.len(), 13);
    let types: Vec<ItemType> = rows.iter().map(|row| row.item_type).collect();
    assert_eq!(
        types,
        METADATA_ITEMS
            .iter()
            .map(|slot| slot.item_type)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        rows.iter().map(|row| row.argument0).collect::<Vec<_>>(),
        METADATA_ITEMS
            .iter()
            .map(|slot| slot.argument0)
            .collect::<Vec<_>>(),
        "argument 0 is 1 on eight of the thirteen and 0 on the rest; the split matches \
         no rule we can name and is reproduced as observed"
    );

    assert_eq!(rows[0].id, track.id);
    assert_eq!(rows[0].label1, track.title);
    assert_eq!(
        rows[0].artwork_id, track.artwork_id,
        "the title item carries the artwork id, or INFO shows no cover"
    );
    assert_eq!(rows[0].flags, MenuItem::TRACK_FLAGS);
    assert_eq!(
        rows[1].id, track.artist_id,
        "the artist's id, not the track's"
    );
    assert_eq!(
        rows[2].id, track.album_id,
        "the album's id, not the track's"
    );
    assert_eq!(rows[6].id, track.key_id);
    assert_eq!(rows[9].id, track.genre_id);
    assert!(
        rows[1..]
            .iter()
            .all(|row| row.flags == 0 && row.artwork_id == 0)
    );
}

#[test]
fn metadata_for_a_track_we_do_not_have_is_empty_and_not_an_error() {
    let shared = shared([usb()]);
    let mut session = Session::default();
    let replies = ask(
        &mut session,
        &shared,
        &menu(MessageKind::GET_METADATA, &[0xdead_beef]),
    );
    assert_eq!(replies[0].kind, MessageKind::SUCCESS);
    assert_eq!(replies[0].number(1), Some(0));
}

#[test]
fn track_info_is_six_items_with_the_container_first_and_the_file_size_on_the_path() {
    // Returning only the path renders the track and walks it over NFS and is
    // not enough to load it (F31). `0x04` is the title in a metadata reply and
    // the container here (F35), and argument 0 of the path item is the file
    // size — zero on every other menu item ever captured.
    let medium = usb();
    let shared = shared([Arc::clone(&medium)]);
    let mut session = Session::default();
    let track_id = 91;
    let track = &medium.library().tracks[&track_id];

    let rows = browse(
        &mut session,
        &shared,
        &menu(MessageKind::GET_TRACK_INFO, &[track_id]),
    );
    assert_eq!(rows.len(), 6);
    assert_eq!(
        rows.iter().map(|row| row.item_type).collect::<Vec<_>>(),
        TRACK_INFO_ITEMS.to_vec()
    );
    assert_eq!(
        rows[0].id,
        u32::from(track.container.0),
        "item 1 is the container, and announcing the wrong one makes a deck fetch the \
         file and refuse to decode it"
    );
    assert!(rows[0].label1.is_empty());
    assert_eq!(rows[1].id, u32::from(track.duration));
    assert_eq!(rows[2].id, track.tempo);
    assert_eq!(rows[4].label1, track.file_path);
    assert_eq!(rows[4].argument0, track.file_size, "the file size (F31)");
    assert_ne!(rows[4].argument0, 0);
    assert_eq!(rows[5].id, 1, "constant on every container ever captured");
    assert!(
        rows.iter()
            .take(4)
            .chain(rows.iter().skip(5))
            .all(|row| row.argument0 == 0),
        "argument 0 is zero on every item but the path"
    );
}

// -- binary replies -------------------------------------------------------

#[test]
fn a_track_with_no_analysis_answers_with_an_empty_blob_and_not_an_error() {
    // A synthetic medium has no files behind it, which is the same shape as a
    // track analysed by an older rekordbox: the request is answered, and the
    // blob is absent from the wire entirely.
    let shared = shared([usb()]);
    let mut session = Session::default();
    for (name, kind, reply) in [
        ("artwork", MessageKind::GET_ARTWORK, MessageKind::ARTWORK),
        ("vbr", MessageKind::GET_VBR_INDEX, MessageKind::VBR_INDEX),
        ("grid", MessageKind::GET_BEAT_GRID, MessageKind::BEAT_GRID),
        (
            "detail",
            MessageKind::GET_WAVEFORM_DETAIL,
            MessageKind::WAVEFORM_DETAIL,
        ),
    ] {
        let replies = ask(&mut session, &shared, &menu(kind, &[91]));
        assert_eq!(replies.len(), 1, "{name}");
        assert_eq!(replies[0].kind, reply, "{name}");
        assert_eq!(
            replies[0].number(0),
            Some(u32::from(kind.0)),
            "{name}: argument 0 echoes the request type, not the track id"
        );
        assert_eq!(replies[0].number(2), Some(0), "{name}");
        assert_eq!(replies[0].blob(3), Some(&[][..]), "{name}");
    }
}

#[test]
fn the_waveform_preview_reads_its_track_id_from_argument_two() {
    // Its arguments are `[descriptor, 3, track id, 0, b""]`, so reading
    // argument 1 asks for the analysis of track 3.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let request = decode(
        CAPTURED_REQUESTS
            .iter()
            .find(|(name, _)| *name == "get_waveform_preview")
            .expect("fixture")
            .1,
    );
    assert_eq!(request.number(1), Some(2), "not a track id");
    assert_eq!(request.number(2), Some(0x468), "the track id");
    let replies = ask(&mut session, &shared, &request);
    assert_eq!(replies[0].kind, MessageKind::WAVEFORM_PREVIEW);
}

#[test]
fn the_cue_points_reply_carries_two_blobs_and_the_record_size() {
    let shared = shared([usb()]);
    let mut session = Session::default();
    let replies = ask(
        &mut session,
        &shared,
        &menu(MessageKind::GET_CUE_POINTS, &[91]),
    );
    assert_eq!(replies[0].kind, MessageKind::CUE_POINTS);
    assert_eq!(replies[0].args.len(), 9);
    assert_eq!(
        replies[0].number(0),
        Some(u32::from(MessageKind::GET_CUE_POINTS.0))
    );
    assert_eq!(
        replies[0].number(4),
        Some(u32::try_from(prolink_proto::analysis::CUE_ENTRY_LEN).unwrap_or(0))
    );
}

#[test]
fn the_beat_grid_prefix_word_is_never_zero() {
    // With zero there the main waveform does not draw (F33), and the value must
    // not go backwards, so it comes from a monotonic clock.
    let early = prolink_proto::analysis::PrefixWord::from_elapsed(Duration::ZERO);
    let later = prolink_proto::analysis::PrefixWord::from_elapsed(Duration::from_secs(1));
    assert_ne!(early.get(), 0);
    assert!(later.get() > early.get());
}

// -- rendering ------------------------------------------------------------

#[test]
fn a_render_never_exceeds_the_documented_batch_size() {
    // Sixty-four is documented safe on a Nexus 2 and thousands demonstrably
    // fail. A deck asks for six at a time; the cap is for clients of our own.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let request = menu(MessageKind::MENU_TRACK, &[SortOrder::DEFAULT.0]);
    let total = ask(&mut session, &shared, &request)[0]
        .number(1)
        .expect("a count");
    assert_eq!(total, 651);

    let render = Message::render_of(2, descriptor(Slot::USB, MenuTarget::MAIN), 0, 5_000, total);
    let rows = ask(&mut session, &shared, &render)
        .iter()
        .filter(|reply| reply.kind == MessageKind::MENU_ITEM)
        .count();
    assert_eq!(u32::try_from(rows), Ok(MAX_RENDER_BATCH));
}

#[test]
fn rendering_past_the_end_is_a_short_page_and_not_a_failure() {
    let shared = shared([usb()]);
    let mut session = Session::default();
    let request = menu(MessageKind::MENU_HISTORY, &[SortOrder::DEFAULT.0]);
    let total = ask(&mut session, &shared, &request)[0]
        .number(1)
        .expect("a count");
    let render = Message::render_of(2, descriptor(Slot::USB, MenuTarget::MAIN), total, 6, total);
    let replies = ask(&mut session, &shared, &render);
    assert_eq!(
        replies.len(),
        2,
        "a header and a footer, and nothing between"
    );
}

#[test]
fn a_render_naming_a_size_we_never_gave_falls_back_to_that_descriptors_menu() {
    // A client that took the count from us cannot get here, but one whose idea
    // of the library is stale can, and a blank page reads as a menu that
    // vanished. The fallback is per descriptor before it is global, because the
    // descriptor is the one thing such a request still gets right.
    let sd = Arc::new(Medium::synthetic(ServedSlot::SD, other_library(), "SD"));
    let shared = shared([usb(), sd]);
    let mut session = Session::default();

    // Establish a list on each slot, the second one most recently.
    let on_usb = Message::menu_request(
        1,
        MessageKind::MENU_HISTORY,
        descriptor(Slot::USB, MenuTarget::MAIN),
        &[SortOrder::DEFAULT.0],
    )
    .expect("a menu request");
    ask(&mut session, &shared, &on_usb);
    let on_sd = Message::menu_request(
        2,
        MessageKind::MENU_ARTIST,
        descriptor(Slot::SD, MenuTarget::MAIN),
        &[SortOrder::DEFAULT.0],
    )
    .expect("a menu request");
    ask(&mut session, &shared, &on_sd);

    // A render for the USB descriptor with a size that names nothing.
    let render = Message::render_of(3, descriptor(Slot::USB, MenuTarget::MAIN), 0, 6, 9_999);
    let rows: Vec<MenuItem> = ask(&mut session, &shared, &render)
        .iter()
        .filter_map(MenuItem::from_message)
        .collect();
    assert_eq!(rows.len(), 6, "not a blank page");
    assert!(
        rows.iter()
            .all(|row| row.item_type == ItemType::HISTORY_PLAYLIST),
        "the USB list, not the SD one that happens to be more recent"
    );
}

// -- the whole thing over a socket ----------------------------------------

#[tokio::test]
async fn a_client_can_browse_us_over_a_real_socket() {
    let server = DbServer::start(
        DbServerConfig {
            port: 0,
            query_port: None,
            ..DbServerConfig::default()
        },
        [usb()],
    )
    .await
    .expect("the server starts");

    let mut stream = TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .expect("connect");
    stream.write_all(&PREAMBLE).await.expect("preamble");
    let mut buffer = Vec::new();
    read_exactly(&mut stream, &mut buffer, PREAMBLE.len()).await;
    assert_eq!(
        buffer, PREAMBLE,
        "the preamble is echoed in both directions"
    );
    buffer.clear();

    let device = BrowsableDeviceNumber::new(1).expect("device 1");
    let replies = round_trip(&mut stream, &mut buffer, &Message::introduce(device), 1).await;
    assert_eq!(replies[0].number(1), Some(1), "the server's own number");

    let descriptor = descriptor(Slot::USB, MenuTarget::MAIN);
    let root = Message::menu_request(1, MessageKind::MENU_ROOT, descriptor, &[0, 0x00ff_ffff])
        .expect("a root request");
    let replies = round_trip(&mut stream, &mut buffer, &root, 1).await;
    let total = replies[0].number(1).expect("a count");
    assert_eq!(total, 11);

    let render = Message::render_of(2, descriptor, 0, 6, total);
    let replies = round_trip(&mut stream, &mut buffer, &render, 8).await;
    assert_eq!(
        replies.first().map(|m| m.kind),
        Some(MessageKind::MENU_HEADER)
    );
    assert_eq!(
        replies.last().map(|m| m.kind),
        Some(MessageKind::MENU_FOOTER)
    );

    // A close mid-scroll, which must draw nothing and discard nothing.
    let close = Message::new(2, MessageKind::MENU_CLOSE, []);
    stream.write_all(&close.encode()).await.expect("write");
    let resumed = Message::render_of(3, descriptor, 6, 6, total);
    let replies = round_trip(&mut stream, &mut buffer, &resumed, 7).await;
    assert_eq!(
        replies.last().map(|m| m.kind),
        Some(MessageKind::MENU_FOOTER)
    );

    let disconnect = Message::disconnect();
    stream.write_all(&disconnect.encode()).await.expect("write");
}

#[tokio::test]
async fn the_port_query_answers_with_the_port_we_are_serving_on() {
    let server = DbServer::start(
        DbServerConfig {
            port: 0,
            // A fixed port would clash with anything else on this machine, and
            // the bind falls back rather than failing, so ask for an ephemeral
            // one and check the answer names the dbserver port.
            query_port: Some(0),
            ..DbServerConfig::default()
        },
        [usb()],
    )
    .await
    .expect("the server starts");
    let query_port = server.query_port().expect("a query listener");

    let mut stream = TcpStream::connect(("127.0.0.1", query_port))
        .await
        .expect("connect");
    stream
        .write_all(&dbserver::PORT_QUERY)
        .await
        .expect("the fixed nineteen-byte query");
    let mut buffer = Vec::new();
    read_exactly(&mut stream, &mut buffer, 2).await;
    assert_eq!(
        dbserver::decode_port_reply(&buffer).expect("two bytes"),
        server.port()
    );
}

#[tokio::test]
async fn a_peer_that_is_not_speaking_the_protocol_is_dropped() {
    // A dbserver message is framed by nothing but its own contents, so there is
    // no boundary to resynchronise on and the only remedy is to hang up.
    let server = DbServer::start(
        DbServerConfig {
            port: 0,
            query_port: None,
            ..DbServerConfig::default()
        },
        [usb()],
    )
    .await
    .expect("the server starts");
    let mut stream = TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .expect("connect");
    stream.write_all(&PREAMBLE).await.expect("preamble");
    stream.write_all(&[0x99; 64]).await.expect("nonsense");

    let mut buffer = vec![0u8; 64];
    // The preamble comes back, and then the connection closes.
    let mut seen = 0;
    loop {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
            .await
            .expect("the server does not hang")
            .expect("read");
        if read == 0 {
            break;
        }
        seen += read;
        assert!(seen <= PREAMBLE.len(), "no reply to nonsense");
    }
}

async fn read_exactly(stream: &mut TcpStream, buffer: &mut Vec<u8>, want: usize) {
    while buffer.len() < want {
        let mut chunk = vec![0u8; want - buffer.len()];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .expect("the server answers")
            .expect("read");
        assert_ne!(read, 0, "the connection closed early");
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
}

/// Send a request and decode `want` replies.
async fn round_trip(
    stream: &mut TcpStream,
    buffer: &mut Vec<u8>,
    request: &Message,
    want: usize,
) -> Vec<Message> {
    stream.write_all(&request.encode()).await.expect("write");
    let mut replies = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    while replies.len() < want {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .expect("the server answers")
            .expect("read");
        assert_ne!(read, 0, "the connection closed early");
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());

        replies.clear();
        let mut consumed = 0;
        while let Ok((message, used)) = Message::decode(buffer.get(consumed..).unwrap_or_default())
        {
            consumed += used;
            replies.push(message);
        }
    }
    buffer.clear();
    replies
}

// -- helpers over the real library ----------------------------------------

/// An album with exactly thirteen tracks, for the collision F41 is about.
fn thirteen_track_album(library: &Library) -> Option<u32> {
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for track in library.tracks.values() {
        if track.album_id != 0 {
            *counts.entry(track.album_id).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .find(|&(_, count)| count == 13)
        .map(|(id, _)| id)
}

fn first_track_of(library: &Library, album_id: u32) -> u32 {
    library
        .tracks
        .values()
        .find(|track| track.album_id == album_id)
        .map_or(0, |track| track.id)
}

/// A genre with several artists and one with exactly one, for the ALL rule.
fn genres_by_artist_count(library: &Library) -> (u32, u32) {
    let mut artists: BTreeMap<u32, std::collections::BTreeSet<u32>> = BTreeMap::new();
    for track in library.tracks.values() {
        if track.genre_id != 0 && track.artist_id != 0 {
            artists
                .entry(track.genre_id)
                .or_default()
                .insert(track.artist_id);
        }
    }
    let crowded = artists
        .iter()
        .find(|(_, set)| set.len() > 1)
        .map_or(0, |(id, _)| *id);
    let lonely = artists
        .iter()
        .find(|(_, set)| set.len() == 1)
        .map_or(0, |(id, _)| *id);
    (crowded, lonely)
}

/// The failure hardware found: a list being paged is evicted by newer menus.
mod eviction {
    use super::*;

    fn rows(count: usize, tag: u32) -> Vec<MenuItem> {
        (0..count)
            .map(|index| {
                MenuItem::track(
                    tag + u32::try_from(index).unwrap_or(0),
                    "title",
                    "",
                    ItemType::TRACK_TITLE,
                    0,
                )
            })
            .collect()
    }

    /// Page a set the way a deck does: `RENDER_MENU` naming its descriptor and
    /// the count it was answered with, never re-issuing the menu request.
    fn page(session: &mut Session, shared: &Shared, descriptor: u32, total: u32) -> Vec<MenuItem> {
        let mut out = Vec::new();
        let parsed = Descriptor::parse(descriptor).expect("a descriptor");
        let request = Message::render_of(1, parsed, 0, 6, total);
        session.render(shared, &request, &mut out);
        let mut offset = 0;
        let mut items = Vec::new();
        while offset < out.len() {
            let Ok((message, used)) = Message::decode(&out[offset..]) else {
                break;
            };
            offset += used;
            if message.kind == MessageKind::MENU_ITEM
                && let Some(item) = MenuItem::from_message(&message)
            {
                items.push(item);
            }
        }
        items
    }

    #[test]
    fn a_list_being_paged_is_not_evicted_by_newer_menus() {
        // A deck polls a loaded track's metadata every couple of seconds and
        // every poll mints a fresh set. Evicting by insertion order threw away
        // the long-lived list the DJ was scrolling, which on hardware looked
        // like every menu going blank about a minute into any track and staying
        // blank until the DJ left LINK. One connection in that capture minted
        // 44 sets against a bound of 32.
        let mut session = Session::default();
        let shared = shared([usb()]);
        let list = 0x0101_0301;
        let metadata = 0x0102_0301;

        session.remember(list, 500, rows(500, 1_000_000));

        let polls = u32::try_from(MAX_PENDING_MENUS)
            .unwrap_or(u32::MAX)
            .saturating_mul(2);
        for poll in 0..polls {
            // Each poll is a distinct key, as a real one is: the count is the
            // same thirteen but the descriptor's menu target differs, and a DJ
            // scrolling albums mints one key per album size besides.
            session.remember(metadata + poll, 13, rows(13, poll));
            let paged = page(&mut session, &shared, list, 500);
            assert_eq!(
                paged.len(),
                6,
                "the list vanished after {poll} metadata polls",
            );
            assert_eq!(
                paged.first().map(|item| item.id),
                Some(1_000_000),
                "a different menu was served in its place after {poll} polls",
            );
        }
    }

    #[test]
    fn the_table_still_has_a_bound() {
        // The fix must not turn the bound off: a connection that never forgets
        // grows for as long as a DJ browses.
        let mut session = Session::default();
        let sets = u32::try_from(MAX_PENDING_MENUS)
            .unwrap_or(u32::MAX)
            .saturating_mul(3);
        for index in 0..sets {
            session.remember(index, 4, rows(4, index));
        }
        assert!(
            session.menus.len() <= MAX_PENDING_MENUS,
            "held {} sets against a bound of {MAX_PENDING_MENUS}",
            session.menus.len(),
        );
    }
}

/// Page the whole set a menu request just established, the way a deck does:
/// repeated `RENDER_MENU`s at increasing offsets, never re-issuing the menu
/// request.
fn render_all(session: &mut Session, shared: &Shared, total: u32) -> Vec<MenuItem> {
    let batch = MAX_RENDER_BATCH;
    let mut items = Vec::new();
    let mut at = 0u32;
    loop {
        let mut out = Vec::new();
        let request = Message::render_of(
            500,
            descriptor(Slot::USB, MenuTarget::MAIN),
            at,
            batch,
            total,
        );
        session.render(shared, &request, &mut out);
        let mut offset = 0;
        let mut page = 0u32;
        while let Some(rest) = out.get(offset..).filter(|rest| !rest.is_empty()) {
            let (message, used) = Message::decode(rest).expect("our own replies decode");
            offset += used;
            if let Some(item) = MenuItem::from_message(&message) {
                items.push(item);
                page += 1;
            }
        }
        if page < batch {
            return items;
        }
        at += page;
    }
}

// -- the tag list -----------------------------------------------------------

/// A request carrying our descriptor and the given arguments.
fn tagged_request(kind: MessageKind, transaction: u32, args: &[u32]) -> Message {
    let descriptor = descriptor(Slot::USB, MenuTarget::MAIN);
    let mut fields = vec![Field::U32(descriptor.to_raw())];
    fields.extend(args.iter().map(|value| Field::U32(*value)));
    Message::new(
        transaction,
        kind,
        Arguments::new(fields).expect("a short argument list"),
    )
}

#[test]
fn a_tagged_track_comes_back_in_the_tag_list() {
    // `0x3002` is "put this track in the tag list" and `0x100f` is "give me the
    // tag list" — the second answered with track rows, not with a category
    // listing (F53). Acknowledging the first without storing anything is what
    // left the menu permanently empty.
    let shared = shared([usb()]);
    let mut session = Session::default();

    let empty = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 1, &[0]),
    );
    assert_eq!(empty[0].number(1), Some(0), "nothing is tagged yet");

    for (index, track) in [2u32, 1].iter().enumerate() {
        let replies = ask(
            &mut session,
            &shared,
            &tagged_request(
                MessageKind::TAG_LIST_ADD,
                10 + u32::try_from(index).unwrap_or(0),
                &[*track, 1],
            ),
        );
        assert_eq!(replies.len(), 1, "tagging draws one acknowledgement");
        assert_eq!(replies[0].kind, MessageKind::SUCCESS);
        assert_eq!(replies[0].number(1), Some(0), "the count is zero, not one");
    }

    let listed = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 2, &[0]),
    );
    assert_eq!(
        listed[0].number(1),
        Some(2),
        "both tagged tracks are listed"
    );
}

#[test]
fn the_tag_list_keeps_the_order_the_dj_tagged_in() {
    // Not sorted: the list is what the DJ built. A real deck's reply looks
    // alphabetical only because the tracks were tagged that way, and its exact
    // collation is unresolved — it puts "antidepressant o44" before "Anti
    // Gravity Racing", which no space-respecting comparison does (F53).
    let shared = shared([usb()]);
    let mut session = Session::default();
    for track in [3u32, 1, 2] {
        ask(
            &mut session,
            &shared,
            &tagged_request(MessageKind::TAG_LIST_ADD, track, &[track, 1]),
        );
    }
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 100, &[0]),
    );
    let rendered = render_all(&mut session, &shared, 3);
    let ids: Vec<u32> = rendered.iter().map(|item| item.id).collect();
    assert_eq!(ids, vec![3, 1, 2], "tag order, not id or title order");
}

#[test]
fn tagging_the_same_track_twice_does_not_duplicate_it() {
    let shared = shared([usb()]);
    let mut session = Session::default();
    for transaction in 0..3 {
        ask(
            &mut session,
            &shared,
            &tagged_request(MessageKind::TAG_LIST_ADD, transaction, &[1, 1]),
        );
    }
    let listed = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 9, &[0]),
    );
    assert_eq!(listed[0].number(1), Some(1));
}

#[test]
fn a_tagged_track_carries_its_marker_in_every_menu_it_appears_in() {
    // Bit 0 of a track row's flags. Established by elimination: it is set on
    // 446 rows in the corpus and every one of them is in the session where
    // tracks were being tagged (F53).
    let shared = shared([usb()]);
    let mut session = Session::default();
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::TAG_LIST_ADD, 1, &[2, 1]),
    );
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TRACK, 2, &[0]),
    );
    let rows = render_all(&mut session, &shared, 3);
    assert!(
        rows.len() > 1,
        "the whole track list, not just the tagged one"
    );
    for row in &rows {
        let tagged = row.flags & MenuItem::TAGGED != 0;
        assert_eq!(
            tagged,
            row.id == 2,
            "row {} carries the wrong marker",
            row.id
        );
        assert!(
            row.flags & MenuItem::TRACK_FLAGS != 0,
            "the marker must not displace the track bit"
        );
    }
}

#[test]
fn two_decks_keep_separate_tag_lists() {
    // The descriptor's requesting-device byte is what the list is keyed on:
    // the TAG LIST button means "what I tagged" on each deck.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let other = Descriptor::new(
        BrowsableDeviceNumber::new(3).expect("device 3 is browsable"),
        Slot::USB,
        MenuTarget::MAIN,
        TrackType::REKORDBOX,
    );

    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::TAG_LIST_ADD, 1, &[1, 1]),
    );
    let theirs = ask(
        &mut session,
        &shared,
        &Message::new(
            2,
            MessageKind::MENU_TAG_LIST,
            [Field::U32(other.to_raw()), Field::U32(0)],
        ),
    );
    assert_eq!(theirs[0].number(1), Some(0), "device 3 tagged nothing");
}

#[test]
fn untagging_removes_a_track() {
    // Argument 2 is `1` in every captured request, so removal is this
    // library's reading of `0` rather than an observation (F53).
    let shared = shared([usb()]);
    let mut session = Session::default();
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::TAG_LIST_ADD, 1, &[1, 1]),
    );
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::TAG_LIST_ADD, 2, &[1, 0]),
    );
    let listed = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 3, &[0]),
    );
    assert_eq!(listed[0].number(1), Some(0));
}

#[test]
fn remove_all_tracks_empties_the_tag_list() {
    // `0x3202` is REMOVE ALL TRACKS. Its twin `0x3402` carries the same bare
    // descriptor and draws the same acknowledgement, and the only thing that
    // separates them is what the tag list held either side: with three tracks
    // tagged, the menu answered 3 across `0x3402` and 0 immediately after
    // `0x3202` (F54).
    let shared = shared([usb()]);
    let mut session = Session::default();
    for track in [1u32, 2, 3] {
        ask(
            &mut session,
            &shared,
            &tagged_request(MessageKind::TAG_LIST_ADD, track, &[track, 1]),
        );
    }
    let before = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 20, &[0]),
    );
    assert_eq!(before[0].number(1), Some(3));

    // The twin must leave it alone.
    let idle = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::UNKNOWN_3402, 21, &[]),
    );
    assert_eq!(idle[0].kind, MessageKind::SUCCESS);
    let across = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 22, &[0]),
    );
    assert_eq!(across[0].number(1), Some(3), "0x3402 is not the clear");

    let cleared = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::TAG_LIST_CLEAR, 23, &[]),
    );
    assert_eq!(cleared[0].kind, MessageKind::SUCCESS);
    assert_eq!(cleared[0].number(1), Some(0), "the reply carries zero");
    let after = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 24, &[0]),
    );
    assert_eq!(after[0].number(1), Some(0), "REMOVE ALL TRACKS emptied it");
}

#[test]
fn clearing_one_decks_tag_list_leaves_the_other_alone() {
    let shared = shared([usb()]);
    let mut session = Session::default();
    let other = Descriptor::new(
        BrowsableDeviceNumber::new(3).expect("device 3 is browsable"),
        Slot::USB,
        MenuTarget::MAIN,
        TrackType::REKORDBOX,
    );
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::TAG_LIST_ADD, 1, &[1, 1]),
    );
    ask(
        &mut session,
        &shared,
        &Message::new(
            2,
            MessageKind::TAG_LIST_ADD,
            Arguments::new(vec![
                Field::U32(other.to_raw()),
                Field::U32(2),
                Field::U32(1),
            ])
            .expect("three arguments"),
        ),
    );
    ask(
        &mut session,
        &shared,
        &Message::new(
            3,
            MessageKind::TAG_LIST_CLEAR,
            Arguments::new(vec![Field::U32(other.to_raw())]).expect("one argument"),
        ),
    );
    let ours = ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 4, &[0]),
    );
    assert_eq!(ours[0].number(1), Some(1), "device 1 keeps its tag");
}

#[test]
fn the_tag_list_honours_the_sort_the_deck_asks_for() {
    // A deck browsing with KEY selected asks for the tag list with sort 0x0c.
    let shared = shared([usb()]);
    let mut session = Session::default();
    for track in [3u32, 1, 2] {
        ask(
            &mut session,
            &shared,
            &tagged_request(MessageKind::TAG_LIST_ADD, track, &[track, 1]),
        );
    }
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TAG_LIST, 30, &[SortOrder::TITLE.0]),
    );
    let sorted = render_all(&mut session, &shared, 3);
    let titles: Vec<&str> = sorted.iter().map(|item| item.label1.as_str()).collect();
    let mut expected = titles.clone();
    expected.sort_by_key(|title| title.to_lowercase());
    assert_eq!(titles, expected, "a sort other than DEFAULT is applied");
}

// -- the loaded-track mark ---------------------------------------------------

#[test]
fn the_loaded_track_row_is_marked_so_the_key_indicator_has_a_reference() {
    // A browsing deck does not compute key compatibility from its own copy of
    // the loaded track. It reads the key off whichever row carries bit 8 and
    // lights every row harmonically compatible with it — so a listing with no
    // marked row lights nothing, which is exactly what a real deck showed for
    // this server until now (F55).
    //
    // Established by serving the *same medium* a real deck was serving and
    // diffing 125 rows: every field matched except this one bit, on the one
    // row the browsing deck had loaded.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let device = descriptor(Slot::USB, MenuTarget::MAIN).device.get();
    shared.loaded.note(device, Slot::USB, 2);

    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TRACK, 1, &[0]),
    );
    let rows = render_all(&mut session, &shared, 3);
    assert!(rows.len() > 1, "the whole list, not just the loaded track");
    for row in &rows {
        let marked = row.flags & MenuItem::LOADED != 0;
        assert_eq!(marked, row.id == 2, "row {} carries the wrong mark", row.id);
    }
}

#[test]
fn nothing_is_marked_when_the_deck_has_loaded_nothing_from_us() {
    // A track a deck loaded from *another* player is not on our medium, and
    // marking a row by id alone would mark whichever of our tracks happened to
    // share that id.
    let shared = shared([usb()]);
    let mut session = Session::default();
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TRACK, 1, &[0]),
    );
    let rows = render_all(&mut session, &shared, 3);
    assert!(
        rows.iter().all(|row| row.flags & MenuItem::LOADED == 0),
        "no row is marked when nothing was loaded from us"
    );
}

#[test]
fn the_mark_follows_the_slot_the_deck_is_browsing() {
    // Two media, and a deck with a track loaded from the USB. Browsing the SD
    // must not mark the row that happens to share the id.
    let sd = Arc::new(Medium::synthetic(ServedSlot::SD, other_library(), "SD"));
    let shared = shared([usb(), sd]);
    let mut session = Session::default();
    let device = descriptor(Slot::USB, MenuTarget::MAIN).device.get();
    shared.loaded.note(device, Slot::USB, 1);

    let sd_request = Message::new(
        1,
        MessageKind::MENU_TRACK,
        Arguments::new(vec![
            Field::U32(descriptor(Slot::SD, MenuTarget::MAIN).to_raw()),
            Field::U32(0),
        ])
        .expect("two arguments"),
    );
    ask(&mut session, &shared, &sd_request);
    let mut out = Vec::new();
    let render = Message::render_of(2, descriptor(Slot::SD, MenuTarget::MAIN), 0, 4, 4);
    session.render(&shared, &render, &mut out);
    let mut offset = 0;
    while let Some(rest) = out.get(offset..).filter(|rest| !rest.is_empty()) {
        let (message, used) = Message::decode(rest).expect("our own replies decode");
        offset += used;
        if let Some(item) = MenuItem::from_message(&message) {
            assert_eq!(
                item.flags & MenuItem::LOADED,
                0,
                "the USB's loaded track marked a row on the SD"
            );
        }
    }
}

#[test]
fn a_deck_that_unloads_clears_the_mark() {
    // Track id zero in a status packet means nothing is loaded, and must forget
    // the entry rather than mark row zero.
    let shared = shared([usb()]);
    let device = descriptor(Slot::USB, MenuTarget::MAIN).device.get();
    shared.loaded.note(device, Slot::USB, 2);
    assert_eq!(shared.loaded.track_on(device, Slot::USB), Some(2));
    shared.loaded.note(device, Slot::USB, 0);
    assert_eq!(shared.loaded.track_on(device, Slot::USB), None);
}

#[test]
fn a_track_loaded_after_the_menu_was_built_still_marks_its_row() {
    // The failure this was found by. A deck asks for the track list, loads a
    // track *out of that list*, and keeps scrolling — without re-issuing the
    // menu request (F27). Marking rows when the set was built therefore marks
    // nothing the deck ever sees, and the key-matching indicator stays dark
    // with every row otherwise byte-perfect against a real deck (F55).
    //
    // So the mark belongs at render time. The set is a snapshot of the
    // library; what is loaded and what is tagged are live state.
    let shared = shared([usb()]);
    let mut session = Session::default();
    let device = descriptor(Slot::USB, MenuTarget::MAIN).device.get();

    // The menu is established first, with nothing loaded and nothing tagged.
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TRACK, 1, &[0]),
    );
    let before = render_all(&mut session, &shared, 3);
    assert!(
        before.iter().all(|row| row.flags & MenuItem::LOADED == 0),
        "nothing is marked before anything is loaded"
    );

    // Only now does the deck load a track and tag another.
    shared.loaded.note(device, Slot::USB, 2);
    shared.tags.set(device, Slot::USB, 3, true);

    // The same set, paged again — no new menu request, exactly as a deck does.
    let after = render_all(&mut session, &shared, 3);
    let loaded: Vec<u32> = after
        .iter()
        .filter(|row| row.flags & MenuItem::LOADED != 0)
        .map(|row| row.id)
        .collect();
    let tagged: Vec<u32> = after
        .iter()
        .filter(|row| row.flags & MenuItem::TAGGED != 0)
        .map(|row| row.id)
        .collect();
    assert_eq!(loaded, vec![2], "the loaded row is marked on the next page");
    assert_eq!(tagged, vec![3], "and so is the tagged one");
}

#[test]
fn unloading_clears_the_mark_on_a_menu_already_paged() {
    let shared = shared([usb()]);
    let mut session = Session::default();
    let device = descriptor(Slot::USB, MenuTarget::MAIN).device.get();
    shared.loaded.note(device, Slot::USB, 2);
    ask(
        &mut session,
        &shared,
        &tagged_request(MessageKind::MENU_TRACK, 1, &[0]),
    );
    assert_eq!(
        render_all(&mut session, &shared, 3)
            .iter()
            .filter(|row| row.flags & MenuItem::LOADED != 0)
            .count(),
        1
    );
    shared.loaded.note(device, Slot::USB, 0);
    assert!(
        render_all(&mut session, &shared, 3)
            .iter()
            .all(|row| row.flags & MenuItem::LOADED == 0),
        "the mark goes when the deck unloads, without a new menu request"
    );
}

// -- a phantom medium -------------------------------------------------------
//
// The stick is out and a player is still playing off it. What a phantom must do
// is finish that track; what it must not do is let anyone start another one it
// cannot produce. These pin the line between the two.

#[test]
fn a_phantom_offers_nothing_to_browse() {
    let medium = usb();
    let full = menu::build(
        MessageKind::MENU_ARTIST,
        &Arguments::default(),
        Some(&medium),
        &[],
    )
    .expect("artists");
    assert!(!full.is_empty(), "the fixture has 329 artists");

    medium.go_phantom();
    let empty = menu::build(
        MessageKind::MENU_ARTIST,
        &Arguments::default(),
        Some(&medium),
        &[],
    )
    .expect("still a menu");
    // Empty, not an error and not absent. A DJ scrolling into it finds nothing
    // -- which is true -- rather than loading a track that would play silence.
    assert!(empty.is_empty());
}

#[test]
fn a_phantom_empties_every_way_in() {
    // The root and the sort options are answered without reference to a medium
    // everywhere else, and they are also the way in: leaving them populated
    // would offer a DJ a menu that turned out to be empty one level down, which
    // reads as a fault rather than as a stick that has gone.
    let medium = usb();
    medium.go_phantom();
    for kind in [
        MessageKind::MENU_ROOT,
        MessageKind::MENU_SORT,
        MessageKind::MENU_TRACK,
        MessageKind::MENU_ALBUM,
        MessageKind::MENU_GENRE,
        MessageKind::MENU_KEY,
        MessageKind::MENU_PLAYLIST,
        MessageKind::MENU_SEARCH,
        MessageKind::MENU_HISTORY,
    ] {
        let items = menu::build(kind, &Arguments::default(), Some(&medium), &[])
            .unwrap_or_else(|| panic!("{kind:?} should still answer"));
        assert!(items.is_empty(), "{kind:?} should be empty on a phantom");
    }
}

#[test]
fn a_phantom_still_answers_about_the_track_that_is_playing() {
    // The one thing it absolutely must keep doing. A deck polls these every
    // couple of seconds for as long as it plays, so answering them empty would
    // blank the display of a track that is playing perfectly well -- and quite
    // possibly convince the deck it had lost the track altogether.
    let medium = usb();
    let track = *medium
        .library()
        .tracks
        .keys()
        .next()
        .expect("the fixture has tracks");

    let request = menu(MessageKind::GET_METADATA, &[track]);
    let before = menu::build(MessageKind::GET_METADATA, &request.args, Some(&medium), &[])
        .expect("metadata");
    assert!(!before.is_empty());

    medium.go_phantom();
    let after = menu::build(MessageKind::GET_METADATA, &request.args, Some(&medium), &[])
        .expect("metadata");
    assert_eq!(before, after, "metadata must not change when a stick goes");
}

#[test]
fn a_medium_is_not_a_phantom_until_it_is_told_it_is() {
    let medium = usb();
    assert!(!medium.is_phantom());
    medium.go_phantom();
    assert!(medium.is_phantom());
}
