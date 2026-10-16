//! Loopback peers speak the public wire layouts; they are not another client.

use std::{
    net::UdpSocket,
    thread,
    time::{Duration, Instant},
};

use ohl_net::{Error, Query, encode_request, query_info, query_players, query_rules};

fn server() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    socket
}

// All field strings are project-authored synthetic identifiers.
fn info_packet() -> Vec<u8> {
    let mut packet =
        b"\xff\xff\xff\xffI\x30Synthetic server\0arena\0testgame\0Test Game\0".to_vec();
    packet.extend_from_slice(&70_u16.to_le_bytes());
    packet.extend_from_slice(&[1, 8, 0, b'd', b'l', 0, 0]);
    packet.extend_from_slice(b"synthetic-version\0");
    packet
}

fn split(id: u32, numbering: u8, data: &[u8]) -> Vec<u8> {
    let mut packet = b"\xfe\xff\xff\xff".to_vec();
    packet.extend_from_slice(&id.to_le_bytes());
    packet.push(numbering);
    packet.extend_from_slice(data);
    packet
}

#[test]
fn info_challenge_then_reordered_goldsrc_split_reply() {
    let socket = server();
    let address = socket.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut request = [0; 100];
        let (length, client) = socket.recv_from(&mut request).unwrap();
        assert_eq!(
            &request[..length],
            b"\xff\xff\xff\xffTSource Engine Query\0"
        );
        socket
            .send_to(b"\xff\xff\xff\xffA\x78\x56\x34\x12", client)
            .unwrap();
        let (length, same_client) = socket.recv_from(&mut request).unwrap();
        assert_eq!(client, same_client);
        assert_eq!(
            &request[..length],
            encode_request(Query::Info, Some(0x1234_5678))
        );
        let full = info_packet();
        let second = split(45, 0x12, &full[20..]);
        socket.send_to(&second, client).unwrap();
        socket.send_to(&second, client).unwrap();
        socket.send_to(&split(45, 2, &full[..20]), client).unwrap();
    });
    let info = query_info(address, Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert_eq!(info.name, "Synthetic server");
    assert_eq!(info.protocol, 48);
}

#[test]
fn player_and_rule_queries_use_original_challenge_and_reply_layouts() {
    for kind in [Query::Players, Query::Rules] {
        let socket = server();
        let address = socket.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut request = [0; 100];
            let (length, client) = socket.recv_from(&mut request).unwrap();
            assert_eq!(&request[..length], encode_request(kind, None));
            socket
                .send_to(b"\xff\xff\xff\xffA\x21\x43\x65\x87", client)
                .unwrap();
            let (length, _) = socket.recv_from(&mut request).unwrap();
            assert_eq!(
                &request[..length],
                encode_request(kind, Some(i32::from_le_bytes([0x21, 0x43, 0x65, 0x87])))
            );
            let response: &[u8] = match kind {
                Query::Players => b"\xff\xff\xff\xffD\0",
                Query::Rules => b"\xff\xff\xff\xffE\x01\0setting\0enabled\0",
                Query::Info => unreachable!(),
            };
            socket.send_to(response, client).unwrap();
        });
        if kind == Query::Players {
            assert!(
                query_players(address, Duration::from_secs(2))
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert_eq!(
                query_rules(address, Duration::from_secs(2)).unwrap()[0].value,
                "enabled"
            );
        }
        handle.join().unwrap();
    }
}

#[test]
fn challenge_loops_are_bounded() {
    let socket = server();
    let address = socket.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for _ in 0..4 {
            let mut request = [0; 100];
            let (_, client) = socket.recv_from(&mut request).unwrap();
            socket
                .send_to(b"\xff\xff\xff\xffA\x01\0\0\0", client)
                .unwrap();
        }
    });
    assert!(matches!(
        query_info(address, Duration::from_secs(2)),
        Err(Error::ChallengeLimit)
    ));
    handle.join().unwrap();
}

#[test]
fn missing_fragment_obeys_one_deadline() {
    let socket = server();
    let address = socket.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut request = [0; 100];
        let (_, client) = socket.recv_from(&mut request).unwrap();
        socket
            .send_to(&split(99, 2, &info_packet()[..20]), client)
            .unwrap();
        // Keep the peer socket open until the client has timed out.
        thread::sleep(Duration::from_millis(250));
    });
    let started = Instant::now();
    assert!(matches!(
        query_info(address, Duration::from_millis(60)),
        Err(Error::Timeout)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    handle.join().unwrap();
}

#[test]
fn malformed_and_oversized_known_peer_packets_fail_with_fixed_errors() {
    for packet in [b"\xff\xff\xff\xffI".to_vec(), vec![255; 2000]] {
        let socket = server();
        let address = socket.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut request = [0; 100];
            let (_, client) = socket.recv_from(&mut request).unwrap();
            socket.send_to(&packet, client).unwrap();
        });
        let error = query_info(address, Duration::from_secs(2)).unwrap_err();
        assert!(matches!(
            error,
            Error::Truncated | Error::Limit("datagram bytes")
        ));
        handle.join().unwrap();
    }
}

#[test]
fn invalid_timeouts_are_rejected_before_socket_io() {
    let address = "127.0.0.1:1".parse().unwrap();
    for timeout in [Duration::ZERO, Duration::from_secs(31), Duration::MAX] {
        assert!(matches!(
            query_info(address, timeout),
            Err(Error::Invalid(_))
        ));
    }
}
