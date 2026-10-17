//! Synthetic loopback peers assert the published ASCII requests directly.
//! All reply fields below are authored test values, never captured packets.

use std::{
    net::UdpSocket,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use ohl_net::{
    LegacyQueryError, MAX_DATAGRAM_BYTES,
    legacy::{Error as CodecError, Profile, Request, Response},
    query_legacy,
};

const DETAILS_REQUEST: &[u8] = b"\xff\xff\xff\xffdetails";
const PLAYERS_REQUEST: &[u8] = b"\xff\xff\xff\xffplayers";
const RULES_REQUEST: &[u8] = b"\xff\xff\xff\xffrules";
const DETAILS: &[u8] = b"\xff\xff\xff\xffmclaimed-address\0private-name\0private-map\0private-dir\0private-description\0\x02\x08\x2edl\x01\0\0";
const PLAYERS: &[u8] = b"\xff\xff\xff\xffD\0";
const RULES: &[u8] = b"\xff\xff\xff\xffE\x01\0setting\0enabled\0";
const TIMEOUT: Duration = Duration::from_secs(2);

fn server() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(TIMEOUT)).unwrap();
    socket
}

fn request_bytes(request: Request) -> &'static [u8] {
    match request {
        Request::Details => DETAILS_REQUEST,
        Request::Players => PLAYERS_REQUEST,
        Request::Rules => RULES_REQUEST,
    }
}

fn details(profile: Profile) -> Vec<u8> {
    let mut reply = DETAILS.to_vec();
    if profile == Profile::ObservedLinux3111d {
        // Deliberately nonzero structural test value, with no assigned meaning.
        reply.push(0xa7);
    }
    reply
}

fn exchange(
    profile: Profile,
    request: Request,
    replies: Vec<Vec<u8>>,
) -> Result<Response, LegacyQueryError> {
    let socket = server();
    let address = socket.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut received = [0; 64];
        let (length, client) = socket.recv_from(&mut received).unwrap();
        // No codec helper produces the expected wire bytes: no terminator,
        // challenge or other suffix may be appended by the transport.
        assert_eq!(&received[..length], request_bytes(request));
        for reply in replies {
            socket.send_to(&reply, client).unwrap();
        }
    });
    let response = query_legacy(address, TIMEOUT, profile, request);
    handle.join().unwrap();
    response
}

#[test]
fn exact_ascii_requests_return_their_separate_selected_response_models() {
    for profile in [Profile::PublishedAscii, Profile::ObservedLinux3111d] {
        for request in [Request::Details, Request::Players, Request::Rules] {
            let reply = match request {
                Request::Details => details(profile),
                Request::Players => PLAYERS.to_vec(),
                Request::Rules => RULES.to_vec(),
            };
            let response = exchange(profile, request, vec![reply]).unwrap();
            match response {
                Response::Details(info) => {
                    assert_eq!(profile, Profile::PublishedAscii);
                    assert_eq!(request, Request::Details);
                    assert_eq!((info.protocol, info.players, info.max_players), (46, 2, 8));
                    assert!(info.password_protected);
                }
                Response::ObservedDetails(info) => {
                    assert_eq!(profile, Profile::ObservedLinux3111d);
                    assert_eq!(request, Request::Details);
                    assert_eq!(info.opaque_extension, 0xa7);
                    assert_eq!(info.published.name, "private-name");
                }
                Response::Players(players) => {
                    assert_eq!(request, Request::Players);
                    assert!(players.is_empty());
                }
                Response::Rules(rules) => {
                    assert_eq!(request, Request::Rules);
                    assert_eq!(rules.len(), 1);
                    assert_eq!(rules[0].value, "enabled");
                }
            }
        }
    }
}

#[test]
fn profile_shapes_and_observed_protocol_guard_never_fall_back() {
    assert_eq!(
        exchange(
            Profile::PublishedAscii,
            Request::Details,
            vec![details(Profile::ObservedLinux3111d)]
        ),
        Err(LegacyQueryError::Codec(CodecError::TrailingData))
    );
    assert_eq!(
        exchange(
            Profile::ObservedLinux3111d,
            Request::Details,
            vec![details(Profile::PublishedAscii)]
        ),
        Err(LegacyQueryError::Codec(CodecError::Truncated))
    );
    let wrong_protocol = b"\xff\xff\xff\xffmX\0A\0B\0C\0D\0\x02\x08\x2adl\x01\0\0\xa7";
    assert_eq!(
        exchange(
            Profile::ObservedLinux3111d,
            Request::Details,
            vec![wrong_protocol.to_vec()]
        ),
        Err(LegacyQueryError::Codec(CodecError::ObservedProtocolGuard))
    );
}

#[test]
fn valid_wrong_kind_replies_do_not_satisfy_any_request() {
    let profile = Profile::ObservedLinux3111d;
    for (request, replies) in [
        (
            Request::Details,
            vec![PLAYERS.to_vec(), RULES.to_vec(), details(profile)],
        ),
        (
            Request::Players,
            vec![details(profile), RULES.to_vec(), PLAYERS.to_vec()],
        ),
        (
            Request::Rules,
            vec![details(profile), PLAYERS.to_vec(), RULES.to_vec()],
        ),
    ] {
        let response = exchange(profile, request, replies).unwrap();
        assert!(matches!(
            (request, response),
            (Request::Details, Response::ObservedDetails(_))
                | (Request::Players, Response::Players(_))
                | (Request::Rules, Response::Rules(_))
        ));
    }
}

#[test]
fn wrong_kind_flood_exhausts_the_bounded_datagram_budget() {
    assert_eq!(
        exchange(
            Profile::ObservedLinux3111d,
            Request::Details,
            vec![PLAYERS.to_vec(); 64]
        ),
        Err(LegacyQueryError::DatagramBudget)
    );
}

#[test]
fn wrong_kind_arrivals_do_not_extend_the_absolute_deadline() {
    let socket = server();
    let address = socket.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut request = [0; 64];
        let (length, client) = socket.recv_from(&mut request).unwrap();
        assert_eq!(&request[..length], DETAILS_REQUEST);
        // Fewer than 64 replies, continuing beyond the client's deadline.
        for _ in 0..20 {
            let _ = socket.send_to(PLAYERS, client);
            thread::sleep(Duration::from_millis(20));
        }
    });
    let started = Instant::now();
    let result = query_legacy(
        address,
        Duration::from_millis(90),
        Profile::ObservedLinux3111d,
        Request::Details,
    );
    let elapsed = started.elapsed();
    handle.join().unwrap();
    assert_eq!(result, Err(LegacyQueryError::Timeout));
    assert!(elapsed < Duration::from_millis(300));
}

#[test]
fn the_extra_receive_byte_detects_oversize_without_rejecting_a_full_datagram() {
    let mut packet = b"\xff\xff\xff\xffE\x01\0k\0".to_vec();
    packet.resize(MAX_DATAGRAM_BYTES - 1, b'x');
    packet.push(0);
    let response = exchange(
        Profile::ObservedLinux3111d,
        Request::Rules,
        vec![packet.clone()],
    )
    .unwrap();
    let Response::Rules(rules) = response else {
        panic!("expected separate legacy rules");
    };
    assert_eq!(rules[0].value.len(), MAX_DATAGRAM_BYTES - 10);
    packet.push(0);
    assert_eq!(
        exchange(Profile::ObservedLinux3111d, Request::Rules, vec![packet]),
        Err(LegacyQueryError::DatagramLimit)
    );
}

#[test]
fn split_replies_fail_explicitly_without_reassembly() {
    for profile in [Profile::PublishedAscii, Profile::ObservedLinux3111d] {
        assert_eq!(
            exchange(
                profile,
                Request::Details,
                vec![b"\xfe\xff\xff\xff\x01\0\0\0\x02synthetic".to_vec()]
            ),
            Err(LegacyQueryError::SplitResponse)
        );
    }
}

#[test]
fn malformed_or_challenge_replies_fail_with_fixed_codec_errors() {
    for (packet, error) in [
        (b"\xff\xff\xff\xffm".as_slice(), CodecError::Truncated),
        (
            b"\xff\xff\xff\xffA\x01\0\0\0".as_slice(),
            CodecError::UnsupportedResponse,
        ),
    ] {
        let result = exchange(
            Profile::ObservedLinux3111d,
            Request::Details,
            vec![packet.to_vec()],
        );
        assert_eq!(result, Err(LegacyQueryError::Codec(error)));
    }
}

#[test]
fn invalid_deadlines_are_rejected_before_a_socket_operation() {
    let address = "127.0.0.1:1".parse().unwrap();
    for timeout in [Duration::ZERO, Duration::from_secs(31), Duration::MAX] {
        assert_eq!(
            query_legacy(
                address,
                timeout,
                Profile::ObservedLinux3111d,
                Request::Details
            ),
            Err(LegacyQueryError::InvalidTimeout)
        );
    }
}

#[test]
fn foreign_peer_packets_cannot_fail_or_satisfy_the_connected_query() {
    let socket = server();
    let address = socket.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        sender
            .send(query_legacy(
                address,
                TIMEOUT,
                Profile::ObservedLinux3111d,
                Request::Details,
            ))
            .unwrap();
    });
    let mut request = [0; 64];
    let (length, client) = socket.recv_from(&mut request).unwrap();
    assert_eq!(&request[..length], DETAILS_REQUEST);
    let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
    stranger
        .send_to(b"malformed foreign response", client)
        .unwrap();
    stranger
        .send_to(&details(Profile::ObservedLinux3111d), client)
        .unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(40)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    socket
        .send_to(&details(Profile::ObservedLinux3111d), client)
        .unwrap();
    assert!(matches!(
        receiver.recv_timeout(TIMEOUT).unwrap().unwrap(),
        Response::ObservedDetails(_)
    ));
    handle.join().unwrap();
}
