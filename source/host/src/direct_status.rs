//! Bounded, explicit unicast observer experiment; never sends load/transport commands.
use axum::{Json, http::StatusCode};
use prolink_proto::{DeviceKind, DeviceName, MacAddress, djl, status};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::Ipv4Addr, time::Duration};
use tokio::{net::UdpSocket, time::Instant};

static ACTIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[derive(Deserialize)]
pub struct Request {
    ip: String,
}

pub async fn run(Json(request): Json<Request>) -> Result<Json<Value>, (StatusCode, String)> {
    let ip =
        super::direct_ip::validate_ip(&request.ip).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let _guard = ACTIVE
        .try_lock()
        .map_err(|_| (StatusCode::CONFLICT, "Status test already running".into()))?;
    probe(ip)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))
}

pub(crate) fn announcement(ip: Ipv4Addr) -> Vec<u8> {
    // Locally administered synthetic identity, not a claim to the hardware MAC.
    let octets = ip.octets();
    djl::Packet::new(
        DeviceName::new("OLC"),
        DeviceKind::CDJ,
        djl::Body::KeepAlive {
            device_number: 15,
            was_first_on_network: 1,
            mac: MacAddress([2, 80, octets[0], octets[1], octets[2], octets[3]]),
            ip,
            peer_count: 2,
            pad_31: [0; 3],
            flags: DeviceKind::CDJ.role(),
            trailing: 0,
        },
    )
    .encode()
}

pub(crate) fn registration(ip: Ipv4Addr) -> Vec<Vec<u8>> {
    let o = ip.octets();
    let mac = MacAddress([2, 80, o[0], o[1], o[2], o[3]]);
    let role = DeviceKind::CDJ.role();
    let mut bodies = vec![djl::Body::Hello { payload: role }; 3];
    for iteration in 1..=3 {
        bodies.push(djl::Body::ClaimMac {
            iteration,
            flags: role,
            mac,
        });
    }
    for iteration in 1..=3 {
        bodies.push(djl::Body::ClaimIp {
            ip,
            mac,
            device_number: 15,
            iteration,
            role,
            assignment_mode: djl::AssignmentMode::MANUAL,
        });
    }
    bodies.push(djl::Body::ClaimNumber {
        device_number: 15,
        iteration: 1,
    });
    bodies
        .into_iter()
        .map(|body| djl::Packet::new(DeviceName::new("OLC"), DeviceKind::CDJ, body).encode())
        .collect()
}

fn record_reply(
    replies: &mut Vec<Value>,
    port: u16,
    from: std::net::SocketAddr,
    bytes: &[u8],
    detail: String,
) {
    if replies.len() < 24 {
        replies.push(json!({"receivedOn":port,"from":from.to_string(),"bytes":bytes.len(),"kind":bytes.get(10),"detail":detail}));
    }
}

async fn probe(ip: Ipv4Addr) -> Result<Value, String> {
    let route = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    route
        .connect((ip, 50000))
        .await
        .map_err(|e| e.to_string())?;
    let std::net::IpAddr::V4(local) = route.local_addr().map_err(|e| e.to_string())?.ip() else {
        return Err("No IPv4 route".into());
    };
    drop(route);
    // Exclusive binds: do not steal status packets from another running session.
    let discovery = UdpSocket::bind((local, 50000))
        .await
        .map_err(|e| format!("Cannot bind announcement port: {e}"))?;
    let receiver = UdpSocket::bind((local, 50002))
        .await
        .map_err(|e| format!("Cannot bind status port: {e}"))?;
    let packet = announcement(local);
    let deadline = Instant::now() + Duration::from_secs(20);
    let stages = registration(local);
    let mut sent = 0usize;
    let mut next_send = Instant::now();
    let mut discovery_buffer = [0u8; 2048];
    let mut discovery_datagrams = 0u32;
    let mut status_datagrams = 0u32;
    let mut rejected = 0u32;
    let mut decode_errors = 0u32;
    let mut replies = Vec::new();
    let mut buffer = [0u8; 2048];
    let mut count = 0u32;
    let mut samples = Vec::new();
    let mut previous = None;
    let mut last = None;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = tokio::time::sleep_until(next_send) => {
                let payload = stages.get(sent).unwrap_or(&packet);
                discovery.send_to(payload,(ip,50000)).await.map_err(|e|format!("Direct registration failed: {e}"))?;
                sent += 1;
                next_send = Instant::now() + if sent <= stages.len() { Duration::from_millis(300) } else { Duration::from_millis(1500) };
            }
            result = discovery.recv_from(&mut discovery_buffer) => {
                let (length,from) = result.map_err(|e|e.to_string())?;
                discovery_datagrams += 1;
                let decoded = djl::Packet::decode(&discovery_buffer[..length]);
                let detail = match &decoded { Ok(p) => format!("{:?}",p.body), Err(e) => format!("Decode error: {e}") };
                record_reply(&mut replies,50000,from,&discovery_buffer[..length],detail);
                if from.ip() == std::net::IpAddr::V4(ip) && let Ok(p) = decoded
                    && matches!(p.body, djl::Body::NumberInUse { device_number:15,.. } | djl::Body::KeepAlive {device_number:15,..}) {
                    return Err("Player reported observer number 15 in use. Test stopped; close other observer apps.".into());
                }
            }
            result = receiver.recv_from(&mut buffer) => {
                let (length,from) = result.map_err(|e|e.to_string())?;
                status_datagrams += 1;
                let decoded = status::decode(&buffer[..length]);
                let detail = match &decoded { Ok(p) => format!("{:?}",p.kind()), Err(e) => format!("Decode error: {e}") };
                record_reply(&mut replies,50002,from,&buffer[..length],detail);
                if from.ip() != std::net::IpAddr::V4(ip) {rejected += 1; continue;}
                if decoded.is_err() {decode_errors += 1;}
                if let Ok(status::Packet::CdjStatus(s)) = decoded {
                    count += 1;
                    last = Some(Instant::now());
                    let key = (s.track_id(),s.play_state());
                    if previous != Some(key) && samples.len() < 40 {
                        samples.push(json!({"player":s.sender().map(|n|n.get()),"trackId":s.track_id(),"playState":s.play_state(),"playing":s.flags().map(|f|f.is_playing()),"bpm":s.effective_bpm(),"beat":s.beat_number()}));
                        previous = Some(key);
                    }
                }
            }
        }
    }
    let age = last.map(|t| t.elapsed().as_millis());
    Ok(
        json!({"version":2,"sent":sent,"discoveryDatagrams":discovery_datagrams,"statusDatagrams":status_datagrams,"otherSourceDatagrams":rejected,"statusDecodeErrors":decode_errors,"replies":replies,"ok":count >= 3 && age.is_some_and(|a|a<2000),"packets":count,"lastStatusAgeMs":age,"sourceIp":local.to_string(),"ip":ip.to_string(),"samples":samples,"note":"20-second unicast registration sequence finished. Synthetic MAC; observer 15. No load or playback commands sent. Beat broadcasts were not tested."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_uses_observer_number_and_complete_sequence() {
        let packets = registration(Ipv4Addr::new(10, 80, 1, 62));
        assert_eq!(packets.len(), 10);
        for (i, bytes) in packets.iter().enumerate() {
            let p = djl::Packet::decode(bytes).unwrap();
            match (i, p.body) {
                (0..=2, djl::Body::Hello { .. })
                | (3..=5, djl::Body::ClaimMac { .. })
                | (
                    6..=8,
                    djl::Body::ClaimIp {
                        device_number: 15, ..
                    },
                )
                | (
                    9,
                    djl::Body::ClaimNumber {
                        device_number: 15, ..
                    },
                ) => {}
                _ => panic!("Unexpected registration stage {i}"),
            }
        }
    }
    #[test]
    fn observer_packet_has_expected_identity() {
        let bytes = announcement(Ipv4Addr::new(10, 80, 1, 172));
        let packet = djl::Packet::decode(&bytes).unwrap();
        assert!(
            matches!(packet.body, djl::Body::KeepAlive { device_number:15, ip, .. } if ip == Ipv4Addr::new(10,80,1,172))
        );
    }
}
