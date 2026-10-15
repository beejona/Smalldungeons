//! Dungeon settings chosen in the Rustclear mod, and the messages the mod and server exchange.
//!
//! Every message is one Minecraft string (VarInt length + UTF-8) holding JSON, on these plugin
//! channels (each under the 20-character limit 1.8 puts on channel names):
//!
//! - `rustclear:hello` (server to client, on join): protocol version and the room catalog the
//!   settings screen lists (`Catalog`).
//! - `rustclear:settings` (client to server): the player's settings and what to do with them
//!   (`ClientSettingsMessage`). The mod sends `"join"` in answer to the hello; a server that
//!   requires the mod kicks players who never send it.
//! - `rustclear:status` (server to client): the result of applying settings (`StatusMessage`).

use crate::dungeon::room::room_data::{RoomData, RoomType};
use crate::utils::hasher::deterministic_hasher::DeterministicHashMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: u32 = 1;
pub const HELLO_CHANNEL: &str = "rustclear:hello";
pub const SETTINGS_CHANNEL: &str = "rustclear:settings";
pub const STATUS_CHANNEL: &str = "rustclear:status";

/// What the next dungeon must look like. The default (everything empty or `None`) is fully
/// random, and only fully random runs count for the leaderboard.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DungeonSettings {
    /// Room names that must appear. Empty means fully random rooms.
    #[serde(rename = "rooms")]
    pub guaranteed_rooms: Vec<String>,
    /// Puzzle names that may appear. `None` means any puzzle (fully random).
    pub puzzles: Option<Vec<String>>,
    /// Inclusive range for the dungeon's total secrets. `None` means any count.
    pub secrets: Option<(u32, u32)>,
}

impl DungeonSettings {
    pub fn is_fully_random(&self) -> bool {
        self.guaranteed_rooms.is_empty() && self.puzzles.is_none() && self.secrets.is_none()
    }

    pub fn puzzle_allowed(&self, name: &str) -> bool {
        self.puzzles.as_ref().is_none_or(|list| list.iter().any(|p| p.eq_ignore_ascii_case(name)))
    }

    /// Drops duplicates and blanks and orders the secret range, so equal choices compare equal
    /// and the planner never sees a room twice.
    pub fn normalized(mut self) -> DungeonSettings {
        let mut seen = Vec::<String>::new();
        self.guaranteed_rooms.retain(|r| {
            let key = r.trim().to_ascii_lowercase();
            if key.is_empty() || seen.contains(&key) {
                return false;
            }
            seen.push(key);
            true
        });
        if let Some(puzzles) = &mut self.puzzles {
            puzzles.retain(|p| !p.trim().is_empty());
            puzzles.sort();
            puzzles.dedup();
        }
        if let Some((a, b)) = self.secrets {
            self.secrets = Some((a.min(b), a.max(b)));
        }
        self
    }

    /// One line for chat, e.g. "rooms: Old Trap; puzzles: any; secrets: 30-40".
    pub fn describe(&self) -> String {
        let rooms = if self.guaranteed_rooms.is_empty() { "random".to_string() } else { self.guaranteed_rooms.join(", ") };
        let puzzles = match &self.puzzles {
            None => "random".to_string(),
            Some(list) => list.join(", "),
        };
        let secrets = match self.secrets {
            None => "random".to_string(),
            Some((a, b)) => format!("{a}-{b}"),
        };
        format!("rooms: {rooms}; puzzles: {puzzles}; secrets: {secrets}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsAction {
    /// Sent once after the hello: confirms the mod is installed and sets the player's settings
    /// for the next dungeon without rebuilding the current one.
    Join,
    /// Store the settings for the next dungeon.
    Apply,
    /// Store the settings and start a new dungeon with them now.
    Generate,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientSettingsMessage {
    pub version: u32,
    pub action: SettingsAction,
    #[serde(default)]
    pub mod_version: String,
    #[serde(default)]
    pub settings: DungeonSettings,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogRoom {
    pub name: String,
    #[serde(rename = "type")]
    pub room_type: String,
    pub shape: String,
    pub secrets: u32,
}

/// Everything the settings screen lists: the rooms that can be guaranteed and the puzzles.
#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub version: u32,
    pub required: bool,
    pub rooms: Vec<CatalogRoom>,
    pub puzzles: Vec<CatalogRoom>,
    pub current: DungeonSettings,
}

impl Catalog {
    pub fn build(storage: &DeterministicHashMap<usize, RoomData>, required: bool, current: &DungeonSettings) -> Catalog {
        let mut rooms: BTreeMap<String, CatalogRoom> = BTreeMap::new();
        let mut puzzles: BTreeMap<String, CatalogRoom> = BTreeMap::new();
        for data in storage.values() {
            // Entrance, blood, fairy and boss rooms are always there; rare rooms never spawn.
            let target = match data.room_type {
                RoomType::Normal | RoomType::Trap | RoomType::Yellow => &mut rooms,
                RoomType::Puzzle => &mut puzzles,
                _ => continue,
            };
            // "Default" is the 1x1 placeholder yellow room, never picked for a layout slot.
            if data.name.is_empty() || data.name == "Default" {
                continue;
            }
            target.entry(data.name.clone()).or_insert_with(|| CatalogRoom {
                name: data.name.clone(),
                room_type: format!("{:?}", data.room_type).to_lowercase(),
                shape: format!("{:?}", data.shape),
                secrets: data.secrets as u32,
            });
        }
        Catalog {
            version: PROTOCOL_VERSION,
            required,
            rooms: rooms.into_values().collect(),
            puzzles: puzzles.into_values().collect(),
            current: current.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusMessage {
    pub ok: bool,
    pub message: String,
    /// Whether the current dungeon counts for the leaderboard (fully random settings).
    pub ranked: bool,
    pub settings: DungeonSettings,
    pub total_secrets: Option<u32>,
}

/// Writes `json` as a Minecraft string payload.
pub fn encode_payload(json: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(json.len() + 5);
    crate::net::var_int::write_var_int(&mut data, json.len() as i32);
    data.extend_from_slice(json.as_bytes());
    data
}

/// Reads a Minecraft string payload.
pub fn decode_payload(payload: &[u8]) -> anyhow::Result<String> {
    let mut len: u32 = 0;
    let mut shift = 0;
    let mut index = 0;
    loop {
        let byte = *payload.get(index).ok_or_else(|| anyhow::anyhow!("payload ends inside its length"))?;
        index += 1;
        len |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            anyhow::bail!("payload length is too long");
        }
    }
    let body = payload.get(index..index + len as usize).ok_or_else(|| anyhow::anyhow!("payload is shorter than its length"))?;
    Ok(String::from_utf8(body.to_vec())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_randomness() {
        let json = r#"{"version":1,"action":"generate","settings":{"rooms":["Old Trap","old trap",""],"puzzles":["Boulder","Ice Fill"],"secrets":[40,30]}}"#;
        let message: ClientSettingsMessage = serde_json::from_str(&decode_payload(&encode_payload(json)).unwrap()).unwrap();
        assert_eq!(message.action, SettingsAction::Generate);
        let settings = message.settings.normalized();
        assert_eq!(settings.guaranteed_rooms, vec!["Old Trap".to_string()]);
        assert_eq!(settings.secrets, Some((30, 40)));
        assert!(!settings.is_fully_random());
        assert!(settings.puzzle_allowed("ice fill"));
        assert!(!settings.puzzle_allowed("Quiz"));

        let random: ClientSettingsMessage = serde_json::from_str(r#"{"version":1,"action":"join"}"#).unwrap();
        assert!(random.settings.is_fully_random());
    }
}
