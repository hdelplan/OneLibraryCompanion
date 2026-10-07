// SPDX-License-Identifier: GPL-3.0-only

//! Listening on UDP 50000: who is on the network.
//!
//! Entirely passive. Starting a [`Discovery`] transmits nothing, which is what
//! makes it safe to run next to a live rig: it cannot contend for a device
//! number and cannot disturb anything. Announcing is a separate, explicit step
//! — see [`crate::virtual_cdj`].
//!
//! # What passive listening can and cannot tell you
//!
//! Keep-alives are broadcast, so a passive listener sees every device, its
//! number, its name, its address and its hardware address. That is enough to
//! reach a player's NFS export and pull its database.
//!
//! It is **not** enough to know what is in a player's slots, or who holds tempo
//! master. Both are published only in UDP-50002 status packets, and those are
//! unicast to peers that have announced themselves: 1507 status packets in one
//! session all went deck-to-deck, and not one reached a host that had been on
//! the network the whole time without announcing (F21).

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use prolink_proto::{DISCOVERY_PORT, MacAddress, djl};
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use crate::device::{Device, DeviceEvent, DeviceTable};
use crate::interface::Interface;
use crate::socket::{self, MAX_DATAGRAM};
use crate::{Error, Result};

/// How many events a slow subscriber may fall behind before it starts missing
/// them. Device churn is rare, so this is generous.
const EVENT_CAPACITY: usize = 256;

/// How often silent devices are checked for staleness.
const REAP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// One decoded discovery datagram and where it came from.
#[derive(Clone, Debug)]
pub struct Announcement {
    /// The decoded packet.
    pub packet: djl::Packet,
    /// The address it arrived from. Not always the address the packet claims —
    /// which is the point of keeping both.
    pub from: Ipv4Addr,
}

/// A passive listener on UDP 50000.
///
/// Dropping it stops the listener.
#[derive(Debug)]
pub struct Discovery {
    interface: Interface,
    socket: Arc<UdpSocket>,
    table: Arc<Mutex<DeviceTable>>,
    events: broadcast::Sender<DeviceEvent>,
    announcements: broadcast::Sender<Announcement>,
    tasks: Vec<JoinHandle<()>>,
}

impl Discovery {
    /// Start listening on `interface`.
    ///
    /// Transmits nothing.
    #[expect(
        clippy::unused_async,
        reason = "spawns tasks, so it needs a tokio runtime; async is how that is documented \
                  at the call site, and keeps the signature stable if setup later awaits"
    )]
    pub async fn start(interface: Interface) -> Result<Self> {
        let socket = Arc::new(socket::bind(DISCOVERY_PORT, Some(&interface))?);
        let table = Arc::new(Mutex::new(DeviceTable::new()));
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let (announcements, _) = broadcast::channel(EVENT_CAPACITY);

        let mut discovery = Self {
            interface,
            socket,
            table,
            events,
            announcements,
            tasks: Vec::new(),
        };
        discovery.tasks.push(discovery.spawn_receiver());
        discovery.tasks.push(discovery.spawn_reaper());
        Ok(discovery)
    }

    /// The interface being listened on.
    pub fn interface(&self) -> &Interface {
        &self.interface
    }

    /// The socket, shared so that a virtual CDJ can transmit from it.
    ///
    /// Replies to a device-number claim — conflicts, and mixer assignments —
    /// are unicast **to port 50000**, so an announcer must send from the port it
    /// is listening on. A second socket would mean never hearing the answer.
    pub fn socket(&self) -> Arc<UdpSocket> {
        Arc::clone(&self.socket)
    }

    /// Every device currently known, ordered by number.
    pub fn devices(&self) -> Vec<Device> {
        self.with_table(DeviceTable::all)
    }

    /// Devices currently answering.
    pub fn online(&self) -> Vec<Device> {
        self.with_table(DeviceTable::online)
    }

    /// Every device number ever observed, including from devices since gone.
    pub fn numbers_seen(&self) -> std::collections::BTreeSet<u8> {
        self.with_table(|table| table.numbers_seen().clone())
    }

    /// Whether nothing else was on the network when this was asked.
    ///
    /// A real deck latches this at boot and never re-evaluates it: it drives
    /// both the stage-3 claim repeat count and keep-alive byte `0x25`, and one
    /// deck held `0x02` while its peer count went 1→2 (F9).
    pub fn is_alone(&self) -> bool {
        self.with_table(DeviceTable::is_empty)
    }

    /// Stop treating packets from this address as coming from a peer.
    ///
    /// We bind `0.0.0.0` and so receive our own broadcasts.
    pub fn ignore(&self, mac: MacAddress) {
        self.with_table_mut(|table| table.ignore(mac));
    }

    /// Subscribe to changes in the device table.
    pub fn subscribe(&self) -> broadcast::Receiver<DeviceEvent> {
        self.events.subscribe()
    }

    /// Subscribe to every decoded discovery datagram, including the ones that
    /// do not change the table.
    ///
    /// This is what a virtual CDJ watches for conflict packets aimed at the
    /// number it is claiming.
    pub fn announcements(&self) -> broadcast::Receiver<Announcement> {
        self.announcements.subscribe()
    }

    fn with_table<T>(&self, read: impl FnOnce(&DeviceTable) -> T) -> T {
        match self.table.lock() {
            Ok(table) => read(&table),
            // The table holds no invariants that a panic mid-update could break
            // — every mutation is a single assignment — so recovering is
            // strictly better than propagating a panic into a DJ's set.
            Err(poisoned) => read(&poisoned.into_inner()),
        }
    }

    fn with_table_mut<T>(&self, write: impl FnOnce(&mut DeviceTable) -> T) -> T {
        match self.table.lock() {
            Ok(mut table) => write(&mut table),
            Err(poisoned) => write(&mut poisoned.into_inner()),
        }
    }

    fn spawn_receiver(&self) -> JoinHandle<()> {
        let socket = Arc::clone(&self.socket);
        let table = Arc::clone(&self.table);
        let events = self.events.clone();
        let announcements = self.announcements.clone();

        tokio::spawn(async move {
            let mut buffer = vec![0u8; MAX_DATAGRAM];
            loop {
                let (len, from) = match socket.recv_from(&mut buffer).await {
                    Ok(received) => received,
                    Err(error) => {
                        warn!(%error, "discovery socket closed");
                        return;
                    }
                };
                let SocketAddr::V4(from) = from else { continue };
                let Some(datagram) = buffer.get(..len) else {
                    continue;
                };

                let packet = match djl::Packet::decode(datagram) {
                    Ok(packet) => packet,
                    Err(error) => {
                        // Something else on port 50000, or a form we do not
                        // model. Not worth a warning at every keep-alive.
                        trace!(%error, bytes = len, "undecodable datagram on 50000");
                        continue;
                    }
                };

                let event = match table.lock() {
                    Ok(mut table) => table.observe(&packet, Instant::now()),
                    Err(poisoned) => poisoned.into_inner().observe(&packet, Instant::now()),
                };
                if let Some(event) = event {
                    debug!(?event, "device table changed");
                    // A send with no subscribers is not an error here.
                    let _ = events.send(event);
                }
                let _ = announcements.send(Announcement {
                    packet,
                    from: *from.ip(),
                });
            }
        })
    }

    fn spawn_reaper(&self) -> JoinHandle<()> {
        let table = Arc::clone(&self.table);
        let events = self.events.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(REAP_INTERVAL);
            loop {
                ticker.tick().await;
                let expired = match table.lock() {
                    Ok(mut table) => table.reap(Instant::now()),
                    Err(poisoned) => poisoned.into_inner().reap(Instant::now()),
                };
                for event in expired {
                    debug!(?event, "device timed out");
                    let _ = events.send(event);
                }
            }
        })
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Watch the network for `duration` and return what was seen.
///
/// A convenience for the common one-shot case. Transmits nothing.
pub async fn scan(interface: Interface, duration: std::time::Duration) -> Result<Vec<Device>> {
    let discovery = Discovery::start(interface).await?;
    tokio::time::sleep(duration).await;
    Ok(discovery.devices())
}

/// How long to watch before a device that is present can be assumed to have
/// been seen.
///
/// A CDJ broadcasts every 2.0 s, so anything longer than that catches a settled
/// device; the margin covers one dropped packet.
pub const SCAN_DURATION: std::time::Duration = std::time::Duration::from_millis(4500);

impl Discovery {
    /// The error to raise when nothing browsable is free, listing what is taken.
    pub(crate) fn no_browsable_number(&self) -> Error {
        Error::NoBrowsableNumber {
            taken: self.numbers_seen().into_iter().collect(),
        }
    }
}
