//! Bounded UDP query transport; no gameplay connection is established.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    io,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use crate::{
    DEFAULT_QUERY_PORT, Error, GoldSrcReassembler, MAX_CHALLENGES, MAX_DATAGRAM_BYTES,
    MAX_QUERY_TIMEOUT, PlayerInfo, Query, Response, ServerInfo, ServerRule, decode_response,
    encode_request,
};

const MAX_QUERY_DATAGRAMS: usize = 64;
const MAX_DISCOVERY_DATAGRAMS: usize = 512;
const MAX_DISCOVERY_SERVERS: usize = 64;
const MAX_PEER_DATAGRAMS: usize = 32;

/// A validated reply tied to its actual UDP sender, never its claimed address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredServer {
    pub address: SocketAddr,
    pub info: ServerInfo,
    /// Elapsed time since the latest INFO request (including fragment arrival).
    pub ping: Duration,
}

/// Query INFO at an explicit server address over the original A2S wire protocol.
///
/// Both modern `I` and obsolete GoldSource `m` replies are accepted. This sends
/// the documented `T` request, so servers requiring pre-A2S text requests are
/// outside this API. Only GoldSource split replies are supported. The timeout
/// covers every challenge and fragment, and the connected socket filters peers.
pub fn query_info(address: SocketAddr, timeout: Duration) -> Result<ServerInfo, Error> {
    match query(address, timeout, Query::Info)? {
        Response::Info(info) => Ok(*info),
        _ => Err(Error::Invalid("unexpected INFO result")),
    }
}

/// Query the standard PLAYER list with bounded challenge retries.
pub fn query_players(address: SocketAddr, timeout: Duration) -> Result<Vec<PlayerInfo>, Error> {
    match query(address, timeout, Query::Players)? {
        Response::Players(players) => Ok(players),
        _ => Err(Error::Invalid("unexpected PLAYER result")),
    }
}

/// Query the standard ordered RULES list, preserving duplicate names.
pub fn query_rules(address: SocketAddr, timeout: Duration) -> Result<Vec<ServerRule>, Error> {
    match query(address, timeout, Query::Rules)? {
        Response::Rules(rules) => Ok(rules),
        _ => Err(Error::Invalid("unexpected RULES result")),
    }
}

fn query(address: SocketAddr, timeout: Duration, kind: Query) -> Result<Response, Error> {
    let deadline = deadline(timeout)?;
    let socket = UdpSocket::bind(if address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })?;
    socket.connect(address)?;
    send_connected(&socket, &encode_request(kind, None), deadline)?;
    let mut challenges = 0;
    let mut reassembler = GoldSrcReassembler::new();
    // One extra byte detects oversized UDP datagrams even when recv truncates.
    let mut datagram = [0; MAX_DATAGRAM_BYTES + 1];
    for _ in 0..MAX_QUERY_DATAGRAMS {
        socket.set_read_timeout(Some(remaining(deadline)?))?;
        let (length, sender) = socket.recv_from(&mut datagram).map_err(receive_error)?;
        if sender != address {
            continue;
        }
        let Some(packet) = reassembler.push(&datagram[..length])? else {
            continue;
        };
        let response = decode_response(&packet)?;
        if let Response::Challenge(challenge) = response {
            if challenges == MAX_CHALLENGES {
                return Err(Error::ChallengeLimit);
            }
            challenges += 1;
            reassembler.reset();
            send_connected(&socket, &encode_request(kind, Some(challenge)), deadline)?;
        } else if response_matches(kind, &response) {
            remaining(deadline)?;
            return Ok(response);
        }
    }
    Err(Error::Limit("received datagrams"))
}

fn response_matches(kind: Query, response: &Response) -> bool {
    matches!(
        (kind, response),
        (Query::Info, Response::Info(_))
            | (Query::Players, Response::Players(_))
            | (Query::Rules, Response::Rules(_))
    )
}

/// Discover servers by sending standard INFO to IPv4 loopback and broadcast.
///
/// Port 27015 comes from Valve's public query document. Sending to both addresses
/// and the 64-peer/512-datagram limits are project choices. Broadcast may be
/// unavailable on a network; loopback discovery still proceeds. Challenge and
/// fragment state are isolated per sender. Malformed/unrelated replies are
/// ignored, and this returns after the single overall deadline (or a budget).
/// Run this synchronous operation on a worker thread in interactive clients.
pub fn discover_lan(timeout: Duration) -> Result<Vec<DiscoveredServer>, Error> {
    let deadline = deadline(timeout)?;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    let targets = [
        SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_QUERY_PORT)),
        SocketAddr::from((Ipv4Addr::BROADCAST, DEFAULT_QUERY_PORT)),
    ];
    discover(&socket, &targets, deadline)
}

#[derive(Default)]
struct Peer {
    reassembler: GoldSrcReassembler,
    datagrams: usize,
    challenges: usize,
    last_sent: Option<Instant>,
}

fn discover(
    socket: &UdpSocket,
    targets: &[SocketAddr],
    deadline: Instant,
) -> Result<Vec<DiscoveredServer>, Error> {
    let started = Instant::now();
    let initial = encode_request(Query::Info, None);
    let mut sent = false;
    for target in targets {
        socket.set_write_timeout(Some(remaining(deadline)?))?;
        // A failed broadcast must not suppress replies from a local server.
        if socket.send_to(&initial, target).is_ok() {
            sent = true;
        }
    }
    if !sent {
        return Err(Error::Invalid("no discovery request sent"));
    }
    let mut peers = BTreeMap::<SocketAddr, Peer>::new();
    let mut servers = BTreeMap::<SocketAddr, DiscoveredServer>::new();
    let mut datagram = [0; MAX_DATAGRAM_BYTES + 1];
    for _ in 0..MAX_DISCOVERY_DATAGRAMS {
        let Ok(left) = remaining(deadline) else {
            break;
        };
        socket.set_read_timeout(Some(left))?;
        let (length, address) = match socket.recv_from(&mut datagram) {
            Ok(received) => received,
            Err(error) if timed_out(&error) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(Error::Io(error)),
        };
        if servers.contains_key(&address) {
            continue;
        }
        let at_capacity = peers.len() == MAX_DISCOVERY_SERVERS;
        let peer = match peers.entry(address) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) if !at_capacity => entry.insert(Peer::default()),
            Entry::Vacant(_) => continue,
        };
        peer.datagrams += 1;
        if peer.datagrams > MAX_PEER_DATAGRAMS {
            continue;
        }
        let Ok(Some(packet)) = peer.reassembler.push(&datagram[..length]) else {
            continue;
        };
        match decode_response(&packet) {
            Ok(Response::Challenge(challenge)) if peer.challenges < MAX_CHALLENGES => {
                peer.challenges += 1;
                let request = encode_request(Query::Info, Some(challenge));
                let Ok(left) = remaining(deadline) else {
                    break;
                };
                socket.set_write_timeout(Some(left))?;
                if socket.send_to(&request, address).is_ok() {
                    peer.last_sent = Some(Instant::now());
                }
            }
            Ok(Response::Info(info)) => {
                if remaining(deadline).is_err() {
                    break;
                }
                servers.insert(
                    address,
                    DiscoveredServer {
                        address,
                        info: *info,
                        ping: peer.last_sent.unwrap_or(started).elapsed(),
                    },
                );
                if servers.len() == MAX_DISCOVERY_SERVERS {
                    break;
                }
            }
            _ => {}
        }
    }
    Ok(servers.into_values().collect())
}

fn deadline(timeout: Duration) -> Result<Instant, Error> {
    if timeout.is_zero() || timeout > MAX_QUERY_TIMEOUT {
        return Err(Error::Invalid("timeout outside supported range"));
    }
    Instant::now()
        .checked_add(timeout)
        .ok_or(Error::Invalid("timeout overflow"))
}

fn remaining(deadline: Instant) -> Result<Duration, Error> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(Error::Timeout)
    } else {
        Ok(left)
    }
}

fn send_connected(socket: &UdpSocket, bytes: &[u8], deadline: Instant) -> Result<(), Error> {
    socket.set_write_timeout(Some(remaining(deadline)?))?;
    if socket.send(bytes).map_err(receive_error)? != bytes.len() {
        return Err(Error::Invalid("partial UDP send"));
    }
    Ok(())
}

fn timed_out(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn receive_error(error: io::Error) -> Error {
    if timed_out(&error) {
        Error::Timeout
    } else {
        Error::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread};

    // Independently authored synthetic server/map/folder strings only.
    fn info_packet() -> Vec<u8> {
        let mut packet =
            b"\xff\xff\xff\xffI\x30Synthetic server\0arena\0testgame\0Test Game\0".to_vec();
        packet.extend_from_slice(&70_u16.to_le_bytes());
        packet.extend_from_slice(&[2, 8, 0, b'd', b'l', 0, 0]);
        packet.extend_from_slice(b"test-version\0");
        packet
    }

    #[test]
    fn discovery_isolates_challenges_and_ignores_malformed_and_other_responses() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_address = server.local_addr().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let handle = thread::spawn(move || {
            let mut request = [0; 100];
            let (count, client) = server.recv_from(&mut request).unwrap();
            assert_eq!(&request[..count], encode_request(Query::Info, None));
            server.send_to(b"not a packet", client).unwrap();
            server.send_to(b"\xff\xff\xff\xffD\0", client).unwrap();
            let mut challenge = b"\xff\xff\xff\xffA".to_vec();
            challenge.extend_from_slice(&(-76_i32).to_le_bytes());
            server.send_to(&challenge, client).unwrap();
            let (count, same_client) = server.recv_from(&mut request).unwrap();
            assert_eq!(client, same_client);
            assert_eq!(&request[..count], encode_request(Query::Info, Some(-76)));
            server.send_to(&info_packet(), client).unwrap();
            server.send_to(&info_packet(), client).unwrap();
        });
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let result = discover(
            &socket,
            &[server_address],
            deadline(Duration::from_millis(200)).unwrap(),
        )
        .unwrap();
        handle.join().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address, server_address);
        assert_eq!(result[0].info.name, "Synthetic server");
    }

    #[test]
    fn connected_query_ignores_other_udp_senders() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_address = server.local_addr().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (sender, received) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut request = [0; 100];
            let (_, client) = server.recv_from(&mut request).unwrap();
            let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
            stranger.send_to(b"\xff\xff\xff\xffI", client).unwrap();
            sender.send(()).unwrap();
            server.send_to(&info_packet(), client).unwrap();
        });
        let result = query_info(server_address, Duration::from_secs(2)).unwrap();
        received.recv().unwrap();
        handle.join().unwrap();
        assert_eq!(result.name, "Synthetic server");
    }

    #[test]
    fn discovery_keeps_two_peers_challenges_and_equal_fragment_ids_separate() {
        let mut targets = Vec::new();
        let mut handles = Vec::new();
        for marker in *b"AB" {
            let server = UdpSocket::bind("127.0.0.1:0").unwrap();
            targets.push(server.local_addr().unwrap());
            server
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            handles.push(thread::spawn(move || {
                let mut request = [0; 100];
                let (_, client) = server.recv_from(&mut request).unwrap();
                let token = i32::from(marker);
                let mut challenge = b"\xff\xff\xff\xffA".to_vec();
                challenge.extend_from_slice(&token.to_le_bytes());
                server.send_to(&challenge, client).unwrap();
                let (count, same_client) = server.recv_from(&mut request).unwrap();
                assert_eq!(same_client, client);
                assert_eq!(&request[..count], encode_request(Query::Info, Some(token)));
                let mut full = info_packet();
                // Correlated markers on opposite sides of the fragmentation cut.
                full[6] = marker;
                full[18] = marker;
                for (numbering, body) in [(0x12, &full[16..]), (2, &full[..16])] {
                    let mut part = b"\xfe\xff\xff\xff".to_vec();
                    part.extend_from_slice(&7_u32.to_le_bytes());
                    part.push(numbering);
                    part.extend_from_slice(body);
                    server.send_to(&part, client).unwrap();
                }
            }));
        }
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let result = discover(
            &socket,
            &targets,
            deadline(Duration::from_millis(300)).unwrap(),
        )
        .unwrap();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(result.len(), 2);
        for (index, address) in targets.into_iter().enumerate() {
            let marker = b"AB"[index];
            let reply = result
                .iter()
                .find(|reply| reply.address == address)
                .unwrap();
            assert_eq!(reply.info.name.as_bytes()[0], marker);
            assert_eq!(reply.info.name.as_bytes()[12], marker);
        }
    }
}
