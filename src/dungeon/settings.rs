//! Rules for which dungeon gets built: which puzzles may appear and how many secrets the
//! dungeon should have in total.
//!
//! Layouts are not generated: they come from the recorded layouts in `dungeons.txt`, and each
//! room slot's contents are picked from the room data. These settings constrain both picks
//! ([`pick_dungeon`]) instead of generating anything new. They're read from
//! `dungeon_settings.json` in the working directory at startup, changed in game with
//! `/dsettings`, and apply to the next dungeon built.

use crate::dungeon::dungeon::Dungeon;
use crate::dungeon::room::room_data::{RoomData, RoomShape, RoomType};
use crate::utils::hasher::deterministic_hasher::DeterministicHashMap;
use crate::utils::seeded_rng::{seeded_rng, SeededRng};
use rand::seq::{IndexedRandom, IteratorRandom, SliceRandom};
use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "dungeon_settings.json";
/// Swaps tried per layout to bring the secret total into range before moving to another layout.
const SECRET_REPAIR_STEPS: usize = 300;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DungeonSettings {
    /// Puzzle names that may appear (any spelling: case, spaces and punctuation are ignored).
    /// `None` allows every puzzle.
    pub allowed_puzzles: Option<Vec<String>>,
    /// The dungeon's total secret count must be at least this.
    pub secrets_min: Option<u32>,
    /// ...and at most this.
    pub secrets_max: Option<u32>,
}

impl DungeonSettings {
    /// The settings file, or the defaults (everything allowed) when there is none.
    pub fn load() -> anyhow::Result<DungeonSettings> {
        match std::fs::read_to_string(SETTINGS_FILE) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DungeonSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::write(SETTINGS_FILE, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn puzzle_allowed(&self, name: &str) -> bool {
        match &self.allowed_puzzles {
            None => true,
            Some(list) => list.iter().any(|p| normalize(p) == normalize(name)),
        }
    }

    pub fn secrets_in_range(&self, total: u32) -> bool {
        self.secrets_min.is_none_or(|min| total >= min) && self.secrets_max.is_none_or(|max| total <= max)
    }

    /// How many different puzzles may appear (a layout with more puzzle rooms can't be used).
    fn allowed_puzzle_count(&self, storage: &DeterministicHashMap<usize, RoomData>) -> usize {
        available_puzzles(storage).iter().filter(|name| self.puzzle_allowed(name)).count()
    }
}

/// `"Tic Tac Toe"`, `"tictactoe"` and `"tic-tac-toe"` all name the same puzzle.
pub fn normalize(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Every distinct puzzle name in the room data, sorted.
pub fn available_puzzles(storage: &DeterministicHashMap<usize, RoomData>) -> Vec<String> {
    let mut names: Vec<String> = storage.values()
        .filter(|data| data.room_type == RoomType::Puzzle)
        .map(|data| data.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// One room slot of a layout and the room data picked for it (`None`: nothing fits, as the old
/// picker's `RoomData::dummy()`).
pub struct Pick {
    pub room_type: RoomType,
    pub shape: RoomShape,
    pub key: Option<usize>,
}

/// Picks room data of this type and shape among what isn't used yet (seeded, so a layout and
/// seed always pick the same rooms), with puzzles limited to the allowed ones. Returns `None` when nothing fits.
pub fn choose_room(
    room_type: RoomType,
    shape: RoomShape,
    storage: &DeterministicHashMap<usize, RoomData>,
    picks: &[Pick],
    settings: &DungeonSettings,
) -> Option<usize> {
    storage.iter()
        .filter(|(key, data)| {
            data.room_type == room_type
                && data.shape == shape
                && (room_type != RoomType::Puzzle || settings.puzzle_allowed(&data.name))
                && !picks.iter().filter_map(|p| p.key).any(|used| {
                    used == **key
                        // Higher/Lower Blaze share the name "Blaze": a dungeon has one or the other.
                        || (room_type == RoomType::Puzzle && storage[&used].name == data.name)
                })
        })
        .map(|(key, _)| *key)
        .choose(&mut seeded_rng())
}

pub fn total_secrets(picks: &[Pick], storage: &DeterministicHashMap<usize, RoomData>) -> u32 {
    picks.iter().filter_map(|p| p.key).map(|key| storage[&key].secrets as u32).sum()
}

/// Swaps normal rooms for others of the same shape until the secret total is in range.
/// Returns whether it got there.
pub fn repair_secrets(picks: &mut [Pick], storage: &DeterministicHashMap<usize, RoomData>, settings: &DungeonSettings) -> bool {
    let mut rng = seeded_rng();
    for _ in 0..SECRET_REPAIR_STEPS {
        let total = total_secrets(picks, storage);
        if settings.secrets_in_range(total) {
            return true;
        }
        let want_more = settings.secrets_min.is_some_and(|min| total < min);
        // Every swap that moves the total the right way: (slot, replacement).
        let used: Vec<usize> = picks.iter().filter_map(|p| p.key).collect();
        let mut swaps = Vec::new();
        for (index, pick) in picks.iter().enumerate() {
            let (RoomType::Normal, Some(current)) = (pick.room_type, pick.key) else { continue };
            let have = storage[&current].secrets;
            for (key, data) in storage.iter() {
                if data.room_type == RoomType::Normal && data.shape == pick.shape && !used.contains(key)
                    && (if want_more { data.secrets > have } else { data.secrets < have })
                {
                    swaps.push((index, *key));
                }
            }
        }
        let Some(&(index, key)) = swaps.choose(&mut rng) else { return false };
        picks[index].key = Some(key);
    }
    settings.secrets_in_range(total_secrets(picks, storage))
}

pub struct PickedDungeon {
    pub layout: String,
    pub seed: u64,
    pub dungeon: Dungeon,
}

/// A random recorded layout and room picks that meet `settings`. Each try gets a fresh seed,
/// so the printed layout and seed rebuild the same dungeon.
pub fn pick_dungeon(storage: &DeterministicHashMap<usize, RoomData>, settings: &DungeonSettings) -> anyhow::Result<PickedDungeon> {
    let mut layouts: Vec<&str> = include_str!("../dungeon_storage/dungeons.txt")
        .lines()
        .filter(|line| line.len() >= 132)
        .collect();
    layouts.shuffle(&mut rand::rng());

    let puzzle_limit = settings.allowed_puzzle_count(storage);
    for layout in layouts {
        if puzzle_slots(layout) > puzzle_limit {
            continue;
        }
        let seed: u64 = rand::random();
        SeededRng::set_seed(seed);
        if let Some(dungeon) = Dungeon::from_str_with(layout, storage, settings)? {
            return Ok(PickedDungeon { layout: layout.to_string(), seed, dungeon });
        }
    }
    anyhow::bail!("no recorded layout fits these dungeon settings")
}

/// Puzzle rooms in a layout string (room id `04` in the 6x6 grid part).
fn puzzle_slots(layout: &str) -> usize {
    (0..36).filter(|i| layout.get(i * 2..i * 2 + 2) == Some("04")).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puzzle_names_match_loosely() {
        let settings = DungeonSettings { allowed_puzzles: Some(vec!["tictactoe".into(), "Water-Board".into()]), ..Default::default() };
        assert!(settings.puzzle_allowed("Tic Tac Toe"));
        assert!(settings.puzzle_allowed("Water Board"));
        assert!(!settings.puzzle_allowed("Blaze"));
        assert!(DungeonSettings::default().puzzle_allowed("Blaze"));
    }

    #[test]
    fn secret_range_is_inclusive_and_open_ended() {
        let settings = DungeonSettings { secrets_min: Some(30), secrets_max: Some(40), ..Default::default() };
        assert!(!settings.secrets_in_range(29));
        assert!(settings.secrets_in_range(30));
        assert!(settings.secrets_in_range(40));
        assert!(!settings.secrets_in_range(41));
        let only_min = DungeonSettings { secrets_min: Some(30), ..Default::default() };
        assert!(only_min.secrets_in_range(1000));
    }

    #[test]
    fn counts_puzzle_slots_in_a_layout() {
        let layout = "080809010400100211121300101415161304171418161300191403161304191905160600919999113099910991099909090099999919990929999999099999999009";
        assert_eq!(puzzle_slots(layout), 3);
    }

    #[test]
    fn settings_file_round_trips() {
        let settings = DungeonSettings { allowed_puzzles: Some(vec!["Blaze".into()]), secrets_min: Some(10), secrets_max: None };
        let text = serde_json::to_string(&settings).unwrap();
        assert_eq!(serde_json::from_str::<DungeonSettings>(&text).unwrap(), settings);
        assert_eq!(serde_json::from_str::<DungeonSettings>("{}").unwrap(), DungeonSettings::default());
    }
}
