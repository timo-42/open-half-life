//! Field readers follow the public Server queries tables cited by the crate.

use crate::{
    Error, ExtraInfo, InfoFormat, LegacyInfo, MAX_RESPONSE_BYTES, MAX_RULES, MAX_STRING_BYTES,
    ModInfo, PlayerInfo, Query, Response, SINGLE_HEADER, ServerInfo, ServerRule, SpectatorInfo,
};

/// Encode an A2S request. INFO omits an absent challenge; PLAYER/RULES use -1.
pub fn encode_request(query: Query, challenge: Option<i32>) -> Vec<u8> {
    let mut packet = Vec::with_capacity(29);
    packet.extend_from_slice(&SINGLE_HEADER);
    match query {
        Query::Info => {
            packet.push(b'T');
            packet.extend_from_slice(b"Source Engine Query\0");
            if let Some(challenge) = challenge {
                packet.extend_from_slice(&challenge.to_le_bytes());
            }
        }
        Query::Players | Query::Rules => {
            packet.push(if query == Query::Players { b'U' } else { b'V' });
            packet.extend_from_slice(&challenge.unwrap_or(-1).to_le_bytes());
        }
    }
    packet
}

/// Decode a complete connectionless response, including its four `FF` bytes.
///
/// Split datagrams must be assembled first. Unknown response IDs, unknown EDF
/// bits, trailing bytes, invalid UTF-8 and noncanonical boolean bytes are rejected.
/// Game-specific response additions are outside this Half-Life query subset.
pub fn decode_response(packet: &[u8]) -> Result<Response, Error> {
    if packet.len() > MAX_RESPONSE_BYTES {
        return Err(Error::Limit("response bytes"));
    }
    let mut reader = Reader::new(packet);
    if reader.take(4)? != SINGLE_HEADER {
        return Err(Error::Invalid("connectionless header"));
    }
    let response = match reader.byte()? {
        b'A' => Response::Challenge(reader.i32()?),
        b'I' => Response::Info(Box::new(read_modern_info(&mut reader)?)),
        b'm' => Response::Info(Box::new(read_legacy_info(&mut reader)?)),
        b'D' => Response::Players(read_players(&mut reader)?),
        b'E' => Response::Rules(read_rules(&mut reader)?),
        _ => return Err(Error::Unsupported("response type")),
    };
    if !reader.remaining().is_empty() {
        return Err(Error::Unsupported("trailing response fields"));
    }
    Ok(response)
}

fn read_modern_info(reader: &mut Reader<'_>) -> Result<ServerInfo, Error> {
    let protocol = reader.byte()?;
    let name = reader.string()?;
    let map = reader.string()?;
    let folder = reader.string()?;
    let game = reader.string()?;
    let app_id = reader.u16()?;
    let players = reader.byte()?;
    let max_players = reader.byte()?;
    let bots = reader.byte()?;
    let server_type = reader.byte()?;
    let environment = reader.byte()?;
    let password_protected = reader.boolean()?;
    let vac = reader.boolean()?;
    // The public table describes an extra three-byte block for this other game.
    // Reject rather than accidentally interpreting those bytes as a version.
    if app_id == 2400 {
        return Err(Error::Unsupported("game-specific INFO extension"));
    }
    let version = Some(reader.string()?);
    let mut extra = ExtraInfo::default();
    if !reader.remaining().is_empty() {
        let flags = reader.byte()?;
        if flags & !0xf1 != 0 {
            return Err(Error::Unsupported("extra-data flags"));
        }
        if flags & 0x80 != 0 {
            extra.game_port = Some(reader.u16()?);
        }
        if flags & 0x10 != 0 {
            extra.steam_id = Some(reader.u64()?);
        }
        if flags & 0x40 != 0 {
            extra.spectator = Some(SpectatorInfo {
                port: reader.u16()?,
                name: reader.string()?,
            });
        }
        if flags & 0x20 != 0 {
            extra.keywords = Some(reader.string()?);
        }
        if flags & 0x01 != 0 {
            extra.game_id = Some(reader.u64()?);
        }
    }
    Ok(ServerInfo {
        format: InfoFormat::Modern,
        protocol,
        name,
        map,
        folder,
        game,
        app_id: Some(app_id),
        players,
        max_players,
        bots,
        server_type,
        environment,
        password_protected,
        vac,
        version,
        legacy: None,
        extra,
    })
}

fn read_legacy_info(reader: &mut Reader<'_>) -> Result<ServerInfo, Error> {
    let address = reader.string()?;
    let name = reader.string()?;
    let map = reader.string()?;
    let folder = reader.string()?;
    let game = reader.string()?;
    let players = reader.byte()?;
    let max_players = reader.byte()?;
    let protocol = reader.byte()?;
    let server_type = reader.byte()?;
    let environment = reader.byte()?;
    let password_protected = reader.boolean()?;
    let modification = if reader.boolean()? {
        let website = reader.string()?;
        let download = reader.string()?;
        if reader.byte()? != 0 {
            return Err(Error::Invalid("legacy reserved byte"));
        }
        Some(ModInfo {
            website,
            download,
            version: reader.u32()?,
            size: reader.u32()?,
            multiplayer_only: reader.boolean()?,
            custom_dll: reader.boolean()?,
        })
    } else {
        None
    };
    let vac = reader.boolean()?;
    let bots = reader.byte()?;
    Ok(ServerInfo {
        format: InfoFormat::GoldSrc,
        protocol,
        name,
        map,
        folder,
        game,
        app_id: None,
        players,
        max_players,
        bots,
        server_type,
        environment,
        password_protected,
        vac,
        version: None,
        legacy: Some(LegacyInfo {
            address,
            modification,
        }),
        extra: ExtraInfo::default(),
    })
}

fn read_players(reader: &mut Reader<'_>) -> Result<Vec<PlayerInfo>, Error> {
    let count = usize::from(reader.byte()?);
    // Each entry must contain an index, NUL, i32 and f32 before any allocation.
    if count > reader.remaining().len() / 10 {
        return Err(Error::Truncated);
    }
    let mut players = Vec::with_capacity(count);
    for _ in 0..count {
        let index = reader.byte()?;
        let name = reader.string()?;
        let score = reader.i32()?;
        let duration_seconds = f32::from_le_bytes(reader.array()?);
        if !duration_seconds.is_finite() {
            return Err(Error::Invalid("non-finite player duration"));
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
    let count = usize::from(reader.u16()?);
    if count > MAX_RULES {
        return Err(Error::Limit("rule count"));
    }
    if count > reader.remaining().len() / 2 {
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
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn remaining(&self) -> &'a [u8] {
        self.bytes
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let value = self.bytes.get(..count).ok_or(Error::Truncated)?;
        self.bytes = &self.bytes[count..];
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| Error::Truncated)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn boolean(&mut self) -> Result<bool, Error> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Invalid("boolean byte")),
        }
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn string(&mut self) -> Result<String, Error> {
        let end = self
            .bytes
            .iter()
            .take(MAX_STRING_BYTES + 1)
            .position(|byte| *byte == 0)
            .ok_or({
                if self.bytes.len() > MAX_STRING_BYTES {
                    Error::Limit("string bytes")
                } else {
                    Error::Truncated
                }
            })?;
        let value = std::str::from_utf8(self.take(end)?)
            .map_err(|_| Error::Invalid("UTF-8 string"))?
            .to_owned();
        self.take(1)?;
        Ok(value)
    }
}
