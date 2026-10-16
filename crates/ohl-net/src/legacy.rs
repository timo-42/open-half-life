//! Bounded, explicitly selected historical ASCII server-query codecs.
//!
//! The independently authored implementation follows the **Key** field tables
//! and string-termination comment in the published [Half-Life Protocol] document.
//! Its packet dumps, names and resource strings are not implementation fixtures.
//! No engine, SDK, disassembly, game media or other implementation is a source.
//!
//! [`Profile::PublishedAscii`] selects little-endian integer words, IEEE-754
//! binary32 player time, strict UTF-8 response text and canonical boolean bytes
//! 0/1. The source documents NUL-terminated response strings and exact ASCII
//! requests without a terminator; it does not establish all the selected byte
//! primitives or applicability to a particular game build. This is therefore
//! **not** a claim of final pre-Steam/WON protocol compatibility. No profiles,
//! terminators, text encodings or later A2S layouts are guessed automatically.
//!
//! The separate [`Details`] record ends with `secure_server`, with no bot count.
//! [`Modification::unused`] is a full string, as published; it is not silently
//! reduced to the later A2S format's one reserved zero byte. There is no implicit
//! conversion to the later query metadata model. Player and rule list field
//! tables match the crate's existing entry models; duplicate entries and raw
//! byte-valued indicators are preserved. Finite negative durations and signed
//! modification values are also preserved, without host-policy interpretation.
//!
//! [`Profile::ObservedLinux3111d`] separately selects the 2026-10-17 public
//! Linux HLDS 3.1.1.1 base plus d-update reference cohort; actual runtime build
//! confirmation remains pending. Its independently reviewed observation found
//! three exact unterminated ASCII `details` requests and three 60-byte `m`
//! replies: the published fields consumed 59 bytes and one zero byte remained.
//! The replies reported protocol 46, zero players and capacity four. Evidence
//! is the complete-capture SHA-256
//! `14af134e701648220352a2d3e46dbedcb3b0678145cfd6b0d055f378d0e2ec57`;
//! no captured contents or names are fixtures. This does not establish behavior
//! for all protocol-46 implementations or a gameplay connection.
//!
//! This explicit profile requires protocol 46 as a project safety guard and
//! exactly one opaque byte after the published details. The byte has no assigned
//! meaning and is never synthesized. Other extension values and other published
//! field combinations, including mod metadata, are structural candidates rather
//! than observed cases. Players and rules retain only the documented candidate
//! codecs under either profile; that observation did not probe those queries.
//! Primitive selections remain provisional. [`Profile::PublishedAscii`] still
//! rejects the extra byte; neither profile falls back to the other.
//!
//! Project bounds are 1400-byte requests, 64 KiB complete logical responses,
//! 4096 UTF-8 bytes per response string, 255 players and 1024 rules. All lengths,
//! counts and text are checked before allocating or publishing their result.
//! Encoding errors preserve the entire caller buffer. Errors and this module's
//! automatic formatting redact packet contents and text. Explicit typed fields
//! and [`EncodedMessage::as_bytes`] give the host deliberate access.
//!
//! This module has no UDP I/O, response splitting/reassembly, challenge policy,
//! player admission or gameplay transport. A logical response over the datagram
//! budget requires a separately evidenced transport; it must not be sent as a
//! supposedly compatible single datagram. Queries are not gameplay connections.
//!
//! [Half-Life Protocol]: https://int64.org/docs/gamestat-protocols/halflife.html

use std::fmt;

use crate::{
    MAX_DATAGRAM_BYTES, MAX_RESPONSE_BYTES, MAX_RULES, MAX_STRING_BYTES, PlayerInfo, SINGLE_HEADER,
    ServerRule,
};

/// Explicit byte-primitive selection; this does not identify a runtime build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    /// Published ASCII commands and field order with the selected primitives
    /// described in the module documentation; no final pre-Steam claim.
    PublishedAscii,
    /// Public Linux HLDS 3.1.1.1 base plus d-update cohort observed 2026-10-17,
    /// with actual build confirmation pending. See the module's bounded
    /// evidence and candidate-layout limitations; this is not all protocol 46.
    ObservedLinux3111d,
}

/// Published ASCII requests, without a NUL, newline or appended challenge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request {
    Details,
    Players,
    Rules,
}

impl Request {
    const fn text(self) -> &'static [u8] {
        match self {
            Self::Details => b"details",
            Self::Players => b"players",
            Self::Rules => b"rules",
        }
    }
}

/// Historical details; the bot count is not present in the published layout.
#[derive(Clone, Eq, PartialEq)]
pub struct Details {
    /// Untrusted informational text, never a replacement for the UDP sender.
    pub address: String,
    pub name: String,
    pub map: String,
    pub game_directory: String,
    pub game_description: String,
    pub players: u8,
    pub max_players: u8,
    pub protocol: u8,
    pub server_type: u8,
    pub environment: u8,
    pub password_protected: bool,
    pub modification: Option<Modification>,
    pub secure_server: bool,
}

impl fmt::Debug for Details {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Details { .. }")
    }
}

/// Published details plus the reference cohort's one opaque trailing byte.
///
/// Only a zero extension was observed. Other byte values are preserved as
/// structural candidates without assigning semantics or claiming observation.
#[derive(Clone, Eq, PartialEq)]
pub struct ObservedDetails {
    pub published: Details,
    pub opaque_extension: u8,
}

impl fmt::Debug for ObservedDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ObservedDetails { .. }")
    }
}

/// Conditional mod fields, preserving the published unused **string**.
#[derive(Clone, Eq, PartialEq)]
pub struct Modification {
    pub website: String,
    pub download_server: String,
    pub unused: String,
    pub version: i32,
    pub size: i32,
    pub server_side_only: bool,
    pub custom_dll: bool,
}

impl fmt::Debug for Modification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Modification { .. }")
    }
}

/// One complete response in the explicitly selected historical profile.
#[derive(Clone, PartialEq)]
pub enum Response {
    Details(Box<Details>),
    ObservedDetails(Box<ObservedDetails>),
    Players(Vec<PlayerInfo>),
    Rules(Vec<ServerRule>),
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Details(_) => f.write_str("Response::Details { .. }"),
            Self::ObservedDetails(_) => f.write_str("Response::ObservedDetails { .. }"),
            Self::Players(entries) => f
                .debug_struct("Response::Players")
                .field("count", &entries.len())
                .finish_non_exhaustive(),
            Self::Rules(entries) => f
                .debug_struct("Response::Rules")
                .field("count", &entries.len())
                .finish_non_exhaustive(),
        }
    }
}

/// Fixed errors; no untrusted byte, field, token, string or address is retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    DatagramLimit,
    ResponseLimit,
    Truncated,
    InvalidHeader,
    UnsupportedRequest,
    UnsupportedResponse,
    TrailingData,
    StringLimit,
    InvalidUtf8,
    EmbeddedNul,
    InvalidBoolean,
    PlayerLimit,
    RuleLimit,
    NonFiniteDuration,
    OutputTooSmall,
    ProfileMismatch,
    ObservedProtocolGuard,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DatagramLimit => "historical query request exceeds datagram limit",
            Self::ResponseLimit => "historical query response exceeds logical limit",
            Self::Truncated => "truncated historical query message",
            Self::InvalidHeader => "invalid historical query header",
            Self::UnsupportedRequest => "unsupported historical query request",
            Self::UnsupportedResponse => "unsupported historical query response",
            Self::TrailingData => "trailing historical query data",
            Self::StringLimit => "historical query string limit exceeded",
            Self::InvalidUtf8 => "historical query text does not fit selected UTF-8 profile",
            Self::EmbeddedNul => "historical query string contains a NUL byte",
            Self::InvalidBoolean => "historical query boolean does not fit selected profile",
            Self::PlayerLimit => "historical query player count limit exceeded",
            Self::RuleLimit => "historical query rule count limit exceeded",
            Self::NonFiniteDuration => "historical query player duration is not finite",
            Self::OutputTooSmall => "historical query output buffer is too small",
            Self::ProfileMismatch => {
                "historical query response shape does not fit selected profile"
            }
            Self::ObservedProtocolGuard => {
                "historical query protocol does not fit observed-profile guard"
            }
        })
    }
}

impl std::error::Error for Error {}

/// Encoded request or complete logical response, with redacted formatting.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct EncodedMessage<'a>(&'a [u8]);

impl<'a> EncodedMessage<'a> {
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

impl fmt::Debug for EncodedMessage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedMessage")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Parse exactly one documented ASCII request; suffixes are never guessed.
pub fn decode_request(profile: Profile, packet: &[u8]) -> Result<Request, Error> {
    match profile {
        Profile::PublishedAscii | Profile::ObservedLinux3111d => {
            if packet.len() > MAX_DATAGRAM_BYTES {
                return Err(Error::DatagramLimit);
            }
            let mut reader = Reader::new(packet);
            reader.header()?;
            let command = reader.bytes;
            for request in [Request::Details, Request::Players, Request::Rules] {
                if command == request.text() {
                    return Ok(request);
                }
                if command.starts_with(request.text()) {
                    return Err(Error::TrailingData);
                }
                if request.text().starts_with(command) {
                    return Err(Error::Truncated);
                }
            }
            Err(Error::UnsupportedRequest)
        }
    }
}

/// Encode an exact ASCII request, preserving all output bytes on error.
pub fn encode_request(
    profile: Profile,
    request: Request,
    output: &mut [u8],
) -> Result<EncodedMessage<'_>, Error> {
    match profile {
        Profile::PublishedAscii | Profile::ObservedLinux3111d => {
            let text = request.text();
            let length = SINGLE_HEADER.len() + text.len();
            let written = output.get_mut(..length).ok_or(Error::OutputTooSmall)?;
            written[..4].copy_from_slice(&SINGLE_HEADER);
            written[4..].copy_from_slice(text);
            Ok(EncodedMessage(written))
        }
    }
}

/// Parse one complete logical response; split and later A2S layouts are absent.
pub fn decode_response(profile: Profile, packet: &[u8]) -> Result<Response, Error> {
    match profile {
        Profile::PublishedAscii | Profile::ObservedLinux3111d => {
            if packet.len() > MAX_RESPONSE_BYTES {
                return Err(Error::ResponseLimit);
            }
            let mut reader = Reader::new(packet);
            reader.header()?;
            let response = match reader.byte()? {
                b'm' => read_profile_details(profile, &mut reader)?,
                b'D' => Response::Players(read_players(&mut reader)?),
                b'E' => Response::Rules(read_rules(&mut reader)?),
                _ => return Err(Error::UnsupportedResponse),
            };
            if !reader.bytes.is_empty() {
                return Err(Error::TrailingData);
            }
            Ok(response)
        }
    }
}

/// Validate and encode before publishing any caller-buffer change.
pub fn encode_response<'output>(
    profile: Profile,
    response: &Response,
    output: &'output mut [u8],
) -> Result<EncodedMessage<'output>, Error> {
    match profile {
        Profile::PublishedAscii | Profile::ObservedLinux3111d => {
            let mut writer = Writer { bytes: Vec::new() };
            writer.bytes(&SINGLE_HEADER)?;
            match response {
                Response::Details(details) => {
                    if profile != Profile::PublishedAscii {
                        return Err(Error::ProfileMismatch);
                    }
                    write_details(&mut writer, details)?;
                }
                Response::ObservedDetails(details) => {
                    if profile != Profile::ObservedLinux3111d {
                        return Err(Error::ProfileMismatch);
                    }
                    observed_protocol_guard(&details.published)?;
                    write_details(&mut writer, &details.published)?;
                    writer.byte(details.opaque_extension)?;
                }
                Response::Players(players) => {
                    let count = u8::try_from(players.len()).map_err(|_| Error::PlayerLimit)?;
                    writer.bytes(&[b'D', count])?;
                    for player in players {
                        if !player.duration_seconds.is_finite() {
                            return Err(Error::NonFiniteDuration);
                        }
                        writer.byte(player.index)?;
                        writer.string(&player.name)?;
                        writer.bytes(&player.score.to_le_bytes())?;
                        writer.bytes(&player.duration_seconds.to_le_bytes())?;
                    }
                }
                Response::Rules(rules) => {
                    if rules.len() > MAX_RULES {
                        return Err(Error::RuleLimit);
                    }
                    let count = u16::try_from(rules.len()).map_err(|_| Error::RuleLimit)?;
                    writer.byte(b'E')?;
                    writer.bytes(&count.to_le_bytes())?;
                    for rule in rules {
                        writer.string(&rule.name)?;
                        writer.string(&rule.value)?;
                    }
                }
            }
            let written = output
                .get_mut(..writer.bytes.len())
                .ok_or(Error::OutputTooSmall)?;
            written.copy_from_slice(&writer.bytes);
            Ok(EncodedMessage(written))
        }
    }
}

fn read_profile_details(profile: Profile, reader: &mut Reader<'_>) -> Result<Response, Error> {
    let published = read_details(reader)?;
    match profile {
        Profile::PublishedAscii => Ok(Response::Details(Box::new(published))),
        Profile::ObservedLinux3111d => {
            observed_protocol_guard(&published)?;
            let opaque_extension = reader.byte()?;
            Ok(Response::ObservedDetails(Box::new(ObservedDetails {
                published,
                opaque_extension,
            })))
        }
    }
}

fn observed_protocol_guard(details: &Details) -> Result<(), Error> {
    if details.protocol == 46 {
        Ok(())
    } else {
        Err(Error::ObservedProtocolGuard)
    }
}

fn read_details(reader: &mut Reader<'_>) -> Result<Details, Error> {
    let address = reader.string()?;
    let name = reader.string()?;
    let map = reader.string()?;
    let game_directory = reader.string()?;
    let game_description = reader.string()?;
    let players = reader.byte()?;
    let max_players = reader.byte()?;
    let protocol = reader.byte()?;
    let server_type = reader.byte()?;
    let environment = reader.byte()?;
    let password_protected = reader.boolean()?;
    let modification = if reader.boolean()? {
        Some(Modification {
            website: reader.string()?,
            download_server: reader.string()?,
            unused: reader.string()?,
            version: reader.i32()?,
            size: reader.i32()?,
            server_side_only: reader.boolean()?,
            custom_dll: reader.boolean()?,
        })
    } else {
        None
    };
    Ok(Details {
        address,
        name,
        map,
        game_directory,
        game_description,
        players,
        max_players,
        protocol,
        server_type,
        environment,
        password_protected,
        modification,
        secure_server: reader.boolean()?,
    })
}

fn write_details(writer: &mut Writer, details: &Details) -> Result<(), Error> {
    writer.byte(b'm')?;
    for text in [
        &details.address,
        &details.name,
        &details.map,
        &details.game_directory,
        &details.game_description,
    ] {
        writer.string(text)?;
    }
    writer.bytes(&[
        details.players,
        details.max_players,
        details.protocol,
        details.server_type,
        details.environment,
        u8::from(details.password_protected),
        u8::from(details.modification.is_some()),
    ])?;
    if let Some(modification) = &details.modification {
        writer.string(&modification.website)?;
        writer.string(&modification.download_server)?;
        writer.string(&modification.unused)?;
        writer.bytes(&modification.version.to_le_bytes())?;
        writer.bytes(&modification.size.to_le_bytes())?;
        writer.bytes(&[
            u8::from(modification.server_side_only),
            u8::from(modification.custom_dll),
        ])?;
    }
    writer.byte(u8::from(details.secure_server))
}

fn read_players(reader: &mut Reader<'_>) -> Result<Vec<PlayerInfo>, Error> {
    let count = usize::from(reader.byte()?);
    // Index, empty-name terminator, score and time require at least ten bytes.
    if count > reader.bytes.len() / 10 {
        return Err(Error::Truncated);
    }
    let mut players = Vec::with_capacity(count);
    for _ in 0..count {
        let index = reader.byte()?;
        let name = reader.string()?;
        let score = reader.i32()?;
        let duration_seconds = f32::from_le_bytes(reader.array()?);
        if !duration_seconds.is_finite() {
            return Err(Error::NonFiniteDuration);
        }
        players.push(PlayerInfo {
            index,
            name,
            score,
            duration_seconds,
        });
    }
    Ok(players)
}

fn read_rules(reader: &mut Reader<'_>) -> Result<Vec<ServerRule>, Error> {
    let count = usize::from(u16::from_le_bytes(reader.array()?));
    if count > MAX_RULES {
        return Err(Error::RuleLimit);
    }
    if count > reader.bytes.len() / 2 {
        return Err(Error::Truncated);
    }
    let mut rules = Vec::with_capacity(count);
    for _ in 0..count {
        rules.push(ServerRule {
            name: reader.string()?,
            value: reader.string()?,
        });
    }
    Ok(rules)
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let part = self.bytes.get(..count).ok_or(Error::Truncated)?;
        self.bytes = &self.bytes[count..];
        Ok(part)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| Error::Truncated)
    }

    fn header(&mut self) -> Result<(), Error> {
        if self.take(4)? == SINGLE_HEADER {
            Ok(())
        } else {
            Err(Error::InvalidHeader)
        }
    }

    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn boolean(&mut self) -> Result<bool, Error> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::InvalidBoolean),
        }
    }

    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn string(&mut self) -> Result<String, Error> {
        let end = self
            .bytes
            .iter()
            .take(MAX_STRING_BYTES + 1)
            .position(|byte| *byte == 0)
            .ok_or(if self.bytes.len() > MAX_STRING_BYTES {
                Error::StringLimit
            } else {
                Error::Truncated
            })?;
        let text = std::str::from_utf8(self.take(end)?)
            .map_err(|_| Error::InvalidUtf8)?
            .to_owned();
        self.take(1)?;
        Ok(text)
    }
}

struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > MAX_RESPONSE_BYTES - self.bytes.len() {
            return Err(Error::ResponseLimit);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn byte(&mut self, byte: u8) -> Result<(), Error> {
        self.bytes(&[byte])
    }

    fn string(&mut self, text: &str) -> Result<(), Error> {
        if text.len() > MAX_STRING_BYTES {
            return Err(Error::StringLimit);
        }
        if text.as_bytes().contains(&0) {
            return Err(Error::EmbeddedNul);
        }
        self.bytes(text.as_bytes())?;
        self.byte(0)
    }
}
