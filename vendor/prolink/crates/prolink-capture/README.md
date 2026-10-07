# prolink-capture

Read Pro DJ Link traffic out of pcap and pcapng captures: per-datagram UDP
payloads and reassembled TCP streams, so codecs can be tested against real
hardware traffic without a network.

Ethernet over IPv4, UDP and TCP, with IP fragment reassembly — the whole of
what a DJ Link capture contains, and nothing speculative beyond it.

```rust,no_run
use prolink_capture::Capture;

for packet in Capture::open("run.pcap")?.udp_to(50002) {
    let packet = packet?;
    println!("{} sent {} bytes", packet.source, packet.payload.len());
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

Two things it is careful about, because both fail silently:

- **Filter UDP on the destination port.** The type byte at `0x0a` is shared
  across ports and the layouts behind it are not — `0x06` is a keep-alive on
  50000 and a media response on 50002 — so a packet merely *sent from* 50002
  decodes into confident nonsense. TCP is the other way round: a connection has
  two ends and both directions belong to the server's port.
- **A TCP stream with a hole in it is reported, never concatenated over.** The
  dbserver protocol has no length framing, so closing a gap up desynchronises
  every message after it with no error to show for it. `Stream::contiguous()`
  returns `None` rather than bytes nobody sent.

Part of [prolink](https://github.com/usr-ein/prolink).
