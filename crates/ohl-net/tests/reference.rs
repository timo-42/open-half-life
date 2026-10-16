//! Opt-in observation of a separately installed, lawfully obtained reference.
//!
//! This test emits no packet dumps, resource names, rule names, or server text.
//! The server is supplied separately; no reference binary is executed here.

use std::{net::SocketAddr, time::Duration};

use ohl_net::{MAX_RULES, query_info, query_players, query_rules};

/// Run with `OHL_GOLDSRC_SERVER=127.0.0.1:PORT cargo test -p ohl-net
/// --test reference -- --ignored`. Keep the reference server on a local network.
#[test]
#[ignore = "requires an explicit separately installed reference server"]
fn original_server_query_interoperability() {
    let address: SocketAddr = std::env::var("OHL_GOLDSRC_SERVER")
        .expect("set OHL_GOLDSRC_SERVER to the reference socket address")
        .parse()
        .expect("reference address must be an IP socket address");
    let timeout = Duration::from_secs(3);
    let info = query_info(address, timeout).expect("reference INFO query failed");
    assert!(info.protocol > 0);
    let players = query_players(address, timeout).expect("reference PLAYER query failed");
    assert!(u8::try_from(players.len()).is_ok());
    assert!(
        players
            .iter()
            .all(|player| player.duration_seconds.is_finite())
    );
    let rules = query_rules(address, timeout).expect("reference RULES query failed");
    assert!(rules.len() <= MAX_RULES);
}
