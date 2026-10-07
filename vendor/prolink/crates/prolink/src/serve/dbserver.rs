// SPDX-License-Identifier: GPL-3.0-only

//! The dbserver **server**: what makes a local rekordbox medium browsable from
//! a real CDJ's LINK screen.
//!
//! Every reference project in this space is a *consumer* — grepping all seven
//! for a TCP listener turns up an OBS overlay and an HTTP API, and nothing that
//! answers a dbserver query. So the shapes here were settled by replaying real
//! deck-to-deck sessions out of the capture corpus and matching what the other
//! deck sent, request type by request type.
//!
//! Two listeners, mirroring a real player:
//!
//! - **TCP [`PORT_QUERY_PORT`]**, answering the fixed nineteen-byte query with
//!   the port our dbserver is actually on. The port is nominally dynamic, which
//!   is what that port exists to say; both reference captures answer
//!   [`PORT`], and so do we when we can bind it.
//! - **the dbserver port**, one task per connection, holding that connection's
//!   menu state — because the protocol *is* stateful. A menu request
//!   establishes a result set and the `0x3000` render that follows pages
//!   through it.
//!
//! # Menus are keyed on `(descriptor, item count)`, not the count alone
//!
//! A deck does not browse one menu at a time. It dips into a metadata menu for
//! the highlighted track and then resumes a 692-item list at the next offset
//! **without re-issuing the menu request**, so several result sets have to be
//! held at once. Keying on the count alone worked until a metadata reply became
//! thirteen items and collided with a thirteen-track album, at which point
//! browsing that album served metadata for every page (F27, then F41). The
//! descriptor supplies the missing bit: its menu-target byte separates the list
//! being scrolled (`M=1`) from the transient menu dipped into (`M=2`), and it
//! is present in both the menu request and the render.
//!
//! The table is bounded — see [`MAX_PENDING_MENUS`] — because a connection that
//! never forgets a result set is a connection that grows for as long as a DJ
//! browses.
//!
//! # `0x0001` draws no reply at all, and must not discard state
//!
//! "Done with that menu, release it" is the natural reading of `MENU_CLOSE` and
//! acting on it is a bug: a deck sends it *while still scrolling* the list it is
//! supposedly finished with, so honouring it destroys the result set mid-scroll
//! (F16, F27). Reply with nothing, discard nothing.
//!
//! # The medium is resolved per message, never per connection
//!
//! A player browsing two media on the same peer opens **one** dbserver
//! connection and tells them apart purely by the descriptor's slot byte (F37).
//! Caching the medium on the connection serves the wrong library the moment the
//! DJ switches slots.
//!
//! # Never answer an unknown request with an error
//!
//! Answering `0x3e03` with `0x4003` made a deck fetch our root menu, render
//! every category, and then disconnect without opening one of them (F25). An
//! error and an empty folder are indistinguishable on a CDJ's screen, so the
//! cost of a refusal is unbounded and the cost of an empty acknowledgement is
//! one blank list. Three undocumented requests are answered explicitly —
//! `0x3e03` with the `0x4b02` a real player sends, `0x3100` and `0x3d03` with a
//! bare `SUCCESS` — and **everything else this module does not understand is
//! answered `SUCCESS[type, 0]`**, which is a shape real hardware also uses and
//! is never a refusal.
//!
//! # Starting one
//!
//! ```no_run
//! # async fn example() -> prolink::Result<()> {
//! use std::sync::{Arc, Mutex};
//! use prolink::serve::dbserver::{DbServer, DbServerConfig};
//! use prolink::serve::{Medium, ServedSlot};
//!
//! let usb = Arc::new(Medium::from_volume("/Volumes/DJ".as_ref(), ServedSlot::USB)?);
//! let sd = Arc::new(Medium::from_volume("/Volumes/SD".as_ref(), ServedSlot::SD)?);
//! let server = DbServer::start(DbServerConfig::default(), [usb, sd]).await?;
//! println!("browsable on {}", server.port());
//! # Ok(())
//! # }
//! ```

mod analysis;
mod keys;
mod menu;

use std::collections::{BTreeMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use prolink_proto::dbserver::{
    self, Descriptor, MAX_RENDER_BATCH, MenuItem, Message, MessageKind, PORT, PORT_QUERY_PORT,
    PREAMBLE,
};
use prolink_proto::{BrowsableDeviceNumber, Slot};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::serve::medium::{Medium, ServedSlot};
use crate::serve::player::MediaSet;
use crate::virtual_cdj::LoadedTracks;
use crate::{Error, Result};

/// How many result sets one connection holds before the oldest is dropped.
///
/// Not a protocol limit. A deck interleaves a handful at most — the list it is
/// scrolling, the metadata menu it dips into, and the preview pane — but the
/// key includes the item count, so a DJ scrolling through albums of every size
/// mints a new key per album. Without a bound the table grows for the life of
/// the connection; with one this size, nothing a deck has been observed to do
/// comes close to evicting a set it still wants.
pub const MAX_PENDING_MENUS: usize = 32;

/// How much of a connection is read at once.
const READ_CHUNK: usize = 16 * 1024;

/// How to present the dbserver.
#[derive(Clone, Copy, Debug)]
pub struct DbServerConfig {
    /// The number we announce ourselves as, which is what the reply to
    /// `INTRODUCE` carries.
    ///
    /// A [`BrowsableDeviceNumber`] because a device outside 1–4 is never
    /// offered as a browse source and so is never asked for a dbserver
    /// connection at all (F45); serving from one is not a thing that can go
    /// wrong, it is a thing that never happens.
    pub device: BrowsableDeviceNumber,
    /// The address to listen on.
    pub address: Ipv4Addr,
    /// The port to serve dbserver on, or 0 for an ephemeral one.
    ///
    /// [`PORT`] by default, which is what every deck we have seen answers the
    /// port query with. If it is taken, an ephemeral port is used instead and
    /// the port query announces that — which is the whole reason the port query
    /// exists.
    pub port: u16,
    /// The port to answer the port query on, or `None` not to.
    ///
    /// Fixed at [`PORT_QUERY_PORT`]. Without it a deck never learns where our
    /// dbserver is and never connects, so this is only optional for tests.
    pub query_port: Option<u16>,
}

impl Default for DbServerConfig {
    #[expect(
        clippy::unwrap_used,
        reason = "1 is a browsable device number by inspection, and `Option` has no const unwrap \
                  to prove it with instead"
    )]
    fn default() -> Self {
        Self {
            device: BrowsableDeviceNumber::new(1).unwrap(),
            address: Ipv4Addr::UNSPECIFIED,
            port: PORT,
            query_port: Some(PORT_QUERY_PORT),
        }
    }
}

/// A dbserver serving one medium per slot.
#[derive(Debug)]
pub struct DbServer {
    shared: Arc<Shared>,
    port: u16,
    query_port: Option<u16>,
    tasks: Vec<JoinHandle<()>>,
}

/// The tracks each deck has tagged, per medium.
///
/// Lives here rather than on a [`Session`] because a deck tags on one
/// connection and opens the TAG LIST on another — a per-connection list would
/// be empty every time it was asked for.
///
/// In memory only, and deliberately: a real deck writes its tag list back to
/// the medium, and this server treats the medium as read-only. The list is
/// therefore lost when the server stops, which is the documented behaviour
/// rather than an omission.
#[derive(Debug, Default)]
struct TagLists {
    /// `(requesting device, slot)` → the track ids it tagged, in tag order.
    by_device: Mutex<BTreeMap<(u8, Slot), Vec<u32>>>,
}

impl TagLists {
    /// Add a track, or remove it when `add` is false. Returns nothing: the
    /// deck's reply is a bare acknowledgement either way.
    fn set(&self, device: u8, slot: Slot, track: u32, add: bool) {
        let Ok(mut lists) = self.by_device.lock() else {
            return;
        };
        let list = lists.entry((device, slot)).or_default();
        match (add, list.iter().position(|&id| id == track)) {
            // Tagging a track already tagged leaves the order alone, so a deck
            // that repeats the request does not shuffle its own list.
            (true, None) => list.push(track),
            (false, Some(at)) => {
                list.remove(at);
            }
            _ => {}
        }
    }

    /// Empty one deck's tag list on one medium.
    fn clear(&self, device: u8, slot: Slot) {
        if let Ok(mut lists) = self.by_device.lock() {
            lists.remove(&(device, slot));
        }
    }

    /// One deck's tagged track ids on one medium.
    fn get(&self, device: u8, slot: Slot) -> Vec<u32> {
        self.by_device
            .lock()
            .ok()
            .and_then(|lists| lists.get(&(device, slot)).cloned())
            .unwrap_or_default()
    }
}

/// Everything a connection task needs, and nothing it may mutate.
#[derive(Debug)]
struct Shared {
    device: BrowsableDeviceNumber,
    /// What each deck has tagged. The one piece of mutable server state, and
    /// the reason it is behind a lock rather than owned by a connection.
    tags: TagLists,
    /// What each deck has loaded from us, so its row can be marked (F55).
    /// Shared with the [`crate::virtual_cdj::VirtualCdj`] that fills it; a
    /// dbserver started without one simply marks nothing.
    loaded: Arc<LoadedTracks>,
    /// What we are serving. The registry itself, not a copy of it: media are
    /// mounted and unmounted while the server runs, and a snapshot taken at
    /// start would answer for a stick that has since been pulled.
    ///
    /// Resolved per *message* and never cached on a connection (F37).
    media: Arc<MediaSet>,
    /// When the server started, for the prefix word the beat grid and detail
    /// waveform carry — which must be non-zero and must not go backwards (F33).
    started: Instant,
}

impl Shared {
    /// The medium a request is about, from its descriptor's slot byte.
    ///
    /// Falls back to the only medium we have when the slot names something
    /// else, which keeps a single-slot server answering a request that names
    /// any slot — a deck asks about the slot it thinks it is browsing, and a
    /// unit with one medium has nothing else to offer. With two media there is
    /// no such licence: the slot byte is the only thing distinguishing them.
    fn medium(&self, slot: Slot) -> Option<Arc<Medium>> {
        if let Some(medium) = self.media.get(slot) {
            return Some(medium);
        }
        let mut media = self.media.all().into_iter();
        match (media.next(), media.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        }
    }
}

impl DbServer {
    /// Bind both listeners and start serving.
    ///
    /// Two media on one server rather than two servers: a player browsing both
    /// opens a single connection and names the slot in every request (F37).
    /// A second medium in a slot already given one replaces it.
    pub async fn start(
        config: DbServerConfig,
        media: impl IntoIterator<Item = Arc<Medium>>,
    ) -> Result<Self> {
        Self::start_watching(
            config,
            Arc::new(MediaSet::new(media)),
            Arc::new(LoadedTracks::default()),
        )
        .await
    }

    /// Start one that marks the loaded track's row.
    ///
    /// `loaded` comes from the [`crate::virtual_cdj::VirtualCdj`] announcing
    /// for this server. Without it no row is marked, and a browsing deck has
    /// no reference key — the key-matching indicator stays dark on every row
    /// (F55).
    pub async fn start_watching(
        config: DbServerConfig,
        media: Arc<MediaSet>,
        loaded: Arc<LoadedTracks>,
    ) -> Result<Self> {
        let listener = bind(config.address, config.port).await?;
        let port = local_port(&listener)?;
        let shared = Arc::new(Shared {
            device: config.device,
            tags: TagLists::default(),
            loaded: Arc::clone(&loaded),
            media,
            started: Instant::now(),
        });

        let mut tasks = vec![spawn_dbserver(listener, Arc::clone(&shared))];
        let query_port = match config.query_port {
            None => None,
            Some(wanted) => match bind_exactly(
                config.address,
                wanted,
                "binding the dbserver port-query port",
            )
            .await
            {
                Ok(listener) => {
                    let bound = local_port(&listener)?;
                    tasks.push(spawn_port_query(listener, port));
                    Some(bound)
                }
                Err(error) => {
                    // Fatal in practice, and worth saying so: a deck that
                    // cannot ask where our dbserver is will never connect to
                    // it. Not an error, because everything else still works and
                    // another tool holding 12523 is a situation a DJ can fix.
                    warn!(
                        %error,
                        port = wanted,
                        "could not answer the dbserver port query; players will not find us"
                    );
                    None
                }
            },
        };

        info!(
            port,
            query_port,
            device = %config.device,
            slots = shared.media.len(),
            "dbserver listening"
        );
        Ok(Self {
            shared,
            port,
            query_port,
            tasks,
        })
    }

    /// The port dbserver connections are accepted on.
    ///
    /// Not necessarily [`PORT`]: if that was taken we bound an ephemeral port,
    /// and this is what the port query announces.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The port the port query is answered on, or `None` if it could not be
    /// bound.
    pub fn query_port(&self) -> Option<u16> {
        self.query_port
    }

    /// The slots this server has a medium for, as of now.
    pub fn slots(&self) -> Vec<ServedSlot> {
        self.shared
            .media
            .all()
            .iter()
            .map(|medium| medium.slot())
            .collect()
    }
}

impl Drop for DbServer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Bind the dbserver listener, falling back to an ephemeral port.
///
/// The dbserver port is nominally dynamic — that is what the port query is for
/// — so failing to take 1051 because rekordbox or a second copy of this library
/// already holds it is not a reason to refuse to serve.
async fn bind(address: Ipv4Addr, port: u16) -> Result<TcpListener> {
    match TcpListener::bind(SocketAddrV4::new(address, port)).await {
        Ok(listener) => Ok(listener),
        Err(_) if port != 0 => {
            debug!(port, "port taken; falling back to an ephemeral one");
            bind_exactly(address, 0, "binding an ephemeral dbserver port").await
        }
        Err(error) => Err(Error::io("binding the dbserver port")(error)),
    }
}

/// Bind a listener on exactly this port, with no fallback.
///
/// The port query has no fallback because a query listener on a port nobody
/// asks about answers nothing.
async fn bind_exactly(address: Ipv4Addr, port: u16, what: &'static str) -> Result<TcpListener> {
    TcpListener::bind(SocketAddrV4::new(address, port))
        .await
        .map_err(Error::io(what))
}

fn local_port(listener: &TcpListener) -> Result<u16> {
    match listener
        .local_addr()
        .map_err(Error::io("reading a listener's address"))?
    {
        SocketAddr::V4(address) => Ok(address.port()),
        SocketAddr::V6(address) => Ok(address.port()),
    }
}

/// Answer the fixed nineteen-byte port query with the port we are serving on.
///
/// The reply does not depend on the query's contents, and the query is short
/// enough to arrive in one segment; reading it at all is politeness towards a
/// peer that expects its bytes to be consumed.
fn spawn_port_query(listener: TcpListener, port: u16) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, peer)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut request = [0u8; dbserver::PORT_QUERY.len()];
                let _ = stream.read(&mut request).await;
                if stream
                    .write_all(&dbserver::encode_port_reply(port))
                    .await
                    .is_ok()
                {
                    debug!(%peer, port, "told a peer where our dbserver is");
                }
                let _ = stream.shutdown().await;
            });
        }
    })
}

/// Accept dbserver connections, one task each.
fn spawn_dbserver(listener: TcpListener, shared: Arc<Shared>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                return;
            };
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                info!(%peer, "dbserver client connected");
                if let Err(error) = serve(stream, &shared).await {
                    debug!(%peer, %error, "dbserver connection ended");
                }
                info!(%peer, "dbserver client disconnected");
            });
        }
    })
}

/// One client conversation, from the preamble to the disconnect.
async fn serve(mut stream: TcpStream, shared: &Shared) -> Result<()> {
    // Small messages, and a render is sixty-six of them; the delay Nagle adds
    // is the difference between a menu that appears and one that crawls.
    let _ = stream.set_nodelay(true);

    let mut buffer = Vec::with_capacity(READ_CHUNK);
    // The five-byte preamble, exchanged in both directions before any message.
    // A decoder that goes straight for the first magic fails on byte zero.
    while buffer.len() < PREAMBLE.len() {
        if read_more(&mut stream, &mut buffer).await? == 0 {
            return Ok(());
        }
    }
    if dbserver::skip_preamble(&buffer).len() == buffer.len() {
        return Err(Error::Protocol(prolink_proto::Error::BadMagic {
            expected: PREAMBLE.as_slice().into(),
            got: buffer.get(..PREAMBLE.len()).unwrap_or_default().into(),
        }));
    }
    buffer.drain(..PREAMBLE.len());
    stream
        .write_all(&PREAMBLE)
        .await
        .map_err(Error::io("answering the dbserver preamble"))?;

    let mut session = Session::default();
    let mut out = Vec::new();
    loop {
        let mut consumed = 0;
        let mut closing = false;
        loop {
            let rest = buffer.get(consumed..).unwrap_or_default();
            match Message::decode(rest) {
                Ok((message, used)) => {
                    consumed += used;
                    debug!(?message, "dbserver request");
                    if session.handle(shared, &message, &mut out) == Flow::Close {
                        closing = true;
                        break;
                    }
                }
                // Running off the end is the expected outcome of trying too
                // early: a dbserver message carries no length prefix, so the
                // only way to know a whole one has arrived is to try.
                Err(error) if error.is_truncated() => break,
                // Anything else means this peer is not speaking the protocol,
                // and since there is no frame boundary to resynchronise on, the
                // only remedy is to drop the connection. The deck reconnects.
                Err(error) => {
                    buffer.drain(..consumed);
                    flush(&mut stream, &mut out).await?;
                    return Err(error.into());
                }
            }
        }
        buffer.drain(..consumed);
        flush(&mut stream, &mut out).await?;
        if closing || read_more(&mut stream, &mut buffer).await? == 0 {
            return Ok(());
        }
    }
}

async fn flush(stream: &mut TcpStream, out: &mut Vec<u8>) -> Result<()> {
    if out.is_empty() {
        return Ok(());
    }
    let result = stream
        .write_all(out)
        .await
        .map_err(Error::io("writing a dbserver reply"));
    out.clear();
    result
}

/// Append whatever arrived, returning how many bytes that was.
async fn read_more(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Result<usize> {
    let start = buffer.len();
    buffer.resize(start + READ_CHUNK, 0);
    let read = match stream
        .read(buffer.get_mut(start..).unwrap_or_default())
        .await
    {
        Ok(read) => read,
        Err(error) => {
            buffer.truncate(start);
            return Err(Error::io("reading from a dbserver client")(error));
        }
    };
    buffer.truncate(start + read);
    Ok(read)
}

/// Whether the connection continues after a message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flow {
    Continue,
    Close,
}

/// One connection's menu state.
#[derive(Debug, Default)]
struct Session {
    /// Result sets awaiting render, keyed on `(descriptor, item count)` — see
    /// the module documentation for why the count alone is not enough (F41).
    menus: BTreeMap<(u32, u32), Arc<Vec<MenuItem>>>,
    /// Insertion order, so the bound on [`Session::menus`] can evict the oldest.
    order: VecDeque<(u32, u32)>,
    /// The most recent result set per descriptor, and the most recent of all,
    /// for a client that pages a set we never established. An empty page would
    /// look like a menu that vanished, which is worse than the wrong menu.
    recent: BTreeMap<u32, Arc<Vec<MenuItem>>>,
    last: Arc<Vec<MenuItem>>,
}

impl Session {
    fn handle(&mut self, shared: &Shared, message: &Message, out: &mut Vec<u8>) -> Flow {
        let transaction = message.transaction_id;
        let kind = message.kind;
        // Argument 0 is the descriptor on nearly every request, and its slot
        // byte decides which library answers — per message, never per
        // connection (F37).
        let descriptor = message.descriptor();
        let slot = descriptor.map_or(Slot::NONE, |descriptor| descriptor.slot);
        // Held as an owned handle rather than a borrow: the registry it comes
        // from can change under us between messages, and a request must be
        // answered from the medium it started against.
        let medium = shared.medium(slot);
        let medium = medium.as_deref();

        match kind {
            MessageKind::INTRODUCE => {
                // Argument 1 of this one `SUCCESS` is our own player number
                // rather than an item count (F7).
                let mut reply = Message::introduce_reply(shared.device.number());
                reply.transaction_id = transaction;
                push(out, &reply);
                return Flow::Continue;
            }
            MessageKind::DISCONNECT => return Flow::Close,
            // No reply at all, and **no state discarded**: a deck sends this
            // while still scrolling the list it is supposedly finished with
            // (F16, F27).
            // `0x3001` is the more expensive of the two to get wrong. A deck
            // sends it about a minute after a load and a real server answers it
            // with nothing; a reply nobody asked for becomes the answer to the
            // `GET_METADATA` that follows, and every reply after that is one
            // behind. See `MessageKind::UNKNOWN_3001`.
            MessageKind::MENU_CLOSE | MessageKind::UNKNOWN_3001 => return Flow::Continue,
            MessageKind::UNKNOWN_3E03 => {
                push(
                    out,
                    &Message::unknown_3e03_reply(transaction, shared.device.number()),
                );
                return Flow::Continue;
            }
            MessageKind::RENDER_MENU => {
                self.render(shared, message, out);
                return Flow::Continue;
            }
            // Tagging is the one request that changes server state. The reply
            // is a bare acknowledgement carrying zero, exactly as a real deck
            // answers it (F53).
            // "REMOVE ALL TRACKS". Its twin `0x3402` looks identical on the
            // wire and must *not* clear anything (F54).
            MessageKind::TAG_LIST_CLEAR => {
                if let Some(descriptor) = descriptor {
                    shared.tags.clear(descriptor.device.get(), descriptor.slot);
                }
                push(out, &Message::success(transaction, kind, 0));
                return Flow::Continue;
            }
            MessageKind::TAG_LIST_ADD => {
                if let (Some(descriptor), Some(track)) = (descriptor, message.number(1)) {
                    // Argument 2 is `1` in every observed request. Anything
                    // else is read as "untag", which is a guess — see
                    // `MessageKind::TAG_LIST_ADD`.
                    let add = message.number(2) != Some(0);
                    shared
                        .tags
                        .set(descriptor.device.get(), descriptor.slot, track, add);
                }
                push(out, &Message::success(transaction, kind, 0));
                return Flow::Continue;
            }
            _ => {}
        }

        if analysis::is_binary_request(kind)
            && let Some(reply) = analysis::reply(message, medium, shared.started.elapsed())
        {
            push(out, &reply);
            return Flow::Continue;
        }

        // The requesting deck's tag list, so a tagged row can carry its
        // marker in whatever menu it turns up in.
        let tags = descriptor.map_or_else(Vec::new, |descriptor| {
            shared.tags.get(descriptor.device.get(), descriptor.slot)
        });
        if let Some(items) = menu::build(kind, &message.args, medium, &tags) {
            let count = u32::try_from(items.len()).unwrap_or(u32::MAX);
            let raw = descriptor.map_or(0, Descriptor::to_raw);
            self.remember(raw, count, items);
            push(out, &Message::success(transaction, kind, count));
        } else {
            // Not a menu, and not a request this module understands. A bare
            // acknowledgement, never an error: `0x3100` is answered exactly
            // this way by a real deck, `0x3d03` likewise by inference, and F25
            // is what erroring on the third one cost.
            debug!(?kind, "acknowledged a request we do not implement");
            push(out, &Message::success(transaction, kind, 0));
        }
        Flow::Continue
    }

    /// Remember a result set, evicting the **least recently used** once the
    /// table is full.
    ///
    /// Least recently *used*, not least recently inserted, and the difference
    /// is the whole point. A deck polls a loaded track's metadata every couple
    /// of seconds while it plays, and every poll mints a fresh set. Evicting by
    /// insertion order therefore throws away the long-lived list the DJ is
    /// scrolling — the one set that is certainly still wanted — after a fixed
    /// number of polls, which on hardware showed up as every menu going blank
    /// about a minute into any track, and staying blank until the DJ left LINK
    /// and came back. Measured on that capture: one connection minted 44 sets
    /// against a bound of 32.
    ///
    /// Touching on render (see [`Self::render`]) makes the set being paged the
    /// *newest* rather than the oldest, so the eviction candidates are the
    /// transient metadata menus, which is what a bound is for.
    fn remember(&mut self, descriptor: u32, count: u32, items: Vec<MenuItem>) {
        let key = (descriptor, count);
        let items = Arc::new(items);
        self.last = Arc::clone(&items);
        self.recent.insert(descriptor, Arc::clone(&items));
        self.menus.insert(key, items);
        self.touch(key);
        self.evict();
    }

    /// Move a key to the back of the eviction order.
    fn touch(&mut self, key: (u32, u32)) {
        if let Some(position) = self.order.iter().position(|existing| *existing == key) {
            self.order.remove(position);
        }
        self.order.push_back(key);
    }

    /// Drop the least recently used sets until the table is inside its bound.
    fn evict(&mut self) {
        while self.order.len() > MAX_PENDING_MENUS {
            if let Some(oldest) = self.order.pop_front() {
                self.menus.remove(&oldest);
                // `recent` maps a descriptor to its newest set. Only drop the
                // entry if it is the set being evicted; a later set for the
                // same descriptor must survive.
                if self
                    .recent
                    .get(&oldest.0)
                    .is_some_and(|items| u32::try_from(items.len()).unwrap_or(u32::MAX) == oldest.1)
                    && !self.order.iter().any(|key| key.0 == oldest.0)
                {
                    self.recent.remove(&oldest.0);
                }
            }
        }
    }

    /// `MENU_HEADER`, a window of items, `MENU_FOOTER`.
    ///
    /// The client names the set it is paging by echoing the descriptor and the
    /// size it was answered with. When that names nothing — which cannot happen
    /// to a client that took the count from us, but does happen to one whose
    /// idea of the library is stale — we fall back to the most recent set for
    /// that *descriptor*, and only then to the most recent of all. An empty
    /// page reads as a menu that vanished, which is worse than the wrong menu.
    ///
    /// The window is capped at [`MAX_RENDER_BATCH`]: sixty-four rows is
    /// documented safe on a Nexus 2 and thousands demonstrably fail. A deck
    /// asks for six at a time anyway, so the cap only ever bites a client of
    /// our own.
    fn render(&mut self, shared: &Shared, message: &Message, out: &mut Vec<u8>) {
        let transaction = message.transaction_id;
        let parsed = message.descriptor();
        let descriptor = parsed.map_or(0, Descriptor::to_raw);
        let offset = usize::try_from(message.number(1).unwrap_or(0)).unwrap_or(usize::MAX);
        let limit = message
            .number(2)
            .unwrap_or(MAX_RENDER_BATCH)
            .min(MAX_RENDER_BATCH);
        let limit = usize::try_from(limit).unwrap_or(0);
        let total = message.number(4).unwrap_or(0);

        let key = (descriptor, total);
        let items = self
            .menus
            .get(&key)
            .or_else(|| self.recent.get(&descriptor))
            .unwrap_or(&self.last)
            .clone();
        // Paging a set is using it, so it must not become the eviction
        // candidate. Without this, the list a DJ scrolls for a minute is
        // exactly the set the bound throws away.
        if self.menus.contains_key(&key) {
            self.touch(key);
        }

        // The two live marks, applied now rather than when the set was built:
        // a deck loads and tags while still paging through a list it will not
        // re-request (F27, F55).
        let (tags, playing) = parsed.map_or_else(
            || (Vec::new(), None),
            |descriptor| {
                let device = descriptor.device.get();
                (
                    shared.tags.get(device, descriptor.slot),
                    shared.loaded.track_on(device, descriptor.slot),
                )
            },
        );

        push(out, &Message::menu_header(transaction));
        for item in items.iter().skip(offset).take(limit) {
            // Items are built without a transaction id; stamp them with the
            // render's so the client can correlate the whole page.
            push(
                out,
                &menu::mark(item, &tags, playing).to_message(transaction),
            );
        }
        push(out, &Message::menu_footer(transaction));
    }
}

fn push(out: &mut Vec<u8>, message: &Message) {
    out.extend_from_slice(&message.encode());
}

#[cfg(test)]
mod tests;
