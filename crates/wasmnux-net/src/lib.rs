//! User-mode virtual Ethernet switch (`10.0.2.0/24`) supporting ARP, ICMPv4, DHCPv4,
//! UDP DNS (`10.0.2.2:53`), and stateful windowed TCPv4 HTTP bridging for guest Linux & `apt`.

use std::collections::{BTreeMap, VecDeque};

/// Virtual gateway MAC (`52:55:0a:00:02:02`).
pub const GATEWAY_MAC: [u8; 6] = [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02];

/// Default guest NIC MAC (`52:54:00:12:34:56`).
pub const GUEST_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Virtual gateway IPv4 address (`10.0.2.2`).
pub const GATEWAY_IPV4: [u8; 4] = [10, 0, 2, 2];

/// Virtual DNS resolver IPv4 address (`10.0.2.3`).
pub const DNS_IPV4: [u8; 4] = [10, 0, 2, 3];

/// Default guest IPv4 address (`10.0.2.15`).
pub const GUEST_IPV4: [u8; 4] = [10, 0, 2, 15];

/// Subnet mask (`255.255.255.0`).
pub const SUBNET_MASK: [u8; 4] = [255, 255, 255, 0];

const ETHERTYPE_IPV4: u16 = 0x0800;
const ETHERTYPE_ARP: u16 = 0x0806;

const IPPROTO_ICMP: u8 = 1;
const IPPROTO_TCP: u8 = 6;
const IPPROTO_UDP: u8 = 17;

const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PSH: u8 = 0x08;
const TCP_ACK: u8 = 0x10;

const TCP_MSS: usize = 1400;
const MAX_IN_FLIGHT_BYTES: usize = 16 * TCP_MSS;
const MAX_HTTP_REQ_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_REQ_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Compute RFC 1071 16-bit ones'-complement Internet checksum.
pub fn internet_checksum(data: &[u8]) -> u16 {
    internet_checksum_parts(&[data])
}

pub fn internet_checksum_parts(parts: &[&[u8]]) -> u16 {
    let mut sum: u32 = 0;
    let mut carry_byte: Option<u8> = None;

    for slice in parts {
        let mut idx = 0usize;
        if let Some(prev) = carry_byte.take() {
            if !slice.is_empty() {
                sum += u32::from(u16::from_be_bytes([prev, slice[0]]));
                idx = 1;
            } else {
                carry_byte = Some(prev);
            }
        }
        while idx + 1 < slice.len() {
            sum += u32::from(u16::from_be_bytes([slice[idx], slice[idx + 1]]));
            idx += 2;
        }
        if idx < slice.len() {
            carry_byte = Some(slice[idx]);
        }
    }
    if let Some(rem) = carry_byte {
        sum += u32::from(u16::from_be_bytes([rem, 0]));
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn transport_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], proto: u8, payload: &[u8]) -> u16 {
    let len_be = (payload.len() as u16).to_be_bytes();
    let pseudo = [
        src_ip[0], src_ip[1], src_ip[2], src_ip[3], dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3], 0,
        proto, len_be[0], len_be[1],
    ];
    let csum = internet_checksum_parts(&[&pseudo, payload]);
    if proto == IPPROTO_UDP && csum == 0 {
        0xFFFF
    } else {
        csum
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeHttpRequest {
    pub request_id: u64,
    pub method: String,
    pub host: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BridgeHttpResponse {
    pub request_id: u64,
    pub status_code: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TcpKey {
    guest_ip: [u8; 4],
    guest_port: u16,
    server_ip: [u8; 4],
    server_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TcpState {
    SynReceived,
    Established,
    FinWait,
    Closed,
}

#[derive(Debug, Clone)]
struct TcpConn {
    state: TcpState,
    guest_mac: [u8; 6],
    /// Next sequence number expected from guest.
    rcv_nxt: u32,
    /// Initial / unacknowledged server sequence number.
    snd_una: u32,
    /// Next sequence number to send from server.
    snd_nxt: u32,
    /// Base sequence number corresponding to `tx_buf[0]`.
    tx_base_seq: u32,
    /// Guest advertised receive window.
    peer_window: u32,
    /// Reassembled guest HTTP request bytes.
    rx_buf: Vec<u8>,
    http_dispatched: bool,
    /// Serialized HTTP response bytes queued to stream to guest.
    tx_buf: Vec<u8>,
    response_ready: bool,
    fin_sent: bool,
    fin_seq: u32,
    duplicate_acks: u8,
}

#[derive(Debug, Clone, Default)]
pub struct NetStats {
    pub tx_frames_from_guest: u64,
    pub rx_frames_to_guest: u64,
    pub tx_bytes_from_guest: u64,
    pub rx_bytes_to_guest: u64,
    pub http_requests: u64,
}

/// Pure-Rust user-mode virtual switch connecting guest `VirtioNet` to the browser bridge.
pub struct VirtualNetworkSwitch {
    enabled: bool,
    rx_queue_to_guest: VecDeque<Vec<u8>>,
    pending_http: VecDeque<BridgeHttpRequest>,
    http_conn_map: BTreeMap<u64, TcpKey>,
    tcp_conns: BTreeMap<TcpKey, TcpConn>,
    dns_ip_to_host: BTreeMap<[u8; 4], String>,
    dns_host_to_ip: BTreeMap<String, [u8; 4]>,
    next_dns_host_octet: u8,
    next_request_id: u64,
    ip_id_counter: u16,
    isn_counter: u32,
    stats: NetStats,
}

impl Default for VirtualNetworkSwitch {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualNetworkSwitch {
    pub fn new() -> Self {
        let mut sw = Self {
            enabled: true,
            rx_queue_to_guest: VecDeque::new(),
            pending_http: VecDeque::new(),
            http_conn_map: BTreeMap::new(),
            tcp_conns: BTreeMap::new(),
            dns_ip_to_host: BTreeMap::new(),
            dns_host_to_ip: BTreeMap::new(),
            next_dns_host_octet: 10,
            next_request_id: 1,
            ip_id_counter: 1,
            isn_counter: 0x1020_3040,
            stats: NetStats::default(),
        };
        sw.register_static_dns("apt.wasmnux.local", GATEWAY_IPV4);
        sw.register_static_dns("gateway.wasmnux.local", GATEWAY_IPV4);
        sw
    }

    pub fn register_static_dns(&mut self, hostname: &str, ip: [u8; 4]) {
        let lower = hostname.to_ascii_lowercase();
        self.dns_host_to_ip.insert(lower.clone(), ip);
        self.dns_ip_to_host.insert(ip, lower);
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn stats(&self) -> &NetStats {
        &self.stats
    }

    /// Pop the next Ethernet frame destined for the guest's `VirtioNet` RX queue.
    pub fn pop_frame_for_guest(&mut self) -> Option<Vec<u8>> {
        let frame = self.rx_queue_to_guest.pop_front()?;
        self.stats.rx_frames_to_guest = self.stats.rx_frames_to_guest.wrapping_add(1);
        self.stats.rx_bytes_to_guest = self
            .stats
            .rx_bytes_to_guest
            .wrapping_add(frame.len() as u64);
        Some(frame)
    }

    pub fn has_frames_for_guest(&self) -> bool {
        !self.rx_queue_to_guest.is_empty()
    }

    /// Pop the next HTTP request initiated by a guest TCP socket for the browser Fetch bridge.
    pub fn pop_http_request(&mut self) -> Option<BridgeHttpRequest> {
        self.pending_http.pop_front()
    }

    pub fn has_pending_http_requests(&self) -> bool {
        !self.pending_http.is_empty()
    }

    /// Complete an HTTP request from the browser Fetch bridge, streaming TCP segments back to guest.
    pub fn complete_http_response(&mut self, resp: BridgeHttpResponse) {
        let Some(key) = self.http_conn_map.remove(&resp.request_id) else {
            return;
        };
        if !self.tcp_conns.contains_key(&key) {
            return;
        }

        let mut raw = Vec::with_capacity(256 + resp.body.len());
        let reason = if resp.status_text.is_empty() {
            "OK"
        } else {
            resp.status_text.as_str()
        };
        raw.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", resp.status_code, reason).as_bytes());

        let mut has_content_len = false;
        let mut has_conn = false;
        for (k, v) in &resp.headers {
            let lk = k.to_ascii_lowercase();
            if lk == "transfer-encoding" {
                continue;
            }
            if lk == "content-length" {
                has_content_len = true;
            }
            if lk == "connection" {
                has_conn = true;
            }
            raw.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
        if !has_content_len {
            raw.extend_from_slice(format!("Content-Length: {}\r\n", resp.body.len()).as_bytes());
        }
        if !has_conn {
            raw.extend_from_slice(b"Connection: close\r\n");
        }
        raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(&resp.body);

        if let Some(conn) = self.tcp_conns.get_mut(&key) {
            conn.tx_buf = raw;
            conn.response_ready = true;
        }
        self.pump_tcp_tx(key);
    }

    /// Process an outbound Ethernet II frame transmitted by the guest's `VirtioNet` TX queue.
    pub fn receive_frame_from_guest(&mut self, frame: &[u8]) {
        if !self.enabled || frame.len() < 14 {
            return;
        }
        self.stats.tx_frames_from_guest = self.stats.tx_frames_from_guest.wrapping_add(1);
        self.stats.tx_bytes_from_guest = self
            .stats
            .tx_bytes_from_guest
            .wrapping_add(frame.len() as u64);

        let mut src_mac = [0u8; 6];
        src_mac.copy_from_slice(&frame[6..12]);
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        let payload = &frame[14..];

        match ethertype {
            ETHERTYPE_ARP => self.handle_arp(src_mac, payload),
            ETHERTYPE_IPV4 => self.handle_ipv4(src_mac, payload),
            _ => {}
        }
    }

    fn handle_arp(&mut self, guest_mac: [u8; 6], arp: &[u8]) {
        if arp.len() < 28 {
            return;
        }
        let htype = u16::from_be_bytes([arp[0], arp[1]]);
        let ptype = u16::from_be_bytes([arp[2], arp[3]]);
        let oper = u16::from_be_bytes([arp[6], arp[7]]);
        if htype != 1 || ptype != ETHERTYPE_IPV4 || oper != 1 {
            return;
        }
        let mut sender_ip = [0u8; 4];
        sender_ip.copy_from_slice(&arp[14..18]);
        let mut target_ip = [0u8; 4];
        target_ip.copy_from_slice(&arp[24..28]);

        // Answer ARP for gateway `10.0.2.2`, DNS `10.0.2.3`, and virtual DNS-mapped `10.0.3.x` hosts.
        if target_ip != GATEWAY_IPV4
            && target_ip != DNS_IPV4
            && !(target_ip[0] == 10 && target_ip[1] == 0 && target_ip[2] == 3)
        {
            return;
        }

        let mut reply = Vec::with_capacity(42);
        reply.extend_from_slice(&guest_mac);
        reply.extend_from_slice(&GATEWAY_MAC);
        reply.extend_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        reply.extend_from_slice(&1u16.to_be_bytes()); // HTYPE Ethernet
        reply.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes()); // PTYPE IPv4
        reply.push(6); // HLEN
        reply.push(4); // PLEN
        reply.extend_from_slice(&2u16.to_be_bytes()); // OPER Reply
        reply.extend_from_slice(&GATEWAY_MAC);
        reply.extend_from_slice(&target_ip);
        reply.extend_from_slice(&guest_mac);
        reply.extend_from_slice(&sender_ip);
        self.rx_queue_to_guest.push_back(reply);
    }

    fn handle_ipv4(&mut self, guest_mac: [u8; 6], pkt: &[u8]) {
        if pkt.len() < 20 {
            return;
        }
        let version = pkt[0] >> 4;
        let ihl = ((pkt[0] & 0x0F) as usize) * 4;
        if version != 4 || ihl < 20 || pkt.len() < ihl {
            return;
        }
        let total_len = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
        if total_len < ihl || total_len > pkt.len() {
            return;
        }
        let proto = pkt[9];
        let mut src_ip = [0u8; 4];
        src_ip.copy_from_slice(&pkt[12..16]);
        let mut dst_ip = [0u8; 4];
        dst_ip.copy_from_slice(&pkt[16..20]);
        let l4 = &pkt[ihl..total_len];

        match proto {
            IPPROTO_ICMP => self.handle_icmp(guest_mac, src_ip, dst_ip, l4),
            IPPROTO_UDP => self.handle_udp(guest_mac, src_ip, dst_ip, l4),
            IPPROTO_TCP => self.handle_tcp(guest_mac, src_ip, dst_ip, l4),
            _ => {}
        }
    }

    fn push_ipv4_frame(
        &mut self,
        dst_mac: [u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        proto: u8,
        l4_payload: &[u8],
    ) {
        let total_len = 20 + l4_payload.len();
        if total_len > 65535 {
            return;
        }
        let ip_id = self.ip_id_counter;
        self.ip_id_counter = self.ip_id_counter.wrapping_add(1);

        let mut ip_hdr = [0u8; 20];
        ip_hdr[0] = 0x45;
        ip_hdr[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        ip_hdr[4..6].copy_from_slice(&ip_id.to_be_bytes());
        ip_hdr[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF
        ip_hdr[8] = 64; // TTL
        ip_hdr[9] = proto;
        ip_hdr[12..16].copy_from_slice(&src_ip);
        ip_hdr[16..20].copy_from_slice(&dst_ip);
        let hdr_csum = internet_checksum(&ip_hdr);
        ip_hdr[10..12].copy_from_slice(&hdr_csum.to_be_bytes());

        let mut frame = Vec::with_capacity(14 + total_len);
        frame.extend_from_slice(&dst_mac);
        frame.extend_from_slice(&GATEWAY_MAC);
        frame.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        frame.extend_from_slice(&ip_hdr);
        frame.extend_from_slice(l4_payload);
        self.rx_queue_to_guest.push_back(frame);
    }

    fn handle_icmp(&mut self, guest_mac: [u8; 6], src_ip: [u8; 4], dst_ip: [u8; 4], icmp: &[u8]) {
        if icmp.len() < 8 || icmp[0] != 8 || icmp[1] != 0 {
            return;
        }
        let mut reply = icmp.to_vec();
        reply[0] = 0; // Echo Reply
        reply[2] = 0;
        reply[3] = 0;
        let csum = internet_checksum(&reply);
        reply[2..4].copy_from_slice(&csum.to_be_bytes());
        self.push_ipv4_frame(guest_mac, dst_ip, src_ip, IPPROTO_ICMP, &reply);
    }

    fn handle_udp(&mut self, guest_mac: [u8; 6], src_ip: [u8; 4], dst_ip: [u8; 4], udp: &[u8]) {
        if udp.len() < 8 {
            return;
        }
        let src_port = u16::from_be_bytes([udp[0], udp[1]]);
        let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
        let len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
        if len < 8 || len > udp.len() {
            return;
        }
        let data = &udp[8..len];

        if src_port == 68 && dst_port == 67 {
            self.handle_dhcp(guest_mac, data);
        } else if dst_port == 53 {
            self.handle_dns(guest_mac, src_ip, src_port, dst_ip, data);
        }
    }

    fn send_udp(
        &mut self,
        dst_mac: [u8; 6],
        src_ip: [u8; 4],
        src_port: u16,
        dst_ip: [u8; 4],
        dst_port: u16,
        data: &[u8],
    ) {
        let udp_len = 8 + data.len();
        let mut pkt = Vec::with_capacity(udp_len);
        pkt.extend_from_slice(&src_port.to_be_bytes());
        pkt.extend_from_slice(&dst_port.to_be_bytes());
        pkt.extend_from_slice(&(udp_len as u16).to_be_bytes());
        pkt.extend_from_slice(&[0, 0]);
        pkt.extend_from_slice(data);
        let csum = transport_checksum(src_ip, dst_ip, IPPROTO_UDP, &pkt);
        pkt[6..8].copy_from_slice(&csum.to_be_bytes());
        self.push_ipv4_frame(dst_mac, src_ip, dst_ip, IPPROTO_UDP, &pkt);
    }

    fn handle_dhcp(&mut self, guest_mac: [u8; 6], dhcp: &[u8]) {
        // BOOTP fixed header is 236 bytes + 4-byte DHCP magic cookie.
        if dhcp.len() < 240 || dhcp[0] != 1 {
            return;
        }
        if dhcp[236..240] != [99, 130, 83, 99] {
            return;
        }
        let xid = &dhcp[4..8];
        let flags = u16::from_be_bytes([dhcp[10], dhcp[11]]);
        let mut chaddr = [0u8; 16];
        chaddr.copy_from_slice(&dhcp[28..44]);

        let mut msg_type: Option<u8> = None;
        let mut idx = 240usize;
        while idx < dhcp.len() {
            let opt = dhcp[idx];
            if opt == 255 {
                break;
            }
            if opt == 0 {
                idx += 1;
                continue;
            }
            if idx + 1 >= dhcp.len() {
                break;
            }
            let olen = dhcp[idx + 1] as usize;
            if idx + 2 + olen > dhcp.len() {
                break;
            }
            if opt == 53 && olen == 1 {
                msg_type = Some(dhcp[idx + 2]);
            }
            idx += 2 + olen;
        }

        let reply_type = match msg_type {
            Some(1) => 2u8, // DISCOVER -> OFFER
            Some(3) => 5u8, // REQUEST  -> ACK
            _ => return,
        };

        let mut resp = vec![0u8; 240];
        resp[0] = 2; // BOOTREPLY
        resp[1] = 1; // Ethernet
        resp[2] = 6; // HLEN
        resp[4..8].copy_from_slice(xid);
        resp[10..12].copy_from_slice(&flags.to_be_bytes());
        resp[16..20].copy_from_slice(&GUEST_IPV4); // yiaddr
        resp[20..24].copy_from_slice(&GATEWAY_IPV4); // siaddr
        resp[28..44].copy_from_slice(&chaddr);
        resp[236..240].copy_from_slice(&[99, 130, 83, 99]);

        // Options: Message Type, Server ID, Lease Time (86400s), Subnet Mask, Router, DNS, End
        resp.extend_from_slice(&[53, 1, reply_type]);
        resp.extend_from_slice(&[54, 4]);
        resp.extend_from_slice(&GATEWAY_IPV4);
        resp.extend_from_slice(&[51, 4]);
        resp.extend_from_slice(&86_400u32.to_be_bytes());
        resp.extend_from_slice(&[1, 4]);
        resp.extend_from_slice(&SUBNET_MASK);
        resp.extend_from_slice(&[3, 4]);
        resp.extend_from_slice(&GATEWAY_IPV4);
        resp.extend_from_slice(&[6, 4]);
        resp.extend_from_slice(&GATEWAY_IPV4);
        resp.push(255);

        let dst_mac = if (flags & 0x8000) != 0 {
            [0xFF; 6]
        } else {
            guest_mac
        };
        let dst_ip = if (flags & 0x8000) != 0 {
            [255, 255, 255, 255]
        } else {
            GUEST_IPV4
        };
        self.send_udp(dst_mac, GATEWAY_IPV4, 67, dst_ip, 68, &resp);
    }

    fn resolve_or_assign_dns(&mut self, host: &str) -> [u8; 4] {
        let lower = host.trim_end_matches('.').to_ascii_lowercase();
        if let Some(&ip) = self.dns_host_to_ip.get(&lower) {
            return ip;
        }
        let ip = [10, 0, 3, self.next_dns_host_octet];
        self.next_dns_host_octet = if self.next_dns_host_octet >= 250 {
            10
        } else {
            self.next_dns_host_octet + 1
        };
        self.dns_host_to_ip.insert(lower.clone(), ip);
        self.dns_ip_to_host.insert(ip, lower);
        ip
    }

    fn handle_dns(
        &mut self,
        guest_mac: [u8; 6],
        guest_ip: [u8; 4],
        guest_port: u16,
        dns_server_ip: [u8; 4],
        dns: &[u8],
    ) {
        if dns.len() < 12 {
            return;
        }
        let txid = [dns[0], dns[1]];
        let flags = u16::from_be_bytes([dns[2], dns[3]]);
        let qdcount = u16::from_be_bytes([dns[4], dns[5]]);
        if (flags & 0x8000) != 0 || qdcount == 0 {
            return;
        }

        // Parse first QNAME.
        let mut pos = 12usize;
        let mut labels = Vec::new();
        while pos < dns.len() {
            let len = dns[pos] as usize;
            pos += 1;
            if len == 0 {
                break;
            }
            if (len & 0xC0) != 0 || pos + len > dns.len() {
                return;
            }
            if let Ok(lbl) = std::str::from_utf8(&dns[pos..pos + len]) {
                labels.push(lbl.to_owned());
            } else {
                return;
            }
            pos += len;
        }
        if pos + 4 > dns.len() {
            return;
        }
        let qtype = u16::from_be_bytes([dns[pos], dns[pos + 1]]);
        let qclass = u16::from_be_bytes([dns[pos + 2], dns[pos + 3]]);
        let question_slice = &dns[12..pos + 4];
        let qname = labels.join(".");

        let mut resp = Vec::with_capacity(64 + question_slice.len());
        resp.extend_from_slice(&txid);
        // QR=1, RD=copied, RA=1, RCODE=0 (NOERROR)
        let resp_flags: u16 = 0x8180 | (flags & 0x0100);
        resp.extend_from_slice(&resp_flags.to_be_bytes());
        resp.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1

        if qtype == 1 && qclass == 1 && !qname.is_empty() {
            // A record answer
            let ipv4 = self.resolve_or_assign_dns(&qname);
            resp.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT = 1
            resp.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
            resp.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
            resp.extend_from_slice(question_slice);
            resp.extend_from_slice(&0xC00Cu16.to_be_bytes()); // Name pointer to offset 12
            resp.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
            resp.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
            resp.extend_from_slice(&60u32.to_be_bytes()); // TTL 60s
            resp.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH 4
            resp.extend_from_slice(&ipv4);
        } else {
            // Empty NOERROR answer (e.g. for AAAA queries so guest immediately uses IPv4)
            resp.extend_from_slice(&0u16.to_be_bytes());
            resp.extend_from_slice(&0u16.to_be_bytes());
            resp.extend_from_slice(&0u16.to_be_bytes());
            resp.extend_from_slice(question_slice);
        }

        self.send_udp(guest_mac, dns_server_ip, 53, guest_ip, guest_port, &resp);
    }

    fn send_tcp_segment(
        &mut self,
        dst_mac: [u8; 6],
        key: TcpKey,
        seq: u32,
        ack: u32,
        flags: u8,
        payload: &[u8],
    ) {
        let include_mss = (flags & TCP_SYN) != 0;
        let hdr_len = if include_mss { 24usize } else { 20usize };
        let mut seg = vec![0u8; hdr_len + payload.len()];
        seg[0..2].copy_from_slice(&key.server_port.to_be_bytes());
        seg[2..4].copy_from_slice(&key.guest_port.to_be_bytes());
        seg[4..8].copy_from_slice(&seq.to_be_bytes());
        seg[8..12].copy_from_slice(&ack.to_be_bytes());
        seg[12] = ((hdr_len / 4) as u8) << 4;
        seg[13] = flags;
        seg[14..16].copy_from_slice(&65_535u16.to_be_bytes()); // Receive window
        if include_mss {
            seg[20..24].copy_from_slice(&[2, 4, (TCP_MSS >> 8) as u8, (TCP_MSS & 0xFF) as u8]);
        }
        seg[hdr_len..].copy_from_slice(payload);
        let csum = transport_checksum(key.server_ip, key.guest_ip, IPPROTO_TCP, &seg);
        seg[16..18].copy_from_slice(&csum.to_be_bytes());
        self.push_ipv4_frame(dst_mac, key.server_ip, key.guest_ip, IPPROTO_TCP, &seg);
    }

    fn handle_tcp(
        &mut self,
        guest_mac: [u8; 6],
        guest_ip: [u8; 4],
        server_ip: [u8; 4],
        tcp: &[u8],
    ) {
        if tcp.len() < 20 {
            return;
        }
        let guest_port = u16::from_be_bytes([tcp[0], tcp[1]]);
        let server_port = u16::from_be_bytes([tcp[2], tcp[3]]);
        let seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
        let ack = u32::from_be_bytes([tcp[8], tcp[9], tcp[10], tcp[11]]);
        let data_offset = ((tcp[12] >> 4) as usize) * 4;
        let flags = tcp[13];
        let window = u32::from(u16::from_be_bytes([tcp[14], tcp[15]]));
        if data_offset < 20 || data_offset > tcp.len() {
            return;
        }
        let payload = &tcp[data_offset..];

        let key = TcpKey {
            guest_ip,
            guest_port,
            server_ip,
            server_port,
        };

        if (flags & TCP_RST) != 0 {
            self.tcp_conns.remove(&key);
            return;
        }

        // Only accept HTTP connections on port 80 (or 8080); send RST on closed ports cleanly.
        if server_port != 80 && server_port != 8080 {
            let rst_seq = if (flags & TCP_ACK) != 0 { ack } else { 0 };
            let rst_ack = seq
                .wrapping_add(payload.len() as u32)
                .wrapping_add(u32::from((flags & TCP_SYN) != 0));
            self.send_tcp_segment(guest_mac, key, rst_seq, rst_ack, TCP_RST | TCP_ACK, &[]);
            return;
        }

        if (flags & TCP_SYN) != 0 && (flags & TCP_ACK) == 0 {
            let isn = self.isn_counter;
            self.isn_counter = self.isn_counter.wrapping_add(0x0100_0000);
            let rcv_nxt = seq.wrapping_add(1);
            let tx_base = isn.wrapping_add(1);
            let conn = TcpConn {
                state: TcpState::SynReceived,
                guest_mac,
                rcv_nxt,
                snd_una: isn,
                snd_nxt: tx_base,
                tx_base_seq: tx_base,
                peer_window: window.max(4096),
                rx_buf: Vec::new(),
                http_dispatched: false,
                tx_buf: Vec::new(),
                response_ready: false,
                fin_sent: false,
                fin_seq: 0,
                duplicate_acks: 0,
            };
            self.tcp_conns.insert(key, conn);
            self.send_tcp_segment(guest_mac, key, isn, rcv_nxt, TCP_SYN | TCP_ACK, &[]);
            return;
        }

        let mut should_remove = false;
        let mut maybe_http: Option<BridgeHttpRequest> = None;

        if let Some(conn) = self.tcp_conns.get_mut(&key) {
            conn.guest_mac = guest_mac;
            conn.peer_window = window.max(TCP_MSS as u32);

            if (flags & TCP_ACK) != 0 {
                if conn.state == TcpState::SynReceived && ack == conn.tx_base_seq {
                    conn.state = TcpState::Established;
                    conn.snd_una = ack;
                } else {
                    let forward = ack.wrapping_sub(conn.snd_una);
                    if forward > 0 && forward <= (conn.tx_buf.len() as u32 + 2) {
                        conn.snd_una = ack;
                        conn.duplicate_acks = 0;
                    } else if ack == conn.snd_una && conn.response_ready && payload.is_empty() {
                        conn.duplicate_acks = conn.duplicate_acks.saturating_add(1);
                        if conn.duplicate_acks >= 3 {
                            // Fast retransmit from snd_una.
                            conn.snd_nxt = conn.snd_una;
                            conn.fin_sent = false;
                            conn.duplicate_acks = 0;
                        }
                    }
                    if conn.fin_sent && ack == conn.fin_seq.wrapping_add(1) {
                        conn.state = TcpState::Closed;
                        should_remove = true;
                    }
                }
            }

            let mut send_ack = false;
            if !payload.is_empty() {
                if seq == conn.rcv_nxt {
                    if conn.rx_buf.len() + payload.len()
                        <= MAX_HTTP_REQ_HEADER_BYTES + MAX_HTTP_REQ_BODY_BYTES
                    {
                        conn.rx_buf.extend_from_slice(payload);
                        conn.rcv_nxt = conn.rcv_nxt.wrapping_add(payload.len() as u32);
                        send_ack = true;
                    }
                } else {
                    // Out-of-order segment: re-send current cumulative ACK.
                    send_ack = true;
                }
            }

            if (flags & TCP_FIN) != 0 {
                if seq.wrapping_add(payload.len() as u32) == conn.rcv_nxt {
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                }
                send_ack = true;
                if conn.fin_sent && conn.snd_una == conn.fin_seq.wrapping_add(1) {
                    should_remove = true;
                }
            }

            if !conn.http_dispatched {
                if let Some(parsed) = Self::try_parse_http(
                    &conn.rx_buf,
                    self.next_request_id,
                    &self.dns_ip_to_host,
                    server_ip,
                ) {
                    conn.http_dispatched = true;
                    maybe_http = Some(parsed);
                }
            }

            if send_ack {
                let ack_num = conn.rcv_nxt;
                let seq_num = conn.snd_nxt;
                self.send_tcp_segment(guest_mac, key, seq_num, ack_num, TCP_ACK, &[]);
            }
        }

        if let Some(req) = maybe_http {
            self.http_conn_map.insert(req.request_id, key);
            self.next_request_id = self.next_request_id.wrapping_add(1);
            self.stats.http_requests = self.stats.http_requests.wrapping_add(1);
            self.pending_http.push_back(req);
        }

        if should_remove {
            self.tcp_conns.remove(&key);
        } else {
            self.pump_tcp_tx(key);
        }
    }

    fn pump_tcp_tx(&mut self, key: TcpKey) {
        struct OutboundTcpSeg {
            seq: u32,
            ack: u32,
            flags: u8,
            data: Vec<u8>,
            dst_mac: [u8; 6],
        }
        let mut segments: Vec<OutboundTcpSeg> = Vec::new();
        if let Some(conn) = self.tcp_conns.get_mut(&key) {
            if !conn.response_ready || conn.state == TcpState::SynReceived {
                return;
            }

            let acked_off = conn.snd_una.wrapping_sub(conn.tx_base_seq) as usize;
            let mut sent_off = conn.snd_nxt.wrapping_sub(conn.tx_base_seq) as usize;
            if sent_off < acked_off {
                sent_off = acked_off;
            }
            let max_wnd = (conn.peer_window as usize).min(MAX_IN_FLIGHT_BYTES);

            while sent_off < conn.tx_buf.len() && (sent_off - acked_off) < max_wnd {
                let budget = max_wnd - (sent_off - acked_off);
                let chunk_len = (conn.tx_buf.len() - sent_off).min(TCP_MSS).min(budget);
                if chunk_len == 0 {
                    break;
                }
                let seq = conn.tx_base_seq.wrapping_add(sent_off as u32);
                let chunk = conn.tx_buf[sent_off..sent_off + chunk_len].to_vec();
                sent_off += chunk_len;
                conn.snd_nxt = conn.tx_base_seq.wrapping_add(sent_off as u32);
                let is_last = sent_off == conn.tx_buf.len();
                let mut flags = TCP_ACK | TCP_PSH;
                if is_last && !conn.fin_sent {
                    flags |= TCP_FIN;
                    conn.fin_sent = true;
                    conn.fin_seq = conn.snd_nxt;
                    conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
                    conn.state = TcpState::FinWait;
                }
                segments.push(OutboundTcpSeg {
                    seq,
                    ack: conn.rcv_nxt,
                    flags,
                    data: chunk,
                    dst_mac: conn.guest_mac,
                });
            }

            if conn.tx_buf.is_empty() && !conn.fin_sent {
                let seq = conn.snd_nxt;
                conn.fin_sent = true;
                conn.fin_seq = seq;
                conn.snd_nxt = seq.wrapping_add(1);
                conn.state = TcpState::FinWait;
                segments.push(OutboundTcpSeg {
                    seq,
                    ack: conn.rcv_nxt,
                    flags: TCP_FIN | TCP_ACK,
                    data: Vec::new(),
                    dst_mac: conn.guest_mac,
                });
            }
        }

        for seg in segments {
            self.send_tcp_segment(seg.dst_mac, key, seg.seq, seg.ack, seg.flags, &seg.data);
        }
    }

    fn try_parse_http(
        buf: &[u8],
        request_id: u64,
        dns_ip_to_host: &BTreeMap<[u8; 4], String>,
        server_ip: [u8; 4],
    ) -> Option<BridgeHttpRequest> {
        let hdr_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let hdr_str = std::str::from_utf8(&buf[..hdr_end]).ok()?;
        let mut lines = hdr_str.split("\r\n");
        let req_line = lines.next()?;
        let mut parts = req_line.split_whitespace();
        let method = parts.next()?.to_owned();
        let path = parts.next()?.to_owned();

        let mut headers = Vec::new();
        let mut content_len = 0usize;
        let mut host_hdr: Option<String> = None;

        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().to_owned();
                let val = v.trim().to_owned();
                if key.eq_ignore_ascii_case("content-length") {
                    content_len = val.parse::<usize>().unwrap_or(0);
                } else if key.eq_ignore_ascii_case("host") {
                    host_hdr = Some(val.clone());
                }
                headers.push((key, val));
            }
        }

        let body_start = hdr_end + 4;
        if buf.len() < body_start + content_len {
            return None;
        }
        let body = buf[body_start..body_start + content_len].to_vec();
        let host = host_hdr
            .or_else(|| dns_ip_to_host.get(&server_ip).cloned())
            .unwrap_or_else(|| {
                format!(
                    "{}.{}.{}.{}",
                    server_ip[0], server_ip[1], server_ip[2], server_ip[3]
                )
            });

        Some(BridgeHttpRequest {
            request_id,
            method,
            host,
            path,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_ipv4_eth(proto: u8, src_ip: [u8; 4], dst_ip: [u8; 4], l4: &[u8]) -> Vec<u8> {
        let total_len = 20 + l4.len();
        let mut ip = [0u8; 20];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        ip[8] = 64;
        ip[9] = proto;
        ip[12..16].copy_from_slice(&src_ip);
        ip[16..20].copy_from_slice(&dst_ip);
        let csum = internet_checksum(&ip);
        ip[10..12].copy_from_slice(&csum.to_be_bytes());

        let mut frame = Vec::with_capacity(14 + total_len);
        frame.extend_from_slice(&GATEWAY_MAC);
        frame.extend_from_slice(&GUEST_MAC);
        frame.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        frame.extend_from_slice(&ip);
        frame.extend_from_slice(l4);
        frame
    }

    #[test]
    fn internet_checksum_verifies_rfc1071_vector() {
        let sample = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        let csum = internet_checksum(&sample);
        let mut with_csum = sample.to_vec();
        with_csum.extend_from_slice(&csum.to_be_bytes());
        assert_eq!(internet_checksum(&with_csum), 0);
    }

    #[test]
    fn responds_to_arp_and_icmp_echo_for_gateway() {
        let mut sw = VirtualNetworkSwitch::new();

        // ARP Request for 10.0.2.2
        let mut arp = Vec::new();
        arp.extend_from_slice(&[0xFF; 6]);
        arp.extend_from_slice(&GUEST_MAC);
        arp.extend_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        arp.extend_from_slice(&1u16.to_be_bytes());
        arp.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        arp.extend_from_slice(&[6, 4, 0, 1]);
        arp.extend_from_slice(&GUEST_MAC);
        arp.extend_from_slice(&GUEST_IPV4);
        arp.extend_from_slice(&[0; 6]);
        arp.extend_from_slice(&GATEWAY_IPV4);

        sw.receive_frame_from_guest(&arp);
        let reply = sw.pop_frame_for_guest().expect("ARP reply");
        assert_eq!(&reply[22..28], &GATEWAY_MAC);
        assert_eq!(&reply[28..32], &GATEWAY_IPV4);

        // ICMP Echo Request
        let mut icmp = vec![8, 0, 0, 0, 0x12, 0x34, 0x00, 0x01];
        icmp.extend_from_slice(b"ping-payload");
        let csum = internet_checksum(&icmp);
        icmp[2..4].copy_from_slice(&csum.to_be_bytes());

        let frame = build_ipv4_eth(IPPROTO_ICMP, GUEST_IPV4, GATEWAY_IPV4, &icmp);
        sw.receive_frame_from_guest(&frame);
        let echo_reply = sw.pop_frame_for_guest().expect("ICMP echo reply");
        assert_eq!(echo_reply[34], 0); // Echo Reply
        assert_eq!(internet_checksum(&echo_reply[34..]), 0);
    }

    #[test]
    fn resolves_dns_and_streams_multi_segment_tcp_http_response() {
        let mut sw = VirtualNetworkSwitch::new();

        // DNS A query for apt.wasmnux.local
        let mut dns_q = vec![
            0xAB, 0xCD, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        for lbl in ["apt", "wasmnux", "local"] {
            dns_q.push(lbl.len() as u8);
            dns_q.extend_from_slice(lbl.as_bytes());
        }
        dns_q.push(0);
        dns_q.extend_from_slice(&1u16.to_be_bytes()); // A
        dns_q.extend_from_slice(&1u16.to_be_bytes()); // IN

        let mut udp = Vec::new();
        udp.extend_from_slice(&50000u16.to_be_bytes());
        udp.extend_from_slice(&53u16.to_be_bytes());
        udp.extend_from_slice(&((8 + dns_q.len()) as u16).to_be_bytes());
        udp.extend_from_slice(&[0, 0]);
        udp.extend_from_slice(&dns_q);

        sw.receive_frame_from_guest(&build_ipv4_eth(IPPROTO_UDP, GUEST_IPV4, GATEWAY_IPV4, &udp));
        let dns_resp = sw.pop_frame_for_guest().expect("DNS response");
        assert_eq!(&dns_resp[dns_resp.len() - 4..], &GATEWAY_IPV4);

        // TCP SYN -> SYN+ACK -> ACK + HTTP GET -> 100 KiB chunked TCP response
        let send_tcp =
            |sw: &mut VirtualNetworkSwitch, seq: u32, ack: u32, flags: u8, data: &[u8]| {
                let mut seg = vec![0u8; 20 + data.len()];
                seg[0..2].copy_from_slice(&42000u16.to_be_bytes());
                seg[2..4].copy_from_slice(&80u16.to_be_bytes());
                seg[4..8].copy_from_slice(&seq.to_be_bytes());
                seg[8..12].copy_from_slice(&ack.to_be_bytes());
                seg[12] = 5 << 4;
                seg[13] = flags;
                seg[14..16].copy_from_slice(&8192u16.to_be_bytes());
                seg[20..].copy_from_slice(data);
                sw.receive_frame_from_guest(&build_ipv4_eth(
                    IPPROTO_TCP,
                    GUEST_IPV4,
                    GATEWAY_IPV4,
                    &seg,
                ));
            };

        send_tcp(&mut sw, 1000, 0, TCP_SYN, &[]);
        let syn_ack = sw.pop_frame_for_guest().expect("SYN-ACK");
        let server_isn = u32::from_be_bytes([syn_ack[38], syn_ack[39], syn_ack[40], syn_ack[41]]);
        assert_eq!(syn_ack[47] & (TCP_SYN | TCP_ACK), TCP_SYN | TCP_ACK);

        let http_get =
            b"GET /debian/dists/trixie/Release HTTP/1.1\r\nHost: apt.wasmnux.local\r\n\r\n";
        send_tcp(
            &mut sw,
            1001,
            server_isn.wrapping_add(1),
            TCP_ACK | TCP_PSH,
            http_get,
        );
        let _ack_frame = sw.pop_frame_for_guest().expect("ACK of HTTP GET");

        let req = sw.pop_http_request().expect("HTTP request queued");
        assert_eq!(req.method, "GET");
        assert_eq!(req.host, "apt.wasmnux.local");
        assert_eq!(req.path, "/debian/dists/trixie/Release");

        let large_body = vec![0x5Au8; 40_000];
        sw.complete_http_response(BridgeHttpResponse {
            request_id: req.request_id,
            status_code: 200,
            status_text: "OK".to_owned(),
            headers: vec![(
                "Content-Type".to_owned(),
                "application/octet-stream".to_owned(),
            )],
            body: large_body.clone(),
        });

        let mut received = Vec::new();
        let mut guest_ack = server_isn.wrapping_add(1);
        let mut saw_fin = false;
        while !saw_fin {
            while let Some(frame) = sw.pop_frame_for_guest() {
                let ihl = ((frame[14] & 0x0F) as usize) * 4;
                let tcp_off = 14 + ihl;
                let doff = ((frame[tcp_off + 12] >> 4) as usize) * 4;
                let flags = frame[tcp_off + 13];
                let data = &frame[tcp_off + doff..];
                received.extend_from_slice(data);
                guest_ack = guest_ack.wrapping_add(data.len() as u32);
                if (flags & TCP_FIN) != 0 {
                    guest_ack = guest_ack.wrapping_add(1);
                    saw_fin = true;
                }
            }
            send_tcp(
                &mut sw,
                1001 + (http_get.len() as u32),
                guest_ack,
                TCP_ACK,
                &[],
            );
        }

        let sep = received
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("HTTP response header");
        assert_eq!(&received[sep + 4..], &large_body[..]);
    }
}
