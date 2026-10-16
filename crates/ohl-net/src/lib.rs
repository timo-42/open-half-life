//! Valve A2S server queries, independently implemented from public protocol prose.
//!
//! This crate implements server discovery and metadata queries. A successful query
//! is not a gameplay connection, Steam authentication, or evidence of gameplay
//! interoperability. There is no project-specific gameplay transport here.
//!
//! # Public technical sources
//!
//! - Valve Developer Community, [Server queries], revision 333666, retrieved from
//!   the [2023-08-31 archive] because the live page requires a browser challenge:
//!   data types, connectionless headers, INFO (modern and obsolete GoldSource),
//!   PLAYER, RULES, challenge exchange, and GoldSource split response framing.
//! - Valve Corporation, *Server Queries* (2004), [public document mirror]: the
//!   conventional query port 27015 and the basic request/response field tables.
//!
//! The Rust implementation and all fixtures are independently authored. Fixtures
//! contain synthetic names only. No engine, SDK implementation, disassembly,
//! decompiled code, game media, or other implementation was used as a source.
//!
//! # Deliberate bounds and compatibility limits
//!
//! Bounds are project choices, not claims about every Valve engine: 1400-byte
//! datagrams, 64 KiB logical responses, 4096-byte UTF-8 strings, 1024 rules,
//! at most 15 GoldSource fragments, three challenge retries, and a 30-second
//! maximum query deadline. Only the documented GoldSource split layout is
//! accepted. Source split variants and compressed responses are unsupported.
//! A split identifier with its high bit set is conservatively rejected because
//! it can indicate a compressed Source response. This also excludes negative
//! GoldSource identifiers; no automatic engine-format guessing is attempted.
//!
//! [Server queries]: https://developer.valvesoftware.com/wiki/Server_queries
//! [2023-08-31 archive]: https://web.archive.org/web/20230831123627id_/https://developer.valvesoftware.com/wiki/Server_queries
//! [public document mirror]: https://github.com/rocketsciencegg/qstat/blob/main/info/a2s.txt

pub mod channel;
mod client;
mod codec;
pub mod legacy;
pub mod message;
pub mod server;
mod split;
pub mod word_transform;

pub use client::{DiscoveredServer, discover_lan, query_info, query_players, query_rules};
pub use codec::{decode_response, encode_request};
pub use split::GoldSrcReassembler;

use std::{fmt, io, time::Duration};

/// Conventional server query/game UDP port in Valve's public query document.
pub const DEFAULT_QUERY_PORT: u16 = 27015;
/// Maximum accepted UDP payload, excluding IP/UDP headers.
pub const MAX_DATAGRAM_BYTES: usize = 1400;
/// Maximum bytes retained for one logical response.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 bytes in a single NUL-terminated query string.
pub const MAX_STRING_BYTES: usize = 4096;
/// Project limit for the response's rule count.
pub const MAX_RULES: usize = 1024;
/// GoldSource's documented low-nibble fragment-count maximum.
pub const MAX_FRAGMENTS: usize = 15;
/// Upper bound for one user-initiated query or discovery operation.
pub const MAX_QUERY_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) const SINGLE_HEADER: [u8; 4] = [255; 4];
pub(crate) const SPLIT_HEADER: [u8; 4] = [254, 255, 255, 255];
pub(crate) const MAX_CHALLENGES: usize = 3;

/// Supported standard connectionless requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Query {
    Info,
    Players,
    Rules,
}

/// Wire format used by an INFO reply; this is not an engine identification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InfoFormat {
    /// The `I` reply, also used by current GoldSource servers.
    Modern,
    /// The obsolete GoldSource `m` reply.
    GoldSrc,
}

/// Validated server metadata. Text remains untrusted server-provided display data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerInfo {
    pub format: InfoFormat,
    pub protocol: u8,
    pub name: String,
    pub map: String,
    pub folder: String,
    pub game: String,
    pub app_id: Option<u16>,
    pub players: u8,
    pub max_players: u8,
    pub bots: u8,
    /// Preserved byte; engine branches use different capitalization conventions.
    pub server_type: u8,
    /// Preserved byte; unknown operating-system indicators are not guessed.
    pub environment: u8,
    pub password_protected: bool,
    pub vac: bool,
    pub version: Option<String>,
    pub legacy: Option<LegacyInfo>,
    pub extra: ExtraInfo,
}

/// Fields which exist only in the obsolete GoldSource INFO response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyInfo {
    /// Informational only: UDP replies are always associated with their sender.
    pub address: String,
    pub modification: Option<ModInfo>,
}

/// Optional modification metadata in an obsolete GoldSource INFO response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModInfo {
    pub website: String,
    pub download: String,
    pub version: u32,
    pub size: u32,
    pub multiplayer_only: bool,
    pub custom_dll: bool,
}

/// Modern INFO extra fields, in the protocol's declared order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtraInfo {
    pub game_port: Option<u16>,
    pub steam_id: Option<u64>,
    pub spectator: Option<SpectatorInfo>,
    pub keywords: Option<String>,
    pub game_id: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpectatorInfo {
    pub port: u16,
    pub name: String,
}

/// One PLAYER response entry. The index need not be unique across entries.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInfo {
    pub index: u8,
    pub name: String,
    pub score: i32,
    pub duration_seconds: f32,
}

/// An ordered RULES entry; duplicate rule names are deliberately preserved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerRule {
    pub name: String,
    pub value: String,
}

/// One complete, validated standard query response.
#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Challenge(i32),
    Info(Box<ServerInfo>),
    Players(Vec<PlayerInfo>),
    Rules(Vec<ServerRule>),
}

/// Fixed diagnostic categories; errors never include server strings or packets.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Timeout,
    Truncated,
    Limit(&'static str),
    Invalid(&'static str),
    Unsupported(&'static str),
    ChallengeLimit,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "query socket I/O failed: {error}"),
            Self::Timeout => f.write_str("server query deadline expired"),
            Self::Truncated => f.write_str("truncated query response"),
            Self::Limit(kind) => write!(f, "query limit exceeded: {kind}"),
            Self::Invalid(kind) => write!(f, "invalid query response: {kind}"),
            Self::Unsupported(kind) => write!(f, "unsupported query variant: {kind}"),
            Self::ChallengeLimit => f.write_str("server query challenge retry limit exceeded"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
