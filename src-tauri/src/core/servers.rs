//! The per-instance multiplayer server list, i.e. `servers.dat`.
//!
//! The file is a gzipped NBT compound holding a single `servers` list of
//! compounds, each with a name, an address and a couple of flags. MineArcade models
//! exactly the fields the game's own `ServerData` serialises, so a list written
//! here is shaped the same as one the game wrote: anything else a third-party
//! tool left in the file is dropped, which is precisely what the game does when
//! it saves the list itself.
//!
//! The format handling is split into [`parse`]/[`encode`] (bytes) and
//! [`read`]/[`write`] (paths) so it can be exercised without touching a disk.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fastnbt::Value;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

use crate::error::{LauncherError, Result};

/// The file the game keeps its multiplayer list in, relative to a game dir.
pub const SERVERS_FILE: &str = "servers.dat";

// The NBT keys of an entry. `ip` is what the game calls an address.
const KEY_NAME: &str = "name";
const KEY_ADDRESS: &str = "ip";
const KEY_ICON: &str = "icon";
const KEY_HIDDEN: &str = "hidden";
const KEY_ACCEPT_TEXTURES: &str = "acceptTextures";

/// One entry of the in-game multiplayer list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    /// Label shown in the list. Falls back to the address when the file has none.
    pub name: String,
    /// `host` or `host:port`, exactly as the game stores it.
    pub address: String,
    /// Base64 PNG shown beside the entry, without the `data:` prefix the game
    /// strips before saving. Left to the frontend to turn into an image.
    pub icon: Option<String>,
    /// Hidden entries only appear in game while "hidden servers" is toggled on.
    pub hidden: bool,
    /// Legacy "server sent resource packs" flag. The game still writes it, and
    /// MineArcade carries it rather than resetting a choice it did not make.
    pub accept_textures: bool,
}

/// A whole `servers.dat`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerList {
    pub servers: Vec<ServerEntry>,
}

impl ServerList {
    /// Where the file lives inside a given game directory.
    pub fn path_in(game_dir: &Path) -> PathBuf {
        game_dir.join(SERVERS_FILE)
    }

    /// Reject entries the game could not use, naming the offending row so the
    /// message points at something the user can actually see and fix.
    pub fn validate(&self) -> Result<()> {
        for (index, server) in self.servers.iter().enumerate() {
            let row = index + 1;

            if server.address.trim().is_empty() {
                return Err(LauncherError::msg(format!(
                    "Server {row} has no address. Give it one, or remove it."
                )));
            }
            if server.address.chars().any(char::is_whitespace) {
                return Err(LauncherError::msg(format!(
                    "Server {row} has a space in its address. Addresses look like \
                     \"play.example.com\" or \"play.example.com:25565\"."
                )));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------------

/// Read the list from `path`, or `None` when the instance has never saved one.
pub fn read(path: &Path) -> Result<Option<ServerList>> {
    if !path.exists() {
        return Ok(None);
    }
    parse(&std::fs::read(path)?).map(Some)
}

/// Write the list to `path`, replacing whatever was there.
///
/// The bytes land beside the target and are then renamed over it, so an
/// interrupted save cannot leave a half-written list for the game to choke on.
pub fn write(path: &Path, list: &ServerList) -> Result<()> {
    list.validate()?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let encoded = encode(list)?;
    let tmp = path.with_extension("dat.tmp");
    std::fs::write(&tmp, &encoded)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Make sure `game_dir`'s server list has an entry for `address`, adding it at
/// the top when it is missing. An existing entry is left exactly as the player
/// arranged it, and a file the game can't read is never overwritten.
///
/// Returns whether the list changed.
pub fn ensure_server(game_dir: &Path, name: &str, address: &str) -> Result<bool> {
    let address = address.trim();
    if address.is_empty() {
        return Ok(false);
    }
    let path = ServerList::path_in(game_dir);
    let mut list = read(&path)?.unwrap_or_default();
    if list
        .servers
        .iter()
        .any(|server| server.address.eq_ignore_ascii_case(address))
    {
        return Ok(false);
    }
    list.servers.insert(
        0,
        ServerEntry {
            name: name.to_string(),
            address: address.to_string(),
            icon: None,
            hidden: false,
            accept_textures: true,
        },
    );
    write(&path, &list)?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------

/// Decode a whole `servers.dat` from its bytes.
pub fn parse(bytes: &[u8]) -> Result<ServerList> {
    let Value::Compound(root) = read_nbt(bytes)? else {
        return Err(LauncherError::msg(
            "That servers.dat doesn't contain the compound the game expects.",
        ));
    };

    let servers = match root.get("servers") {
        Some(Value::List(entries)) => entries.iter().filter_map(entry_from).collect(),
        // No list at all is a valid, empty list.
        None => Vec::new(),
        // A malformed list is worth reporting: guessing "empty" here would let
        // the next save write an empty list over the user's servers.
        Some(_) => {
            return Err(LauncherError::msg(
                "That servers.dat has a \"servers\" tag that isn't a list.",
            ))
        }
    };

    Ok(ServerList { servers })
}

/// Serialise a list into the gzipped NBT the game reads.
pub fn encode(list: &ServerList) -> Result<Vec<u8>> {
    let entries = list.servers.iter().map(entry_value).collect();
    let mut root = HashMap::new();
    root.insert("servers".to_string(), Value::List(entries));

    let nbt = fastnbt::to_bytes(&Value::Compound(root))
        .map_err(|error| LauncherError::msg(format!("Couldn't write the server list: {error}")))?;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&nbt)?;
    Ok(encoder.finish()?)
}

/// Pull one entry out of its compound.
///
/// Returns `None` only for a non-compound inside the list, which the game would
/// refuse to load either. An entry whose address is missing or not a string is
/// kept so the user can see and repair it, rather than losing it silently.
fn entry_from(value: &Value) -> Option<ServerEntry> {
    let Value::Compound(map) = value else {
        return None;
    };

    let text = |key: &str| match map.get(key) {
        Some(Value::String(found)) => Some(found.as_str()),
        _ => None,
    };
    let flag = |key: &str| match map.get(key) {
        Some(Value::Byte(found)) => *found != 0,
        _ => false,
    };

    let address = text(KEY_ADDRESS).unwrap_or_default().to_string();
    let name = text(KEY_NAME)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(address.as_str())
        .to_string();

    Some(ServerEntry {
        name,
        address,
        icon: text(KEY_ICON)
            .filter(|icon| !icon.is_empty())
            .map(str::to_string),
        hidden: flag(KEY_HIDDEN),
        accept_textures: flag(KEY_ACCEPT_TEXTURES),
    })
}

/// The compound for one entry, in the same shape the game writes.
fn entry_value(entry: &ServerEntry) -> Value {
    let mut map = HashMap::new();
    map.insert(KEY_NAME.to_string(), Value::String(entry.name.clone()));
    map.insert(KEY_ADDRESS.to_string(), Value::String(entry.address.clone()));
    map.insert(KEY_HIDDEN.to_string(), Value::Byte(i8::from(entry.hidden)));
    map.insert(
        KEY_ACCEPT_TEXTURES.to_string(),
        Value::Byte(i8::from(entry.accept_textures)),
    );

    // Omitted rather than written empty: the game reads a present icon as PNG
    // data and fails to decode an empty string.
    if let Some(icon) = &entry.icon {
        map.insert(KEY_ICON.to_string(), Value::String(icon.clone()));
    }

    Value::Compound(map)
}

/// Decode NBT, accepting both the gzipped form the game writes and the plain
/// form some third-party editors leave behind.
fn read_nbt(bytes: &[u8]) -> Result<Value> {
    let mut raw = Vec::new();
    let decompressed = GzDecoder::new(bytes)
        .read_to_end(&mut raw)
        .ok()
        .and_then(|_| fastnbt::from_bytes::<Value>(&raw).ok());

    match decompressed {
        Some(value) => Ok(value),
        None => fastnbt::from_bytes::<Value>(bytes).map_err(|error| {
            LauncherError::msg(format!("That servers.dat isn't readable NBT: {error}"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gzip an arbitrary NBT value, so fixtures never depend on `encode`.
    fn gzipped(root: Value) -> Vec<u8> {
        let nbt = fastnbt::to_bytes(&root).expect("nbt");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt).expect("gzip");
        encoder.finish().expect("gzip")
    }

    /// A file shaped the way the game writes one.
    fn game_file(entries: Vec<Value>) -> Vec<u8> {
        gzipped(compound(vec![("servers", Value::List(entries))]))
    }

    fn compound(pairs: Vec<(&str, Value)>) -> Value {
        Value::Compound(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    fn text(value: &str) -> Value {
        Value::String(value.to_string())
    }

    fn vanilla_entry(name: &str, address: &str) -> Value {
        compound(vec![
            ("name", text(name)),
            ("ip", text(address)),
            ("hidden", Value::Byte(0)),
            ("acceptTextures", Value::Byte(1)),
        ])
    }

    fn entry(name: &str, address: &str) -> ServerEntry {
        ServerEntry {
            name: name.to_string(),
            address: address.to_string(),
            icon: None,
            hidden: false,
            accept_textures: false,
        }
    }

    /// Gunzip a file back into an NBT value, to assert on the wire format.
    fn ungzip(bytes: &[u8]) -> Value {
        let mut raw = Vec::new();
        GzDecoder::new(bytes)
            .read_to_end(&mut raw)
            .expect("gunzip");
        fastnbt::from_bytes(&raw).expect("nbt")
    }

    #[test]
    fn parses_a_file_the_game_wrote() {
        let bytes = game_file(vec![
            vanilla_entry("My Server", "play.example.com"),
            vanilla_entry("Local", "localhost:25566"),
        ]);

        let list = parse(&bytes).expect("parse");

        assert_eq!(list.servers.len(), 2);
        assert_eq!(list.servers[0].name, "My Server");
        assert_eq!(list.servers[0].address, "play.example.com");
        assert!(list.servers[0].accept_textures, "the flag must survive");
        assert!(!list.servers[0].hidden);
        assert_eq!(list.servers[1].name, "Local");
        assert_eq!(list.servers[1].address, "localhost:25566");
    }

    #[test]
    fn round_trips_through_encode_and_parse() {
        let list = ServerList {
            servers: vec![
                entry("Survival", "play.example.com:25565"),
                ServerEntry {
                    icon: Some("iVBORw0KGgo=".to_string()),
                    hidden: true,
                    accept_textures: true,
                    ..entry("Creative", "c.example.com")
                },
            ],
        };

        let readback = parse(&encode(&list).expect("encode")).expect("parse");
        assert_eq!(readback, list);
    }

    #[test]
    fn writes_the_root_compound_with_an_empty_name() {
        let list = ServerList {
            servers: vec![entry("A", "a.example.com")],
        };

        let mut raw = Vec::new();
        GzDecoder::new(encode(&list).expect("encode").as_slice())
            .read_to_end(&mut raw)
            .expect("gunzip");

        // TAG_Compound (0x0A) then a zero-length name, which is what the game
        // writes and what NbtIo.readCompressed expects at the front of the file.
        assert_eq!(raw[0], 0x0A, "root tag must be a compound");
        assert_eq!(&raw[1..3], &[0x00, 0x00], "root name must be empty");
    }

    #[test]
    fn omits_the_icon_key_when_there_is_no_icon() {
        let list = ServerList {
            servers: vec![
                ServerEntry {
                    icon: Some("AAAA".to_string()),
                    ..entry("With icon", "a.example.com")
                },
                entry("Without icon", "b.example.com"),
            ],
        };

        let Value::Compound(root) = ungzip(&encode(&list).expect("encode")) else {
            panic!("root must be a compound");
        };
        let Some(Value::List(entries)) = root.get("servers") else {
            panic!("servers must be a list");
        };
        let (Value::Compound(first), Value::Compound(second)) = (&entries[0], &entries[1]) else {
            panic!("entries must be compounds");
        };

        assert_eq!(first.get("icon"), Some(&text("AAAA")));
        assert_eq!(second.get("icon"), None, "no icon means no icon key");
    }

    #[test]
    fn ignores_keys_it_does_not_model() {
        // A field a future game version might add. It parses fine and is simply
        // not carried, exactly as the game's own ServerData would discard it.
        let bytes = game_file(vec![compound(vec![
            ("name", text("Modded")),
            ("ip", text("play.example.com")),
            ("someFutureFlag", Value::Int(7)),
        ])]);

        let list = parse(&bytes).expect("parse");
        assert_eq!(list.servers[0].name, "Modded");
        assert_eq!(list.servers[0].address, "play.example.com");
    }

    #[test]
    fn reads_nbt_that_is_not_gzipped() {
        // Some third-party editors leave the file uncompressed.
        let nbt = fastnbt::to_bytes(&compound(vec![(
            "servers",
            Value::List(vec![vanilla_entry("Plain", "plain.example.com")]),
        )]))
        .expect("nbt");

        let list = parse(&nbt).expect("parse");
        assert_eq!(list.servers[0].address, "plain.example.com");
    }

    #[test]
    fn an_empty_list_is_not_an_error() {
        assert!(parse(&game_file(Vec::new()))
            .expect("parse")
            .servers
            .is_empty());

        // A file with no `servers` tag at all is an empty list too.
        assert!(parse(&gzipped(compound(vec![])))
            .expect("parse")
            .servers
            .is_empty());
    }

    #[test]
    fn falls_back_to_the_address_when_a_name_is_missing() {
        let bytes = game_file(vec![compound(vec![("ip", text("bare.example.com"))])]);
        assert_eq!(
            parse(&bytes).expect("parse").servers[0].name,
            "bare.example.com"
        );

        // A blank name is worth no more than a missing one.
        let bytes = game_file(vec![compound(vec![
            ("name", text("   ")),
            ("ip", text("bare.example.com")),
        ])]);
        assert_eq!(
            parse(&bytes).expect("parse").servers[0].name,
            "bare.example.com"
        );
    }

    #[test]
    fn keeps_an_entry_with_no_address_so_it_can_be_repaired() {
        let bytes = game_file(vec![compound(vec![("name", text("Broken"))])]);

        let list = parse(&bytes).expect("parse");
        assert_eq!(list.servers.len(), 1, "the entry must not be dropped");
        assert!(list.servers[0].address.is_empty());

        // Saving is refused until it is fixed, rather than writing a server the
        // game cannot use.
        assert!(list.validate().is_err());
    }

    #[test]
    fn refuses_addresses_the_game_cannot_use() {
        let blank = ServerList {
            servers: vec![entry("Blank", "   ")],
        };
        let message = blank.validate().expect_err("blank address").to_string();
        assert!(
            message.contains("Server 1"),
            "message must name the row: {message}"
        );

        let spaced = ServerList {
            servers: vec![
                entry("Fine", "ok.example.com"),
                entry("Spaced", "play example com"),
            ],
        };
        let message = spaced.validate().expect_err("spaced address").to_string();
        assert!(
            message.contains("Server 2"),
            "message must name the row: {message}"
        );

        let good = ServerList {
            servers: vec![entry("Good", "play.example.com:25565")],
        };
        assert!(good.validate().is_ok());
    }

    #[test]
    fn ensure_server_adds_once_and_keeps_existing_entries() {
        let dir = std::env::temp_dir().join(format!("minearcade-ensure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = ServerList::path_in(&dir);

        assert!(ensure_server(&dir, "MineArcade", "play.minearcade.org").expect("first"));
        assert!(!ensure_server(&dir, "MineArcade", "PLAY.minearcade.org").expect("second"));
        assert_eq!(read(&path).expect("read").expect("list").servers.len(), 1);

        // A player's own list keeps its entries; ours goes on top.
        write(
            &path,
            &ServerList {
                servers: vec![entry("Mine", "mine.example.com")],
            },
        )
        .expect("write");
        assert!(ensure_server(&dir, "MineArcade", "play.minearcade.org").expect("third"));
        let servers = read(&path).expect("read").expect("list").servers;
        assert_eq!(servers[0].address, "play.minearcade.org");
        assert_eq!(servers[1].address, "mine.example.com");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_files_that_are_not_a_server_list() {
        // TAG_Int named "" holding 3: valid NBT, wrong shape.
        assert!(parse(&[0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03]).is_err());

        // A `servers` tag of the wrong type is reported, not treated as empty.
        let wrong_type = gzipped(compound(vec![("servers", text("nope"))]));
        assert!(parse(&wrong_type).is_err());
    }

    #[test]
    fn writes_and_reads_back_through_the_filesystem() {
        let dir = std::env::temp_dir().join(format!("minearcade-servers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = ServerList::path_in(&dir);

        assert!(
            read(&path).expect("read").is_none(),
            "nothing saved yet means no list"
        );

        let list = ServerList {
            servers: vec![entry("Survival", "play.example.com:25565")],
        };
        write(&path, &list).expect("write");

        let readback = read(&path).expect("read").expect("list");
        assert_eq!(readback, list);
        assert!(
            !path.with_extension("dat.tmp").exists(),
            "the temporary file must not be left behind"
        );

        // An invalid list must not reach the disk at all.
        let invalid = ServerList {
            servers: vec![entry("Blank", "")],
        };
        assert!(write(&path, &invalid).is_err());
        assert_eq!(read(&path).expect("read").expect("list"), list);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
