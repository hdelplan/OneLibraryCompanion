// SPDX-License-Identifier: GPL-3.0-only

//! Browsing a player's library: the dbserver client.
//!
//! What the LINK button drives, from the other end. NFS makes a medium's files
//! *readable*; this makes it *browsable*, and the two are independent — a peer
//! whose every byte we can read will still show nothing without this. It is
//! also the only way to get album art out of a player: a real CDJ never asks
//! NFS for an image.
//!
//! ```text
//! TCP 12523  ─►  19 bytes out, 2 back: which port?
//! TCP 1051   ─►  preamble ─► INTRODUCE ─► SUCCESS[0, their device number]
//!                MENU_*   ─► SUCCESS[type, item count]
//!                RENDER   ─► MENU_HEADER, n× MENU_ITEM, MENU_FOOTER
//!                MENU_CLOSE (no reply)
//! ```
//!
//! # Announcing is a precondition, so it is a precondition of the type
//!
//! The descriptor that opens nearly every request carries **our** device
//! number, and a player validates it: it must be in 1–4 and it must belong to
//! a device actually on the network. [`DbClient::connect`] therefore takes a
//! [`BrowsableDeviceNumber`], which only [`crate::VirtualCdj`] running
//! [`crate::virtual_cdj::Numbering::Claim`] can produce. The observer number a
//! passive listener takes cannot be borrowed for a session here, and that is a
//! compile error rather than a puzzling refusal at run time (F45).
//!
//! # One request in flight, always
//!
//! The stream carries **no length framing** — a message is delimited by nothing
//! but its own contents — so a second request in flight would be answered with
//! no way to tell which answer belonged to which. Worse, a reply that arrives
//! after we have stopped waiting for it is parsed as the *next* request's reply
//! and every answer after that is one out, silently. So requests are strictly
//! serialised, a reply under an unexpected transaction id marks the connection
//! desynchronised, and a desynchronised connection is finished: there is no
//! frame boundary to resynchronise on.
//!
//! # `0x0001` draws no reply at all
//!
//! `MENU_CLOSE` is the one message with no answer — 23 sends in one
//! CDJ-to-CDJ browse, and the packet accounting in that capture leaves nothing
//! over for a response (F16). A client that waits for one hangs until its
//! timeout. It reuses the transaction id of the `RENDER_MENU` it follows, which
//! is why [`DbClient::menu`] keeps the last render's id rather than allocating
//! a fresh one.
//!
//! # Two counts, and only one of them is the page size
//!
//! A menu request is answered with `SUCCESS [type, count]`, and every
//! `RENDER_MENU` that pages it echoes that **whole-result-set count**, not the
//! page size. That is not cosmetic: a server keys its pending result sets on
//! `(descriptor, count)` because a deck interleaves a metadata lookup with a
//! 692-item track list and then resumes the list at the next offset without
//! re-issuing its request (F27, F41). Sending the page size instead names a
//! result set nobody has.
//!
//! A count of [`NOT_FOUND`] means exactly that, and an empty menu is not an
//! error: on a CDJ's screen an error and an empty folder look identical, so the
//! two are kept apart here deliberately.
//!
//! # Transaction ids start at `0x03800001`
//!
//! Not at 1, whatever the pre-hardware literature says (C10). The value is
//! opaque and a server only echoes it, so nothing breaks either way — but a
//! client counting from 1 is one more way to look unlike a CDJ.
//!
//! # What this client does *not* send
//!
//! `0x3e03`, the undocumented request a player fires immediately after
//! `INTRODUCE` when the thing it is browsing is a **foreign** device. It never
//! appears between two CDJs, and against a real player we are the client, so
//! sending it would be a message no deck ever sends. `crate::serve::dbserver`
//! has to *answer* it, which is the other half of the same finding (F25).
//!
//! # Strings here are the opposite of strings over NFS
//!
//! UTF-16 **big**-endian, counted in **characters including a trailing NUL**;
//! the NFS half is UTF-16 little-endian counted in bytes. Two endiannesses and
//! two units in one protocol. Nothing is shared between the two encoders and
//! nothing should be — [`prolink_proto::dbserver::encode_string`] here,
//! `prolink_proto::rpc::xdr` there.
//!
//! # The path a track load needs comes from here
//!
//! [`DbClient::track_info`] returns a path like
//! `/Contents/Tomcraft/Loneliness/… .mp3`, already relative to the mount root,
//! which is exactly what [`crate::consume::NfsClient::open`] takes. **Argument
//! 0 of that path item is the file size** — zero on every other menu item ever
//! captured, which is precisely why it reads as structural padding, and it is
//! the one thing a load needs that browsing does not (F31).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use prolink_proto::dbserver::{
    self, Arguments, Descriptor, Drill, Field, ItemType, MenuItem, MenuTarget, Message,
    MessageKind, SortOrder, TrackType,
};
use prolink_proto::{BrowsableDeviceNumber, DeviceNumber, Slot};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, trace, warn};

use crate::{Error, Result};

/// The count a player answers with when the thing asked for does not exist.
///
/// A successful reply reporting absence, not a refusal — and the difference is
/// user-visible, because an error and an empty folder look identical on a
/// deck's screen.
pub const NOT_FOUND: u32 = 0xFFFF_FFFF;

/// Argument 2 of a root-menu request.
///
/// Reproduced without being understood: every `MENU_ROOT` in the corpus carries
/// exactly this, across separate capture sessions and both device pairs. It
/// reads like a "show me every category" mask.
pub const ROOT_MENU_MASK: u32 = 0x00FF_FFFF;

/// Argument 1 of a `GET_WAVEFORM_PREVIEW`: a constant `3` in every capture.
///
/// Reproduced without being understood.
pub const WAVEFORM_PREVIEW_ARGUMENT1: u32 = 3;

/// How long to wait, and how many rows to ask for at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbConfig {
    /// How long to wait for the TCP connection and the port query.
    pub connect_timeout: Duration,
    /// How long to wait for one reply.
    ///
    /// Ten seconds, and it is not paranoia: a player answers these off the same
    /// processor that is decoding audio, and a deck mid-track has been seen to
    /// take seconds.
    pub request_timeout: Duration,
    /// Rows per `RENDER_MENU`.
    ///
    /// [`dbserver::MAX_RENDER_BATCH`] is 64, which is documented safe on a
    /// Nexus 2; thousands in one render demonstrably fail. Paging costs one
    /// round trip per batch and nothing else.
    pub batch: u32,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(10),
            batch: dbserver::MAX_RENDER_BATCH,
        }
    }
}

/// Which analysis blob to fetch.
///
/// The wire form of each is **transformed** from what rekordbox wrote to the
/// medium — the file is big-endian and the wire little-endian, and three of the
/// five change layout as well (F30). This client returns the bytes the player
/// sent, undecoded: `prolink_proto::analysis` implements the file-to-wire
/// direction for the serving side, and the inverse does not exist yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Analysis {
    /// The preview waveform shown above the track. 900 bytes, not 800: the
    /// packed `PWAV` bytes split in two, then the tiny `PWV2` appended.
    WaveformPreview,
    /// The scrolling detailed waveform.
    WaveformDetail,
    /// The beat grid.
    BeatGrid,
    /// Memory points and hot cues, as two blobs sorted by time.
    CuePoints,
    /// Extended cue points, CDJ-2000NXS2 and later.
    ExtendedCuePoints,
    /// **The gate on playback.** The MP3 variable-bitrate seek index: without a
    /// time-to-byte-offset table a player cannot seek, so it never issues a
    /// single `READ` — a load that resolves the path perfectly and then does
    /// nothing.
    VbrIndex,
}

impl Analysis {
    /// The request this fetch sends.
    pub fn request(self) -> MessageKind {
        match self {
            Self::WaveformPreview => MessageKind::GET_WAVEFORM_PREVIEW,
            Self::WaveformDetail => MessageKind::GET_WAVEFORM_DETAIL,
            Self::BeatGrid => MessageKind::GET_BEAT_GRID,
            Self::CuePoints => MessageKind::GET_CUE_POINTS,
            Self::ExtendedCuePoints => MessageKind::GET_CUE_POINTS_EXT,
            Self::VbrIndex => MessageKind::GET_VBR_INDEX,
        }
    }
}

/// A track's thirteen metadata items, named.
///
/// Assembled by **item type, not by position**: `0x04` is the title here and
/// the container in a `GET_TRACK_INFO` reply, the same byte meaning two things
/// in two replies (F35), and a CDJ-3000 packs extra data into an item type's
/// high half so every comparison masks first.
///
/// Every id is the id of the row the item **references** — the artist item
/// carries the artist's id, not the track's — which is what lets a caller offer
/// "more by this artist" (F32). The raw rows are kept in
/// [`TrackMetadata::items`] because thirteen named fields cannot be the whole
/// truth about a reply from firmware nobody here has seen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackMetadata {
    /// The track this describes.
    pub id: u32,
    /// The title.
    pub title: String,
    /// **The artwork to fetch**, off the title item. Without it a player never
    /// requests the image and INFO shows no cover (F32); zero means no art.
    pub artwork_id: u32,
    /// The artist's name.
    pub artist: String,
    /// The artist's row id.
    pub artist_id: u32,
    /// The album's name.
    pub album: String,
    /// The album's row id.
    pub album_id: u32,
    /// The genre's name.
    pub genre: String,
    /// The genre's row id.
    pub genre_id: u32,
    /// The musical key, as the deck writes it — `12A`.
    pub key: String,
    /// The key's row id.
    pub key_id: u32,
    /// The record label's name.
    pub label: String,
    /// The record label's row id.
    pub label_id: u32,
    /// The colour's name, empty for "no colour".
    pub colour: String,
    /// The colour's id.
    pub colour_id: u32,
    /// The comment, which on a rekordbox medium is often a URL.
    pub comment: String,
    /// The date the track was added, `2025-11-13`.
    pub date_added: String,
    /// Playing time in seconds.
    pub duration_seconds: u32,
    /// Tempo in **hundredths** of a BPM: `13201` is 132.01.
    pub tempo_centibpm: u32,
    /// Star rating, 0–5.
    pub rating: u32,
    /// Bitrate in kbps.
    pub bitrate: u32,
    /// Every row exactly as it arrived, in order.
    pub items: Vec<MenuItem>,
}

impl TrackMetadata {
    /// Tempo in BPM.
    pub fn tempo(&self) -> f64 {
        f64::from(self.tempo_centibpm) / 100.0
    }

    /// Playing time.
    pub fn duration(&self) -> Duration {
        Duration::from_secs(u64::from(self.duration_seconds))
    }

    /// Read the items of a `GET_METADATA` reply.
    ///
    /// A track a player does not have answers with no items at all, and that is
    /// not an error: the result is this struct with nothing but the id.
    pub fn from_items(id: u32, items: Vec<MenuItem>) -> Self {
        let mut out = Self {
            id,
            ..Self::default()
        };
        for item in &items {
            let text = item.label1.clone();
            match item.item_type.masked() {
                ItemType::TRACK_TITLE => {
                    out.title = text;
                    out.artwork_id = item.artwork_id;
                }
                ItemType::ARTIST => {
                    out.artist = text;
                    out.artist_id = item.id;
                }
                ItemType::ALBUM => {
                    out.album = text;
                    out.album_id = item.id;
                }
                ItemType::GENRE => {
                    out.genre = text;
                    out.genre_id = item.id;
                }
                ItemType::KEY => {
                    out.key = text;
                    out.key_id = item.id;
                }
                ItemType::LABEL => {
                    out.label = text;
                    out.label_id = item.id;
                }
                ItemType::COLOR => {
                    out.colour = text;
                    out.colour_id = item.id;
                }
                ItemType::COMMENT => out.comment = text,
                ItemType::DATE_ADDED => out.date_added = text,
                // Numeric items send an empty label and carry the value in the
                // id, because the deck formats it itself (F43).
                ItemType::DURATION => out.duration_seconds = item.id,
                ItemType::TEMPO => out.tempo_centibpm = item.id,
                ItemType::RATING => out.rating = item.id,
                ItemType::BITRATE => out.bitrate = item.id,
                other => trace!(?other, "an unnamed metadata item"),
            }
        }
        out.items = items;
        out
    }
}

/// What a `GET_TRACK_INFO` reply says, which is what a load needs.
///
/// Six items, not one. Returning only the path is enough to render a track and
/// to walk it over NFS and **not enough to load it**: a deck sat at "NOW
/// LOADING…" and then reported that it could not decode the format, having
/// issued no `READ` of any kind (F31).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    /// The track this describes.
    pub id: u32,
    /// The path on the medium, relative to the mount root, ready for
    /// [`crate::consume::NfsClient::open`].
    pub path: String,
    /// **The file's length**, from argument 0 of the path item (F31).
    pub size: u64,
    /// The container, from the id of the `0x04` item — **not** the title, which
    /// is what that byte means in a `GET_METADATA` reply (F35). The value is a
    /// rekordbox `FileType`.
    pub container: u32,
    /// Playing time in seconds.
    pub duration_seconds: u32,
    /// Tempo in hundredths of a BPM.
    pub tempo_centibpm: u32,
    /// The comment.
    pub comment: String,
    /// Every row exactly as it arrived, in order.
    pub items: Vec<MenuItem>,
}

impl TrackInfo {
    /// Read the six items of a `GET_TRACK_INFO` reply, or `None` when there is
    /// no path item.
    ///
    /// `None` rather than an empty path: a track whose path a player will not
    /// name cannot be loaded, and a struct that pretended otherwise would push
    /// the check onto everyone downstream.
    pub fn from_items(id: u32, items: Vec<MenuItem>) -> Option<Self> {
        let path_item = items
            .iter()
            .find(|item| item.item_type.masked() == ItemType::PATH)?;
        let mut out = Self {
            id,
            path: path_item.label1.clone(),
            size: u64::from(path_item.argument0),
            container: 0,
            duration_seconds: 0,
            tempo_centibpm: 0,
            comment: String::new(),
            items: Vec::new(),
        };
        for item in &items {
            match item.item_type.masked() {
                // The trap: `0x04` is the container here, with an empty label
                // and the rekordbox file type in the id.
                ItemType::TRACK_TITLE => out.container = item.id,
                ItemType::DURATION => out.duration_seconds = item.id,
                ItemType::TEMPO => out.tempo_centibpm = item.id,
                ItemType::COMMENT => out.comment.clone_from(&item.label1),
                _ => {}
            }
        }
        out.items = items;
        Some(out)
    }
}

/// The largest receive buffer a well-behaved player can justify.
///
/// The biggest thing this protocol carries is a cover image, and the largest
/// observed is under a kilobyte. Eight megabytes is a backstop against a peer
/// that never completes a message, not a working size.
const MAX_BUFFER: usize = 8 * 1024 * 1024;

/// Bytes to ask the socket for at a time.
const READ_CHUNK: usize = 16 * 1024;

/// A connection to one player's dbserver.
///
/// Requests are strictly serialised; see the module documentation. Dropping it
/// closes the socket without saying goodbye — call [`DbClient::close`] to send
/// the `DISCONNECT` a real deck sends.
#[derive(Debug)]
pub struct DbClient {
    stream: TcpStream,
    peer: Ipv4Addr,
    port: u16,
    device: BrowsableDeviceNumber,
    server: DeviceNumber,
    config: DbConfig,
    buffer: Vec<u8>,
    consumed: usize,
    transaction: u32,
    /// Set when a reply arrives under a transaction id nobody is waiting for.
    /// There is no frame boundary to resynchronise on, so this is terminal.
    desynchronised: bool,
}

impl DbClient {
    /// Ask a player which port its dbserver is on.
    ///
    /// A short-lived TCP connection to [`dbserver::PORT_QUERY_PORT`]: nineteen
    /// bytes out, two back. Every device anyone has looked at answers
    /// [`dbserver::PORT`], but it is documented as dynamic and the capture
    /// corpus carries dbserver conversations on 1054, 1056 and a dozen other
    /// numbers — so asking is not ceremony.
    pub async fn query_port(peer: Ipv4Addr, timeout: Duration) -> Result<u16> {
        Self::query_port_at(peer, dbserver::PORT_QUERY_PORT, timeout).await
    }

    /// As [`DbClient::query_port`], against a port other than 12523.
    ///
    /// The query port is fixed by the protocol; this exists so that a test can
    /// stand a player up next to whatever else is already bound.
    pub async fn query_port_at(peer: Ipv4Addr, query_port: u16, timeout: Duration) -> Result<u16> {
        let address = SocketAddr::V4(SocketAddrV4::new(peer, query_port));
        let mut stream = tokio::time::timeout(timeout, TcpStream::connect(address))
            .await
            .map_err(|_| Error::Timeout {
                what: "connecting to the dbserver port query",
                after: timeout,
            })?
            .map_err(Error::io("connecting to the dbserver port query"))?;
        stream
            .write_all(&dbserver::PORT_QUERY)
            .await
            .map_err(Error::io("sending the dbserver port query"))?;

        let mut answer = [0u8; 2];
        tokio::time::timeout(timeout, stream.read_exact(&mut answer))
            .await
            .map_err(|_| Error::Timeout {
                what: "the dbserver port query",
                after: timeout,
            })?
            .map_err(Error::io("reading the dbserver port answer"))?;
        let port = dbserver::decode_port_reply(&answer)?;
        if port == 0 {
            return Err(Error::Refused {
                what: "the dbserver port query",
                detail: "the player answered port 0, which is not a port".to_owned(),
            });
        }
        debug!(%peer, port, "dbserver port");
        Ok(port)
    }

    /// Ask for the port, connect, and introduce ourselves.
    pub async fn connect(peer: Ipv4Addr, device: BrowsableDeviceNumber) -> Result<Self> {
        let config = DbConfig::default();
        let port = Self::query_port(peer, config.connect_timeout).await?;
        Self::connect_at(peer, port, device, config).await
    }

    /// Connect to a known port and introduce ourselves.
    ///
    /// The preamble goes out with the `INTRODUCE` behind it, without waiting
    /// for the echo: the player answers them in order, and a round trip spent
    /// waiting buys nothing.
    pub async fn connect_at(
        peer: Ipv4Addr,
        port: u16,
        device: BrowsableDeviceNumber,
        config: DbConfig,
    ) -> Result<Self> {
        let address = SocketAddr::V4(SocketAddrV4::new(peer, port));
        let stream = tokio::time::timeout(config.connect_timeout, TcpStream::connect(address))
            .await
            .map_err(|_| Error::Timeout {
                what: "connecting to a dbserver",
                after: config.connect_timeout,
            })?
            .map_err(Error::io("connecting to a dbserver"))?;

        let mut client = Self {
            stream,
            peer,
            port,
            device,
            // Replaced by the INTRODUCE reply before this is observable.
            server: DeviceNumber::ONE,
            config,
            buffer: Vec::with_capacity(READ_CHUNK),
            consumed: 0,
            transaction: dbserver::FIRST_TRANSACTION_ID,
            desynchronised: false,
        };

        let mut opening = dbserver::PREAMBLE.to_vec();
        opening.extend_from_slice(&Message::introduce(device).encode());
        client
            .stream
            .write_all(&opening)
            .await
            .map_err(Error::io("opening a dbserver session"))?;

        client.expect_preamble().await?;
        client.server = client.expect_introduce_reply().await?;
        debug!(%peer, port, server = %client.server, "dbserver session open");
        Ok(client)
    }

    /// The player at the other end.
    pub fn peer(&self) -> Ipv4Addr {
        self.peer
    }

    /// The port this session is on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Our own device number, as it goes into every descriptor.
    pub fn device(&self) -> BrowsableDeviceNumber {
        self.device
    }

    /// The player's own device number, from its `INTRODUCE` reply.
    ///
    /// The one `SUCCESS` whose second argument is not an item count (F7).
    pub fn server(&self) -> DeviceNumber {
        self.server
    }

    /// Whether this connection has lost track of the stream and must be
    /// dropped.
    pub fn is_desynchronised(&self) -> bool {
        self.desynchronised
    }

    /// The descriptor a request against `slot` carries.
    ///
    /// One connection serves both media: a player browsing a deck's SD and its
    /// USB opens exactly one dbserver connection and tells them apart by the
    /// slot byte alone (F37).
    pub fn descriptor(&self, slot: Slot, menu: MenuTarget, track_type: TrackType) -> Descriptor {
        Descriptor::new(self.device, slot, menu, track_type)
    }

    /// Say goodbye the way a real deck does, then drop the connection.
    ///
    /// Best effort: a player that has already gone will not mind.
    pub async fn close(mut self) -> Result<()> {
        let farewell = Message::disconnect().encode();
        let _ = self.stream.write_all(&farewell).await;
        let _ = self.stream.shutdown().await;
        Ok(())
    }

    // -- browsing ---------------------------------------------------------

    /// The root category list.
    ///
    /// Twelve rows on a full library, each label wrapped in U+FFFA … U+FFFB.
    /// [`dbserver::unwrap_menu_label`] takes the wrapper off for display; a
    /// bare label renders and is then not openable, which is what makes the
    /// wrapper worth knowing about (F26).
    pub async fn root_menu(&mut self, slot: Slot) -> Result<Vec<MenuItem>> {
        let descriptor = self.main(slot);
        self.menu(
            MessageKind::MENU_ROOT,
            descriptor,
            &[SortOrder::DEFAULT.0, ROOT_MENU_MASK],
        )
        .await
    }

    /// The sort orders a menu offers.
    ///
    /// Argument 2 names the menu being sorted and a real player answers the
    /// same twelve regardless — why the argument exists is a known unknown.
    /// Real decks were observed sending this with an undocumented menu-target
    /// byte of `0x05`; the target says where an answer is *shown*, which a
    /// client has no opinion about, so this uses the ordinary one.
    pub async fn sort_menu(&mut self, slot: Slot, menu: MessageKind) -> Result<Vec<MenuItem>> {
        let descriptor = self.main(slot);
        self.menu(
            MessageKind::MENU_SORT,
            descriptor,
            &[SortOrder::DEFAULT.0, u32::from(menu.0)],
        )
        .await
    }

    /// One flat category — `MENU_ARTIST`, `MENU_ALBUM`, `MENU_GENRE`,
    /// `MENU_KEY`, `MENU_BITRATE`, `MENU_HISTORY` and the rest.
    ///
    /// The sort order chooses each row's **second column**, which is what makes
    /// sorting useful rather than cosmetic (F43).
    pub async fn category(
        &mut self,
        slot: Slot,
        kind: MessageKind,
        sort: SortOrder,
    ) -> Result<Vec<MenuItem>> {
        let descriptor = self.main(slot);
        self.menu(kind, descriptor, &[sort.0]).await
    }

    /// Every track on the medium.
    pub async fn tracks(&mut self, slot: Slot, sort: SortOrder) -> Result<Vec<MenuItem>> {
        self.category(slot, MessageKind::MENU_TRACK, sort).await
    }

    /// A playlist, or a folder of playlists.
    ///
    /// `(0, folder = true)` is the root of the tree. Inside a playlist,
    /// [`SortOrder::DEFAULT`] must keep the curated order — that is what a
    /// playlist is for.
    pub async fn playlist(
        &mut self,
        slot: Slot,
        id: u32,
        folder: bool,
        sort: SortOrder,
    ) -> Result<Vec<MenuItem>> {
        let descriptor = self.main(slot);
        self.menu(
            MessageKind::MENU_PLAYLIST,
            descriptor,
            &[sort.0, id, u32::from(folder)],
        )
        .await
    }

    /// Drill `depth` levels into a category.
    ///
    /// One systematic message type addresses every drill-down (F42), and the
    /// chains differ per category: GENRE narrows to an artist, then an album,
    /// then tracks; ARTIST skips straight to albums; KEY has an extra level no
    /// other category has, a harmonic tolerance (F44). A filter of
    /// [`dbserver::FILTER_ALL`] means "do not narrow at this level".
    ///
    /// **The category byte is the menu-request numbering, not the root-item id
    /// numbering** — KEY is `0x14` in a [`Drill`] and `0x0c` in
    /// [`dbserver::ROOT_CATEGORIES`]. Two schemes that disagree is exactly how
    /// a deck opening "KEY" once ended up asking for bitrates (F40).
    pub async fn drill(
        &mut self,
        slot: Slot,
        drill: Drill,
        sort: SortOrder,
        filters: &[u32],
    ) -> Result<Vec<MenuItem>> {
        let descriptor = self.main(slot);
        let mut extra = Vec::with_capacity(filters.len().saturating_add(1));
        extra.push(sort.0);
        extra.extend_from_slice(filters);
        self.menu(drill.kind(), descriptor, &extra).await
    }

    /// Unanalysed files, by directory.
    ///
    /// Track type 2 in the descriptor, which is what makes this a different
    /// tree from everything else here. [`dbserver::FILTER_ALL`] is the root.
    pub async fn folder(&mut self, slot: Slot, id: u32) -> Result<Vec<MenuItem>> {
        let descriptor = self.descriptor(slot, MenuTarget::MAIN, TrackType::UNANALYSED);
        self.menu(
            MessageKind::MENU_FOLDER,
            descriptor,
            &[SortOrder::DEFAULT.0, id, 0],
        )
        .await
    }

    /// Search as you type.
    ///
    /// A deck sends one of these per keystroke. **Argument 3 is the text** and
    /// argument 2 its UTF-16 size including the NUL; reading argument 2 as the
    /// term is why search once matched nothing (F44). [`Message::search`]
    /// builds it, so the order cannot be got wrong here.
    ///
    /// # The term goes out upper-cased
    ///
    /// **A real player's search is case-sensitive against an upper-cased
    /// index** — *a new observation, not yet in the research record.* Every one
    /// of the eleven `MENU_SEARCH` requests in `S20-browse-ground-truth` carries
    /// its term in capitals (`H`, `HE`, `HEL`, `HELO`, then `B`, `BI`, `BIT`),
    /// because the deck's on-screen keyboard has no lower case to send. A
    /// player therefore never has to fold case, and evidently does not: sending
    /// `bit` to a real CDJ matches nothing at all, where `BIT` matches.
    ///
    /// So this upper-cases on the way out, which is what the hardware we are
    /// imitating does. Our own server folds both sides and would accept either.
    pub async fn search(
        &mut self,
        slot: Slot,
        term: &str,
        sort: SortOrder,
    ) -> Result<Vec<MenuItem>> {
        let descriptor = self.main(slot);
        let request = Message::search(
            self.next_transaction(),
            descriptor,
            sort,
            &term.to_uppercase(),
        );
        self.paged(request, descriptor).await
    }

    // -- one track --------------------------------------------------------

    /// A track's metadata.
    ///
    /// Uses the **transient** menu target, which is how a deck dips into
    /// metadata without disturbing the list it is scrolling (F27, F41). A track
    /// the player does not have answers with no items, and that is not an
    /// error.
    pub async fn metadata(&mut self, slot: Slot, track_id: u32) -> Result<TrackMetadata> {
        let descriptor = self.descriptor(slot, MenuTarget::SUB, TrackType::REKORDBOX);
        let items = self
            .menu(MessageKind::GET_METADATA, descriptor, &[track_id])
            .await?;
        Ok(TrackMetadata::from_items(track_id, items))
    }

    /// A track's path, size and container.
    ///
    /// The bridge to the NFS half: [`TrackInfo::path`] goes straight into
    /// [`crate::consume::NfsClient::open`] and [`TrackInfo::size`] is what
    /// tells a transfer where the file ends.
    pub async fn track_info(&mut self, slot: Slot, track_id: u32) -> Result<TrackInfo> {
        let descriptor = self.descriptor(slot, MenuTarget::BINARY, TrackType::REKORDBOX);
        let items = self
            .menu(MessageKind::GET_TRACK_INFO, descriptor, &[track_id])
            .await?;
        TrackInfo::from_items(track_id, items).ok_or(Error::Refused {
            what: "a dbserver GET_TRACK_INFO",
            detail: format!("track {track_id} came back with no path item, so it cannot be loaded"),
        })
    }

    /// Cover art, by the artwork id a [`TrackMetadata`] carries.
    ///
    /// **An empty result is a success.** A track with no art is answered with
    /// the length argument set to zero and the blob omitted from the wire
    /// entirely, which is the common case rather than an exotic one.
    pub async fn artwork(&mut self, slot: Slot, artwork_id: u32) -> Result<Vec<u8>> {
        let descriptor = self.binary(slot);
        let request = self.binary_request(MessageKind::GET_ARTWORK, descriptor, &[artwork_id])?;
        let reply = self.request(request).await?;
        Ok(blob_of(&reply))
    }

    /// One analysis blob, as the player put it on the wire.
    ///
    /// Every reply shares one envelope — `[request type, 0, byte length, blob,
    /// trailing…]` — whose argument 0 echoes the **request's message type**,
    /// not the track id.
    pub async fn analysis(
        &mut self,
        slot: Slot,
        track_id: u32,
        which: Analysis,
    ) -> Result<Vec<u8>> {
        let request = match which {
            // Five arguments declared and four on the wire: argument 3 is zero,
            // so the blob behind it is absent entirely. This is the message
            // that desynchronises a naive parser, and the track id is at
            // argument **2** rather than 1 like its siblings.
            Analysis::WaveformPreview => {
                let descriptor = self.binary(slot);
                Message::new(
                    self.next_transaction(),
                    MessageKind::GET_WAVEFORM_PREVIEW,
                    Arguments::from([
                        Field::from(descriptor),
                        Field::U32(WAVEFORM_PREVIEW_ARGUMENT1),
                        Field::U32(track_id),
                        Field::U32(0),
                        Field::Blob(Vec::new()),
                    ]),
                )
            }
            // Three arguments, and the **main** menu target where every other
            // analysis fetch uses the binary one — consistent across four
            // capture sessions, and reproduced without being understood.
            Analysis::WaveformDetail => {
                let descriptor = self.main(slot);
                Message::new(
                    self.next_transaction(),
                    MessageKind::GET_WAVEFORM_DETAIL,
                    Arguments::from([Field::from(descriptor), Field::U32(track_id), Field::U32(0)]),
                )
            }
            other => {
                let descriptor = self.binary(slot);
                self.binary_request(other.request(), descriptor, &[track_id])?
            }
        };
        let reply = self.request(request).await?;
        Ok(blob_of(&reply))
    }

    // -- the request/reply machinery --------------------------------------

    /// Issue a menu request and page every row of its result.
    ///
    /// `SUCCESS [type, count]`, then one render per [`DbConfig::batch`] rows,
    /// then one `MENU_CLOSE` under the last render's transaction id.
    pub async fn menu(
        &mut self,
        kind: MessageKind,
        descriptor: Descriptor,
        extra: &[u32],
    ) -> Result<Vec<MenuItem>> {
        let request = Message::menu_request(self.next_transaction(), kind, descriptor, extra)
            .ok_or(Error::Refused {
                what: "a dbserver menu request",
                detail: format!(
                    "{} arguments is more than the twelve a header can describe",
                    extra.len().saturating_add(1)
                ),
            })?;
        self.paged(request, descriptor).await
    }

    async fn paged(&mut self, request: Message, descriptor: Descriptor) -> Result<Vec<MenuItem>> {
        let kind = request.kind;
        let reply = self.request(request).await?;
        if reply.kind != MessageKind::SUCCESS {
            return Err(Error::Refused {
                what: "a dbserver menu request",
                detail: format!("{kind:?} was answered with {:?}", reply.kind),
            });
        }
        let count = reply.number(1).unwrap_or(0);
        if count == NOT_FOUND {
            // A successful reply reporting absence. An empty menu is not a
            // refusal, and the two must not look the same.
            debug!(?kind, "the player has nothing under that");
            return Ok(Vec::new());
        }
        self.render(descriptor, count).await
    }

    async fn render(&mut self, descriptor: Descriptor, count: u32) -> Result<Vec<MenuItem>> {
        let mut items = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        let mut offset = 0;
        let mut last = None;
        while offset < count {
            let limit = self.config.batch.min(count - offset).max(1);
            let transaction = self.next_transaction();
            // `count`, not `limit`: a render names which pending result set it
            // is paging, and a server keys those on (descriptor, count).
            let render = Message::render_of(transaction, descriptor, offset, limit, count);
            self.send(&render).await?;
            items.extend(self.read_page(transaction).await?);
            offset = offset.saturating_add(limit);
            last = Some(transaction);
        }
        if let Some(transaction) = last {
            self.close_menu(transaction).await?;
        }
        Ok(items)
    }

    /// One render's worth of rows: a header, the items, a footer.
    async fn read_page(&mut self, transaction: u32) -> Result<Vec<MenuItem>> {
        let mut items = Vec::new();
        loop {
            let message = self.recv().await?;
            if message.transaction_id != transaction {
                return Err(self.desynchronise(transaction, &message));
            }
            match message.kind {
                MessageKind::MENU_HEADER => {}
                MessageKind::MENU_FOOTER => return Ok(items),
                MessageKind::MENU_ITEM => match MenuItem::from_message(&message) {
                    Some(item) => items.push(item),
                    None => {
                        return Err(Error::Refused {
                            what: "a dbserver menu item",
                            detail: format!("a 0x4101 that does not decode as a row: {message:?}"),
                        });
                    }
                },
                MessageKind::ERROR => {
                    return Err(Error::Refused {
                        what: "a dbserver render",
                        detail: format!("the player refused mid-page: {message:?}"),
                    });
                }
                other => trace!(?other, "an unexpected message inside a menu page"),
            }
        }
    }

    /// "Done with that menu." Draws **no reply**; waiting for one hangs (F16).
    async fn close_menu(&mut self, transaction: u32) -> Result<()> {
        let close = Message::new(transaction, MessageKind::MENU_CLOSE, []);
        debug_assert!(
            close.kind.expects_no_reply(),
            "MENU_CLOSE is the one message with no answer"
        );
        self.send(&close).await
    }

    /// Send one message and read its reply.
    ///
    /// Refuses [`MessageKind::MENU_CLOSE`], which has no reply to read: a
    /// caller that got here with one would block until the request timeout.
    pub async fn request(&mut self, message: Message) -> Result<Message> {
        if message.kind.expects_no_reply() {
            return Err(Error::Refused {
                what: "a dbserver request",
                detail: format!("{:?} draws no reply and must not be awaited", message.kind),
            });
        }
        let transaction = message.transaction_id;
        let kind = message.kind;
        self.send(&message).await?;
        let reply = self.recv().await?;
        if reply.transaction_id != transaction {
            return Err(self.desynchronise(transaction, &reply));
        }
        if reply.kind == MessageKind::ERROR {
            return Err(Error::Refused {
                what: "a dbserver request",
                detail: format!("the player refused {kind:?}: {reply:?}"),
            });
        }
        Ok(reply)
    }

    async fn send(&mut self, message: &Message) -> Result<()> {
        if self.desynchronised {
            return Err(Error::Refused {
                what: "a dbserver request",
                detail: "this connection lost track of the stream and cannot recover".to_owned(),
            });
        }
        trace!(?message, "->");
        self.stream
            .write_all(&message.encode())
            .await
            .map_err(Error::io("sending a dbserver message"))
    }

    async fn recv(&mut self) -> Result<Message> {
        loop {
            if let Some(message) = self.take_message()? {
                trace!(?message, "<-");
                return Ok(message);
            }
            self.fill().await?;
        }
    }

    /// Decode one message out of what has already arrived.
    ///
    /// `None` means "not yet" — the expected outcome of trying too early on a
    /// stream with no length framing, and the only decode failure that is not
    /// fatal. Parsing resumes from a cursor rather than re-reading the whole
    /// buffer, so a multi-kilobyte cover does not cost a re-parse per segment.
    fn take_message(&mut self) -> Result<Option<Message>> {
        let pending = self.buffer.get(self.consumed..).unwrap_or_default();
        if pending.is_empty() {
            return Ok(None);
        }
        match Message::decode(pending) {
            Ok((message, used)) => {
                self.consumed = self.consumed.saturating_add(used);
                self.compact();
                Ok(Some(message))
            }
            Err(error) if error.is_truncated() => Ok(None),
            Err(error) => {
                // No frame boundary to skip to, so this connection is over.
                self.desynchronised = true;
                Err(error.into())
            }
        }
    }

    /// Drop what has been consumed, so a long session does not grow without
    /// bound.
    fn compact(&mut self) {
        if self.consumed == self.buffer.len() {
            self.buffer.clear();
            self.consumed = 0;
        } else if self.consumed >= READ_CHUNK {
            self.buffer.drain(..self.consumed);
            self.consumed = 0;
        }
    }

    async fn fill(&mut self) -> Result<()> {
        if self.buffer.len() >= MAX_BUFFER {
            self.desynchronised = true;
            return Err(Error::Refused {
                what: "a dbserver reply",
                detail: format!("{MAX_BUFFER} bytes arrived without completing one message"),
            });
        }
        // Read straight into the buffer rather than through a stack array: a
        // 16 KiB array inside an async fn makes every future that awaits it
        // 16 KiB too, all the way up the call chain.
        let start = self.buffer.len();
        self.buffer.resize(start.saturating_add(READ_CHUNK), 0);
        let target = self.buffer.get_mut(start..).unwrap_or_default();
        let outcome =
            tokio::time::timeout(self.config.request_timeout, self.stream.read(target)).await;
        let read = match outcome {
            Ok(Ok(read)) => read,
            Ok(Err(error)) => {
                self.buffer.truncate(start);
                return Err(Error::io("reading from a dbserver")(error));
            }
            Err(_elapsed) => {
                self.buffer.truncate(start);
                return Err(Error::Timeout {
                    what: "a dbserver reply",
                    after: self.config.request_timeout,
                });
            }
        };
        self.buffer.truncate(start.saturating_add(read));
        if read == 0 {
            return Err(Error::Refused {
                what: "a dbserver reply",
                detail: "the player closed the connection".to_owned(),
            });
        }
        Ok(())
    }

    /// Step over the five-byte preamble the player sends before any message.
    ///
    /// [`dbserver::skip_preamble`] is the same rule applied to a whole captured
    /// stream; here the bytes arrive over time, so the wait is explicit.
    async fn expect_preamble(&mut self) -> Result<()> {
        while self.buffer.len() < dbserver::PREAMBLE.len() {
            self.fill().await?;
        }
        if self.buffer.get(..dbserver::PREAMBLE.len()) != Some(&dbserver::PREAMBLE[..]) {
            self.desynchronised = true;
            return Err(Error::Refused {
                what: "a dbserver connection",
                detail: format!(
                    "the peer opened with {:02x?}, not the five-byte preamble",
                    self.buffer.get(..dbserver::PREAMBLE.len())
                ),
            });
        }
        self.consumed = dbserver::PREAMBLE.len();
        self.compact();
        Ok(())
    }

    async fn expect_introduce_reply(&mut self) -> Result<DeviceNumber> {
        let reply = self.recv().await?;
        if reply.transaction_id != dbserver::SETUP_TRANSACTION_ID {
            return Err(self.desynchronise(dbserver::SETUP_TRANSACTION_ID, &reply));
        }
        if reply.kind != MessageKind::SUCCESS {
            return Err(Error::Refused {
                what: "a dbserver INTRODUCE",
                detail: format!("the player answered {:?}", reply.kind),
            });
        }
        // Argument 1 is the player's **own** number here, not an item count —
        // the one SUCCESS whose second argument means something else (F7).
        let raw = reply.number(1).unwrap_or(0);
        DeviceNumber::new(u8::try_from(raw).unwrap_or(0)).ok_or(Error::Refused {
            what: "a dbserver INTRODUCE",
            detail: format!("the player called itself device {raw}"),
        })
    }

    fn desynchronise(&mut self, wanted: u32, got: &Message) -> Error {
        self.desynchronised = true;
        warn!(
            wanted = format!("{wanted:#x}"),
            got = format!("{:#x}", got.transaction_id),
            "dbserver replies are out of step"
        );
        Error::Refused {
            what: "a dbserver reply",
            detail: format!(
                "expected transaction {wanted:#x}, got {:#x}; the stream has no framing to \
                 resynchronise on",
                got.transaction_id
            ),
        }
    }

    fn next_transaction(&mut self) -> u32 {
        let transaction = self.transaction;
        // Wrapping back to the first id keeps us in the region a real player
        // uses; the value only has to be unique among the calls in flight, and
        // there is never more than one.
        self.transaction = self
            .transaction
            .checked_add(1)
            .unwrap_or(dbserver::FIRST_TRANSACTION_ID);
        transaction
    }

    fn main(&self, slot: Slot) -> Descriptor {
        self.descriptor(slot, MenuTarget::MAIN, TrackType::REKORDBOX)
    }

    fn binary(&self, slot: Slot) -> Descriptor {
        self.descriptor(slot, MenuTarget::BINARY, TrackType::REKORDBOX)
    }

    fn binary_request(
        &mut self,
        kind: MessageKind,
        descriptor: Descriptor,
        extra: &[u32],
    ) -> Result<Message> {
        Message::menu_request(self.next_transaction(), kind, descriptor, extra).ok_or(
            Error::Refused {
                what: "a dbserver binary request",
                detail: format!("{kind:?} needs more arguments than a message can carry"),
            },
        )
    }
}

/// The payload of a binary reply.
///
/// Argument 3 of the shared envelope, and an **empty** result is a success: a
/// player answers `GetArtwork` for a track with no art by setting the length
/// argument to zero and omitting the blob from the wire entirely. The fallback
/// to "the first blob argument" covers a reply shape we have not seen; every
/// observed one puts it at 3.
fn blob_of(reply: &Message) -> Vec<u8> {
    if let Some(blob) = reply.blob(3) {
        return blob.to_vec();
    }
    reply
        .args
        .as_slice()
        .iter()
        .find_map(Field::blob)
        .unwrap_or_default()
        .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use prolink_proto::dbserver::{FIRST_TRANSACTION_ID, ROOT_CATEGORIES};
    use tokio::net::TcpListener;

    fn us() -> BrowsableDeviceNumber {
        BrowsableDeviceNumber::new(1).expect("device 1 is browsable")
    }

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            digits.len().is_multiple_of(2),
            "a hex literal needs an even length"
        );
        digits
            .chunks_exact(2)
            .map(|pair| {
                let byte: String = pair.iter().collect();
                u8::from_str_radix(&byte, 16).expect("a hex literal must be hex")
            })
            .collect()
    }

    // -- a loopback player ------------------------------------------------
    //
    // Built out of the codec crate's own reply builders, because there are no
    // CDJs on this machine. It logs every request it is sent, so a test can
    // assert the *sequence* a browse produces and not merely its result.

    #[derive(Clone, Debug)]
    struct Library {
        /// Rows to render for an ordinary menu.
        rows: Vec<MenuItem>,
        /// Rows a `GET_METADATA` renders.
        metadata: Vec<MenuItem>,
        /// Rows a `GET_TRACK_INFO` renders.
        track_info: Vec<MenuItem>,
        /// The blob a `GET_ARTWORK` answers with; empty means "no art".
        artwork: Vec<u8>,
        /// Answer every menu request with this count instead.
        force_count: Option<u32>,
        /// Answer under a transaction id nobody asked about.
        wrong_transaction: bool,
        /// Accept the connection and then say nothing at all.
        deaf: bool,
    }

    impl Default for Library {
        fn default() -> Self {
            Self {
                rows: ROOT_CATEGORIES.iter().map(|c| c.to_item()).collect(),
                metadata: metadata_rows(),
                track_info: track_info_rows(),
                artwork: Vec::new(),
                force_count: None,
                wrong_transaction: false,
                deaf: false,
            }
        }
    }

    /// The nine rows a real CDJ-2000NXS rendered for track `0xc8` in
    /// `S06-load-and-play`, transcribed from the capture.
    fn metadata_rows() -> Vec<MenuItem> {
        vec![
            MenuItem {
                argument0: 1,
                id: 0xc8,
                label1: "Loneliness - Klub Cut".to_owned(),
                label2: String::new(),
                item_type: ItemType::TRACK_TITLE,
                flags: MenuItem::TRACK_FLAGS,
                artwork_id: 0xba,
                playlist_position: 0,
                key_index: 0,
            },
            MenuItem::named(0x7a, ItemType::ARTIST, "Tomcraft"),
            MenuItem::named(0x56, ItemType::ALBUM, "Loneliness"),
            MenuItem::named(0x1d7, ItemType::DURATION, ""),
            MenuItem::named(0x3391, ItemType::TEMPO, ""),
            MenuItem::named(
                0xc8,
                ItemType::COMMENT,
                "https://music.youtube.com/watch?v=6gnPFu8KD74",
            ),
            MenuItem::named(9, ItemType::KEY, "12A"),
            MenuItem::named(3, ItemType::RATING, ""),
            MenuItem::named(0, ItemType::COLOR, ""),
        ]
    }

    /// The six rows of the same track's `GET_TRACK_INFO`, likewise.
    fn track_info_rows() -> Vec<MenuItem> {
        vec![
            // `0x04` here is the container, not the title (F35).
            MenuItem::named(1, ItemType::TRACK_TITLE, ""),
            MenuItem::named(0x1d7, ItemType::DURATION, ""),
            MenuItem::named(0x3391, ItemType::TEMPO, ""),
            MenuItem::named(
                0xc8,
                ItemType::COMMENT,
                "https://music.youtube.com/watch?v=6gnPFu8KD74",
            ),
            MenuItem {
                // The file size, in the slot that is zero on every other menu
                // item ever captured (F31).
                argument0: 0x0074_7a7b,
                id: 0xc8,
                label1: "/Contents/Tomcraft/Loneliness/Tomcraft - Loneliness - Klub Cut.mp3"
                    .to_owned(),
                label2: String::new(),
                item_type: ItemType::PATH,
                flags: 0,
                artwork_id: 0,
                playlist_position: 0,
                key_index: 0,
            },
            MenuItem::named(1, ItemType::TRACK_INFO_UNKNOWN, ""),
        ]
    }

    #[derive(Debug)]
    struct Player {
        port: u16,
        query_port: u16,
        seen: Arc<Mutex<Vec<Message>>>,
        tasks: Vec<tokio::task::JoinHandle<()>>,
    }

    impl Drop for Player {
        fn drop(&mut self) {
            for task in &self.tasks {
                task.abort();
            }
        }
    }

    impl Player {
        fn requests(&self) -> Vec<Message> {
            match self.seen.lock() {
                Ok(seen) => seen.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        fn of_kind(&self, kind: MessageKind) -> Vec<Message> {
            self.requests()
                .into_iter()
                .filter(|message| message.kind == kind)
                .collect()
        }
    }

    async fn listener() -> (TcpListener, u16) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("an ephemeral loopback listener");
        let port = match listener.local_addr().expect("a bound listener") {
            SocketAddr::V4(address) => address.port(),
            SocketAddr::V6(_) => unreachable!("bound as IPv4"),
        };
        (listener, port)
    }

    async fn player(library: Library) -> Player {
        let (dbserver_listener, port) = listener().await;
        let (query_listener, query_port) = listener().await;
        let seen = Arc::new(Mutex::new(Vec::new()));

        let mut tasks = Vec::new();
        tasks.push(tokio::spawn(async move {
            while let Ok((mut socket, _)) = query_listener.accept().await {
                let mut query = [0u8; dbserver::PORT_QUERY.len()];
                if socket.read_exact(&mut query).await.is_err() {
                    continue;
                }
                assert_eq!(query, dbserver::PORT_QUERY, "the fixed 19-byte query");
                let _ = socket.write_all(&dbserver::encode_port_reply(port)).await;
            }
        }));

        let log = Arc::clone(&seen);
        tasks.push(tokio::spawn(async move {
            while let Ok((socket, _)) = dbserver_listener.accept().await {
                let library = library.clone();
                let log = Arc::clone(&log);
                tokio::spawn(serve(socket, library, log));
            }
        }));

        Player {
            port,
            query_port,
            seen,
            tasks,
        }
    }

    async fn serve(mut socket: TcpStream, library: Library, log: Arc<Mutex<Vec<Message>>>) {
        if library.deaf {
            // Accept and say nothing, so the client has to time out.
            std::future::pending::<()>().await;
        }
        if socket.write_all(&dbserver::PREAMBLE).await.is_err() {
            return;
        }
        // The client opens with one too, and it is not a message: a decoder
        // that goes straight for the first magic fails on byte zero of every
        // connection. `skip_preamble` is the same rule over a captured stream.
        let mut opening = [0u8; dbserver::PREAMBLE.len()];
        if socket.read_exact(&mut opening).await.is_err() {
            return;
        }
        assert_eq!(
            opening,
            dbserver::PREAMBLE,
            "a client must open with the five-byte preamble"
        );
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let mut consumed = 0usize;
            while let Ok((message, used)) = Message::decode(&buffer[consumed..]) {
                consumed += used;
                match log.lock() {
                    Ok(mut seen) => seen.push(message.clone()),
                    Err(poisoned) => poisoned.into_inner().push(message.clone()),
                }
                for reply in answer(&message, &library) {
                    if socket.write_all(&reply.encode()).await.is_err() {
                        return;
                    }
                }
            }
            buffer.drain(..consumed);
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            }
        }
    }

    fn answer(message: &Message, library: &Library) -> Vec<Message> {
        let transaction = if library.wrong_transaction {
            message.transaction_id ^ 0xFFFF
        } else {
            message.transaction_id
        };
        match message.kind {
            MessageKind::INTRODUCE => vec![Message {
                transaction_id: transaction,
                ..Message::introduce_reply(DeviceNumber::new(2).unwrap_or(DeviceNumber::ONE))
            }],
            // The two a real player answers with silence.
            MessageKind::DISCONNECT | MessageKind::MENU_CLOSE => Vec::new(),
            MessageKind::RENDER_MENU => render_page(message, library, transaction),
            MessageKind::GET_ARTWORK => Message::binary_reply(
                transaction,
                MessageKind::ARTWORK,
                MessageKind::GET_ARTWORK,
                library.artwork.clone(),
                &[],
            )
            .into_iter()
            .collect(),
            MessageKind::GET_WAVEFORM_PREVIEW
            | MessageKind::GET_WAVEFORM_DETAIL
            | MessageKind::GET_BEAT_GRID
            | MessageKind::GET_CUE_POINTS
            | MessageKind::GET_CUE_POINTS_EXT
            | MessageKind::GET_VBR_INDEX => Message::binary_reply(
                transaction,
                MessageKind::WAVEFORM_PREVIEW,
                message.kind,
                vec![7; 16],
                &[],
            )
            .into_iter()
            .collect(),
            kind => {
                let count = library
                    .force_count
                    .unwrap_or_else(|| default_count(kind, library));
                vec![Message::success(transaction, kind, count)]
            }
        }
    }

    fn render_page(render: &Message, library: &Library, transaction: u32) -> Vec<Message> {
        let offset = render.number(1).unwrap_or(0);
        let limit = render.number(2).unwrap_or(0);
        let total = render.number(4).unwrap_or(0);
        let rows = rows_for(render, library, total);
        let mut page = vec![Message::menu_header(transaction)];
        for index in offset..offset.saturating_add(limit).min(total) {
            let position = usize::try_from(index).unwrap_or(0) % rows.len().max(1);
            let row = rows
                .get(position)
                .cloned()
                .unwrap_or_else(|| MenuItem::named(index, ItemType::TRACK_TITLE, "row"));
            page.push(row.to_message(transaction));
        }
        page.push(Message::menu_footer(transaction));
        page
    }

    fn default_count(kind: MessageKind, library: &Library) -> u32 {
        let rows = match kind {
            MessageKind::GET_METADATA => library.metadata.len(),
            MessageKind::GET_TRACK_INFO => library.track_info.len(),
            _ => library.rows.len(),
        };
        u32::try_from(rows).unwrap_or(0)
    }

    /// A render does not name the menu it is paging, so the fake player does
    /// what a real one does: it keys on the descriptor and the result-set size.
    fn rows_for(render: &Message, library: &Library, total: u32) -> Vec<MenuItem> {
        let target = render.descriptor().map(|descriptor| descriptor.menu);
        if target == Some(MenuTarget::SUB) && total == count_of(&library.metadata) {
            return library.metadata.clone();
        }
        if target == Some(MenuTarget::BINARY) && total == count_of(&library.track_info) {
            return library.track_info.clone();
        }
        library.rows.clone()
    }

    fn count_of(rows: &[MenuItem]) -> u32 {
        u32::try_from(rows.len()).unwrap_or(0)
    }

    async fn connect(player: &Player) -> DbClient {
        DbClient::connect_at(Ipv4Addr::LOCALHOST, player.port, us(), quick())
            .await
            .expect("connecting to the loopback player")
    }

    fn quick() -> DbConfig {
        DbConfig {
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_secs(2),
            ..DbConfig::default()
        }
    }

    // -- the handshake ----------------------------------------------------

    #[tokio::test]
    async fn the_port_query_is_nineteen_bytes_and_the_answer_is_two() {
        let player = player(Library::default()).await;
        let port = DbClient::query_port_at(
            Ipv4Addr::LOCALHOST,
            player.query_port,
            Duration::from_secs(2),
        )
        .await
        .expect("the port query");
        assert_eq!(port, player.port);
        assert_eq!(dbserver::PORT_QUERY.len(), 19);
        assert_eq!(
            dbserver::PORT_QUERY.get(..4),
            Some([0x00, 0x00, 0x00, 0x0f].as_slice()),
            "a big-endian 15: fourteen name bytes plus the NUL"
        );
    }

    #[tokio::test]
    async fn introduce_reports_the_players_own_number_not_an_item_count() {
        // The one SUCCESS whose second argument is not a count (F7).
        let player = player(Library::default()).await;
        let client = connect(&player).await;
        assert_eq!(client.server().get(), 2);
        assert_eq!(client.device().get(), 1);

        let requests = player.requests();
        assert_eq!(
            requests.first().map(|message| message.kind),
            Some(MessageKind::INTRODUCE)
        );
        assert_eq!(
            requests.first().map(|message| message.transaction_id),
            Some(dbserver::SETUP_TRANSACTION_ID),
            "INTRODUCE has its own reserved id"
        );
    }

    #[tokio::test]
    async fn a_transaction_id_starts_where_a_real_players_does() {
        // C10: not at 1, whatever the pre-hardware literature says.
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        client.root_menu(Slot::USB).await.expect("root menu");
        let ids: Vec<u32> = player
            .requests()
            .iter()
            .map(|message| message.transaction_id)
            .filter(|id| *id != dbserver::SETUP_TRANSACTION_ID)
            .collect();
        assert_eq!(ids.first(), Some(&FIRST_TRANSACTION_ID));
        assert_eq!(FIRST_TRANSACTION_ID, 0x0380_0001);
        assert!(
            ids.windows(2).all(|pair| pair[0] <= pair[1]),
            "and counts up: {ids:#x?}"
        );
    }

    // -- menus ------------------------------------------------------------

    #[tokio::test]
    async fn a_menu_larger_than_one_render_is_paged() {
        let player = player(Library {
            force_count: Some(150),
            ..Library::default()
        })
        .await;
        let mut client = connect(&player).await;
        let items = client
            .tracks(Slot::USB, SortOrder::DEFAULT)
            .await
            .expect("tracks");
        assert_eq!(items.len(), 150);

        let renders = player.of_kind(MessageKind::RENDER_MENU);
        assert_eq!(renders.len(), 3, "150 rows at 64 a render is three pages");
        assert_eq!(renders.first().and_then(|m| m.number(1)), Some(0));
        assert_eq!(renders.first().and_then(|m| m.number(2)), Some(64));
        assert_eq!(renders.get(2).and_then(|m| m.number(1)), Some(128));
        assert_eq!(
            renders.get(2).and_then(|m| m.number(2)),
            Some(22),
            "the last page asks only for what is left"
        );
    }

    #[tokio::test]
    async fn every_render_echoes_the_whole_result_set_size_not_the_page_size() {
        // A server keys its pending result sets on (descriptor, count), because
        // a deck interleaves a metadata lookup with a long list and resumes it
        // without re-issuing the request (F27, F41). The page size would name a
        // result set nobody has.
        let player = player(Library {
            force_count: Some(150),
            ..Library::default()
        })
        .await;
        let mut client = connect(&player).await;
        client
            .tracks(Slot::USB, SortOrder::DEFAULT)
            .await
            .expect("tracks");
        for render in player.of_kind(MessageKind::RENDER_MENU) {
            assert_eq!(render.number(4), Some(150), "argument 4 is the total");
        }
    }

    #[tokio::test]
    async fn menu_close_reuses_the_last_renders_transaction_id_and_draws_no_reply() {
        // F16: 23 of these in one CDJ-to-CDJ browse, and the packet accounting
        // in that capture leaves nothing over for a response.
        let player = player(Library {
            force_count: Some(150),
            ..Library::default()
        })
        .await;
        let mut client = connect(&player).await;
        client
            .tracks(Slot::USB, SortOrder::DEFAULT)
            .await
            .expect("tracks");

        // The session is still usable, which it would not be if the client had
        // gone looking for a reply to that — and by the time this returns the
        // player has certainly seen the close, because it was written first.
        client.root_menu(Slot::USB).await.expect("still in step");

        // Everything the track list sent is everything before the follow-up
        // menu request, which the player has certainly seen by now.
        let requests = player.requests();
        let follow_up = requests
            .iter()
            .position(|message| message.kind == MessageKind::MENU_ROOT)
            .expect("the follow-up menu");
        let closes: Vec<&Message> = requests[..follow_up]
            .iter()
            .filter(|message| message.kind == MessageKind::MENU_CLOSE)
            .collect();
        assert_eq!(closes.len(), 1, "one per menu, after its last page");
        let close = closes.first().expect("a close");
        let last_render = requests[..follow_up]
            .iter()
            .rev()
            .find(|message| message.kind == MessageKind::RENDER_MENU)
            .map(|message| message.transaction_id);
        assert_eq!(
            Some(close.transaction_id),
            last_render,
            "the third page's id"
        );
        assert!(close.args.is_empty(), "zero arguments, 32 bytes");
    }

    #[tokio::test]
    async fn a_count_of_ffffffff_is_an_empty_menu_and_not_an_error() {
        let player = player(Library {
            force_count: Some(NOT_FOUND),
            ..Library::default()
        })
        .await;
        let mut client = connect(&player).await;
        let items = client
            .drill(
                Slot::USB,
                Drill {
                    depth: 1,
                    category: 0x01,
                },
                SortOrder::DEFAULT,
                &[9999],
            )
            .await
            .expect("not found is not a failure");
        assert!(items.is_empty());
        assert!(
            player.of_kind(MessageKind::RENDER_MENU).is_empty(),
            "nothing to page, so nothing is paged"
        );
    }

    #[tokio::test]
    async fn an_empty_menu_and_a_refused_request_do_not_look_the_same() {
        // On a CDJ's screen they are indistinguishable, which is exactly why
        // the API must keep them apart.
        let player = player(Library {
            force_count: Some(0),
            ..Library::default()
        })
        .await;
        let mut client = connect(&player).await;
        assert!(
            client
                .category(Slot::USB, MessageKind::MENU_GENRE, SortOrder::DEFAULT)
                .await
                .expect("an empty category is a success")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_root_menu_comes_back_wrapped_for_localisation() {
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        let items = client.root_menu(Slot::USB).await.expect("root menu");
        assert_eq!(items.len(), ROOT_CATEGORIES.len());
        let first = items.first().expect("a first row");
        assert_eq!(
            dbserver::unwrap_menu_label(&first.label1),
            Some("PLAYLIST"),
            "a bare label renders and is then not openable (F26)"
        );
        assert_eq!(first.flags, 0, "category rows carry no track flags");
    }

    #[tokio::test]
    async fn a_playlist_folder_and_a_playlist_differ_by_one_argument() {
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        client
            .playlist(Slot::USB, 0, true, SortOrder::DEFAULT)
            .await
            .expect("the playlist root");
        client
            .playlist(Slot::USB, 3, false, SortOrder::DEFAULT)
            .await
            .expect("a playlist");
        let requests = player.of_kind(MessageKind::MENU_PLAYLIST);
        assert_eq!(requests.first().and_then(|m| m.number(3)), Some(1));
        assert_eq!(requests.get(1).and_then(|m| m.number(2)), Some(3));
        assert_eq!(requests.get(1).and_then(|m| m.number(3)), Some(0));
    }

    // -- one track --------------------------------------------------------

    #[tokio::test]
    async fn metadata_is_read_by_item_type_and_the_ids_are_the_rows_it_references() {
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        let track = client.metadata(Slot::USB, 0xc8).await.expect("metadata");

        assert_eq!(track.title, "Loneliness - Klub Cut");
        assert_eq!(track.artist, "Tomcraft");
        assert_eq!(
            track.artist_id, 0x7a,
            "the artist's own id, not the track's — that is what lets a caller \
             offer 'more by this artist' (F32)"
        );
        assert_eq!(track.album_id, 0x56);
        assert_eq!(
            track.artwork_id, 0xba,
            "off the title item; without it a player never asks for the cover"
        );
        assert_eq!(track.duration_seconds, 471);
        assert_eq!(track.tempo_centibpm, 13_201);
        assert!((track.tempo() - 132.01).abs() < 1e-9);
        assert_eq!(track.rating, 3);
        assert_eq!(track.key, "12A");
        assert_eq!(track.items.len(), 9);

        assert_eq!(
            player
                .of_kind(MessageKind::GET_METADATA)
                .first()
                .and_then(Message::descriptor)
                .map(|d| d.menu),
            Some(MenuTarget::SUB),
            "the transient menu, so the list being scrolled is undisturbed"
        );
    }

    #[tokio::test]
    async fn a_track_the_player_does_not_have_is_empty_rather_than_an_error() {
        let player = player(Library {
            force_count: Some(NOT_FOUND),
            ..Library::default()
        })
        .await;
        let mut client = connect(&player).await;
        let track = client.metadata(Slot::USB, 12_345).await.expect("no error");
        assert_eq!(track.id, 12_345);
        assert!(track.title.is_empty() && track.items.is_empty());
    }

    #[tokio::test]
    async fn track_info_takes_the_file_size_from_argument_zero_of_the_path_item() {
        // F31: zero on every other menu item ever captured, so it reads as
        // structural padding — and without it a deck resolves the path, never
        // issues a READ, and reports that it cannot decode the format.
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        let info = client
            .track_info(Slot::USB, 0xc8)
            .await
            .expect("track info");
        assert_eq!(info.size, 7_633_531);
        assert_eq!(
            info.path, "/Contents/Tomcraft/Loneliness/Tomcraft - Loneliness - Klub Cut.mp3",
            "already relative to the mount root, ready for NfsClient::open"
        );
        assert_eq!(info.duration_seconds, 471);
    }

    #[tokio::test]
    async fn the_same_type_byte_is_the_title_in_metadata_and_the_container_in_track_info() {
        // F35. Two earlier readings had these the other way round and the
        // errors cancelled for the only format that had ever been captured.
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        let metadata = client.metadata(Slot::USB, 0xc8).await.expect("metadata");
        let info = client
            .track_info(Slot::USB, 0xc8)
            .await
            .expect("track info");
        assert_eq!(metadata.title, "Loneliness - Klub Cut");
        assert_eq!(info.container, 1, "an id, and an empty label beside it");
        assert!(
            !info.items.iter().any(|item| item.label1 == metadata.title),
            "nothing in a track-info reply carries the title"
        );
    }

    #[tokio::test]
    async fn a_track_with_no_artwork_answers_with_an_omitted_blob_and_that_is_success() {
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        let art = client
            .artwork(Slot::USB, 0)
            .await
            .expect("no art is not an error");
        assert!(art.is_empty());
        assert_eq!(
            player
                .of_kind(MessageKind::GET_ARTWORK)
                .first()
                .and_then(Message::descriptor)
                .map(|d| d.menu),
            Some(MenuTarget::BINARY),
            "artwork is a binary load"
        );
    }

    #[tokio::test]
    async fn artwork_bytes_come_back_whole() {
        let player = player(Library {
            artwork: vec![0xab; 2740],
            ..Library::default()
        })
        .await;
        let mut client = connect(&player).await;
        let art = client.artwork(Slot::USB, 0xba).await.expect("artwork");
        assert_eq!(art.len(), 2740, "the size of a real cover in the corpus");
        assert_eq!(art.first(), Some(&0xab));
    }

    #[tokio::test]
    async fn the_waveform_preview_carries_its_track_id_at_argument_two() {
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        client
            .analysis(Slot::USB, 0xc8, Analysis::WaveformPreview)
            .await
            .expect("preview");
        let request = player
            .of_kind(MessageKind::GET_WAVEFORM_PREVIEW)
            .first()
            .cloned()
            .expect("a preview request");
        assert_eq!(request.args.len(), 5, "five declared");
        assert_eq!(request.number(1), Some(3));
        assert_eq!(
            request.number(2),
            Some(0xc8),
            "not argument 1 like its siblings"
        );
        assert_eq!(request.number(3), Some(0));
        assert_eq!(request.blob(4), Some(&[][..]));
        assert_eq!(
            request.encode().len(),
            32 + 4 * 5,
            "four on the wire: the trailing blob is omitted entirely"
        );
    }

    #[tokio::test]
    async fn the_detailed_waveform_uses_the_main_target_where_its_siblings_use_binary() {
        // Consistent across S06, S11, S13 and S17. Reproduced without being
        // understood.
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        for which in [
            Analysis::WaveformDetail,
            Analysis::BeatGrid,
            Analysis::CuePoints,
            Analysis::VbrIndex,
        ] {
            client
                .analysis(Slot::USB, 0xc8, which)
                .await
                .expect("analysis");
        }
        let targets: Vec<(MessageKind, Option<MenuTarget>)> = player
            .requests()
            .iter()
            .filter(|message| message.kind.0 & 0xf000 == 0x2000)
            .map(|message| (message.kind, message.descriptor().map(|d| d.menu)))
            .collect();
        assert!(targets.contains(&(MessageKind::GET_WAVEFORM_DETAIL, Some(MenuTarget::MAIN))));
        assert!(targets.contains(&(MessageKind::GET_BEAT_GRID, Some(MenuTarget::BINARY))));
        assert!(targets.contains(&(MessageKind::GET_VBR_INDEX, Some(MenuTarget::BINARY))));
    }

    // -- failure modes ----------------------------------------------------

    #[tokio::test]
    async fn a_reply_under_the_wrong_transaction_id_ends_the_connection() {
        // There is no frame boundary to resynchronise on, so carrying on would
        // read every later answer one message out with nothing to show for it.
        let player = player(Library {
            wrong_transaction: true,
            ..Library::default()
        })
        .await;
        let error = DbClient::connect_at(Ipv4Addr::LOCALHOST, player.port, us(), quick())
            .await
            .expect_err("the handshake must not proceed");
        assert!(error.to_string().contains("transaction"), "{error}");
    }

    #[tokio::test]
    async fn a_silent_player_times_out_rather_than_hanging() {
        let player = player(Library {
            deaf: true,
            ..Library::default()
        })
        .await;
        let started = std::time::Instant::now();
        let error = DbClient::connect_at(
            Ipv4Addr::LOCALHOST,
            player.port,
            us(),
            DbConfig {
                connect_timeout: Duration::from_millis(200),
                request_timeout: Duration::from_millis(200),
                ..DbConfig::default()
            },
        )
        .await
        .expect_err("a deaf player must not hang");
        assert!(matches!(error, Error::Timeout { .. }), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn menu_close_cannot_be_awaited() {
        let player = player(Library::default()).await;
        let mut client = connect(&player).await;
        let error = client
            .request(Message::new(1, MessageKind::MENU_CLOSE, []))
            .await
            .expect_err("waiting for a reply to this hangs a real session");
        assert!(error.to_string().contains("draws no reply"), "{error}");
    }

    #[tokio::test]
    async fn saying_goodbye_sends_the_disconnect_a_deck_sends() {
        let player = player(Library::default()).await;
        let client = connect(&player).await;
        client.close().await.expect("goodbye");
        // The player's reader has to notice it before the assertion runs.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(player.of_kind(MessageKind::DISCONNECT).len(), 1);
    }

    // -- the bytes a real deck sends --------------------------------------
    //
    // Everything above proves our client and our loopback player agree with
    // each other, which is not the same as agreeing with a CDJ. These are the
    // exact request datagrams two CDJ-2000NXS put on the wire, lifted out of
    // the capture corpus by reassembling the dbserver stream, and they pin the
    // argument layout of every request this client can build. The descriptors
    // are the decks' own, so the menu-target byte is whatever that deck was
    // rendering into at the time.

    fn deck(slot: Slot, menu: MenuTarget, track_type: TrackType, device: u8) -> Descriptor {
        Descriptor::new(
            BrowsableDeviceNumber::new(device).expect("a browsable number"),
            slot,
            menu,
            track_type,
        )
    }

    /// `S06-load-and-play`: the root menu of deck B's USB, asked by deck A.
    #[test]
    fn our_root_menu_request_is_byte_for_byte_a_real_one() {
        let captured = hex(
            "11872349ae11038003201010000f03140000000c060606000000000000000000\
             110101030111000000001100ffffff",
        );
        let descriptor = deck(Slot::USB, MenuTarget::MAIN, TrackType::REKORDBOX, 1);
        assert_eq!(descriptor.to_raw(), 0x0101_0301);
        let ours = Message::menu_request(
            0x0380_0320,
            MessageKind::MENU_ROOT,
            descriptor,
            &[SortOrder::DEFAULT.0, ROOT_MENU_MASK],
        )
        .expect("three arguments");
        assert_eq!(ours.encode(), captured);
    }

    /// The `RENDER_MENU` that follows it: offset 0, six rows, of twelve.
    #[test]
    fn our_render_is_byte_for_byte_a_real_one() {
        let captured = hex(
            "11872349ae11038003211030000f06140000000c060606060606000000000000\
             1101010301110000000011000000061100000000110000000c1100000000",
        );
        let ours = Message::render_of(
            0x0380_0321,
            deck(Slot::USB, MenuTarget::MAIN, TrackType::REKORDBOX, 1),
            0,
            6,
            12,
        );
        assert_eq!(ours.encode(), captured);
        assert_eq!(
            ours.number(4),
            Some(12),
            "the whole result set, not the page"
        );
    }

    /// A bare 32-byte message: zero arguments and an all-zero tag blob.
    #[test]
    fn our_menu_close_is_byte_for_byte_a_real_one() {
        let captured = hex("11872349ae11038003291000010f00140000000c000000000000000000000000");
        let ours = Message::new(0x0380_0329, MessageKind::MENU_CLOSE, []);
        assert_eq!(ours.encode(), captured);
        assert_eq!(captured.len(), 32);
    }

    #[test]
    fn our_metadata_and_track_info_requests_are_byte_for_byte_real_ones() {
        let metadata = hex(
            "11872349ae11038003651020020f02140000000c060600000000000000000000\
             110102030111000000c8",
        );
        assert_eq!(
            Message::menu_request(
                0x0380_0365,
                MessageKind::GET_METADATA,
                deck(Slot::USB, MenuTarget::SUB, TrackType::REKORDBOX, 1),
                &[0xc8],
            )
            .expect("two arguments")
            .encode(),
            metadata,
            "metadata is fetched through the transient menu target"
        );

        let track_info = hex(
            "11872349ae110380036a1021020f02140000000c060600000000000000000000\
             110108030111000000c8",
        );
        assert_eq!(
            Message::menu_request(
                0x0380_036a,
                MessageKind::GET_TRACK_INFO,
                deck(Slot::USB, MenuTarget::BINARY, TrackType::REKORDBOX, 1),
                &[0xc8],
            )
            .expect("two arguments")
            .encode(),
            track_info
        );
    }

    #[test]
    fn our_artwork_request_is_byte_for_byte_a_real_one() {
        let captured = hex(
            "11872349ae11038003671020030f02140000000c060600000000000000000000\
             110108030111000000ba",
        );
        assert_eq!(
            Message::menu_request(
                0x0380_0367,
                MessageKind::GET_ARTWORK,
                deck(Slot::USB, MenuTarget::BINARY, TrackType::REKORDBOX, 1),
                &[0xba],
            )
            .expect("two arguments")
            .encode(),
            captured
        );
    }

    /// `S06`, and the message that desynchronises a naive parser: five
    /// arguments declared, four on the wire.
    #[test]
    fn our_waveform_preview_request_is_byte_for_byte_a_real_one() {
        let captured = hex(
            "11872349ae110380036c1020040f05140000000c060606060300000000000000\
             1101080301110000000311000000c81100000000",
        );
        let ours = Message::new(
            0x0380_036c,
            MessageKind::GET_WAVEFORM_PREVIEW,
            Arguments::from([
                Field::from(deck(Slot::USB, MenuTarget::BINARY, TrackType::REKORDBOX, 1)),
                Field::U32(WAVEFORM_PREVIEW_ARGUMENT1),
                Field::U32(0xc8),
                Field::U32(0),
                Field::Blob(Vec::new()),
            ]),
        );
        assert_eq!(ours.encode(), captured);
    }

    /// `S20-browse-ground-truth`, the drill grid at three depths. Each is
    /// `0x1000 | depth << 8 | category` with the **menu-request** category
    /// byte (F42).
    #[test]
    fn our_drill_requests_are_byte_for_byte_real_ones() {
        let descriptor = deck(Slot::USB, MenuTarget::SUB, TrackType::REKORDBOX, 2);
        assert_eq!(descriptor.to_raw(), 0x0202_0301);
        let genre = 0x01;

        let depth1 = hex(
            "11872349ae11038005801011010f03140000000c060606000000000000000000\
             110202030111000000001100000004",
        );
        assert_eq!(
            drill_message(0x0380_0580, descriptor, genre, 1, &[4]).encode(),
            depth1
        );

        let depth2 = hex(
            "11872349ae110380059b1012010f04140000000c060606060000000000000000\
             11020203011100000000110000000611ffffffff",
        );
        assert_eq!(
            drill_message(
                0x0380_059b,
                descriptor,
                genre,
                2,
                &[6, dbserver::FILTER_ALL]
            )
            .encode(),
            depth2
        );

        let depth3 = hex(
            "11872349ae11038005a31013010f05140000000c060606060600000000000000\
             110202030111000000001100000006110000002611ffffffff",
        );
        assert_eq!(
            drill_message(
                0x0380_05a3,
                descriptor,
                genre,
                3,
                &[6, 0x26, dbserver::FILTER_ALL]
            )
            .encode(),
            depth3
        );
    }

    fn drill_message(
        transaction: u32,
        descriptor: Descriptor,
        category: u8,
        depth: u8,
        filters: &[u32],
    ) -> Message {
        let mut extra = vec![SortOrder::DEFAULT.0];
        extra.extend_from_slice(filters);
        Message::menu_request(
            transaction,
            Drill { depth, category }.kind(),
            descriptor,
            &extra,
        )
        .expect("a drill fits")
    }

    /// `S20`: `[descriptor, sort, the menu being sorted]`. The deck used an
    /// undocumented menu-target byte of `0x05` here.
    #[test]
    fn our_sort_menu_request_is_byte_for_byte_a_real_one() {
        let captured = hex(
            "11872349ae11038008c71014000f03140000000c060606000000000000000000\
             110205030111000000001100001105",
        );
        let ours = Message::menu_request(
            0x0380_08c7,
            MessageKind::MENU_SORT,
            Descriptor::new(
                BrowsableDeviceNumber::new(2).expect("browsable"),
                Slot::USB,
                MenuTarget(0x05),
                TrackType::REKORDBOX,
            ),
            &[
                SortOrder::DEFAULT.0,
                u32::from(MessageKind::MENU_PLAYLIST.0),
            ],
        )
        .expect("three arguments");
        assert_eq!(ours.encode(), captured);
    }

    /// `S20`: one request per keystroke, and **argument 3 is the text** (F44).
    #[test]
    fn our_search_request_is_byte_for_byte_a_real_one() {
        let captured = hex(
            "11872349ae11038008171013000f05140000000c060606020600000000000000\
             1102010301110000000011000000042600000002004800001100000000",
        );
        let ours = Message::search(
            0x0380_0817,
            deck(Slot::USB, MenuTarget::MAIN, TrackType::REKORDBOX, 2),
            SortOrder::DEFAULT,
            "H",
        );
        assert_eq!(ours.encode(), captured);
        assert_eq!(
            ours.number(2),
            Some(4),
            "one character plus the NUL, doubled"
        );
        assert_eq!(ours.text(3), Some("H"));
    }

    /// `S20`: the flat track list, two arguments.
    #[test]
    fn our_track_list_request_is_byte_for_byte_a_real_one() {
        let captured = hex(
            "11872349ae11038006e51010040f02140000000c060600000000000000000000\
             11020203011100000000",
        );
        let ours = Message::menu_request(
            0x0380_06e5,
            MessageKind::MENU_TRACK,
            deck(Slot::USB, MenuTarget::SUB, TrackType::REKORDBOX, 2),
            &[SortOrder::DEFAULT.0],
        )
        .expect("two arguments");
        assert_eq!(ours.encode(), captured);
    }

    // -- units ------------------------------------------------------------

    #[test]
    fn the_batch_size_is_the_one_documented_safe_on_a_nexus_2() {
        assert_eq!(DbConfig::default().batch, 64);
        assert_eq!(dbserver::MAX_RENDER_BATCH, 64);
    }

    #[test]
    fn a_track_info_with_no_path_cannot_be_built() {
        // Without a path there is nothing to load, so an empty-path struct
        // would only push the check downstream.
        assert!(TrackInfo::from_items(1, Vec::new()).is_none());
        assert!(TrackInfo::from_items(1, vec![MenuItem::named(1, ItemType::TEMPO, "")]).is_none());
        assert!(TrackInfo::from_items(1, track_info_rows()).is_some());
    }

    #[test]
    fn an_item_type_is_masked_before_it_is_matched() {
        // A CDJ-3000 packs extra information into the high half; a comparison
        // that forgets to mask silently stops matching on newer hardware.
        let mut rows = metadata_rows();
        for row in &mut rows {
            row.item_type = ItemType(row.item_type.0 | 0x00ab_0000);
        }
        let track = TrackMetadata::from_items(0xc8, rows);
        assert_eq!(track.title, "Loneliness - Klub Cut");
        assert_eq!(track.tempo_centibpm, 13_201);
    }

    #[test]
    fn a_binary_reply_with_no_payload_reads_as_empty_not_as_missing() {
        let reply = Message::binary_reply(
            1,
            MessageKind::ARTWORK,
            MessageKind::GET_ARTWORK,
            Vec::new(),
            &[],
        )
        .expect("four arguments");
        assert_eq!(reply.number(2), Some(0), "the length argument says zero");
        assert!(blob_of(&reply).is_empty());

        let full = Message::binary_reply(
            1,
            MessageKind::ARTWORK,
            MessageKind::GET_ARTWORK,
            vec![1, 2, 3],
            &[],
        )
        .expect("four arguments");
        assert_eq!(blob_of(&full), vec![1, 2, 3]);
    }
}
