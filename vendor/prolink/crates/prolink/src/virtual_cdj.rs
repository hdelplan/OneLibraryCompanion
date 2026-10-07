// SPDX-License-Identifier: GPL-3.0-only

//! Announcing ourselves as a player: the claim chain, the keep-alive and the
//! status stream.
//!
//! This is the **only** part of the library that transmits on a Pro DJ Link
//! port, which is what makes everything else passive by construction.
//!
//! # Two modes, and the difference is not cosmetic
//!
//! [`Numbering::Observer`] emits keep-alives at a number outside the player
//! range and never contends for anything. It is enough to be *seen*, and enough
//! for peers to start unicasting status to us — which is what makes their slot
//! contents and the tempo master visible at all. It is **not** enough to be
//! browsed, and it cannot issue dbserver queries.
//!
//! [`Numbering::Claim`] runs the full handshake and takes a real player slot in
//! 1–4. Necessary to be browsable, and the only mode that can disturb a live
//! rig, so it is opt-in, it refuses a number it has seen anyone else use, and it
//! backs off on conflict.
//!
//! The range is a requirement rather than a preference. At device 5 a deck
//! accepts the announcement completely — it puts us in its device table and
//! unicasts hundreds of status packets to us — and then never sends a single
//! media query, so it never offers us as a source. The check precedes the whole
//! browse path and the failure is silent (F45). [`BrowsableDeviceNumber`] exists
//! so that this cannot be configured by accident.
//!
//! # The handshake
//!
//! ```text
//! 3× hello → 3× claim_mac → 3× claim_ip → N× claim_number → keep_alive forever
//! ```
//!
//! All broadcast, ~300 ms apart. **N is 3 when the device boots into an empty
//! network and 1 when it joins one that already has peers** (C13) — it is not
//! governed by the auto/manual assignment setting, which three controlled boots
//! with the setting held constant settle.
//!
//! Before any of that, we watch for [`PRESCAN`]. Silence is not evidence a
//! number is free: an XDJ-XZ and an Opus Quad do not defend their numbers with
//! conflict packets at all, so only having watched the network is.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prolink_proto::djl::{self, Body};
use prolink_proto::status::{self, CdjStatus, MediaState};
use prolink_proto::{
    BEAT_PORT, BrowsableDeviceNumber, DISCOVERY_PORT, DeviceKind, DeviceName, DeviceNumber,
    STATUS_PORT, Slot,
};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::discovery::Discovery;
use crate::media::{MediaDescription, MediaSource, NoMedia};
use crate::playback::{BeatPosition, Playback, PlaybackCell};
use crate::socket::{self, MAX_DATAGRAM};
use crate::{Error, Result};

/// How long to watch the network before claiming a number.
pub const PRESCAN: Duration = Duration::from_millis(2500);

/// How often [`VirtualCdj::query_media`] looks for its answer.
///
/// A deck answers a media query in **0.7 to 1.2 ms** — three deck-to-deck
/// exchanges across `S4b-media-insert`, `S15b-sd-and-usb` and
/// `S16a-settings-over-link`, where this server takes 5 to 6 ms (S18). So the
/// wait is dominated by whether the datagram arrives at all, not by the poll.
const MEDIA_POLL: Duration = Duration::from_millis(10);

/// The number an observer takes: outside the 1–6 player range, so it can never
/// collide with hardware — and, being above 4, one a peer will never browse.
pub const OBSERVER_NUMBER: DeviceNumber = match DeviceNumber::new(7) {
    Some(number) => number,
    // Unreachable, and settled at compile time: 7 is not zero.
    None => DeviceNumber::ONE,
};

/// How often the beat emitter looks to see whether a beat is due.
///
/// The worst-case lateness of a beat packet, so it is a latency budget rather
/// than a rate: at 145 BPM a beat is 414 ms apart and 5 ms is 1.2% of one, well
/// inside what a deck's own jitter contributes.
const BEAT_TICK: Duration = Duration::from_millis(5);

/// Keep-alive byte `0x25` when this device was first onto the network.
const FIRST_ON_NETWORK: u8 = 0x02;
/// ...and when peers were already present. Latched at start and never
/// re-evaluated: a deck held `0x02` while its peer count went 1→2 (F9).
const JOINED_PEERS: u8 = 0x01;

/// How a virtual CDJ gets its device number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Numbering {
    /// Run the claim chain and hold a browsable number.
    ///
    /// Required to be browsed by a peer, and to issue dbserver queries.
    Claim {
        /// The number to try first. Any other browsable number is tried if it
        /// turns out to be taken; if all four are, starting fails with
        /// [`crate::Error::NoBrowsableNumber`] rather than silently degrading,
        /// because the degraded state cannot serve and the caller has to know.
        preferred: Option<BrowsableDeviceNumber>,
    },
    /// Announce at a fixed number without contending for it.
    ///
    /// Safe next to a live rig. Cannot be browsed, and cannot browse over
    /// dbserver.
    Observer(DeviceNumber),
}

impl Default for Numbering {
    fn default() -> Self {
        Self::Observer(OBSERVER_NUMBER)
    }
}

/// How to present ourselves.
#[derive(Clone, Debug)]
pub struct VirtualCdjConfig {
    /// The name to announce. `CDJ-2000nexus` is the exact casing real hardware
    /// uses (F1).
    pub name: DeviceName,
    /// What kind of device to claim to be.
    pub kind: DeviceKind,
    /// How to get a device number.
    pub numbering: Numbering,
    /// Keep-alive byte `0x35`, a product-generation code.
    ///
    /// `0x00` is what real nexus gear sends — 148 packets from a CDJ-2000nexus
    /// and 91 from a DJM-2000nexus, all `0x00`, where the literature says
    /// `0x01` (C3). **`0x64` is required to coexist with CDJ-3000s**: the wrong
    /// value can make a CDJ-3000 set to player 5 or 6 repeatedly kick itself off
    /// the network.
    pub generation: u8,
    /// The firmware string to report in status packets.
    pub firmware: String,
    /// Whether to emit status packets on UDP 50002.
    ///
    /// Required for a peer to learn what is in our slots, and therefore for
    /// being browsable at all. Off means we are visible but have nothing to
    /// offer.
    pub emit_status: bool,
}

impl Default for VirtualCdjConfig {
    fn default() -> Self {
        Self {
            name: DeviceName::default(),
            kind: DeviceKind::CDJ,
            numbering: Numbering::default(),
            generation: 0x00,
            firmware: "1.44".to_owned(),
            emit_status: true,
        }
    }
}

/// Where the claim chain has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Watching the network before claiming.
    Watching,
    /// Broadcasting the claim chain.
    Claiming,
    /// Holding a number and sending keep-alives.
    Announcing,
    /// Every candidate number was defended.
    Failed,
}

/// What each peer has loaded **from our media**, as its status packets report it.
///
/// A browsing deck needs a reference to compare rows against — the key of the
/// track it has loaded — and it does not compute that locally. It expects the
/// *server* to mark the loaded track's row, and it takes the key from whatever
/// row carries the mark. A listing with no marked row gives it nothing to
/// compare against and no row lights (F55).
///
/// So a server has to know what its clients have loaded. It does: a deck
/// unicasts status to us every ~200 ms naming the track, the player it came
/// from and the slot, and a track whose source player is us came off our
/// medium. That is the whole mechanism, and it is why this lives beside the
/// socket that already receives those packets rather than in the dbserver.
#[derive(Debug, Default)]
pub struct LoadedTracks {
    /// `(device, slot)` → the track id that device loaded from that slot.
    by_device: Mutex<BTreeMap<(u8, Slot), u32>>,
}

impl LoadedTracks {
    /// Record that `device` has `track` loaded from our `slot`.
    ///
    /// A track id of zero means nothing is loaded, and forgets the entry
    /// rather than marking row zero.
    pub fn note(&self, device: u8, slot: Slot, track: u32) {
        let Ok(mut loaded) = self.by_device.lock() else {
            return;
        };
        if track == 0 {
            loaded.remove(&(device, slot));
        } else {
            loaded.insert((device, slot), track);
        }
    }

    /// Forget everything a device had loaded, when it leaves the network.
    pub fn forget(&self, device: u8) {
        if let Ok(mut loaded) = self.by_device.lock() {
            loaded.retain(|(had, _), _| *had != device);
        }
    }

    /// Everyone reading from us, as `(device, slot, track)`.
    ///
    /// A serving host shows this: which players have taken a track off our
    /// media and which one each is holding. It is the same registry that marks
    /// the loaded row in a browse listing (F55), read the other way round.
    pub fn consumers(&self) -> Vec<(u8, Slot, u32)> {
        self.by_device.lock().map_or_else(
            |_| Vec::new(),
            |loaded| {
                loaded
                    .iter()
                    .map(|((device, slot), track)| (*device, *slot, *track))
                    .collect()
            },
        )
    }

    /// What `device` has loaded from `slot`, if anything.
    pub fn track_on(&self, device: u8, slot: Slot) -> Option<u32> {
        self.by_device.lock().ok()?.get(&(device, slot)).copied()
    }
}

/// What a peer says is in one of its slots.
///
/// The two halves arrive by different routes and neither substitutes for the
/// other. **Occupancy** is published in status packets and nowhere else (F20),
/// so [`Self::state`] is current the moment a peer unicasts to us and needs
/// nothing asked. **The name and the counts** only come from a media response,
/// which has to be asked for — and a deck answers for an empty slot too, with
/// everything zeroed (F51), so a description is not evidence of a medium. That
/// is what [`Self::has_media`] is for.
#[derive(Clone, Debug)]
pub struct PeerSlot {
    /// Whose slot it is.
    pub device: DeviceNumber,
    /// Which slot.
    pub slot: Slot,
    /// What the owner publishes at `0x6f`/`0x73`, including the states an
    /// eject passes through.
    pub state: MediaState,
    /// The label and the counts, once a media query has been answered.
    pub description: Option<MediaDescription>,
}

impl PeerSlot {
    /// Whether a medium is present and mounted.
    ///
    /// From the status byte, not from the description: an unlabelled stick with
    /// a full library reports no name, and an empty slot still gets a reply.
    pub fn has_media(&self) -> bool {
        self.state.has_media()
    }

    /// The volume label, or empty when unknown or unlabelled.
    pub fn volume_name(&self) -> &str {
        self.description
            .as_ref()
            .map_or("", |description| description.volume_name.as_str())
    }

    /// How many tracks the medium holds, once it has been described.
    pub fn track_count(&self) -> Option<u32> {
        Some(self.description.as_ref()?.track_count)
    }

    /// How many playlists the medium holds, once it has been described.
    pub fn playlist_count(&self) -> Option<u32> {
        Some(self.description.as_ref()?.playlist_count)
    }
}

/// What our peers have in their slots, kept current by the status socket.
///
/// Filled from two kinds of datagram as they arrive: every peer status packet
/// updates the slot states, and every media response fills in a description.
/// Reading it never transmits — [`VirtualCdj::survey_media`] is what asks.
#[derive(Debug, Default)]
pub struct PeerMedia {
    /// `(device, slot)` → what that peer says about it.
    slots: Mutex<BTreeMap<(u8, Slot), PeerSlot>>,
}

impl PeerMedia {
    /// Every slot we have heard anything about, by device and then slot.
    pub fn all(&self) -> Vec<PeerSlot> {
        self.with(|slots| slots.values().cloned().collect())
    }

    /// Every slot of one device.
    pub fn of(&self, device: DeviceNumber) -> Vec<PeerSlot> {
        self.with(|slots| {
            slots
                .values()
                .filter(|entry| entry.device == device)
                .cloned()
                .collect()
        })
    }

    /// One slot of one device.
    pub fn get(&self, device: DeviceNumber, slot: Slot) -> Option<PeerSlot> {
        self.with(|slots| slots.get(&(device.get(), slot)).cloned())
    }

    /// Only the slots that currently hold a medium.
    pub fn occupied(&self) -> Vec<PeerSlot> {
        self.with(|slots| {
            slots
                .values()
                .filter(|entry| entry.has_media())
                .cloned()
                .collect()
        })
    }

    /// Record what a status packet says about a slot.
    fn note_state(&self, device: DeviceNumber, slot: Slot, state: MediaState) {
        self.entry(device, slot, |entry| entry.state = state);
    }

    /// Record what a media response says about a slot.
    fn note_description(&self, device: DeviceNumber, slot: Slot, description: &MediaDescription) {
        self.entry(device, slot, |entry| {
            entry.description = Some(description.clone());
        });
    }

    fn entry(&self, device: DeviceNumber, slot: Slot, update: impl FnOnce(&mut PeerSlot)) {
        let Ok(mut slots) = self.slots.lock() else {
            return;
        };
        let entry = slots.entry((device.get(), slot)).or_insert(PeerSlot {
            device,
            slot,
            // Until a status packet says otherwise. A slot we have only ever
            // queried is not a slot we have been told holds anything.
            state: MediaState::EMPTY,
            description: None,
        });
        update(entry);
    }

    fn with<T>(&self, read: impl FnOnce(&BTreeMap<(u8, Slot), PeerSlot>) -> T) -> T {
        match self.slots.lock() {
            Ok(slots) => read(&slots),
            Err(poisoned) => read(&poisoned.into_inner()),
        }
    }
}

/// A device on the network: keep-alives, and optionally status.
#[derive(Debug)]
pub struct VirtualCdj {
    config: VirtualCdjConfig,
    interface: crate::Interface,
    /// The number currently held. Changes during the claim chain, so it is
    /// shared rather than copied.
    number: Arc<AtomicU8>,
    phase: watch::Sender<Phase>,
    media: Arc<dyn MediaSource>,
    /// What our peers have loaded from us, kept current by the status socket.
    loaded: Arc<LoadedTracks>,
    /// What our peers have in their own slots, kept current by the same socket.
    peers: Arc<PeerMedia>,
    /// UDP 50002, when we hold it. A media query has to leave from the port its
    /// answer will come back to, and only one socket in a `SO_REUSEPORT` group
    /// receives a given unicast datagram — so this is `None` when
    /// [`VirtualCdjConfig::emit_status`] is off and something else, typically a
    /// [`crate::Monitor`], has the port.
    status_socket: Option<Arc<UdpSocket>>,
    /// Every datagram the status socket receives, for anything else that needs
    /// to read UDP 50002.
    ///
    /// A second socket is not an option. Only one member of a `SO_REUSEPORT`
    /// group receives a given *unicast* datagram, and status is only ever
    /// unicast (F21) — so two readers would each get an arbitrary half of the
    /// stream and both would be wrong. `None` when we do not hold the port.
    status_tap: Option<broadcast::Sender<Arc<Vec<u8>>>>,
    status_counter: Arc<AtomicU32>,
    /// What this host is playing, for the status stream and the beat emitter.
    playback: Arc<PlaybackCell>,
    /// What this host has loaded, for the status stream.
    ///
    /// Separate from `playback` because it changes once per load rather than
    /// thirty times a second, and because it must **not** go stale: a tempo
    /// with no track beside it is a state no deck has ever published.
    playing_track: Arc<Mutex<Option<status::LoadedTrack>>>,
    /// Whether we claim tempo master.
    ///
    /// A plain flag rather than a state machine: taking mastership *from*
    /// another device needs the `0x26`/`0x27` handshake on the beat port, and
    /// the beat port is held by a [`crate::Monitor`] in every configuration
    /// this library supports — one member of a `SO_REUSEPORT` group receives a
    /// given unicast datagram, so we could not hear the reply. So the policy
    /// this supports is the safe half: hold master while nobody else does, and
    /// step aside the moment somebody claims it.
    tempo_master: Arc<AtomicBool>,
    /// The device we are handing mastership to, as byte `0x9f` of our status,
    /// or zero for no handover. Held beside `tempo_master` because the two are
    /// published together and only mean anything together: naming a successor
    /// while not claiming mastership describes a deck that has never existed.
    yielding_to: Arc<AtomicU8>,
    /// Whether SYNC is engaged here, for flag bit 4.
    ///
    /// Published so the rest of the network can see that this deck is
    /// following rather than flying its own tempo -- which is what a CDJ shows
    /// on its SYNC button and what decides whether it is part of the tempo
    /// group at all.
    synced: Arc<AtomicBool>,
    tasks: Vec<JoinHandle<()>>,
}

/// How many datagrams a slow tap subscriber may fall behind by.
///
/// Status arrives every 200 ms per peer, so four decks make 20 a second: a
/// subscriber this far behind has stopped reading altogether, and dropping is
/// better than growing without bound.
const TAP_CAPACITY: usize = 64;

impl VirtualCdj {
    /// Announce on the network `discovery` is listening to.
    ///
    /// Returns once a number is held — after the claim chain in
    /// [`Numbering::Claim`], immediately in [`Numbering::Observer`].
    pub async fn start(
        discovery: &Discovery,
        config: VirtualCdjConfig,
        media: Arc<dyn MediaSource>,
    ) -> Result<Self> {
        let interface = discovery.interface().clone();
        // We bind 0.0.0.0 and so receive our own broadcasts. Without this we
        // would list ourselves as a peer, which corrupts the peer count we put
        // in our own keep-alive.
        discovery.ignore(interface.mac);

        let was_first = discovery.is_alone();
        info!(
            first_on_network = was_first,
            peers = discovery.devices().len(),
            "starting virtual CDJ"
        );

        let (phase, _) = watch::channel(Phase::Watching);
        let number = match config.numbering {
            Numbering::Observer(number) => number,
            Numbering::Claim { preferred } => {
                match claim(discovery, &config, &interface, preferred, was_first, &phase).await {
                    Ok(number) => number,
                    Err(error) => {
                        let _ = phase.send(Phase::Failed);
                        return Err(error);
                    }
                }
            }
        };
        let _ = phase.send(Phase::Announcing);

        // Bound here rather than inside the responder, because asking a peer
        // what is in its slots uses the same socket as answering that question
        // for ourselves, and the answer comes back to this port.
        let status_socket = if config.emit_status {
            Some(Arc::new(socket::bind(STATUS_PORT, Some(&interface))?))
        } else {
            None
        };
        let status_tap = status_socket
            .as_ref()
            .map(|_| broadcast::Sender::new(TAP_CAPACITY));

        let mut cdj = Self {
            config,
            interface,
            number: Arc::new(AtomicU8::new(number.get())),
            phase,
            media,
            loaded: Arc::new(LoadedTracks::default()),
            peers: Arc::new(PeerMedia::default()),
            status_socket,
            status_tap,
            status_counter: Arc::new(AtomicU32::new(0)),
            playback: Arc::new(PlaybackCell::default()),
            playing_track: Arc::new(Mutex::new(None)),
            tempo_master: Arc::new(AtomicBool::new(false)),
            yielding_to: Arc::new(AtomicU8::new(0)),
            synced: Arc::new(AtomicBool::new(false)),
            tasks: Vec::new(),
        };
        cdj.tasks.push(cdj.spawn_keep_alive(discovery));
        cdj.tasks.push(cdj.spawn_defender(discovery));
        if let Some(socket) = cdj.status_socket.clone() {
            cdj.tasks.push(cdj.spawn_status(discovery)?);
            cdj.tasks.push(cdj.spawn_query_responder(socket));
            // Beats ride with status, not with the keep-alive: a device that
            // does not publish what it is doing has no business claiming a
            // beat, and the two are read together by everything downstream.
            cdj.tasks.push(cdj.spawn_beats()?);
        }
        Ok(cdj)
    }

    /// Announce without serving anything: enough to make peers talk to us.
    pub async fn observe(discovery: &Discovery, config: VirtualCdjConfig) -> Result<Self> {
        Self::start(discovery, config, Arc::new(NoMedia)).await
    }

    /// The number currently held.
    pub fn number(&self) -> DeviceNumber {
        DeviceNumber::new(self.number.load(Ordering::Relaxed)).unwrap_or(OBSERVER_NUMBER)
    }

    /// Every datagram arriving on UDP 50002, when we are the one holding it.
    ///
    /// `None` means we do not have the port and a reader should bind its own.
    /// See the `status_tap` field for why there is no third option.
    pub fn status_datagrams(&self) -> Option<broadcast::Receiver<Arc<Vec<u8>>>> {
        self.status_tap.as_ref().map(broadcast::Sender::subscribe)
    }

    /// The number currently held, if a peer would browse it.
    ///
    /// `None` means serving is pointless: a peer will accept everything and
    /// then never ask (F45). The serve side requires this rather than a plain
    /// device number, so a server that can never be browsed cannot be started.
    pub fn browsable_number(&self) -> Option<BrowsableDeviceNumber> {
        self.number().browsable()
    }

    /// What our peers have loaded from us. Shared, and kept current for as
    /// long as this virtual CDJ runs.
    pub fn loaded(&self) -> Arc<LoadedTracks> {
        Arc::clone(&self.loaded)
    }

    /// What our peers have in their own slots.
    ///
    /// Reading this never transmits. The slot states are current from the
    /// moment peers start unicasting status to us, which announcing is what
    /// earns (F21); the names and counts appear once [`Self::survey_media`] or
    /// [`Self::query_media`] has asked for them.
    pub fn peer_media(&self) -> Arc<PeerMedia> {
        Arc::clone(&self.peers)
    }

    /// Watch the claim state machine.
    pub fn phase(&self) -> watch::Receiver<Phase> {
        self.phase.subscribe()
    }

    /// State what this host is playing.
    ///
    /// Call this several times a second while a track is loaded. The playhead
    /// is projected between calls, so the rate governs how quickly a tempo
    /// *change* reaches the network, not how accurately beats are timed — see
    /// [`crate::playback`].
    ///
    /// It takes about [`crate::playback::STALE_AFTER`] of silence for the
    /// network to conclude that nothing is playing here, which is deliberate:
    /// the alternative is a stale tempo other decks stay locked to.
    pub fn set_playback(&self, playback: Playback) {
        self.playback.set(playback);
    }

    /// Nothing is playing here.
    ///
    /// Deliberately does **not** clear the loaded track. The two are set by
    /// different things at different moments: the browser says what it loaded
    /// the instant it loads it, and the tempo poll says what is playing thirty
    /// times a second -- so a poll that happened to land in the gap before the
    /// engine had the track was wiping the announcement that had just been
    /// made, and the track never reached the wire at all.
    pub fn clear_playback(&self) {
        self.playback.clear();
    }

    /// State what is loaded, and whose medium it came from.
    ///
    /// **Required alongside a tempo, not optional beside it.** A deck that
    /// publishes a tempo always publishes this too, and a CDJ asked to follow a
    /// master with no track simply does not.
    pub fn set_loaded_track(&self, track: Option<status::LoadedTrack>) {
        match self.playing_track.lock() {
            Ok(mut held) => *held = track,
            Err(poisoned) => *poisoned.into_inner() = track,
        }
    }

    /// What we are announcing as loaded.
    #[must_use]
    pub fn loaded_track(&self) -> Option<status::LoadedTrack> {
        match self.playing_track.lock() {
            Ok(held) => *held,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// What we are announcing as playing, playhead projected to now.
    pub fn playback(&self) -> Playback {
        self.playback.now()
    }

    /// Claim or release tempo master.
    ///
    /// **Only ever call this with `true` when nothing else holds it.** There is
    /// no handshake here (see the `tempo_master` field), so claiming it while
    /// another deck also claims it puts two masters on the network and every
    /// follower flickers between them. [`crate::Monitor::players`] is where to
    /// find out.
    pub fn set_tempo_master(&self, master: bool) {
        // Any handover in flight ends here, whichever way this goes: naming a
        // successor is only meaningful while the claim it is handing over still
        // stands, and this sets that claim outright.
        self.yielding_to.store(0, Ordering::Relaxed);
        self.tempo_master.store(master, Ordering::Relaxed);
    }

    /// Whether SYNC is engaged here.
    pub fn set_synced(&self, synced: bool) {
        self.synced.store(synced, Ordering::Relaxed);
    }

    /// Whether we are claiming tempo master.
    pub fn is_tempo_master(&self) -> bool {
        self.tempo_master.load(Ordering::Relaxed)
    }

    /// Answer a `0x26` and stand down.
    ///
    /// **A CDJ's MASTER button always wins.** Pressing it on another deck takes
    /// mastership from whoever has it; there is no refusing, and a device that
    /// ignored the request would go on claiming a mastership the rest of the
    /// network had already moved off — two masters, and every follower
    /// flickering between them.
    ///
    /// Synchronous, and deliberately so: this is one datagram sent in answer to
    /// one datagram, the reply arrives within 5 ms on real hardware, and making
    /// it `async` would mean holding the session lock across an await at every
    /// call site.
    pub fn yield_tempo_master(&self, requester: DeviceNumber, address: Ipv4Addr) -> Result<()> {
        if !self.is_tempo_master() {
            // Not ours to give. The request was broadcast at us by mistake, or
            // we stood down a moment ago; either way answering would claim a
            // handover that is not happening.
            return Ok(());
        }
        let response = prolink_proto::beat::MasterResponse {
            name: self.config.name,
            device: self.number(),
            granted: true,
        };
        let to = SocketAddr::V4(SocketAddrV4::new(address, BEAT_PORT));
        socket::send_once(&self.interface, to, &response.encode())?;
        // **The claim is kept, and the successor is named.** This is the half
        // that was missing, and without it the handover never completed: a real
        // deck watches byte `0x9f` for its own number, because the grant is
        // that byte and not the absence of byte `0x9e`. We answered the `0x26`
        // and then simply stopped claiming mastership, which from the
        // requester's side is indistinguishable from a refusal -- it asked, we
        // went quiet, and nobody ended up master at all.
        //
        // `finish_yield` drops both, once the requester has picked it up.
        self.yielding_to.store(requester.get(), Ordering::Relaxed);
        info!(%requester, "granted tempo master; naming it as successor");
        Ok(())
    }

    /// The device we are handing mastership to, if a handover is in flight.
    pub fn yielding_to(&self) -> Option<DeviceNumber> {
        DeviceNumber::new(self.yielding_to.load(Ordering::Relaxed))
    }

    /// Complete a handover: stop claiming mastership and stop naming a
    /// successor.
    ///
    /// Called once the requester is claiming it, which is what the yield was
    /// waiting for — or once waiting has gone on long enough that holding a
    /// mastership the network is not acting on is the worse of the two states.
    pub fn finish_yield(&self) {
        self.yielding_to.store(0, Ordering::Relaxed);
        self.set_tempo_master(false);
    }

    /// Ask the deck at *holder* to hand mastership over.
    ///
    /// One `0x26` on the beat port, unicast, exactly as a deck sends when its
    /// MASTER button is pressed. The holder answers with a `0x27` within about
    /// 5 ms and its status byte `0x9e` drops within ~70 ms (S28).
    ///
    /// **This does not set our own flag.** The `0x27` comes back unicast to
    /// port 50001, which a [`crate::Monitor`] is holding — only one socket in a
    /// `SO_REUSEPORT` group receives a given unicast datagram, so we cannot
    /// hear it. The handover is instead observed the way every other device on
    /// the network observes it: the old master's *status* stops saying it is
    /// master. That is a slower signal by a few packets and a more honest one,
    /// because it is the state the rest of the network is acting on.
    pub async fn request_tempo_master(&self, holder: Ipv4Addr) -> Result<()> {
        let request = prolink_proto::beat::MasterRequest {
            name: self.config.name,
            device: self.number(),
        };
        let socket = socket::bind_at(self.interface.ip, 0, Some(&self.interface))?;
        let to = SocketAddr::V4(SocketAddrV4::new(holder, BEAT_PORT));
        socket
            .send_to(&request.encode(), to)
            .await
            .map_err(Error::io("asking for tempo master"))?;
        info!(%holder, "asked the tempo master to hand over");
        Ok(())
    }

    /// Ask every online peer what is in both of its slots, and return the table.
    ///
    /// One `0x05` per peer per slot, then a single wait — rather than a wait per
    /// slot — because the answers land in [`Self::peer_media`] as they arrive
    /// and nothing here needs to match them up. A peer that does not answer in
    /// time keeps whatever the table already knew, so a slow deck costs its own
    /// row and not the survey.
    ///
    /// A deck answers for an empty slot too, with everything zeroed (F51), so
    /// use [`PeerSlot::has_media`] — which reads the status byte — rather than
    /// the presence of a description.
    pub async fn survey_media(
        &self,
        discovery: &Discovery,
        wait: Duration,
    ) -> Result<Vec<PeerSlot>> {
        let ours = self.number();
        let mut asked = 0usize;
        for device in discovery.online() {
            if device.number == ours {
                continue;
            }
            for slot in [Slot::USB, Slot::SD] {
                self.send_media_query(device.ip, device.number, slot)
                    .await?;
                asked += 1;
            }
        }
        debug!(queries = asked, "asked our peers what is in their slots");
        if asked > 0 {
            tokio::time::sleep(wait).await;
        }
        Ok(self.peers.all())
    }

    /// Ask one peer about one slot, and wait for that answer.
    ///
    /// For the whole network prefer [`Self::survey_media`], which asks
    /// everything at once instead of paying the timeout per slot.
    pub async fn query_media(
        &self,
        peer: Ipv4Addr,
        target: DeviceNumber,
        slot: Slot,
        wait: Duration,
    ) -> Result<MediaDescription> {
        self.send_media_query(peer, target, slot).await?;

        // Polled rather than notified: the answer arrives on another task, this
        // is a once-per-slot question, and a poll loop needs no channel that
        // could drop an answer while nobody is listening.
        let deadline = tokio::time::Instant::now() + wait;
        while tokio::time::Instant::now() < deadline {
            if let Some(entry) = self.peers.get(target, slot)
                && let Some(description) = entry.description
            {
                return Ok(description);
            }
            tokio::time::sleep(MEDIA_POLL).await;
        }
        Err(Error::Timeout {
            what: "a media query",
            after: wait,
        })
    }

    /// Send one `0x05`, addressed the way a deck addresses it.
    async fn send_media_query(
        &self,
        peer: Ipv4Addr,
        target: DeviceNumber,
        slot: Slot,
    ) -> Result<()> {
        let Some(socket) = self.status_socket.as_ref() else {
            return Err(Error::NoStatusPort { port: STATUS_PORT });
        };
        // We name ourselves by number *and* by address, because the answer is
        // sent to the address in the query rather than to the sender of it.
        let query = status::MediaQuery {
            requester: self.number(),
            requester_ip: self.interface.ip,
            target,
            slot,
        };
        let to = SocketAddr::V4(SocketAddrV4::new(peer, STATUS_PORT));
        socket
            .send_to(&query.encode(self.config.name), to)
            .await
            .map_err(Error::io("asking a peer about a slot"))?;
        Ok(())
    }

    /// The status packet we are emitting, for byte-diffing against a real deck.
    ///
    /// Built through the same function the emitter uses, so what this shows is
    /// what goes out rather than something assembled alongside it.
    pub fn status_packet(&self, peers: usize) -> CdjStatus {
        status_packet(
            &self.config,
            &self.number,
            self.media.as_ref(),
            &self.status_counter,
            peers,
            Transport {
                playback: &self.playback.now(),
                loaded: self.loaded_track(),
                tempo_master: self.is_tempo_master(),
                yielding_to: self.yielding_to(),
                synced: self.synced.load(Ordering::Relaxed),
            },
        )
    }

    /// The keep-alive we are broadcasting, for byte-diffing against a real deck.
    pub fn keep_alive(&self, peers: usize, was_first: bool) -> djl::Packet {
        djl::Packet::new(
            self.config.name,
            self.config.kind,
            Body::KeepAlive {
                device_number: self.number().get(),
                was_first_on_network: if was_first {
                    FIRST_ON_NETWORK
                } else {
                    JOINED_PEERS
                },
                mac: self.interface.mac,
                ip: self.interface.ip,
                // Includes ourselves.
                peer_count: u8::try_from(peers.saturating_add(1)).unwrap_or(u8::MAX),
                pad_31: [0; 3],
                flags: self.config.kind.role(),
                trailing: self.config.generation,
            },
        )
    }

    fn spawn_keep_alive(&self, discovery: &Discovery) -> JoinHandle<()> {
        let socket = discovery.socket();
        let broadcast = SocketAddr::V4(SocketAddrV4::new(
            self.interface.broadcast(),
            DISCOVERY_PORT,
        ));
        let was_first = discovery.is_alone();
        let config = self.config.clone();
        let interface = self.interface.clone();
        let number = Arc::clone(&self.number);
        let peers = PeerAddresses::new(discovery);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(djl::KEEPALIVE_INTERVAL);
            loop {
                ticker.tick().await;
                let packet =
                    keep_alive_packet(&config, &interface, &number, peers.len(), was_first);
                if let Err(error) = socket.send_to(&packet.encode(), broadcast).await {
                    warn!(%error, "keep-alive failed");
                }
            }
        })
    }

    /// Emit status, unicast, one copy per announced peer.
    ///
    /// Status is unicast on real hardware — not one of 1507 captured packets was
    /// broadcast (F21) — so this sends per peer rather than once. Peers that
    /// have not announced themselves get nothing, which mirrors why we received
    /// nothing until we announced.
    fn spawn_status(&self, discovery: &Discovery) -> Result<JoinHandle<()>> {
        // A real deck sends each status packet from a different, incrementing
        // source port. We do not imitate that; an ephemeral port is enough, and
        // the socket bound to 50002 is kept free to receive queries.
        let socket = socket::bind_at(self.interface.ip, 0, Some(&self.interface))?;
        let config = self.config.clone();
        let number = Arc::clone(&self.number);
        let media = Arc::clone(&self.media);
        let counter = Arc::clone(&self.status_counter);
        let table = PeerAddresses::new(discovery);
        let playback = Arc::clone(&self.playback);
        let playing_track = Arc::clone(&self.playing_track);
        let master = Arc::clone(&self.tempo_master);
        let yielding = Arc::clone(&self.yielding_to);
        let synced = Arc::clone(&self.synced);

        Ok(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(status::STATUS_INTERVAL);
            loop {
                ticker.tick().await;
                let peers = table.get();
                let packet = status_packet(
                    &config,
                    &number,
                    media.as_ref(),
                    &counter,
                    peers.len(),
                    Transport {
                        playback: &playback.now(),
                        loaded: playing_track.lock().ok().and_then(|held| *held),
                        tempo_master: master.load(Ordering::Relaxed),
                        yielding_to: DeviceNumber::new(yielding.load(Ordering::Relaxed)),
                        synced: synced.load(Ordering::Relaxed),
                    },
                );
                counter.fetch_add(1, Ordering::Relaxed);
                for peer in peers {
                    let to = SocketAddr::V4(SocketAddrV4::new(peer, STATUS_PORT));
                    if let Err(error) = socket.send_to(packet.as_bytes(), to).await {
                        debug!(%error, %peer, "status unicast failed");
                    }
                }
            }
        }))
    }

    /// Broadcast a beat packet on UDP 50001 as each beat arrives.
    ///
    /// **This is what makes us a tempo other devices can follow.** A status
    /// packet says what our tempo *is*, five times a second; a beat packet says
    /// where the beat *is*, and a follower phase-locks to the arrival time of
    /// this datagram, not to anything inside it. So the one property that
    /// matters is that it leaves on the beat.
    ///
    /// Hence the polling rather than a sleep-until-next-beat: a tempo change or
    /// a seek arrives between beats, and a task parked on a deadline computed
    /// before it would send the next beat in the wrong place. [`BEAT_TICK`] is
    /// the resulting worst-case lateness, and it is well under a millisecond of
    /// a DJ's perception at any tempo.
    ///
    /// Beats are **broadcast**, unlike status: they arrive on 50001 from decks
    /// that have never been told we exist, which is the whole reason a passive
    /// listener can see a tempo without announcing.
    fn spawn_beats(&self) -> Result<JoinHandle<()>> {
        let socket = socket::bind_at(self.interface.ip, 0, Some(&self.interface))?;
        let broadcast = SocketAddr::V4(SocketAddrV4::new(self.interface.broadcast(), BEAT_PORT));
        let name = self.config.name;
        let number = Arc::clone(&self.number);
        let playback = Arc::clone(&self.playback);

        Ok(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(BEAT_TICK);
            let mut emitted: Option<u32> = None;
            loop {
                ticker.tick().await;
                let now = playback.now();
                let Some(position) = beat_to_emit(&now, &mut emitted) else {
                    continue;
                };
                let Some(device) = DeviceNumber::new(number.load(Ordering::Relaxed)) else {
                    continue;
                };
                let Some(packet) = now.beat_packet(device, name, position) else {
                    continue;
                };
                if let Err(error) = socket.send_to(&packet.encode(), broadcast).await {
                    debug!(%error, "beat broadcast failed");
                }
            }
        }))
    }

    /// Answer media and settings queries on UDP 50002.
    ///
    /// **The step no reference implementation performs, because none of them
    /// serve.** Until these are answered, a deck that has otherwise fully
    /// accepted us — it is unicasting status to us and has completed a portmap
    /// and mount against our NFS server — still refuses to list us as a LINK
    /// source, because as far as it knows our slots hold nothing (F24).
    fn spawn_query_responder(&self, socket: Arc<UdpSocket>) -> JoinHandle<()> {
        let name = self.config.name;
        let number = Arc::clone(&self.number);
        let media = Arc::clone(&self.media);
        let loaded = Arc::clone(&self.loaded);
        let peers = Arc::clone(&self.peers);
        let tap = self.status_tap.clone();

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
                let SocketAddr::V4(from) = from else { continue };
                let Some(datagram) = buffer.get(..len) else {
                    continue;
                };
                let Some(ours) = DeviceNumber::new(number.load(Ordering::Relaxed)) else {
                    continue;
                };
                // Before the decode, and unconditionally: a tap subscriber is
                // interested in packets this responder ignores, and copying is
                // only paid for when somebody is listening.
                if let Some(tap) = tap.as_ref()
                    && tap.receiver_count() > 0
                {
                    let _ = tap.send(Arc::new(datagram.to_vec()));
                }

                let reply = match status::decode(datagram) {
                    // Not a query, but the packet that tells us what this deck
                    // has loaded from us — which is what lets a track row be
                    // marked as the loaded one (F55). Only a track whose
                    // source player is *us* came off our medium.
                    Ok(status::Packet::CdjStatus(peer)) => {
                        observe_loaded(&loaded, ours, &peer);
                        observe_peer_slots(&peers, &peer);
                        None
                    }
                    // The answer to a question we asked. It is not addressed to
                    // us by number — a deck sends it to whoever asked — so the
                    // table is filled here and `survey_media` only has to wait.
                    Ok(status::Packet::MediaResponse(response)) => {
                        observe_peer_media(&peers, &response);
                        None
                    }
                    Ok(status::Packet::MediaQuery(query)) => {
                        answer_media_query(query, ours, name, media.as_ref())
                            .map(|reply| (reply, query.requester_ip))
                    }
                    Ok(status::Packet::SettingsQuery(query)) => {
                        let settings = media.settings(query.slot);
                        let response = status::SettingsResponse::build(
                            name,
                            ours,
                            query.requester,
                            query.slot,
                            &settings,
                        );
                        info!(slot = %query.slot, bytes = settings.len(), "answered a settings query");
                        Some((response.into_bytes(), *from.ip()))
                    }
                    _ => None,
                };

                if let Some((datagram, to)) = reply {
                    let to = SocketAddr::V4(SocketAddrV4::new(to, STATUS_PORT));
                    if let Err(error) = socket.send_to(&datagram, to).await {
                        warn!(%error, "could not answer a query");
                    }
                }
            }
        })
    }

    /// Defend our number, and watch for anyone defending it against us.
    ///
    /// A device that takes a number but never defends it simply loses it to the
    /// next player that boots.
    fn spawn_defender(&self, discovery: &Discovery) -> JoinHandle<()> {
        let mut announcements = discovery.announcements();
        let socket = discovery.socket();
        let config = self.config.clone();
        let interface = self.interface.clone();
        let number = Arc::clone(&self.number);

        tokio::spawn(async move {
            loop {
                let Ok(announcement) = announcements.recv().await else {
                    return;
                };
                let ours = number.load(Ordering::Relaxed);
                let (Body::ClaimIp {
                    device_number: proposed,
                    ..
                }
                | Body::ClaimNumber {
                    device_number: proposed,
                    ..
                }) = announcement.packet.body
                else {
                    continue;
                };
                if proposed != ours || announcement.packet.body.mac() == Some(interface.mac) {
                    continue;
                }

                info!(number = ours, challenger = %announcement.from, "defending our device number");
                let packet = djl::Packet::new(
                    config.name,
                    config.kind,
                    Body::NumberConflict {
                        device_number: ours,
                        ip: interface.ip,
                    },
                );
                let to = SocketAddr::V4(SocketAddrV4::new(announcement.from, DISCOVERY_PORT));
                if let Err(error) = socket.send_to(&packet.encode(), to).await {
                    warn!(%error, "could not defend our number");
                }
            }
        })
    }
}

impl Drop for VirtualCdj {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Build the answer to a media query, or nothing if the slot is not ours.
///
/// Saying nothing about a slot we do not serve is right: an empty reply would
/// tell the deck the slot exists and holds no tracks, and it would then offer an
/// empty medium.
fn answer_media_query(
    query: status::MediaQuery,
    ours: DeviceNumber,
    name: DeviceName,
    media: &dyn MediaSource,
) -> Option<Vec<u8>> {
    if query.target != ours {
        return None;
    }
    let description = media.describe(query.slot)?;
    info!(
        requester = %query.requester,
        slot = %query.slot,
        tracks = description.track_count,
        playlists = description.playlist_count,
        "answered a media query",
    );
    let mut builder = status::MediaResponse::builder()
        .device_number(ours)
        .slot(query.slot)
        .name(name)
        .volume_name(&description.volume_name)
        .counts(description.track_count, description.playlist_count);
    if let (Some(total), Some(free)) = (description.total_bytes, description.free_bytes) {
        builder = builder.size(total, free);
    }
    Some(builder.build().into_bytes())
}

fn keep_alive_packet(
    config: &VirtualCdjConfig,
    interface: &crate::Interface,
    number: &AtomicU8,
    peers: usize,
    was_first: bool,
) -> djl::Packet {
    djl::Packet::new(
        config.name,
        config.kind,
        Body::KeepAlive {
            device_number: number.load(Ordering::Relaxed),
            was_first_on_network: if was_first {
                FIRST_ON_NETWORK
            } else {
                JOINED_PEERS
            },
            mac: interface.mac,
            ip: interface.ip,
            peer_count: u8::try_from(peers.saturating_add(1)).unwrap_or(u8::MAX),
            pad_31: [0; 3],
            flags: config.kind.role(),
            trailing: config.generation,
        },
    )
}

/// Record what a peer's status packet says it has loaded **from us**.
///
/// Only a track whose source player is our own number came off our medium; a
/// track a deck loaded from another player is not ours to mark, and marking by
/// id alone would mark whichever of our tracks happened to share that id.
fn observe_loaded(loaded: &LoadedTracks, ours: DeviceNumber, peer: &CdjStatus) {
    let Some(device) = peer.sender() else {
        return;
    };
    if peer.source_player() != Some(ours) {
        return;
    }
    loaded.note(device.get(), peer.source_slot(), peer.track_id());
}

/// Record what a peer's status packet says about its own two slots.
///
/// Free of charge and unasked-for: occupancy is published here and nowhere else
/// (F20), so every status packet that arrives is a fresh answer to "does that
/// deck have media in it".
fn observe_peer_slots(peers: &PeerMedia, peer: &CdjStatus) {
    let Some(device) = peer.sender() else {
        return;
    };
    for slot in [Slot::USB, Slot::SD] {
        if let Some(state) = peer.slot_state(slot) {
            peers.note_state(device, slot, state);
        }
    }
}

/// Record what a media response says about the slot it describes.
fn observe_peer_media(peers: &PeerMedia, response: &status::MediaResponse) {
    let Some(device) = response.device() else {
        return;
    };
    let slot = response.slot();
    let description = MediaDescription {
        volume_name: response.volume_name(),
        created: response.created(),
        track_count: response.track_count(),
        playlist_count: response.playlist_count(),
        total_bytes: response.total_bytes(),
        free_bytes: response.free_bytes(),
    };
    debug!(
        %device,
        %slot,
        tracks = description.track_count,
        playlists = description.playlist_count,
        volume = description.volume_name,
        "a peer described one of its slots",
    );
    peers.note_description(device, slot, &description);
}

/// Which beat, if any, is due to be broadcast right now.
///
/// `emitted` is the last beat number sent, and this is where the awkward cases
/// live rather than in the task above:
///
///  * **A beat is due** when the playhead has reached a beat we have not sent.
///    Only ever one datagram per tick, so a stall in the task cannot produce a
///    burst of backdated beats — a follower would read that as a tempo spike.
///  * **A seek moves the playhead backwards**, and the beat it lands on has
///    usually been sent before. Refusing to re-send it would go silent until
///    the track caught up to where it had been, which after a jump to the start
///    of a five-minute track is five minutes. So a position *behind* the last
///    one resets the count and beats resume immediately.
///  * **Stopping** clears the count, so the first beat after pressing play is
///    sent rather than skipped.
fn beat_to_emit(playback: &Playback, emitted: &mut Option<u32>) -> Option<BeatPosition> {
    let (Some(position), true) = (playback.beat, playback.playing) else {
        *emitted = None;
        return None;
    };
    match *emitted {
        Some(last) if position.number == last => None,
        Some(last) if position.number > last => {
            // One at a time, and the next tick will bring the one after.
            let next = BeatPosition {
                number: last + 1,
                fraction: 0.0,
            };
            *emitted = Some(next.number);
            Some(next)
        }
        // Behind where we were, or nothing sent yet: start again from here.
        _ => {
            *emitted = Some(position.number);
            Some(position)
        }
    }
}

/// Everything the status packet says about what this deck is *doing*, as
/// against what media it has.
///
/// One struct because they travel together and are read together: a follower
/// checks several of them against each other, and a packet where the tempo,
/// the loaded track and the play state disagree describes a deck that has
/// never existed. Passing them as loose arguments made that easy to get wrong
/// and easy to leave out.
#[derive(Clone, Copy, Debug)]
struct Transport<'a> {
    playback: &'a Playback,
    loaded: Option<status::LoadedTrack>,
    tempo_master: bool,
    /// Byte `0x9f`: who we are handing mastership to, mid-handover.
    yielding_to: Option<DeviceNumber>,
    synced: bool,
}

fn status_packet(
    config: &VirtualCdjConfig,
    number: &AtomicU8,
    media: &dyn MediaSource,
    counter: &AtomicU32,
    peers: usize,
    transport: Transport<'_>,
) -> CdjStatus {
    let Transport {
        playback,
        loaded,
        tempo_master,
        yielding_to,
        synced,
    } = transport;
    let occupied = media.occupied_slots();
    let state = |slot: Slot| media.slot_state(slot);
    let mut builder = CdjStatus::builder()
        .name(config.name)
        .slot_state(Slot::USB, state(Slot::USB))
        .slot_state(Slot::SD, state(Slot::SD))
        .link_available(!occupied.is_empty() || peers > 0)
        .firmware(&config.firmware)
        .packet_counter(counter.load(Ordering::Relaxed))
        .tempo(playback.bpm_centi, playback.pitch)
        .loaded_track(loaded)
        .playing(playback.playing)
        .tempo_master(tempo_master)
        .yielding_to(yielding_to)
        .synced(synced)
        .beat(
            playback.beat.map(|beat| beat.number),
            playback.beat.map(BeatPosition::in_bar),
        )
        .play_state(play_state_for(playback).0);
    if let Some(number) = DeviceNumber::new(number.load(Ordering::Relaxed)) {
        builder = builder.device_number(number);
    }
    builder.build()
}

/// Byte `0x7b` for what we are doing.
///
/// Three of the twelve values a CDJ can send, because three is all a host with
/// no platter can honestly be in: nothing loaded, playing, or paused. Reporting
/// a cue or search state we do not have would be inventing a transport.
fn play_state_for(playback: &Playback) -> crate::monitor::PlayState {
    use crate::monitor::PlayState;
    if playback.bpm_centi.is_none() {
        PlayState::NO_TRACK
    } else if playback.playing {
        PlayState::PLAYING
    } else {
        PlayState::PAUSED
    }
}

/// Run the claim chain and return the number we ended up holding.
async fn claim(
    discovery: &Discovery,
    config: &VirtualCdjConfig,
    interface: &crate::Interface,
    preferred: Option<BrowsableDeviceNumber>,
    was_first: bool,
    phase: &watch::Sender<Phase>,
) -> Result<DeviceNumber> {
    debug!(?preferred, "watching before claiming");
    let _ = phase.send(Phase::Watching);
    tokio::time::sleep(PRESCAN).await;

    let seen = discovery.numbers_seen();
    let mut candidates: Vec<BrowsableDeviceNumber> = preferred
        .into_iter()
        .chain(BrowsableDeviceNumber::descending())
        .filter(|number| !seen.contains(&number.get()))
        .collect();
    candidates.dedup();
    if candidates.is_empty() {
        return Err(discovery.no_browsable_number());
    }

    let socket = discovery.socket();
    let to = SocketAddr::V4(SocketAddrV4::new(interface.broadcast(), DISCOVERY_PORT));
    let _ = phase.send(Phase::Claiming);

    for candidate in candidates {
        let mut conflicts = discovery.announcements();
        match claim_one(
            &socket,
            to,
            config,
            interface,
            candidate,
            was_first,
            &mut conflicts,
        )
        .await
        {
            Ok(()) => {
                info!(number = %candidate, "claimed a browsable device number");
                return Ok(candidate.number());
            }
            Err(holder) => {
                warn!(number = %candidate, %holder, "number is taken; backing off");
            }
        }
    }
    Err(discovery.no_browsable_number())
}

/// One pass of the claim chain for one candidate.
///
/// `Err(holder)` means somebody defended it.
async fn claim_one(
    socket: &UdpSocket,
    to: SocketAddr,
    config: &VirtualCdjConfig,
    interface: &crate::Interface,
    candidate: BrowsableDeviceNumber,
    was_first: bool,
    conflicts: &mut broadcast::Receiver<crate::discovery::Announcement>,
) -> std::result::Result<(), Ipv4Addr> {
    // N is 3 into an empty network and 1 into a populated one (C13).
    let final_stage = if was_first { 3 } else { 1 };
    let mut stages: Vec<Body> = Vec::new();
    for iteration in 1..=3 {
        stages.push(Body::Hello {
            payload: config.kind.role(),
        });
        let _ = iteration;
    }
    for iteration in 1..=3 {
        stages.push(Body::ClaimMac {
            iteration,
            flags: config.kind.role(),
            mac: interface.mac,
        });
    }
    for iteration in 1..=3 {
        stages.push(Body::ClaimIp {
            ip: interface.ip,
            mac: interface.mac,
            device_number: candidate.get(),
            iteration,
            role: config.kind.role(),
            // We always ask for a specific number, having picked it ourselves.
            assignment_mode: djl::AssignmentMode::MANUAL,
        });
    }
    for iteration in 1..=final_stage {
        stages.push(Body::ClaimNumber {
            device_number: candidate.get(),
            iteration,
        });
    }

    for body in stages {
        let packet = djl::Packet::new(config.name, config.kind, body);
        if let Err(error) = socket.send_to(&packet.encode(), to).await {
            warn!(%error, "claim packet failed");
        }
        // Sleep between packets, watching for a conflict the whole time.
        let deadline = tokio::time::sleep(djl::DISCOVERY_INTERVAL);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = &mut deadline => break,
                received = conflicts.recv() => {
                    let Ok(announcement) = received else { break };
                    if let Body::NumberConflict { device_number, ip } = announcement.packet.body
                        && device_number == candidate.get()
                    {
                        return Err(if ip.is_unspecified() { announcement.from } else { ip });
                    }
                }
            }
        }
    }
    Ok(())
}

/// The addresses of every peer currently answering, refreshed as the table
/// changes.
///
/// Status is unicast per peer and the keep-alive carries a peer count, so both
/// timers need this and neither wants to lock the whole device table at 200 ms.
#[derive(Debug)]
struct PeerAddresses {
    devices: Arc<Mutex<Vec<Ipv4Addr>>>,
    task: JoinHandle<()>,
}

impl PeerAddresses {
    fn new(discovery: &Discovery) -> Self {
        let devices = Arc::new(Mutex::new(
            discovery
                .online()
                .into_iter()
                .map(|device| device.ip)
                .collect::<Vec<_>>(),
        ));
        let mut events = discovery.subscribe();
        let shared = Arc::clone(&devices);
        let task = tokio::spawn(async move {
            loop {
                let Ok(event) = events.recv().await else {
                    return;
                };
                let ip = event.device().ip;
                let add = !matches!(
                    event,
                    crate::device::DeviceEvent::Forgotten(_)
                        | crate::device::DeviceEvent::WentOffline(_)
                );
                let mut devices = match shared.lock() {
                    Ok(devices) => devices,
                    Err(poisoned) => poisoned.into_inner(),
                };
                devices.retain(|known| *known != ip);
                if add {
                    devices.push(ip);
                }
            }
        });
        Self { devices, task }
    }

    fn get(&self) -> Vec<Ipv4Addr> {
        match self.devices.lock() {
            Ok(devices) => devices.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn len(&self) -> usize {
        match self.devices.lock() {
            Ok(devices) => devices.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }
}

impl Drop for PeerAddresses {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    /// Every status packet in a capture, fed through [`observe_loaded`] the way
    /// the live socket feeds it.
    ///
    /// The one end-to-end check that the registry behind the key-matching
    /// indicator fills at all: a real deck-to-deck session in which device 2
    /// loaded track 472 off device 1's USB (F55).
    #[test]
    fn a_capture_of_a_deck_loading_from_us_fills_the_registry() {
        let Some(corpus) = prolink_capture::Corpus::locate() else {
            return;
        };
        let path = corpus.root().join("S27-sort-by-and-key/run.pcap");
        let Ok(capture) = prolink_capture::Capture::open(&path) else {
            return;
        };
        let loaded = LoadedTracks::default();
        let mut seen = 0usize;
        for packet in capture.udp_to(STATUS_PORT).flatten() {
            let Ok(status::Packet::CdjStatus(peer)) = status::decode(&packet.payload) else {
                continue;
            };
            let Some(source) = peer.source_player() else {
                continue;
            };
            // Stand in for whichever deck was serving in this session.
            observe_loaded(&loaded, source, &peer);
            seen += 1;
        }
        assert!(
            seen > 100,
            "the corpus should hold plenty of status packets"
        );
        assert!(
            !loaded.by_device.lock().expect("not poisoned").is_empty(),
            "no deck's loaded track was recorded from {seen} status packets"
        );
    }

    #[test]
    fn a_track_loaded_from_another_player_is_not_ours_to_mark() {
        let loaded = LoadedTracks::default();
        let ours = DeviceNumber::new(4).expect("4 is a device number");
        let theirs = DeviceNumber::new(1).expect("1 is a device number");
        let mut raw = CdjStatus::builder()
            .device_number(DeviceNumber::new(2).expect("2 is a device number"))
            .name(DeviceName::default())
            .build()
            .into_bytes();
        raw[0x28] = theirs.get();
        raw[0x29] = Slot::USB.0;
        raw[0x2c..0x30].copy_from_slice(&99u32.to_be_bytes());
        let peer = CdjStatus::parse(&raw).expect("a status packet");
        observe_loaded(&loaded, ours, &peer);
        assert_eq!(loaded.track_on(2, Slot::USB), None);

        raw[0x28] = ours.get();
        let peer = CdjStatus::parse(&raw).expect("a status packet");
        observe_loaded(&loaded, ours, &peer);
        assert_eq!(loaded.track_on(2, Slot::USB), Some(99));
    }

    use super::*;
    use prolink_proto::status::MediaState;

    fn interface() -> crate::Interface {
        crate::Interface {
            name: "test0".to_owned(),
            index: 1,
            ip: Ipv4Addr::new(169, 254, 99, 100),
            netmask: Ipv4Addr::new(255, 255, 0, 0),
            mac: prolink_proto::MacAddress([0xa0, 0xce, 0xc8, 0xe2, 0x26, 0xde]),
        }
    }

    #[test]
    fn the_default_numbering_cannot_disturb_a_live_rig() {
        let Numbering::Observer(number) = Numbering::default() else {
            panic!("the default must not claim");
        };
        assert_eq!(number, OBSERVER_NUMBER);
        assert!(
            !number.is_player(),
            "outside 1-6, so it cannot collide with hardware"
        );
        assert!(
            number.browsable().is_none(),
            "and cannot be browsed, by construction"
        );
    }

    #[test]
    fn our_keep_alive_matches_a_real_deck_byte_for_byte() {
        // The captured keep-alive from `captures/S24b-e9-control`, which the
        // codec's own test also pins. Emitting a packet that differs from this
        // is the definition of being distinguishable from a real deck.
        const REAL: &[u8] = &[
            0x51, 0x73, 0x70, 0x74, 0x31, 0x57, 0x6d, 0x4a, 0x4f, 0x4c, 0x06, 0x00, 0x43, 0x44,
            0x4a, 0x2d, 0x32, 0x30, 0x30, 0x30, 0x6e, 0x65, 0x78, 0x75, 0x73, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x36, 0x05, 0x02, 0xa0, 0xce, 0xc8, 0xe2,
            0x26, 0xde, 0xa9, 0xfe, 0x63, 0x64, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
        ];
        let config = VirtualCdjConfig {
            numbering: Numbering::Observer(DeviceNumber::new(5).unwrap()),
            ..VirtualCdjConfig::default()
        };
        let number = AtomicU8::new(5);
        let packet = keep_alive_packet(&config, &interface(), &number, 0, true);
        assert_eq!(packet.encode(), REAL);
    }

    #[test]
    fn a_status_packet_reports_only_the_slots_we_serve() {
        #[derive(Debug)]
        struct UsbOnly;
        impl MediaSource for UsbOnly {
            fn occupied_slots(&self) -> std::collections::BTreeSet<Slot> {
                [Slot::USB].into_iter().collect()
            }
            fn describe(&self, slot: Slot) -> Option<MediaDescription> {
                (slot == Slot::USB).then(MediaDescription::default)
            }
        }

        let number = AtomicU8::new(3);
        let counter = AtomicU32::new(0);
        let packet = status_packet(
            &VirtualCdjConfig::default(),
            &number,
            &UsbOnly,
            &counter,
            1,
            Transport {
                playback: &Playback::default(),
                loaded: None,
                tempo_master: false,
                yielding_to: None,
                synced: false,
            },
        );
        assert_eq!(packet.usb_state(), MediaState::LOADED);
        assert_eq!(
            packet.sd_state(),
            MediaState::EMPTY,
            "a slot we do not serve is empty"
        );
        assert!(packet.link_available());
        assert_eq!(
            packet.bpm_centi(),
            None,
            "a server with nothing playing states no tempo"
        );
        assert_eq!(packet.beat_in_bar(), None);
    }

    /// Track 42 off player 2's USB, which is where `playing()` came from.
    fn playing_track() -> status::LoadedTrack {
        status::LoadedTrack {
            source_player: DeviceNumber::new(2).expect("device 2"),
            slot: Slot::USB,
            id: 42,
        }
    }

    /// A deck playing 145.00 BPM, on beat 6, halfway through it.
    fn playing() -> Playback {
        Playback {
            bpm_centi: Some(14_500),
            pitch: prolink_proto::beat::Pitch::UNITY,
            playing: true,
            beat: Some(BeatPosition {
                number: 6,
                fraction: 0.5,
            }),
        }
    }

    #[test]
    fn what_we_are_playing_reaches_the_status_packet() {
        // The whole point of the tempo half: without these fields a CDJ sees a
        // device with no tempo, has nothing to beat-match against, and draws no
        // phase for us at all.
        let number = AtomicU8::new(2);
        let counter = AtomicU32::new(0);
        let packet = status_packet(
            &VirtualCdjConfig::default(),
            &number,
            &NoMedia,
            &counter,
            1,
            Transport {
                playback: &playing(),
                loaded: Some(playing_track()),
                tempo_master: true,
                yielding_to: None,
                synced: false,
            },
        );
        assert_eq!(packet.bpm_centi(), Some(14_500));
        assert_eq!(packet.beat_number(), Some(6));
        assert_eq!(packet.beat_in_bar(), prolink_proto::beat::BeatInBar::new(2));
        assert_eq!(packet.is_tempo_master(), Some(true));
        // The track the tempo belongs to. A master that states one without the
        // other is a state no deck has ever published, and a CDJ will not
        // follow it -- which is how this was found.
        assert_eq!(packet.track_id(), 42);
        assert_eq!(packet.source_slot(), Slot::USB);
        assert_eq!(packet.track_type(), 1);
        assert_eq!(
            packet.play_state(),
            Some(crate::monitor::PlayState::PLAYING.0)
        );
    }

    #[test]
    fn a_handover_names_its_successor_and_keeps_the_claim() {
        // **The grant is byte `0x9f`, not the absence of byte `0x9e`.** A deck
        // asking for mastership watches for its own number to appear at 0x9f
        // and acts on nothing else; the holder's claim simply going away is
        // what a refusal looks like from the requester's side.
        //
        // So a handover in flight publishes BOTH: still master, and naming who
        // it is handing to. Publishing only the second would be a deck that has
        // never existed, and publishing only the first is the bug this test
        // exists for -- a CDJ that pressed MASTER and never got it, with both
        // decks believing the handover had happened.
        let number = AtomicU8::new(2);
        let counter = AtomicU32::new(0);
        let successor = DeviceNumber::new(3).expect("a device number");
        let packet = status_packet(
            &VirtualCdjConfig::default(),
            &number,
            &NoMedia,
            &counter,
            1,
            Transport {
                playback: &playing(),
                loaded: Some(playing_track()),
                tempo_master: true,
                yielding_to: Some(successor),
                synced: false,
            },
        );
        assert_eq!(packet.is_tempo_master(), Some(true));
        assert_eq!(packet.yielding_to(), Some(successor));
    }

    #[test]
    fn no_handover_leaves_the_successor_byte_empty() {
        // 0xff, not zero, and written every packet rather than left to the
        // captured skeleton: a field only written when it is interesting keeps
        // whatever the last interesting packet put there.
        let number = AtomicU8::new(2);
        let counter = AtomicU32::new(0);
        let packet = status_packet(
            &VirtualCdjConfig::default(),
            &number,
            &NoMedia,
            &counter,
            1,
            Transport {
                playback: &playing(),
                loaded: Some(playing_track()),
                tempo_master: true,
                yielding_to: None,
                synced: false,
            },
        );
        assert_eq!(packet.yielding_to(), None);
    }

    #[test]
    fn a_paused_deck_still_states_its_tempo() {
        // It has a track loaded and a tempo to state; what it does not have is
        // sound coming out. Reporting no tempo would make the deck vanish from
        // every other player's tempo display the moment it is paused.
        let number = AtomicU8::new(2);
        let counter = AtomicU32::new(0);
        let packet = status_packet(
            &VirtualCdjConfig::default(),
            &number,
            &NoMedia,
            &counter,
            0,
            Transport {
                playback: &Playback {
                    playing: false,
                    ..playing()
                },
                loaded: Some(playing_track()),
                tempo_master: false,
                yielding_to: None,
                synced: false,
            },
        );
        assert_eq!(packet.bpm_centi(), Some(14_500));
        assert_eq!(
            packet.play_state(),
            Some(crate::monitor::PlayState::PAUSED.0)
        );
        assert_eq!(
            packet.flags().map(status::StatusFlags::is_playing),
            Some(false)
        );
    }

    #[test]
    fn beats_are_emitted_once_each_and_in_order() {
        let mut emitted = None;
        let mut playback = playing();
        // The first beat seen is sent immediately -- there is nothing to be
        // late for yet.
        assert_eq!(
            beat_to_emit(&playback, &mut emitted).map(|p| p.number),
            Some(6)
        );
        // ...and not again while the playhead is still inside it.
        playback.beat = Some(BeatPosition {
            number: 6,
            fraction: 0.9,
        });
        assert_eq!(beat_to_emit(&playback, &mut emitted), None);
        // The next beat goes out on its downbeat, not at the fraction the poll
        // happened to catch it at: a follower phase-locks to arrival time.
        playback.beat = Some(BeatPosition {
            number: 7,
            fraction: 0.3,
        });
        let next = beat_to_emit(&playback, &mut emitted).expect("beat 7");
        assert_eq!(next.number, 7);
        assert!(next.fraction.abs() < f64::EPSILON);
    }

    #[test]
    fn a_stalled_emitter_does_not_fire_a_burst_of_backdated_beats() {
        // If the task misses its tick -- a busy Pi, a scheduler hiccup -- the
        // playhead may be several beats further on. Sending all of them now
        // would put four beats on the wire in one millisecond, which every
        // follower reads as an enormous tempo spike.
        let mut emitted = None;
        let mut playback = playing();
        beat_to_emit(&playback, &mut emitted);
        playback.beat = Some(BeatPosition {
            number: 11,
            fraction: 0.0,
        });
        for expected in 7..=11 {
            assert_eq!(
                beat_to_emit(&playback, &mut emitted).map(|p| p.number),
                Some(expected),
                "one beat per tick, catching up in order"
            );
        }
    }

    #[test]
    fn a_seek_backwards_resumes_beats_from_where_it_landed() {
        // The bug this prevents is silence: after a jump to the start of a
        // five-minute track, "only send beats we have not sent" means no beats
        // for five minutes.
        let mut emitted = None;
        let mut playback = playing();
        beat_to_emit(&playback, &mut emitted);
        playback.beat = Some(BeatPosition {
            number: 2,
            fraction: 0.0,
        });
        assert_eq!(
            beat_to_emit(&playback, &mut emitted).map(|p| p.number),
            Some(2)
        );
    }

    #[test]
    fn nothing_playing_emits_nothing_and_forgets_where_it_was() {
        let mut emitted = None;
        let playback = playing();
        beat_to_emit(&playback, &mut emitted);
        let stopped = Playback {
            playing: false,
            ..playback
        };
        assert_eq!(beat_to_emit(&stopped, &mut emitted), None);
        assert_eq!(emitted, None, "so the first beat after play is sent");
        assert_eq!(
            beat_to_emit(&playback, &mut emitted).map(|p| p.number),
            Some(6)
        );
    }

    #[test]
    fn a_slot_being_ejected_publishes_the_state_a_consumer_acts_on() {
        // `0x03` is what makes a deck send UMNT — it did so within 16 ms of it
        // in both captured ejects, and did nothing on `0x02`. So a source that
        // reports it must reach the wire unchanged rather than being flattened
        // to "no media here".
        #[derive(Debug)]
        struct Ejecting;
        impl MediaSource for Ejecting {
            fn occupied_slots(&self) -> std::collections::BTreeSet<Slot> {
                std::collections::BTreeSet::new()
            }
            fn describe(&self, _slot: Slot) -> Option<MediaDescription> {
                None
            }
            fn slot_state(&self, slot: Slot) -> MediaState {
                if slot == Slot::USB {
                    MediaState::UNMOUNTING_ALT
                } else {
                    MediaState::EMPTY
                }
            }
        }

        let number = AtomicU8::new(3);
        let counter = AtomicU32::new(0);
        let packet = status_packet(
            &VirtualCdjConfig::default(),
            &number,
            &Ejecting,
            &counter,
            1,
            Transport {
                playback: &Playback::default(),
                loaded: None,
                tempo_master: false,
                yielding_to: None,
                synced: false,
            },
        );
        assert_eq!(packet.usb_state(), MediaState::UNMOUNTING_ALT);
        assert_eq!(packet.sd_state(), MediaState::EMPTY);
    }

    /// A peer's status packet, with whatever it says about its slots.
    fn peer_status(device: u8, usb: MediaState, sd: MediaState) -> CdjStatus {
        let number = DeviceNumber::new(device).expect("a real device number");
        CdjStatus::builder()
            .device_number(number)
            .name(DeviceName::default())
            .slot_state(Slot::USB, usb)
            .slot_state(Slot::SD, sd)
            .build()
    }

    #[test]
    fn a_peers_status_packet_says_which_of_its_slots_hold_media() {
        // Occupancy is published here and nowhere else (F20), so this needs no
        // query and is current as soon as a peer unicasts to us.
        let peers = PeerMedia::default();
        observe_peer_slots(
            &peers,
            &peer_status(2, MediaState::LOADED, MediaState::EMPTY),
        );

        let device = DeviceNumber::new(2).unwrap();
        let usb = peers.get(device, Slot::USB).expect("the USB is known");
        assert!(usb.has_media());
        assert!(
            !peers
                .get(device, Slot::SD)
                .expect("the SD is known too")
                .has_media(),
            "an empty slot is known to be empty, which is not the same as unknown"
        );
        assert_eq!(peers.occupied().len(), 1);
        assert_eq!(
            usb.track_count(),
            None,
            "nothing has described it yet, and occupancy does not imply a count"
        );
    }

    #[test]
    fn a_media_response_names_the_medium_and_counts_it() {
        // Round-tripped through the wire form rather than constructed, so this
        // exercises the same parse a real deck's answer goes through.
        let peers = PeerMedia::default();
        let device = DeviceNumber::new(2).unwrap();
        let raw = status::MediaResponse::builder()
            .device_number(device)
            .slot(Slot::USB)
            .name(DeviceName::default())
            .volume_name("SAM2")
            .counts(692, 35)
            .build()
            .into_bytes();
        let response = status::MediaResponse::parse(&raw).expect("our own encoding parses");
        observe_peer_media(&peers, &response);

        let usb = peers.get(device, Slot::USB).expect("the USB is described");
        assert_eq!(usb.volume_name(), "SAM2");
        assert_eq!(usb.track_count(), Some(692));
        assert_eq!(usb.playlist_count(), Some(35));
        assert!(
            !usb.has_media(),
            "a description is not evidence of a medium: a deck answers for an empty slot too (F51)"
        );
    }

    #[test]
    fn the_two_halves_of_a_slot_arrive_separately_and_do_not_overwrite_each_other() {
        let peers = PeerMedia::default();
        let device = DeviceNumber::new(2).unwrap();
        let raw = status::MediaResponse::builder()
            .device_number(device)
            .slot(Slot::USB)
            .name(DeviceName::default())
            .volume_name("SAM2")
            .counts(692, 35)
            .build()
            .into_bytes();
        observe_peer_media(
            &peers,
            &status::MediaResponse::parse(&raw).expect("it parses"),
        );
        observe_peer_slots(
            &peers,
            &peer_status(2, MediaState::LOADED, MediaState::EMPTY),
        );

        let usb = peers.get(device, Slot::USB).expect("both halves landed");
        assert!(usb.has_media(), "the status packet supplied the occupancy");
        assert_eq!(usb.track_count(), Some(692), "and the response the counts");

        // And the other way round: a later status packet must not erase what a
        // response taught us, or an eject would take the name with it.
        observe_peer_slots(
            &peers,
            &peer_status(2, MediaState::UNMOUNTING, MediaState::EMPTY),
        );
        let usb = peers.get(device, Slot::USB).expect("still there");
        assert_eq!(usb.state, MediaState::UNMOUNTING);
        assert_eq!(usb.volume_name(), "SAM2");
    }

    #[test]
    fn peers_are_listed_by_device_and_slot() {
        let peers = PeerMedia::default();
        observe_peer_slots(
            &peers,
            &peer_status(3, MediaState::LOADED, MediaState::LOADED),
        );
        observe_peer_slots(
            &peers,
            &peer_status(1, MediaState::LOADED, MediaState::EMPTY),
        );

        let listed: Vec<_> = peers
            .all()
            .into_iter()
            .map(|entry| (entry.device.get(), entry.slot))
            .collect();
        assert_eq!(
            listed,
            vec![(1, Slot::SD), (1, Slot::USB), (3, Slot::SD), (3, Slot::USB)],
            "device order first, so a listing reads like the rig looks"
        );
        assert_eq!(peers.of(DeviceNumber::new(3).unwrap()).len(), 2);
    }

    #[test]
    fn a_media_query_for_a_slot_we_do_not_serve_gets_no_answer() {
        // An empty reply would tell the deck the slot exists and holds no
        // tracks, and it would then offer an empty medium (F24).
        let ours = DeviceNumber::new(3).unwrap();
        let query = status::MediaQuery {
            requester: DeviceNumber::new(2).unwrap(),
            requester_ip: Ipv4Addr::new(169, 254, 1, 2),
            target: ours,
            slot: Slot::SD,
        };
        assert!(answer_media_query(query, ours, DeviceName::default(), &NoMedia).is_none());
    }

    #[test]
    fn a_media_query_addressed_elsewhere_gets_no_answer() {
        let ours = DeviceNumber::new(3).unwrap();
        let query = status::MediaQuery {
            requester: DeviceNumber::new(2).unwrap(),
            requester_ip: Ipv4Addr::new(169, 254, 1, 2),
            target: DeviceNumber::new(4).unwrap(),
            slot: Slot::USB,
        };
        assert!(answer_media_query(query, ours, DeviceName::default(), &NoMedia).is_none());
    }
}
