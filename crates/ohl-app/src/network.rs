//! Original Valve server queries, separate from the unimplemented gameplay
//! channel. Remote server strings are displayed only in the local browser;
//! they never reach logs, telemetry, or test snapshots.

use std::net::{IpAddr, SocketAddr};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ohl_ui::menu::LanGame;

/// The documented GoldSrc default server port (Valve's Server queries and
/// GoldSrc Dedicated Server command-line documentation).
const DEFAULT_PORT: u16 = 27015;
/// Project-chosen query deadline; it is not a gameplay protocol constant.
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);

type BrowserResult = Result<Vec<LanGame>, &'static str>;

/// Explicit metadata-query transport selection; neither option joins a game.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
pub enum QueryProfile {
    #[default]
    A2s,
    Won46,
}

/// Accepts numeric IPv4/IPv6 addresses, optionally with an explicit port.
/// There is no blocking DNS resolution in the window's event loop.
pub fn parse_address(value: &str) -> Result<SocketAddr, String> {
    let value = value.trim();
    let address = value.parse::<SocketAddr>().or_else(|_| {
        value
            .parse::<IpAddr>()
            .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
    });
    address
        .ok()
        .filter(|address| address.port() != 0 && !address.ip().is_unspecified())
        .ok_or_else(|| "expected a numeric IP address, optionally with a nonzero port".to_owned())
}

fn display_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect()
}

fn listing(address: SocketAddr, info: &ohl_net::ServerInfo, ping: Duration) -> LanGame {
    LanGame {
        address: address.to_string(),
        name: display_text(&info.name),
        map: display_text(&info.map),
        players: info.players,
        max_players: info.max_players,
        protocol: info.protocol,
        password_protected: info.password_protected,
        ping_millis: u32::try_from(ping.as_millis()).unwrap_or(u32::MAX),
    }
}

/// One bounded worker at a time: query work never blocks event processing,
/// and a refresh cannot create an unbounded collection of worker threads.
#[derive(Default)]
pub struct Browser {
    pending: Option<Receiver<BrowserResult>>,
}

impl Browser {
    fn start(&mut self, work: impl FnOnce() -> BrowserResult + Send + 'static) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("goldsrc-query".to_owned())
            .spawn(move || {
                let _ = sender.send(work());
            })
            .is_err()
        {
            return false;
        }
        self.pending = Some(receiver);
        true
    }

    pub fn discover(&mut self) -> bool {
        self.start(|| {
            ohl_net::discover_lan(QUERY_TIMEOUT)
                .map(|servers| {
                    servers
                        .into_iter()
                        .map(|server| listing(server.address, &server.info, server.ping))
                        .collect()
                })
                .map_err(|_| "The local server search could not be completed.")
        })
    }

    pub fn query(&mut self, address: &str) -> Result<(), &'static str> {
        let address = parse_address(address)
            .map_err(|_| "Enter a numeric IP address with an optional port.")?;
        if self.start(move || {
            let started = Instant::now();
            ohl_net::query_info(address, QUERY_TIMEOUT)
                .map(|info| vec![listing(address, &info, started.elapsed())])
                .map_err(|_| "That server did not return a supported query response.")
        }) {
            Ok(())
        } else {
            Err("A server query is already in progress or could not be started.")
        }
    }

    pub fn poll(&mut self) -> Option<BrowserResult> {
        let result = match self.pending.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("The server query stopped before completing."),
        };
        self.pending = None;
        Some(result)
    }
}

/// CLI inspection publishes only bounded protocol/player facts. Server
/// names, map paths, rule strings and endpoint addresses are never logged.
pub fn run_query(address: SocketAddr, profile: QueryProfile) -> bool {
    let facts = match profile {
        QueryProfile::A2s => ohl_net::query_info(address, QUERY_TIMEOUT)
            .ok()
            .map(|info| {
                (
                    info.protocol,
                    info.players,
                    info.max_players,
                    info.password_protected,
                )
            }),
        QueryProfile::Won46 => {
            use ohl_net::legacy::{Profile, Request, Response};
            match ohl_net::query_legacy(
                address,
                QUERY_TIMEOUT,
                Profile::ObservedLinux3111d,
                Request::Details,
            ) {
                Ok(Response::ObservedDetails(details)) => Some((
                    details.published.protocol,
                    details.published.players,
                    details.published.max_players,
                    details.published.password_protected,
                )),
                _ => None,
            }
        }
    };
    if let Some((protocol, players, capacity, password_protected)) = facts {
        println!(
            "Server query succeeded: protocol={protocol}, players={players}, capacity={capacity}, password_protected={password_protected}."
        );
        true
    } else {
        eprintln!("Server query failed: no supported response was received.");
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_addresses_support_the_original_default_and_explicit_ports() {
        assert_eq!(parse_address("127.0.0.1").unwrap().port(), DEFAULT_PORT);
        assert_eq!(parse_address("127.0.0.1:27031").unwrap().port(), 27031);
        assert!(parse_address("::1").unwrap().is_ipv6());
        assert_eq!(parse_address("[::1]:27031").unwrap().port(), 27031);
        for invalid in ["", "example.invalid", "127.0.0.1:0", "0.0.0.0", "[::]:2"] {
            assert!(parse_address(invalid).is_err());
        }
    }

    #[test]
    fn browser_has_one_pending_job_and_can_restart_after_completion() {
        let mut browser = Browser::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        browser.pending = Some(receiver);
        assert!(!browser.start(|| Ok(Vec::new())));
        assert!(browser.poll().is_none());
        sender.send(Ok(Vec::new())).unwrap();
        assert_eq!(browser.poll(), Some(Ok(Vec::new())));
        assert!(browser.pending.is_none());
        assert!(browser.start(|| Ok(Vec::new())));
    }

    #[test]
    fn failed_workers_release_the_pending_slot() {
        let mut browser = Browser::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        browser.pending = Some(receiver);
        drop(sender);
        assert!(browser.poll().unwrap().is_err());
        assert!(browser.pending.is_none());
    }

    #[test]
    fn untrusted_display_strings_are_bounded_plain_text() {
        assert_eq!(display_text("server\n\0\tname"), "servername");
        assert_eq!(display_text(&"é".repeat(200)).chars().count(), 128);
    }
}
