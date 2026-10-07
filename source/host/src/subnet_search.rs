//! Bounded unicast discovery; never claims a player number or controls playback.
use axum::{
    Json,
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use futures_util::{StreamExt, stream};
use network_interface::{NetworkInterface, NetworkInterfaceConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    convert::Infallible,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, UdpSocket},
    sync::Semaphore,
};

static SEARCH: std::sync::LazyLock<Arc<Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(Semaphore::new(1)));
#[derive(Clone, Serialize)]
pub struct Network {
    id: String,
    name: String,
    ip: Ipv4Addr,
    cidr: String,
    hosts: u32,
}
#[derive(Deserialize)]
pub struct Request {
    network: String,
}
fn range(ip: Ipv4Addr, mask: Ipv4Addr) -> Option<(u32, u32)> {
    let mask = u32::from(mask);
    let prefix = mask.leading_ones();
    if !(16..=30).contains(&prefix) || mask != u32::MAX << (32 - prefix) {
        return None;
    }
    let network = u32::from(ip) & mask;
    Some((network + 1, (network | !mask) - 1))
}
fn networks() -> Result<Vec<(Network, u32, u32)>, String> {
    let mut result = Vec::new();
    for interface in NetworkInterface::show().map_err(|e| e.to_string())? {
        for address in interface.addr {
            let (IpAddr::V4(ip), Some(IpAddr::V4(mask))) = (address.ip(), address.netmask()) else {
                continue;
            };
            if !(ip.is_private() || ip.is_link_local()) {
                continue;
            }
            let Some((first, last)) = range(ip, mask) else {
                continue;
            };
            result.push((
                Network {
                    id: format!("{}@{}", interface.name, ip),
                    name: interface.name.clone(),
                    ip,
                    cidr: format!(
                        "{}/{}",
                        Ipv4Addr::from(first - 1),
                        u32::from(mask).leading_ones()
                    ),
                    hosts: last - first,
                },
                first,
                last,
            ));
        }
    }
    Ok(result)
}
pub async fn list() -> Result<Json<Vec<Network>>, (StatusCode, String)> {
    networks()
        .map(|n| Json(n.into_iter().map(|(n, _, _)| n).collect()))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}
fn rpc_port(bytes: &[u8], xid: u32) -> Option<u32> {
    let word = |i: usize| {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
    };
    if word(0)? != xid || word(4)? != 1 || word(8)? != 0 {
        return None;
    }
    let length = usize::try_from(word(16)?).ok()?;
    let offset = 20usize.checked_add(length.checked_add(3)? & !3)?;
    if word(offset)? != 0 {
        return None;
    }
    let port = word(offset + 4)?;
    (port > 0 && port <= 65535).then_some(port)
}
async fn probe(local: Ipv4Addr, ip: Ipv4Addr) -> Option<&'static str> {
    probe_ports(local, ip, prolink_proto::dbserver::PORT_QUERY_PORT, 111).await
}
async fn probe_ports(
    local: Ipv4Addr,
    ip: Ipv4Addr,
    db_port: u16,
    rpc_port_number: u16,
) -> Option<&'static str> {
    let db = async {
        let socket = TcpSocket::new_v4().ok()?;
        socket.bind((local, 0).into()).ok()?;
        let mut socket = socket.connect((ip, db_port).into()).await.ok()?;
        socket
            .write_all(&prolink_proto::dbserver::PORT_QUERY)
            .await
            .ok()?;
        let mut answer = [0; 2];
        socket.read_exact(&mut answer).await.ok()?;
        let port = u16::from_be_bytes(answer);
        (port != 0 && port != u16::MAX).then_some("Pro DJ Link database service")
    };
    let nfs = async {
        let socket = UdpSocket::bind((local, 0)).await.ok()?;
        socket.connect((ip, rpc_port_number)).await.ok()?;
        let xid = u32::from(ip) ^ 0x50430000;
        let packet: Vec<u8> = [xid, 0, 2, 100000, 2, 3, 0, 0, 0, 0, 100003, 2, 17, 0]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect();
        let mut answer = [0; 256];
        for _ in 0..2 {
            socket.send(&packet).await.ok()?;
            if let Ok(Ok(n)) =
                tokio::time::timeout(Duration::from_millis(650), socket.recv(&mut answer)).await
                && rpc_port(&answer[..n], xid).is_some()
            {
                return Some("NFS service · unverified CDJ candidate");
            }
        }
        None
    };
    let (db, nfs) = tokio::join!(tokio::time::timeout(Duration::from_millis(1500), db), nfs);
    db.ok().flatten().or(nfs)
}
pub async fn search(Json(request): Json<Request>) -> Result<Response, (StatusCode, String)> {
    let (network, first, last) = networks()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .into_iter()
        .find(|(n, _, _)| n.id == request.network)
        .ok_or((
            StatusCode::BAD_REQUEST,
            "Select a current local IPv4 network (/16 to /30).".into(),
        ))?;
    let guard = SEARCH.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "A network search is already running.".into(),
        )
    })?;
    let local = network.ip;
    let total = network.hosts;
    let probes = stream::iter(
        (first..=last)
            .map(Ipv4Addr::from)
            .filter(move |ip| *ip != local),
    )
    .map(move |ip| async move { (ip, probe(local, ip).await) })
    .buffer_unordered(32);
    // The guard and sockets belong to the response stream; disconnect/cancel drops them.
    let events = probes.enumerate().map(move |(index, (ip, service))| {
        let _hold = &guard;
        Ok::<_, Infallible>(format!(
            "{}\n",
            json!({"completed":index+1,"total":total,"ip":ip,"service":service})
        ))
    });
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(events))
        .unwrap())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_subnet_excludes_network_and_broadcast() {
        let (first, last) = range(
            "10.80.1.172".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(Ipv4Addr::from(first).to_string(), "10.80.1.1");
        assert_eq!(Ipv4Addr::from(last).to_string(), "10.80.1.254");
        assert!(range(Ipv4Addr::LOCALHOST, "255.0.255.0".parse().unwrap()).is_none());
        assert!(range(Ipv4Addr::LOCALHOST, "255.0.0.0".parse().unwrap()).is_none());
    }
    #[test]
    fn rpc_requires_matching_success_and_valid_port() {
        let reply = |words: Vec<u32>| {
            words
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect::<Vec<_>>()
        };
        let valid = reply(vec![42, 1, 0, 0, 0, 0, 2049]);
        assert_eq!(rpc_port(&valid, 42), Some(2049));
        assert_eq!(rpc_port(&valid, 43), None);
        assert_eq!(rpc_port(&valid[..20], 42), None);
        assert_eq!(rpc_port(&reply(vec![42, 1, 0, 0, 0, 0, 0]), 42), None);
        assert_eq!(rpc_port(&reply(vec![42, 1, 0, 0, 0, 1, 2049]), 42), None);
    }
    #[tokio::test]
    async fn discovers_database_service_without_observer_or_usb() {
        let local = Ipv4Addr::LOCALHOST;
        let listener = tokio::net::TcpListener::bind((local, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let rpc = UdpSocket::bind((local, 0)).await.unwrap();
        let rpc_port = rpc.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut query = [0; 19];
            socket.read_exact(&mut query).await.unwrap();
            assert_eq!(query, prolink_proto::dbserver::PORT_QUERY);
            socket.write_all(&1051u16.to_be_bytes()).await.unwrap();
        });
        assert_eq!(
            probe_ports(local, local, port, rpc_port).await,
            Some("Pro DJ Link database service")
        );
        server.await.unwrap();
    }
    #[tokio::test]
    async fn nfs_alone_is_only_a_candidate() {
        let local = Ipv4Addr::LOCALHOST;
        let listener = tokio::net::TcpListener::bind((local, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let rpc = UdpSocket::bind((local, 0)).await.unwrap();
        let rpc_port = rpc.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut query = [0; 256];
            let (_, from) = rpc.recv_from(&mut query).await.unwrap();
            let xid = u32::from_be_bytes(query[..4].try_into().unwrap());
            let reply: Vec<u8> = [xid, 1, 0, 0, 0, 0, 2049]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect();
            rpc.send_to(&reply, from).await.unwrap();
        });
        assert_eq!(
            probe_ports(local, local, port, rpc_port).await,
            Some("NFS service · unverified CDJ candidate")
        );
        server.await.unwrap();
    }
}
