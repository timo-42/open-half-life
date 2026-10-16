//! Bounded server-side Valve A2S request and response codecs.
//!
//! Wire fields come from the Valve Developer Community [query specification],
//! revision 333666, and Valve's 2004 [Server Queries prose]. The latter also
//! documents the obsolete five-byte `W` challenge request. These are independent
//! implementations of the published field tables; no engine, SDK, other engine
//! implementation, game media, or packet capture is an implementation source.
//! These A2S sources date from 2004 and later and do not establish compatibility
//! with the final pre-Steam WON release, which requires separate evidence.
//!
//! This module performs no socket I/O, token generation, sender association,
//! rate limiting, expiry, player admission, or gameplay transport. An A2S token
//! belongs to metadata querying, not player join or authentication. The host
//! supplies both response data and any challenge policy. Request parsing never
//! treats a token as proof of a peer's identity.
//!
//! Response models are the crate's existing public query data types. Fields
//! which do not fit the selected INFO layout are rejected rather than dropped.
//! All finite player durations and byte-valued population/indicator fields are
//! preserved; semantic population consistency belongs to the host. Optional
//! modern INFO data uses the published EDF order, with a canonical zero EDF
//! byte when no extras are present. The unrelated app-2400 extension remains
//! unsupported, as on the query-client path.
//!
//! Project bounds match that path: 1400-byte datagrams, 64 KiB logical responses,
//! 4096 UTF-8 bytes per string, 255 players, 1024 rules and 15 GoldSource split
//! parts. Only the documented uncompressed GoldSource query split layout is
//! emitted. A caller-selected split ID must have its high bit clear, matching
//! the client's conservative rejection of ambiguous/compressed Source variants.
//! No gameplay-fragment format is inferred. Larger logical responses may fit
//! the response encoder while exceeding the bounded split packetizer's capacity.
//!
//! Returned packet/token wrappers and all errors redact input bytes in Debug.
//! Response encoding validates in bounded private scratch storage before writing
//! caller output; errors preserve the entire output buffer. Fixtures contain
//! independently authored synthetic values only.
//!
//! [query specification]: https://web.archive.org/web/20230831123627id_/https://developer.valvesoftware.com/wiki/Server_queries
//! [Server Queries prose]: https://github.com/rocketsciencegg/qstat/blob/main/info/a2s.txt

use std::fmt;

use crate::{
    ExtraInfo, InfoFormat, MAX_DATAGRAM_BYTES, MAX_FRAGMENTS, MAX_RESPONSE_BYTES, MAX_RULES,
    MAX_STRING_BYTES, Query, Response, SINGLE_HEADER, SPLIT_HEADER, ServerInfo,
};

const INFO_QUERY: &[u8] = b"Source Engine Query\0";
const SPLIT_HEADER_BYTES: usize = 9;
const SPLIT_PAYLOAD_BYTES: usize = MAX_DATAGRAM_BYTES - SPLIT_HEADER_BYTES;

/// An opaque A2S query token; it is not a player-authentication credential.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct QueryChallenge(i32);

impl QueryChallenge {
    /// Construct the documented initial PLAYER/RULES request token, -1.
    pub const fn initial() -> Self {
        Self(-1)
    }

    /// Explicit raw access/construction for the host's separate challenge policy.
    pub const fn from_i32(value: i32) -> Self {
        Self(value)
    }

    pub const fn as_i32(self) -> i32 {
        self.0
    }

    pub const fn is_initial(self) -> bool {
        self.0 == -1
    }
}

impl fmt::Debug for QueryChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("QueryChallenge { .. }")
    }
}

/// Exact documented query requests; no arbitrary request text is retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryRequest {
    Info {
        challenge: Option<QueryChallenge>,
    },
    Players {
        challenge: QueryChallenge,
    },
    Rules {
        challenge: QueryChallenge,
    },
    /// Obsolete A2S_SERVERQUERY_GETCHALLENGE, documented by Valve in 2004.
    GetChallenge,
}

impl QueryRequest {
    pub const fn query(self) -> Option<Query> {
        match self {
            Self::Info { .. } => Some(Query::Info),
            Self::Players { .. } => Some(Query::Players),
            Self::Rules { .. } => Some(Query::Rules),
            Self::GetChallenge => None,
        }
    }
}

/// Fixed diagnostics; no error retains request bytes, strings, tokens or IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerCodecError {
    DatagramLimit,
    ResponseLimit,
    TruncatedRequest,
    InvalidRequestHeader,
    UnsupportedRequest,
    InvalidInfoQuery,
    TrailingRequestData,
    StringLimit,
    EmbeddedNul,
    PlayerLimit,
    RuleLimit,
    NonFiniteDuration,
    IncompatibleInfoFields,
    UnsupportedInfoExtension,
    OutputTooSmall,
    FragmentLimit,
    UnsupportedSplitIdentifier,
}

impl fmt::Display for ServerCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DatagramLimit => "server query datagram limit exceeded",
            Self::ResponseLimit => "server query response limit exceeded",
            Self::TruncatedRequest => "truncated server query request",
            Self::InvalidRequestHeader => "invalid server query request header",
            Self::UnsupportedRequest => "unsupported server query request",
            Self::InvalidInfoQuery => "invalid server INFO request text",
            Self::TrailingRequestData => "trailing server query request data",
            Self::StringLimit => "server query string limit exceeded",
            Self::EmbeddedNul => "server query string contains a NUL byte",
            Self::PlayerLimit => "server query player count limit exceeded",
            Self::RuleLimit => "server query rule count limit exceeded",
            Self::NonFiniteDuration => "server query player duration is not finite",
            Self::IncompatibleInfoFields => "server INFO fields do not fit the selected layout",
            Self::UnsupportedInfoExtension => "unsupported game-specific INFO extension",
            Self::OutputTooSmall => "server query output buffer is too small",
            Self::FragmentLimit => "server query split count limit exceeded",
            Self::UnsupportedSplitIdentifier => "unsupported server query split identifier",
        })
    }
}

impl std::error::Error for ServerCodecError {}

/// Parse one complete request datagram, rejecting every unsupported suffix.
pub fn decode_request(packet: &[u8]) -> Result<QueryRequest, ServerCodecError> {
    if packet.len() > MAX_DATAGRAM_BYTES {
        return Err(ServerCodecError::DatagramLimit);
    }
    if packet.get(..4).ok_or(ServerCodecError::TruncatedRequest)? != SINGLE_HEADER {
        return Err(ServerCodecError::InvalidRequestHeader);
    }
    let kind = *packet.get(4).ok_or(ServerCodecError::TruncatedRequest)?;
    match kind {
        b'T' => {
            let base_length = 5 + INFO_QUERY.len();
            let text = packet
                .get(5..base_length)
                .ok_or(ServerCodecError::TruncatedRequest)?;
            if text != INFO_QUERY {
                return Err(ServerCodecError::InvalidInfoQuery);
            }
            let challenge = match packet.len() - base_length {
                0 => None,
                1..=3 => return Err(ServerCodecError::TruncatedRequest),
                4 => Some(read_challenge(&packet[base_length..])?),
                _ => return Err(ServerCodecError::TrailingRequestData),
            };
            Ok(QueryRequest::Info { challenge })
        }
        b'U' | b'V' => {
            exact_length(packet, 9)?;
            let challenge = read_challenge(&packet[5..])?;
            Ok(if kind == b'U' {
                QueryRequest::Players { challenge }
            } else {
                QueryRequest::Rules { challenge }
            })
        }
        b'W' => {
            exact_length(packet, 5)?;
            Ok(QueryRequest::GetChallenge)
        }
        _ => Err(ServerCodecError::UnsupportedRequest),
    }
}

fn exact_length(packet: &[u8], expected: usize) -> Result<(), ServerCodecError> {
    match packet.len().cmp(&expected) {
        std::cmp::Ordering::Less => Err(ServerCodecError::TruncatedRequest),
        std::cmp::Ordering::Greater => Err(ServerCodecError::TrailingRequestData),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

fn read_challenge(bytes: &[u8]) -> Result<QueryChallenge, ServerCodecError> {
    let raw: [u8; 4] = bytes
        .try_into()
        .map_err(|_| ServerCodecError::TruncatedRequest)?;
    Ok(QueryChallenge::from_i32(i32::from_le_bytes(raw)))
}

/// A complete encoded connectionless response, not yet packetized for UDP.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct EncodedResponse<'a>(&'a [u8]);

impl<'a> EncodedResponse<'a> {
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    pub const fn len(self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for EncodedResponse<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedResponse")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Validate and encode the existing query response model into caller storage.
///
/// The entire buffer remains unchanged on error. On success, bytes outside the
/// returned region remain unchanged. This builds no challenge policy and sends
/// no packets. Use [`goldsrc_response_datagrams`] for bounded UDP packetization.
pub fn encode_response<'output>(
    response: &Response,
    output: &'output mut [u8],
) -> Result<EncodedResponse<'output>, ServerCodecError> {
    let mut writer = Writer { bytes: Vec::new() };
    writer.bytes(&SINGLE_HEADER)?;
    match response {
        Response::Challenge(token) => {
            writer.byte(b'A')?;
            writer.bytes(&token.to_le_bytes())?;
        }
        Response::Info(info) => write_info(&mut writer, info)?,
        Response::Players(players) => {
            let count = u8::try_from(players.len()).map_err(|_| ServerCodecError::PlayerLimit)?;
            writer.bytes(&[b'D', count])?;
            for player in players {
                if !player.duration_seconds.is_finite() {
                    return Err(ServerCodecError::NonFiniteDuration);
                }
                writer.byte(player.index)?;
                writer.string(&player.name)?;
                writer.bytes(&player.score.to_le_bytes())?;
                writer.bytes(&player.duration_seconds.to_le_bytes())?;
            }
        }
        Response::Rules(rules) => {
            if rules.len() > MAX_RULES {
                return Err(ServerCodecError::RuleLimit);
            }
            let count = u16::try_from(rules.len()).map_err(|_| ServerCodecError::RuleLimit)?;
            writer.byte(b'E')?;
            writer.bytes(&count.to_le_bytes())?;
            for rule in rules {
                writer.string(&rule.name)?;
                writer.string(&rule.value)?;
            }
        }
    }
    if output.len() < writer.bytes.len() {
        return Err(ServerCodecError::OutputTooSmall);
    }
    let written = &mut output[..writer.bytes.len()];
    written.copy_from_slice(&writer.bytes);
    Ok(EncodedResponse(written))
}

fn write_info(writer: &mut Writer, info: &ServerInfo) -> Result<(), ServerCodecError> {
    match info.format {
        InfoFormat::Modern => {
            let app_id = info
                .app_id
                .ok_or(ServerCodecError::IncompatibleInfoFields)?;
            let version = info
                .version
                .as_ref()
                .ok_or(ServerCodecError::IncompatibleInfoFields)?;
            if info.legacy.is_some() {
                return Err(ServerCodecError::IncompatibleInfoFields);
            }
            if app_id == 2400 {
                return Err(ServerCodecError::UnsupportedInfoExtension);
            }
            writer.bytes(&[b'I', info.protocol])?;
            for text in [&info.name, &info.map, &info.folder, &info.game] {
                writer.string(text)?;
            }
            writer.bytes(&app_id.to_le_bytes())?;
            writer.bytes(&[
                info.players,
                info.max_players,
                info.bots,
                info.server_type,
                info.environment,
                u8::from(info.password_protected),
                u8::from(info.vac),
            ])?;
            writer.string(version)?;
            write_extra(writer, &info.extra)?;
        }
        InfoFormat::GoldSrc => {
            let legacy = info
                .legacy
                .as_ref()
                .ok_or(ServerCodecError::IncompatibleInfoFields)?;
            if info.app_id.is_some() || info.version.is_some() || info.extra != ExtraInfo::default()
            {
                return Err(ServerCodecError::IncompatibleInfoFields);
            }
            writer.byte(b'm')?;
            for text in [
                &legacy.address,
                &info.name,
                &info.map,
                &info.folder,
                &info.game,
            ] {
                writer.string(text)?;
            }
            writer.bytes(&[
                info.players,
                info.max_players,
                info.protocol,
                info.server_type,
                info.environment,
                u8::from(info.password_protected),
                u8::from(legacy.modification.is_some()),
            ])?;
            if let Some(modification) = &legacy.modification {
                writer.string(&modification.website)?;
                writer.string(&modification.download)?;
                writer.byte(0)?;
                writer.bytes(&modification.version.to_le_bytes())?;
                writer.bytes(&modification.size.to_le_bytes())?;
                writer.bytes(&[
                    u8::from(modification.multiplayer_only),
                    u8::from(modification.custom_dll),
                ])?;
            }
            writer.bytes(&[u8::from(info.vac), info.bots])?;
        }
    }
    Ok(())
}

fn write_extra(writer: &mut Writer, extra: &ExtraInfo) -> Result<(), ServerCodecError> {
    let flags = (u8::from(extra.game_port.is_some()) * 0x80)
        | (u8::from(extra.steam_id.is_some()) * 0x10)
        | (u8::from(extra.spectator.is_some()) * 0x40)
        | (u8::from(extra.keywords.is_some()) * 0x20)
        | u8::from(extra.game_id.is_some());
    writer.byte(flags)?;
    if let Some(port) = extra.game_port {
        writer.bytes(&port.to_le_bytes())?;
    }
    if let Some(id) = extra.steam_id {
        writer.bytes(&id.to_le_bytes())?;
    }
    if let Some(spectator) = &extra.spectator {
        writer.bytes(&spectator.port.to_le_bytes())?;
        writer.string(&spectator.name)?;
    }
    if let Some(keywords) = &extra.keywords {
        writer.string(keywords)?;
    }
    if let Some(id) = extra.game_id {
        writer.bytes(&id.to_le_bytes())?;
    }
    Ok(())
}

struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn bytes(&mut self, value: &[u8]) -> Result<(), ServerCodecError> {
        if value.len() > MAX_RESPONSE_BYTES - self.bytes.len() {
            return Err(ServerCodecError::ResponseLimit);
        }
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn byte(&mut self, value: u8) -> Result<(), ServerCodecError> {
        self.bytes(&[value])
    }

    fn string(&mut self, value: &str) -> Result<(), ServerCodecError> {
        if value.len() > MAX_STRING_BYTES {
            return Err(ServerCodecError::StringLimit);
        }
        if value.as_bytes().contains(&0) {
            return Err(ServerCodecError::EmbeddedNul);
        }
        self.bytes(value.as_bytes())?;
        self.byte(0)
    }
}

/// One complete UDP datagram with redacted automatic formatting.
#[derive(Clone, Eq, PartialEq)]
pub struct QueryDatagram(Vec<u8>);

impl QueryDatagram {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for QueryDatagram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryDatagram")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// All datagrams for one validated response, built before any are published.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseDatagrams(Vec<QueryDatagram>);

impl ResponseDatagrams {
    pub fn datagrams(&self) -> &[QueryDatagram] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Packetize a validated response using only the documented GoldSource layout.
///
/// Small responses keep their ordinary unsplit header. For larger responses,
/// each fragment has a nine-byte header; fragment zero retains the complete
/// unsplit response header. Both the 15-part budget and high-bit-ID restriction
/// are enforced before allocating fragments. The ID is caller-selected opaque
/// metadata, not a query challenge or an authenticated session identifier.
pub fn goldsrc_response_datagrams(
    response: EncodedResponse<'_>,
    response_id: u32,
) -> Result<ResponseDatagrams, ServerCodecError> {
    if response_id & 0x8000_0000 != 0 {
        return Err(ServerCodecError::UnsupportedSplitIdentifier);
    }
    if response.len() <= MAX_DATAGRAM_BYTES {
        return Ok(ResponseDatagrams(vec![QueryDatagram(
            response.as_bytes().to_vec(),
        )]));
    }
    let count = response.len().div_ceil(SPLIT_PAYLOAD_BYTES);
    if count > MAX_FRAGMENTS {
        return Err(ServerCodecError::FragmentLimit);
    }
    let count_byte = u8::try_from(count).map_err(|_| ServerCodecError::FragmentLimit)?;
    let mut packets = Vec::with_capacity(count);
    for (index, part) in response.as_bytes().chunks(SPLIT_PAYLOAD_BYTES).enumerate() {
        let index_byte = u8::try_from(index).map_err(|_| ServerCodecError::FragmentLimit)?;
        let mut packet = Vec::with_capacity(SPLIT_HEADER_BYTES + part.len());
        packet.extend_from_slice(&SPLIT_HEADER);
        packet.extend_from_slice(&response_id.to_le_bytes());
        packet.push(index_byte << 4 | count_byte);
        packet.extend_from_slice(part);
        packets.push(QueryDatagram(packet));
    }
    Ok(ResponseDatagrams(packets))
}
