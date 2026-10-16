//! The real CLI over a project-authored, original-format loopback peer.
//! No original server packets or media-derived strings are fixtures.

use std::net::UdpSocket;
use std::process::Command;
use std::time::Duration;

#[test]
fn query_only_launch_does_not_prompt_for_media_or_log_server_strings() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let address = socket.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut buffer = [0; 1500];
        let (length, client) = socket.recv_from(&mut buffer).unwrap();
        assert_eq!(
            &buffer[..length],
            ohl_net::encode_request(ohl_net::Query::Info, None)
        );
        let mut reply = vec![255, 255, 255, 255, b'I', 48];
        // All strings below were invented for this synthetic peer.
        for field in [
            "private-server-sentinel",
            "private-arena-sentinel",
            "synthetic-folder",
            "Synthetic Test Game",
        ] {
            reply.extend_from_slice(field.as_bytes());
            reply.push(0);
        }
        reply.extend_from_slice(&0_u16.to_le_bytes());
        reply.extend_from_slice(&[2, 8, 0, b'd', b'l', 0, 0]);
        reply.extend_from_slice(b"synthetic-version\0");
        socket.send_to(&reply, client).unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .args(["--query-server", &address.to_string()])
        .output()
        .unwrap();
    peer.join().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("protocol=48, players=2, capacity=8"));
    for private in [
        "private-server-sentinel",
        "private-arena-sentinel",
        "synthetic-folder",
    ] {
        assert!(!stdout.contains(private));
        assert!(!stderr.contains(private));
    }
    assert!(!stdout.contains("Path to a legally obtained"));
}

#[test]
fn query_mode_rejects_ambiguous_media_and_gameplay_requests() {
    for extra in ["--play", "--skirmish", "--training"] {
        let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
            .args(["--query-server", "127.0.0.1", extra])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}
