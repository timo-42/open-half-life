//! Bounded parsing of a documented, byte-aligned GoldSrc server-message subset.
//!
//! This module accepts an already isolated server-message payload. It does not
//! implement a network channel, reliability, authentication, sign-on, delta
//! compression, or a playable connection. An eventual channel adapter must
//! establish the payload boundary before calling this reader.
//!
//! # Public prose and evidence limits
//!
//! Opcode numbers and message field order come only from AlliedModders Wiki's
//! [Half-Life 1 Engine Messages], revision 11325, retrieved 2026-10-16. Each
//! supported variant links to its field table. The page's engine/SDK code
//! excerpts are not implementation sources.
//!
//! That page does not fully define primitive framing. [`ByteModeProfile`] makes
//! the selected interpretation explicit: little-endian 16/32-bit integer words,
//! four uninterpreted bytes for a `float`, and NUL-terminated byte strings. These
//! rules are a **provisional profile**, inferred from the published Valve
//! [query data types]; their application to these svc messages has not
//! been independently verified. Float bit patterns are never interpreted.
//! Selecting the profile does not prove compatibility with a server build.
//!
//! No bit-packed or delta-coded fields are guessed. In particular,
//! `SVC_NEWUSERMSG` is unsupported: its published name field says 16 bits while
//! the prose says four `long`s. `SVC_SIGNONNUM` accepts only the literal byte 1
//! in its table. Unknown opcodes and unsupported layouts terminate the reader;
//! it never scans the remainder looking for another apparent opcode.
//!
//! Text is borrowed opaque data, including `SVC_STUFFTEXT`. No command is
//! executed, text encoding guessed, resource opened, or cvar changed. Debug
//! output redacts text, opaque hashes, and map CRC fingerprints. All fixtures are
//! independently authored synthetic bytes, not captured or extracted game data.
//!
//! [Half-Life 1 Engine Messages]: https://wiki.alliedmods.net/index.php?title=Half-Life_1_Engine_Messages&oldid=11325
//! [query data types]: https://web.archive.org/web/20230831123627id_/https://developer.valvesoftware.com/wiki/Server_queries#Data_types

use std::{fmt, iter::FusedIterator};

/// Project bound on one already isolated server-message payload.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
/// Project bound on successfully parsed messages in one payload.
pub const MAX_MESSAGES: usize = 4096;
/// Project bound on bytes before a text field's NUL terminator.
pub const MAX_TEXT_BYTES: usize = 4096;
/// Project bound informed by the server-info table's "up to 8 KB" note.
pub const MAX_MAPCYCLE_BYTES: usize = 8 * 1024;

/// Explicit selection of provisional svc primitive framing.
///
/// Field order is documented separately. This profile is not a declaration that
/// the primitive rules have been verified for an original gameplay channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteModeProfile {
    /// LE 16/32-bit integer words, raw four-byte floats, NUL byte strings.
    ///
    /// Applying the published query primitive rules to svc is an inference.
    InferredLittleEndianNulStrings,
}

/// Borrowed untrusted string bytes, excluding the wire NUL terminator.
///
/// No UTF-8, command, file-name, or display interpretation is performed.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct WireText<'a>(&'a [u8]);

impl<'a> WireText<'a> {
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

impl fmt::Debug for WireText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WireText")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Uninterpreted four-byte field described as `float` in the message table.
///
/// Keeping wire bytes avoids claiming verified svc float encoding or accepting
/// an untrusted floating-point value into simulation state.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct WireFloat([u8; 4]);

impl WireFloat {
    pub const fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

impl fmt::Debug for WireFloat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WireFloat { .. }")
    }
}

/// Opaque 16-byte hash whose debug representation never reveals its contents.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct WireHash([u8; 16]);

impl WireHash {
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Debug for WireHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WireHash { .. }")
    }
}

/// The exact field sequence in the published [`SVC_SERVERINFO` table].
///
/// Byte fields are preserved rather than inventing build-specific value ranges.
/// The table's final reserved zero is validated and omitted from this value.
///
/// [`SVC_SERVERINFO` table]: https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_SERVERINFO
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ServerInfoPayload<'a> {
    pub protocol: i32,
    pub spawn_count: i32,
    /// Exact fingerprint, available explicitly but redacted in Debug output.
    pub map_crc: i32,
    pub client_dll_hash: WireHash,
    pub max_players: u8,
    pub player_index: u8,
    pub is_deathmatch: u8,
    pub game_dir: WireText<'a>,
    pub hostname: WireText<'a>,
    pub map_file_name: WireText<'a>,
    pub mapcycle: WireText<'a>,
}

impl fmt::Debug for ServerInfoPayload<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerInfoPayload")
            .field("protocol", &self.protocol)
            .field("spawn_count", &self.spawn_count)
            .field("map_crc", &"redacted")
            .field("client_dll_hash", &self.client_dll_hash)
            .field("max_players", &self.max_players)
            .field("player_index", &self.player_index)
            .field("is_deathmatch", &self.is_deathmatch)
            .field("game_dir", &self.game_dir)
            .field("hostname", &self.hostname)
            .field("map_file_name", &self.map_file_name)
            .field("mapcycle", &self.mapcycle)
            .finish()
    }
}

/// Supported original opcode/field tables, interpreted with the selected profile.
///
/// These values describe payload data. They trigger no client action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerMessage<'a> {
    /// Opcode 1, no fields: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_NOP).
    Nop,
    /// Opcode 2, reason string: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_DISCONNECT).
    Disconnect { reason: WireText<'a> },
    /// Opcode 4, protocol long: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_VERSION).
    Version { protocol: i32 },
    /// Opcode 5, entity short: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_SETVIEW).
    SetView { entity_index: i16 },
    /// Opcode 7, uninterpreted float: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_TIME).
    Time { time: WireFloat },
    /// Opcode 8, message string: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_PRINT).
    Print { text: WireText<'a> },
    /// Opcode 9, opaque command string: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_STUFFTEXT).
    ///
    /// This parser never executes server-supplied commands.
    StuffText { command: WireText<'a> },
    /// Opcode 10, three raw angle shorts in pitch/yaw/roll order:
    /// [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_SETANGLE).
    SetAngle { angles: [i16; 3] },
    /// Opcode 11: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_SERVERINFO).
    ServerInfo(ServerInfoPayload<'a>),
    /// Opcode 24, pause byte 0 or 1: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_SETPAUSE).
    SetPause { paused: bool },
    /// Opcode 25, literal byte 1: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_SIGNONNUM).
    ///
    /// Other values are unsupported; no Quake sign-on stages are inferred.
    SignOnActive,
    /// Opcode 26, message string: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_CENTERPRINT).
    CenterPrint { text: WireText<'a> },
    /// Opcode 30, no fields: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_INTERMISSION).
    Intermission,
    /// Opcode 42, no fields: [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_CHOKE).
    Choke,
    /// Opcode 54, fallback string and preserved cheat byte:
    /// [table](https://wiki.alliedmods.net/Half-Life_1_Engine_Messages#SVC_SENDEXTRAINFO).
    SendExtraInfo {
        fallback_directory: WireText<'a>,
        can_cheat: u8,
    },
}

impl ServerMessage<'_> {
    pub const fn opcode(self) -> u8 {
        match self {
            Self::Nop => 1,
            Self::Disconnect { .. } => 2,
            Self::Version { .. } => 4,
            Self::SetView { .. } => 5,
            Self::Time { .. } => 7,
            Self::Print { .. } => 8,
            Self::StuffText { .. } => 9,
            Self::SetAngle { .. } => 10,
            Self::ServerInfo(_) => 11,
            Self::SetPause { .. } => 24,
            Self::SignOnActive => 25,
            Self::CenterPrint { .. } => 26,
            Self::Intermission => 30,
            Self::Choke => 42,
            Self::SendExtraInfo { .. } => 54,
        }
    }
}

/// Fixed diagnostic classes. Automatic formatting omits packet content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageErrorKind {
    PayloadLimit,
    MessageLimit,
    TextLimit,
    Truncated,
    UnsupportedOpcode,
    UnsupportedLayout,
    InvalidPause,
    NonZeroReservedByte,
}

/// Typed error context with an absolute payload offset and optional raw opcode.
///
/// The opcode remains available for explicit inspection; automatic Debug and
/// Display output omit that untrusted packet byte.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct MessageError {
    pub kind: MessageErrorKind,
    pub offset: usize,
    pub opcode: Option<u8>,
}

impl fmt::Debug for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageError")
            .field("kind", &self.kind)
            .field("offset", &self.offset)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self.kind {
            MessageErrorKind::PayloadLimit => "payload limit exceeded",
            MessageErrorKind::MessageLimit => "message count limit exceeded",
            MessageErrorKind::TextLimit => "text field limit exceeded",
            MessageErrorKind::Truncated => "truncated message",
            MessageErrorKind::UnsupportedOpcode => "unsupported opcode",
            MessageErrorKind::UnsupportedLayout => "unsupported message layout",
            MessageErrorKind::InvalidPause => "invalid pause byte",
            MessageErrorKind::NonZeroReservedByte => "nonzero reserved byte",
        };
        write!(f, "server message {description} at byte {}", self.offset)
    }
}

impl std::error::Error for MessageError {}

/// Borrowing, allocation-free, fail-stop reader of one complete payload.
///
/// A valid prefix is yielded as individual messages. The first malformed,
/// unsupported, or over-budget message produces one error and permanently ends
/// iteration. [`Self::position`] remains at the start of that failed message;
/// the error's offset identifies the field which failed. Do not apply a prefix
/// to client state unless the channel/session policy permits partial payloads.
pub struct MessageReader<'a> {
    payload: &'a [u8],
    profile: ByteModeProfile,
    position: usize,
    message_count: usize,
    stopped: bool,
}

impl<'a> MessageReader<'a> {
    pub fn new(payload: &'a [u8], profile: ByteModeProfile) -> Result<Self, MessageError> {
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(MessageError {
                kind: MessageErrorKind::PayloadLimit,
                offset: 0,
                opcode: None,
            });
        }
        Ok(Self {
            payload,
            profile,
            position: 0,
            message_count: 0,
            stopped: false,
        })
    }

    /// Number of bytes belonging to successfully parsed messages.
    pub const fn position(&self) -> usize {
        self.position
    }

    pub const fn message_count(&self) -> usize {
        self.message_count
    }

    pub const fn profile(&self) -> ByteModeProfile {
        self.profile
    }
}

impl fmt::Debug for MessageReader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageReader")
            .field("payload_length", &self.payload.len())
            .field("profile", &self.profile)
            .field("position", &self.position)
            .field("message_count", &self.message_count)
            .field("stopped", &self.stopped)
            .finish()
    }
}

impl<'a> Iterator for MessageReader<'a> {
    type Item = Result<ServerMessage<'a>, MessageError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.stopped || self.position == self.payload.len() {
            self.stopped = true;
            return None;
        }
        let opcode = self.payload[self.position];
        if self.message_count == MAX_MESSAGES {
            self.stopped = true;
            return Some(Err(MessageError {
                kind: MessageErrorKind::MessageLimit,
                offset: self.position,
                opcode: Some(opcode),
            }));
        }
        let mut fields = Fields {
            payload: self.payload,
            position: self.position + 1,
            opcode,
        };
        let result = fields.message();
        match result {
            Ok(message) => {
                self.position = fields.position;
                self.message_count += 1;
                Some(Ok(message))
            }
            Err(error) => {
                self.stopped = true;
                Some(Err(error))
            }
        }
    }
}

impl FusedIterator for MessageReader<'_> {}

struct Fields<'a> {
    payload: &'a [u8],
    position: usize,
    opcode: u8,
}

impl<'a> Fields<'a> {
    fn error(&self, kind: MessageErrorKind) -> MessageError {
        MessageError {
            kind,
            offset: self.position,
            opcode: Some(self.opcode),
        }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], MessageError> {
        let bytes = self
            .payload
            .get(self.position..)
            .and_then(|remaining| remaining.get(..N))
            .ok_or_else(|| self.error(MessageErrorKind::Truncated))?;
        let mut value = [0; N];
        value.copy_from_slice(bytes);
        self.position += N;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, MessageError> {
        Ok(self.array::<1>()?[0])
    }

    fn short(&mut self) -> Result<i16, MessageError> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    fn long(&mut self) -> Result<i32, MessageError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn text(&mut self, limit: usize) -> Result<WireText<'a>, MessageError> {
        let remaining = &self.payload[self.position..];
        let inspected = &remaining[..remaining.len().min(limit + 1)];
        let Some(length) = inspected.iter().position(|byte| *byte == 0) else {
            return Err(self.error(if remaining.len() > limit {
                MessageErrorKind::TextLimit
            } else {
                MessageErrorKind::Truncated
            }));
        };
        let value = WireText(&remaining[..length]);
        self.position += length + 1;
        Ok(value)
    }

    fn server_info(&mut self) -> Result<ServerInfoPayload<'a>, MessageError> {
        let value = ServerInfoPayload {
            protocol: self.long()?,
            spawn_count: self.long()?,
            map_crc: self.long()?,
            client_dll_hash: WireHash(self.array()?),
            max_players: self.byte()?,
            player_index: self.byte()?,
            is_deathmatch: self.byte()?,
            game_dir: self.text(MAX_TEXT_BYTES)?,
            hostname: self.text(MAX_TEXT_BYTES)?,
            map_file_name: self.text(MAX_TEXT_BYTES)?,
            mapcycle: self.text(MAX_MAPCYCLE_BYTES)?,
        };
        let offset = self.position;
        if self.byte()? != 0 {
            return Err(MessageError {
                kind: MessageErrorKind::NonZeroReservedByte,
                offset,
                opcode: Some(self.opcode),
            });
        }
        Ok(value)
    }

    fn message(&mut self) -> Result<ServerMessage<'a>, MessageError> {
        Ok(match self.opcode {
            1 => ServerMessage::Nop,
            2 => ServerMessage::Disconnect {
                reason: self.text(MAX_TEXT_BYTES)?,
            },
            4 => ServerMessage::Version {
                protocol: self.long()?,
            },
            5 => ServerMessage::SetView {
                entity_index: self.short()?,
            },
            7 => ServerMessage::Time {
                time: WireFloat(self.array()?),
            },
            8 => ServerMessage::Print {
                text: self.text(MAX_TEXT_BYTES)?,
            },
            9 => ServerMessage::StuffText {
                command: self.text(MAX_TEXT_BYTES)?,
            },
            10 => ServerMessage::SetAngle {
                angles: [self.short()?, self.short()?, self.short()?],
            },
            11 => ServerMessage::ServerInfo(self.server_info()?),
            24 => {
                let offset = self.position;
                let paused = match self.byte()? {
                    0 => false,
                    1 => true,
                    _ => {
                        return Err(MessageError {
                            kind: MessageErrorKind::InvalidPause,
                            offset,
                            opcode: Some(self.opcode),
                        });
                    }
                };
                ServerMessage::SetPause { paused }
            }
            25 => {
                let offset = self.position;
                if self.byte()? != 1 {
                    return Err(MessageError {
                        kind: MessageErrorKind::UnsupportedLayout,
                        offset,
                        opcode: Some(self.opcode),
                    });
                }
                ServerMessage::SignOnActive
            }
            26 => ServerMessage::CenterPrint {
                text: self.text(MAX_TEXT_BYTES)?,
            },
            30 => ServerMessage::Intermission,
            39 => {
                return Err(MessageError {
                    kind: MessageErrorKind::UnsupportedLayout,
                    offset: self.position - 1,
                    opcode: Some(self.opcode),
                });
            }
            42 => ServerMessage::Choke,
            54 => ServerMessage::SendExtraInfo {
                fallback_directory: self.text(MAX_TEXT_BYTES)?,
                can_cheat: self.byte()?,
            },
            _ => {
                return Err(MessageError {
                    kind: MessageErrorKind::UnsupportedOpcode,
                    offset: self.position - 1,
                    opcode: Some(self.opcode),
                });
            }
        })
    }
}
