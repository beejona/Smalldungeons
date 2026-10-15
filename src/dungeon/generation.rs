//! Builds a dungeon that meets a player's `DungeonSettings`: rooms that must appear, the puzzles
//! that may appear, and a range for the total secret count.
//!
//! Layouts are not generated. The server picks from the recorded layouts in
//! `dungeon_storage/dungeons.txt` and fills each room slot with room data, so meeting the
//! settings is a selection problem:
//!
//! 1. Shuffle the layouts and try them in order.
//! 2. Group a layout's slots by (room type, shape). Each group has `k` slots and a list of
//!    candidate rooms (by name; a name with several variants, like Higher/Lower Blaze, is one
//!    candidate whose variant is chosen at the end). Groups never share candidates, because a
//!    room's type and shape are part of its data.
//! 3. Put every guaranteed room into a group that can hold it (small backtracking search, since
//!    a name can exist in more than one shape).
//! 4. For each group, count the ways to choose its remaining slots by total secrets
//!    (`ways[j][s]` = ways to choose `j` rooms worth `s` secrets). Convolve the groups to get the
//!    ways to reach every dungeon total. The layout works when any total in the secret range has
//!    a way.
//! 5. Sample a total weighted by its number of ways, then walk back through the groups and their
//!    rooms with the same counts. This picks uniformly among every room assignment of that layout
//!    that meets the settings, so no valid dungeon is favoured over another.
//!
//! A fully random run never comes through here: it keeps the original random path
//! (`Dungeon::from_str`) so leaderboard runs are dealt exactly as before.

use crate::dungeon::dungeon::Dungeon;
use crate::dungeon::room::room::RoomSegment;
use crate::dungeon::room::room_data::{RoomData, RoomShape, RoomType};
use crate::dungeon::settings::DungeonSettings;
use crate::utils::hasher::deterministic_hasher::DeterministicHashMap;
use rand::seq::{IndexedRandom, SliceRandom};
use rand::Rng;
use std::collections::BTreeMap;

/// One room the layout needs data for: its type, its shape and the grid cells it covers.
pub struct LayoutSlot {
    pub room_type: RoomType,
    pub shape: RoomShape,
    pub segments: Vec<RoomSegment>,
}

/// A layout plus the room data key chosen for each of its slots, in `Dungeon::parse_layout`
/// order. Build it with `Dungeon::from_plan`.
#[derive(Debug, Clone)]
pub struct DungeonPlan {
    pub layout: String,
    pub picks: Vec<usize>,
    pub total_secrets: u32,
}

/// Room types the planner may fill. Entrance, blood and fairy have one candidate each, so they
/// carry no choice, but they go through the same path to keep the slot order intact.
fn group_key(room_type: RoomType, shape: &RoomShape) -> String {
    format!("{room_type:?}/{shape:?}")
}

/// The secret total can't exceed this. Real dungeons stay well under 100.
const MAX_SECRETS: usize = 512;

/// A candidate room for one group: every storage key that shares its name.
#[derive(Debug, Clone)]
struct Candidate {
    name: String,
    secrets: u32,
    keys: Vec<usize>,
}

#[derive(Debug, Clone)]
struct Group {
    /// Slot indices (into the layout's slot list) this group fills.
    slots: Vec<usize>,
    candidates: Vec<Candidate>,
}

/// Groups a layout's slots and lists each group's candidates, with the puzzle filter applied.
/// `None` when a group has fewer candidates than slots (the layout can't be filled without
/// repeating a room).
fn build_groups(
    slots: &[LayoutSlot],
    storage: &DeterministicHashMap<usize, RoomData>,
    settings: &DungeonSettings,
) -> Result<BTreeMap<String, Group>, String> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();

    for (index, slot) in slots.iter().enumerate() {
        let key = group_key(slot.room_type, &slot.shape);
        if let Some(group) = groups.get_mut(&key) {
            group.slots.push(index);
            continue;
        }

        let mut by_name: BTreeMap<&str, Candidate> = BTreeMap::new();
        let mut keys: Vec<(&usize, &RoomData)> = storage.iter()
            .filter(|(_, data)| data.room_type == slot.room_type && data.shape == slot.shape)
            .collect();
        keys.sort_by_key(|(key, _)| **key);
        for (key, data) in keys {
            if slot.room_type == RoomType::Puzzle && !settings.puzzle_allowed(&data.name) {
                continue;
            }
            by_name.entry(&data.name)
                .or_insert_with(|| Candidate { name: data.name.clone(), secrets: data.secrets as u32, keys: Vec::new() })
                .keys
                .push(*key);
        }

        groups.insert(key, Group { slots: vec![index], candidates: by_name.into_values().collect() });
    }

    for group in groups.values() {
        if group.candidates.len() < group.slots.len() {
            let slot = &slots[group.slots[0]];
            return Err(if slot.room_type == RoomType::Puzzle {
                format!("needs {} puzzles", group.slots.len())
            } else {
                format!("needs {} {:?} {:?} rooms but only {} exist", group.slots.len(), slot.room_type, slot.shape, group.candidates.len())
            });
        }
    }

    Ok(groups)
}

/// `ways[j][s]`: ways to choose `j` of `candidates` worth `s` secrets in total.
fn count_ways(candidates: &[&Candidate], pick: usize) -> Vec<Vec<f64>> {
    let mut ways = vec![vec![0.0f64; MAX_SECRETS]; pick + 1];
    ways[0][0] = 1.0;
    for candidate in candidates {
        let secrets = candidate.secrets as usize;
        for j in (1..=pick).rev() {
            for s in (secrets..MAX_SECRETS).rev() {
                let add = ways[j - 1][s - secrets];
                if add > 0.0 {
                    ways[j][s] += add;
                }
            }
        }
    }
    ways
}

/// Picks `pick` of `candidates` worth exactly `target` secrets, uniformly among the ways to do
/// it. Walks the candidates backwards with per-prefix counts.
fn sample_subset<'a>(candidates: &[&'a Candidate], pick: usize, target: usize, rng: &mut impl Rng) -> Vec<&'a Candidate> {
    // prefix[i] = ways using only the first i candidates.
    let mut prefix: Vec<Vec<Vec<f64>>> = Vec::with_capacity(candidates.len() + 1);
    for i in 0..=candidates.len() {
        prefix.push(count_ways(&candidates[..i], pick));
    }

    let mut chosen = Vec::with_capacity(pick);
    let (mut j, mut s) = (pick, target);
    for i in (1..=candidates.len()).rev() {
        if j == 0 {
            break;
        }
        let candidate = candidates[i - 1];
        let secrets = candidate.secrets as usize;
        let total = prefix[i][j][s];
        let with = if s >= secrets { prefix[i - 1][j - 1][s - secrets] } else { 0.0 };
        if with > 0.0 && rng.random::<f64>() * total < with {
            chosen.push(candidate);
            j -= 1;
            s -= secrets;
        }
    }
    chosen
}

/// One way to meet the settings on a layout, before the final sampling: which candidates each
/// group must take (guaranteed rooms), and the counting tables for the rest.
struct Assignment<'a> {
    forced: BTreeMap<&'a str, Vec<&'a Candidate>>,
}

/// Every way to place the guaranteed rooms into groups (each into a group that lists it, never
/// more rooms than slots). Usually there's exactly one.
fn assign_guaranteed<'a>(
    groups: &'a BTreeMap<String, Group>,
    names: &[String],
    out: &mut Vec<Assignment<'a>>,
    current: &mut BTreeMap<&'a str, Vec<&'a Candidate>>,
) {
    if out.len() >= 16 {
        return;
    }
    let Some((name, rest)) = names.split_first() else {
        out.push(Assignment { forced: current.clone() });
        return;
    };
    for (key, group) in groups {
        let Some(candidate) = group.candidates.iter().find(|c| c.name.eq_ignore_ascii_case(name)) else { continue };
        let used = current.get(key.as_str()).map_or(0, |v| v.len());
        if used >= group.slots.len() {
            continue;
        }
        current.entry(key.as_str()).or_default().push(candidate);
        assign_guaranteed(groups, rest, out, current);
        let list = current.get_mut(key.as_str()).unwrap();
        list.pop();
        if list.is_empty() {
            current.remove(key.as_str());
        }
    }
}

/// What a layout can reach under an assignment: the per-group tables and the dungeon totals.
struct Tables<'a> {
    /// Per group: (key, free candidates, slots left to fill, secrets from forced rooms, ways table).
    groups: Vec<(&'a str, Vec<&'a Candidate>, usize, usize, Vec<Vec<f64>>)>,
    /// `totals[n][s]`: ways for the first `n` groups to add up to `s`.
    totals: Vec<Vec<f64>>,
}

fn build_tables<'a>(groups: &'a BTreeMap<String, Group>, assignment: &Assignment<'a>) -> Tables<'a> {
    let mut out = Tables { groups: Vec::new(), totals: vec![vec![0.0; MAX_SECRETS]] };
    out.totals[0][0] = 1.0;

    for (key, group) in groups {
        let forced = assignment.forced.get(key.as_str()).cloned().unwrap_or_default();
        let forced_secrets: usize = forced.iter().map(|c| c.secrets as usize).sum();
        let free: Vec<&Candidate> = group.candidates.iter()
            .filter(|c| !forced.iter().any(|f| f.name == c.name))
            .collect();
        let pick = group.slots.len() - forced.len();
        let ways = count_ways(&free, pick);

        let previous = out.totals.last().unwrap();
        let mut next = vec![0.0; MAX_SECRETS];
        for (a, &wa) in previous.iter().enumerate() {
            if wa == 0.0 {
                continue;
            }
            for (b, &wb) in ways[pick].iter().enumerate() {
                if wb == 0.0 {
                    continue;
                }
                let s = a + b + forced_secrets;
                if s < MAX_SECRETS {
                    next[s] += wa * wb;
                }
            }
        }
        out.totals.push(next);
        out.groups.push((key.as_str(), free, pick, forced_secrets, ways));
    }
    out
}

/// Why no layout met the settings, for the message shown to the player.
#[derive(Debug, Default)]
pub struct PlanFailure {
    pub fewest_puzzles: Option<usize>,
    pub missing_rooms: Vec<String>,
    /// Lowest and highest secret totals reachable by layouts that passed the room and puzzle
    /// checks.
    pub reachable_secrets: Option<(u32, u32)>,
}

impl PlanFailure {
    pub fn message(&self, settings: &DungeonSettings) -> String {
        if !self.missing_rooms.is_empty() {
            return format!("No dungeon can hold all of: {}.", self.missing_rooms.join(", "));
        }
        if let (Some(puzzles), Some(fewest)) = (&settings.puzzles, self.fewest_puzzles) {
            if puzzles.len() < fewest {
                return format!("Every layout has at least {fewest} puzzle rooms; enable at least {fewest} puzzles (you have {}).", puzzles.len());
            }
        }
        if let (Some((min, max)), Some((low, high))) = (settings.secrets, self.reachable_secrets) {
            return format!("No dungeon has {min}-{max} secrets with these settings; the reachable range is {low}-{high}.");
        }
        "No dungeon meets these settings.".to_string()
    }
}

/// Finds a layout and room assignment that meets `settings`. Uses the seeded RNG passed in, so a
/// (seed, settings) pair always gives the same dungeon.
pub fn plan_dungeon(
    layouts: &[&str],
    storage: &DeterministicHashMap<usize, RoomData>,
    settings: &DungeonSettings,
    rng: &mut impl Rng,
) -> Result<DungeonPlan, PlanFailure> {
    let settings = &settings.clone().normalized();
    let (secret_min, secret_max) = settings.secrets.map_or((0, MAX_SECRETS as u32 - 1), |(a, b)| (a, b));
    let guaranteed: Vec<String> = settings.guaranteed_rooms.clone();

    let mut order: Vec<usize> = (0..layouts.len()).collect();
    order.shuffle(rng);

    let mut failure = PlanFailure::default();
    let mut any_rooms_fit = guaranteed.is_empty();

    for layout_index in order {
        let layout = layouts[layout_index].trim();
        let Ok((_, slots)) = Dungeon::parse_layout(layout) else { continue };

        let puzzle_slots = slots.iter().filter(|s| s.room_type == RoomType::Puzzle).count();
        failure.fewest_puzzles = Some(failure.fewest_puzzles.map_or(puzzle_slots, |f| f.min(puzzle_slots)));

        let Ok(groups) = build_groups(&slots, storage, settings) else { continue };

        let mut assignments = Vec::new();
        assign_guaranteed(&groups, &guaranteed, &mut assignments, &mut BTreeMap::new());
        if assignments.is_empty() {
            continue;
        }
        any_rooms_fit = true;
        assignments.shuffle(rng);

        for assignment in &assignments {
            let tables = build_tables(&groups, assignment);
            let totals = tables.totals.last().unwrap();

            let reachable: Vec<usize> = (0..MAX_SECRETS).filter(|&s| totals[s] > 0.0).collect();
            if let (Some(&low), Some(&high)) = (reachable.first(), reachable.last()) {
                let (l, h) = failure.reachable_secrets.unwrap_or((low as u32, high as u32));
                failure.reachable_secrets = Some((l.min(low as u32), h.max(high as u32)));
            }

            // Pick a total in range, weighted by how many assignments reach it.
            let in_range: Vec<usize> = reachable.into_iter()
                .filter(|&s| s as u32 >= secret_min && s as u32 <= secret_max)
                .collect();
            let weight: f64 = in_range.iter().map(|&s| totals[s]).sum();
            if weight <= 0.0 {
                continue;
            }
            let mut roll = rng.random::<f64>() * weight;
            let mut target = *in_range.last().unwrap();
            for &s in &in_range {
                if roll < totals[s] {
                    target = s;
                    break;
                }
                roll -= totals[s];
            }

            // Walk back through the groups, splitting the total between them.
            let mut picks = vec![usize::MAX; slots.len()];
            let mut remaining = target;
            for n in (0..tables.groups.len()).rev() {
                let (key, free, pick, forced_secrets, ways) = &tables.groups[n];
                let before = &tables.totals[n];
                let free_secrets = remaining.checked_sub(*forced_secrets).unwrap_or(0);
                let options: Vec<(usize, f64)> = (0..=free_secrets)
                    .filter_map(|b| {
                        let a = remaining.checked_sub(forced_secrets + b)?;
                        let w = ways[*pick][b] * before[a];
                        (w > 0.0).then_some((b, w))
                    })
                    .collect();
                let total: f64 = options.iter().map(|o| o.1).sum();
                let mut roll = rng.random::<f64>() * total;
                let mut group_secrets = options.last().map(|o| o.0).unwrap_or(0);
                for (b, w) in &options {
                    if roll < *w {
                        group_secrets = *b;
                        break;
                    }
                    roll -= w;
                }
                remaining = remaining.saturating_sub(forced_secrets + group_secrets);

                let mut chosen = sample_subset(free, *pick, group_secrets, rng);
                chosen.extend(assignment.forced.get(key).cloned().unwrap_or_default());
                chosen.shuffle(rng);

                let group = &groups[*key];
                for (slot, candidate) in group.slots.iter().zip(chosen) {
                    picks[*slot] = *candidate.keys.choose(rng).unwrap();
                }
            }

            if picks.contains(&usize::MAX) {
                // Can't happen when the tables agree with the sampler; skip the layout if it does.
                continue;
            }

            return Ok(DungeonPlan { layout: layout.to_string(), picks, total_secrets: target as u32 });
        }
    }

    if !any_rooms_fit {
        failure.missing_rooms = guaranteed;
    }
    Err(failure)
}

/// The layouts whose every room slot has a distinct room to fill it. 65 recorded layouts have
/// normal dead-end rooms, a shape no normal room has, so a random fill would put placeholder
/// stone rooms there; fully random runs pick from this list instead.
pub fn fillable_layouts<'a>(layouts: &[&'a str], storage: &DeterministicHashMap<usize, RoomData>) -> Vec<&'a str> {
    let settings = DungeonSettings::default();
    layouts.iter()
        .map(|layout| layout.trim())
        .filter(|layout| {
            Dungeon::parse_layout(layout).is_ok_and(|(_, slots)| build_groups(&slots, storage, &settings).is_ok())
        })
        .collect()
}

/// How often each secret total comes up in fully random dungeons, scaled so the most common
/// total is 1.0 (index = total secrets). Exact for the planner's model: every layout weighted
/// equally, every room assignment within a layout equally likely.
pub fn secret_distribution(layouts: &[&str], storage: &DeterministicHashMap<usize, RoomData>) -> Vec<f32> {
    let settings = DungeonSettings::default();
    let mut sum = vec![0.0f64; MAX_SECRETS];
    for layout in layouts {
        let Ok((_, slots)) = Dungeon::parse_layout(layout.trim()) else { continue };
        let Ok(groups) = build_groups(&slots, storage, &settings) else { continue };
        let tables = build_tables(&groups, &Assignment { forced: BTreeMap::new() });
        let totals = tables.totals.last().unwrap();
        let weight: f64 = totals.iter().sum();
        if weight > 0.0 {
            for (s, w) in totals.iter().enumerate() {
                sum[s] += w / weight;
            }
        }
    }
    let last = sum.iter().rposition(|&w| w > 0.0).map_or(0, |i| i + 1);
    let peak = sum.iter().cloned().fold(0.0, f64::max);
    sum[..last].iter().map(|&w| if peak > 0.0 { (w / peak) as f32 } else { 0.0 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon::room::room_data::load_room_data_storage;
    use crate::utils::seeded_rng::{seeded_rng, SeededRng};

    fn layouts() -> Vec<&'static str> {
        include_str!("../dungeon_storage/dungeons.txt").split('\n').filter(|l| !l.trim().is_empty()).collect()
    }

    fn check(plan: &DungeonPlan, storage: &DeterministicHashMap<usize, RoomData>, settings: &DungeonSettings) {
        let (_, slots) = Dungeon::parse_layout(&plan.layout).unwrap();
        assert_eq!(slots.len(), plan.picks.len());
        let mut names = Vec::new();
        let mut secrets = 0u32;
        for (slot, pick) in slots.iter().zip(&plan.picks) {
            let data = &storage[pick];
            assert_eq!(data.room_type, slot.room_type);
            assert_eq!(data.shape, slot.shape);
            if slot.room_type == RoomType::Puzzle {
                assert!(settings.puzzle_allowed(&data.name), "{} not allowed", data.name);
            }
            assert!(!names.contains(&data.name), "{} twice", data.name);
            names.push(data.name.clone());
            secrets += data.secrets as u32;
        }
        assert_eq!(secrets, plan.total_secrets);
        if let Some((min, max)) = settings.secrets {
            assert!(secrets >= min && secrets <= max, "{secrets} not in {min}-{max}");
        }
        for room in &settings.guaranteed_rooms {
            assert!(names.iter().any(|n| n.eq_ignore_ascii_case(room)), "{room} missing");
        }
    }

    #[test]
    fn plans_meet_settings() {
        let storage = load_room_data_storage();
        let layouts = layouts();
        let puzzles: Vec<String> = ["Ice Fill", "Creeper Beams", "Three Weirdos", "Boulder"].iter().map(|s| s.to_string()).collect();

        let cases = vec![
            DungeonSettings { guaranteed_rooms: vec![], puzzles: None, secrets: Some((40, 45)) },
            DungeonSettings { guaranteed_rooms: vec!["Old Trap".into()], puzzles: Some(puzzles.clone()), secrets: None },
            DungeonSettings { guaranteed_rooms: vec![], puzzles: Some(puzzles), secrets: Some((30, 35)) },
        ];
        for settings in cases {
            for seed in 0..20 {
                SeededRng::set_seed(seed);
                let plan = plan_dungeon(&layouts, &storage, &settings, &mut seeded_rng())
                    .unwrap_or_else(|f| panic!("{settings:?}: {}", f.message(&settings)));
                check(&plan, &storage, &settings);
                Dungeon::from_plan(&plan, &storage).unwrap();
            }
        }
    }

    #[test]
    fn guaranteed_normal_rooms_appear() {
        let storage = load_room_data_storage();
        let layouts = layouts();
        // Two normal rooms of different shapes, plus a secret range.
        let mut names: Vec<(&RoomShape, &str)> = storage.values()
            .filter(|d| d.room_type == RoomType::Normal && !d.name.is_empty())
            .map(|d| (&d.shape, d.name.as_str()))
            .collect();
        names.sort_by_key(|(_, n)| *n);
        let first = names[0];
        let second = names.iter().find(|(shape, _)| *shape != first.0).unwrap();
        let settings = DungeonSettings {
            guaranteed_rooms: vec![first.1.to_string(), second.1.to_string()],
            puzzles: None,
            secrets: Some((25, 50)),
        };
        for seed in 0..10 {
            SeededRng::set_seed(seed);
            let plan = plan_dungeon(&layouts, &storage, &settings, &mut seeded_rng())
                .unwrap_or_else(|f| panic!("{}", f.message(&settings)));
            check(&plan, &storage, &settings);
        }
    }

    #[test]
    fn distribution_matches_random_dungeons() {
        let storage = load_room_data_storage();
        let histogram = secret_distribution(&layouts(), &storage);
        println!("secret histogram: {:?}", histogram.iter().map(|w| (w * 100.0).round() as u32).collect::<Vec<_>>());
        let peak = histogram.iter().cloned().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
        // Random dungeons built through `Dungeon::from_str` peak in the low 50s.
        assert!((45..=60).contains(&peak), "peak at {peak}");
    }

    #[test]
    fn impossible_settings_explain_why() {
        let storage = load_room_data_storage();
        let layouts = layouts();

        let one_puzzle = DungeonSettings { guaranteed_rooms: vec![], puzzles: Some(vec!["Boulder".into()]), secrets: None };
        let err = plan_dungeon(&layouts, &storage, &one_puzzle, &mut seeded_rng()).unwrap_err();
        assert!(err.message(&one_puzzle).contains("at least 2"), "{}", err.message(&one_puzzle));

        let too_many = DungeonSettings { guaranteed_rooms: vec![], puzzles: None, secrets: Some((400, 450)) };
        let err = plan_dungeon(&layouts, &storage, &too_many, &mut seeded_rng()).unwrap_err();
        assert!(err.message(&too_many).contains("reachable range"), "{}", err.message(&too_many));

        let unknown = DungeonSettings { guaranteed_rooms: vec!["Not A Room".into()], puzzles: None, secrets: None };
        let err = plan_dungeon(&layouts, &storage, &unknown, &mut seeded_rng()).unwrap_err();
        assert!(err.message(&unknown).contains("Not A Room"), "{}", err.message(&unknown));
    }

    /// Looks for "irregular" dungeons either generation path could build: rooms that fall back
    /// to the stone placeholder (empty `block_data`), rooms whose openings don't line up with the
    /// layout's doors after `Room::new` rotates them, and layouts where a (type, shape) group has
    /// more slots than candidates (random path deals `RoomData::dummy()`, planner skips the
    /// layout; fully random runs avoid those through `fillable_layouts`). Slow, so it's ignored
    /// by default:
    /// `cargo test --release irregular_dungeon_maps -- --ignored --nocapture --test-threads=1`
    /// `IRREGULAR_DUMP=<room name>` also prints that room's per-side wall profiles.
    #[test]
    #[ignore]
    fn irregular_dungeon_maps() {
        use crate::dungeon::door::Door;
        use crate::dungeon::dungeon::DUNGEON_ORIGIN;
        use crate::dungeon::room::room::Room;
        use crate::server::block::block_collision::is_block_passable;
        use crate::server::block::block_position::BlockPos;
        use crate::server::block::blocks::Blocks;
        use std::collections::{BTreeSet, HashSet};

        const SIDES: [&str; 4] = ["N", "E", "S", "W"];
        // Outward step for each side, matching `get_1x1_shape_and_type`'s door order.
        const OUT: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        // `Door::load_into_world` clears 3 blocks into each room (door.x +- 3 along its axis), so
        // the room's own blocks only matter from depth 4 (depth 1 = the room's edge row).
        const FIRST_UNCLEARED: i32 = 4;
        // A door is fine when the walk from it reaches at least this many columns past the door's
        // cleared area. A door into a wall or a pocket reaches under ~15; the tightest real rooms
        // (Three Weirdos' corridor, Teleport Maze's first pad, Water's sunken pit) reach 34-62.
        const FLOOD_CAP: usize = 24;

        let storage = load_room_data_storage();
        let layouts = layouts();
        let dump = std::env::var("IRREGULAR_DUMP").ok();

        // Block at a world position as `load_into_world` would place it (None = outside the
        // room's captured box, which the world leaves as air). Like the placement loop, the
        // vertical extent comes from `block_data.len()`, not `height` (often 0 in the files).
        fn block_at(room: &Room, x: i32, y: i32, z: i32) -> Option<Blocks> {
            let data = &room.room_data;
            let local = room.get_local_block_pos(&BlockPos { x, y, z });
            let ly = y - data.bottom;
            if local.x < 0 || local.x >= data.width || local.z < 0 || local.z >= data.length || ly < 0 {
                return None;
            }
            let index = (ly * data.width * data.length + local.z * data.width + local.x) as usize;
            data.block_data.get(index).copied()
        }
        fn passable(room: &Room, x: i32, y: i32, z: i32) -> bool {
            block_at(room, x, y, z).map_or(true, is_block_passable)
        }
        // Can a player stand in this column at depth `d` (feet at 69, or at 70 for a step up)?
        fn walkable(room: &Room, x: i32, z: i32) -> bool {
            (passable(room, x, 69, z) && passable(room, x, 70, z))
                || (passable(room, x, 70, z) && passable(room, x, 71, z))
        }
        // Per-depth count (0..=3) of walkable columns across the 3-wide door opening.
        fn profile(room: &Room, door: (i32, i32), side: usize, depths: i32) -> Vec<u8> {
            let (ox, oz) = OUT[side];
            let (lx, lz) = (oz.abs(), ox.abs());
            (1..=depths).map(|d| {
                (-1..=1).filter(|l| walkable(room, door.0 - d * ox + l * lx, door.1 - d * oz + l * lz)).count() as u8
            }).collect()
        }
        // Walks from a door into the room the way a player could: stand where there's a solid
        // block under the feet and two passable blocks above, step up at most 1, drop at most 3.
        // The world is the room's own blocks plus what every door of this room overwrites
        // (`Door::load_into_world`: air y69..73 over door +-3 along its axis and +-2 across,
        // bedrock y66..67, a stone floor line at y68 across the gap). Returns how many columns
        // past the doors' cleared areas the walk reaches, capped at `FLOOD_CAP` - a door into a
        // wall or a small pocket reaches only a handful.
        fn flood(room: &Room, doors: &[((i32, i32), usize)], start: usize) -> usize {
            let cleared = |x: i32, z: i32| -> Option<(i32, i32, usize)> {
                doors.iter().find_map(|&((dx, dz), side)| {
                    let (ox, _) = OUT[side];
                    let (ax, az) = if ox != 0 { (3, 2) } else { (2, 3) };
                    ((x - dx).abs() <= ax && (z - dz).abs() <= az).then_some((dx, dz, side))
                })
            };
            let solid_at = |x: i32, y: i32, z: i32| -> bool {
                if let Some((dx, dz, side)) = cleared(x, z) {
                    if (69..=73).contains(&y) { return false; }
                    if (66..=67).contains(&y) { return true; }
                    let on_gap_line = if OUT[side].0 != 0 { x == dx } else { z == dz };
                    if y == 68 && on_gap_line { return true; }
                }
                !passable(room, x, y, z)
            };
            let stand = |x: i32, f: i32, z: i32| solid_at(x, f - 1, z) && !solid_at(x, f, z) && !solid_at(x, f + 1, z);
            let (min, max) = room.get_world_bounds();
            let inside = |x: i32, z: i32| (x >= min.x && x <= max.x && z >= min.z && z <= max.z) || cleared(x, z).is_some();

            let ((sx, sz), _) = doors[start];
            let mut seen: HashSet<(i32, i32, i32)> = HashSet::new();
            let mut columns: HashSet<(i32, i32)> = HashSet::new();
            let mut queue = std::collections::VecDeque::new();
            seen.insert((sx, 69, sz));
            queue.push_back((sx, 69, sz));
            while let Some((x, f, z)) = queue.pop_front() {
                for (dx, dz) in OUT {
                    let (nx, nz) = (x + dx, z + dz);
                    if !inside(nx, nz) {
                        continue;
                    }
                    // Step up 1 (needs headroom where we are), walk level, or drop up to 3.
                    let mut options = Vec::new();
                    if !solid_at(x, f + 2, z) && stand(nx, f + 1, nz) {
                        options.push(f + 1);
                    }
                    if !solid_at(nx, f, nz) && !solid_at(nx, f + 1, nz) {
                        if let Some(landing) = (f - 3..=f).rev().find(|&g| solid_at(nx, g - 1, nz)) {
                            if (landing..=f + 1).all(|y| !solid_at(nx, y, nz)) {
                                options.push(landing);
                            }
                        }
                    }
                    for nf in options {
                        if seen.insert((nx, nf, nz)) {
                            if cleared(nx, nz).is_none() {
                                columns.insert((nx, nz));
                                if columns.len() >= FLOOD_CAP {
                                    return columns.len();
                                }
                            }
                            queue.push_back((nx, nf, nz));
                        }
                    }
                }
            }
            columns.len()
        }

        #[derive(Default)]
        struct Failure { count: usize, example: String }
        let mut door_failures: BTreeMap<(String, String, String, String), Failure> = BTreeMap::new();
        let mut wall_doors: BTreeMap<(String, String, String, String), Failure> = BTreeMap::new();
        let mut hole_failures: BTreeMap<(String, String, String, String), Failure> = BTreeMap::new();
        let mut empty_rooms: BTreeMap<(String, String), Failure> = BTreeMap::new();
        let mut short_groups: BTreeMap<(String, usize, usize, usize), Failure> = BTreeMap::new();
        let mut short_layouts: HashSet<&str> = HashSet::new();
        let mut seen: HashSet<(usize, Vec<(usize, usize)>, BTreeSet<(usize, usize, usize)>)> = HashSet::new();
        let mut combos = 0usize;
        let mut slots_checked = 0usize;

        for layout in &layouts {
            let layout = layout.trim();
            let Ok((doors, slots)) = Dungeon::parse_layout(layout) else { continue };

            // 3. Slots per (type, shape) vs what each path can fill them with.
            let mut groups: BTreeMap<String, (RoomType, RoomShape, usize)> = BTreeMap::new();
            for slot in &slots {
                groups.entry(group_key(slot.room_type, &slot.shape)).or_insert((slot.room_type, slot.shape.clone(), 0)).2 += 1;
            }
            for (key, (room_type, shape, count)) in &groups {
                let entries: Vec<&RoomData> = storage.values().filter(|d| d.room_type == *room_type && d.shape == *shape).collect();
                let names: HashSet<&str> = entries.iter().map(|d| d.name.as_str()).collect();
                // Random path dedupes exact data, plus names for puzzles.
                let random_capacity = if *room_type == RoomType::Puzzle { names.len() } else { entries.len() };
                if *count > names.len() || *count > random_capacity {
                    short_layouts.insert(layout);
                    let f = short_groups.entry((key.clone(), *count, names.len(), random_capacity)).or_default();
                    f.count += 1;
                    if f.example.is_empty() { f.example = layout.to_string(); }
                }
            }

            for slot in &slots {
                slots_checked += 1;
                let min_x = slot.segments.iter().map(|s| s.x).min().unwrap();
                let min_z = slot.segments.iter().map(|s| s.z).min().unwrap();
                let cells: HashSet<(usize, usize)> = slot.segments.iter().map(|s| (s.x, s.z)).collect();
                let mut pattern: Vec<(usize, usize)> = cells.iter().map(|(x, z)| (x - min_x, z - min_z)).collect();
                pattern.sort();

                // Every outside edge of the room: (segment, side, door position, has door).
                let mut edges = Vec::new();
                for seg in &slot.segments {
                    let cx = seg.x as i32 * 32 + 15 + DUNGEON_ORIGIN.0;
                    let cz = seg.z as i32 * 32 + 15 + DUNGEON_ORIGIN.1;
                    for side in 0..4 {
                        let (ox, oz) = OUT[side];
                        let (nx, nz) = (seg.x as i32 + ox, seg.z as i32 + oz);
                        if nx >= 0 && nz >= 0 && cells.contains(&(nx as usize, nz as usize)) {
                            continue;
                        }
                        let pos = (cx + ox * 16, cz + oz * 16);
                        let has_door = doors.iter().any(|d: &Door| d.x == pos.0 && d.z == pos.1);
                        edges.push((seg.x - min_x, seg.z - min_z, side, pos, has_door));
                    }
                }
                let door_set: BTreeSet<(usize, usize, usize)> = edges.iter().filter(|e| e.4).map(|e| (e.0, e.1, e.2)).collect();

                let mut keys: Vec<usize> = storage.iter()
                    .filter(|(_, d)| d.room_type == slot.room_type && d.shape == slot.shape)
                    .map(|(k, _)| *k)
                    .collect();
                keys.sort();

                for key in keys {
                    if !seen.insert((key, pattern.clone(), door_set.clone())) {
                        continue;
                    }
                    combos += 1;
                    let data = storage[&key].clone();
                    let shape = format!("{:?}", data.shape);

                    // 1. Placeholder stone room.
                    if data.block_data.is_empty() {
                        let f = empty_rooms.entry((data.name.clone(), shape.clone())).or_default();
                        f.count += 1;
                        if f.example.is_empty() { f.example = layout.to_string(); }
                        continue;
                    }

                    let segments = slot.segments.iter()
                        .map(|s| crate::dungeon::room::room::RoomSegment { x: s.x, z: s.z, neighbours: [const { None }; 4] })
                        .collect();
                    let room = Room::new(segments, &doors, data);
                    let rotation = format!("{:?}", room.rotation);
                    let name = room.room_data.name.clone();

                    // 2. Door alignment, plus holes where there is no door.
                    let room_doors: Vec<((i32, i32), usize)> = edges.iter().filter(|e| e.4).map(|e| (e.3, e.2)).collect();
                    // Many rooms (every multi-cell room, Fairy) are captured fully walled and rely
                    // on the doors to clear their way in; only rooms captured with door gaps in
                    // their walls can tell a wrong rotation from a closed edge.
                    let has_gaps = edges.iter().any(|e| profile(&room, e.3, e.2, 1)[0] > 0);
                    for &(sx, sz, side, pos, has_door) in &edges {
                        let prof = profile(&room, pos, side, 8);
                        let reach = match room_doors.iter().position(|d| d.0 == pos) {
                            Some(index) => flood(&room, &room_doors, index),
                            None => 0,
                        };
                        if dump.as_deref() == Some(name.as_str()) {
                            println!("{name} rot={rotation} seg=({sx},{sz}) side={} door={has_door} profile={:?} reach={reach}", SIDES[side], prof);
                        }
                        let side_label = format!("{} of cell ({sx},{sz})", SIDES[side]);
                        if has_door {
                            // The room has to open up behind the door, not just have a pocket.
                            if reach < FLOOD_CAP {
                                let f = door_failures.entry((name.clone(), shape.clone(), rotation.clone(), side_label.clone())).or_default();
                                f.count += 1;
                                if f.example.is_empty() { f.example = layout.to_string(); }
                            }
                            // The room's own edge row is closed where the door is: the door only
                            // gets through by clearing the wall (often a sign of a wrong rotation).
                            if has_gaps && prof[0] == 0 {
                                let f = wall_doors.entry((name.clone(), shape.clone(), rotation.clone(), side_label)).or_default();
                                f.count += 1;
                                if f.example.is_empty() { f.example = layout.to_string(); }
                            }
                        } else if prof.iter().take(FIRST_UNCLEARED as usize + 1).all(|&n| n == 3) {
                            // Fully open 3-wide, 2-tall passage from the edge inwards with no door:
                            // a hole into the gap/void.
                            let f = hole_failures.entry((name.clone(), shape.clone(), rotation.clone(), side_label)).or_default();
                            f.count += 1;
                            if f.example.is_empty() { f.example = layout.to_string(); }
                        }
                    }
                }
            }
        }

        println!("\n=== irregular dungeon check ===");
        println!("layouts: {}, slots: {slots_checked}, unique (room, segment pattern, door sides) combos checked: {combos}", layouts.len());

        println!("\n-- 1. empty block_data (placeholder stone room): {}", empty_rooms.len());
        for ((name, shape), f) in &empty_rooms {
            println!("  {name:?} {shape} x{} e.g. {}", f.count, f.example);
        }

        println!("\n-- 2a. door that leads nowhere (walk from it reaches < {FLOOD_CAP} columns): {}", door_failures.len());
        for ((name, shape, rotation, side), f) in &door_failures {
            println!("  {name:?} {shape} rot={rotation} door {side} x{} e.g. {}", f.count, f.example);
        }

        println!("\n-- 2b. door whose room edge is a wall (door clears through it; rooms captured with door gaps only): {}", wall_doors.len());
        for ((name, shape, rotation, side), f) in &wall_doors {
            println!("  {name:?} {shape} rot={rotation} door {side} x{} e.g. {}", f.count, f.example);
        }

        println!("\n-- 2c. open wall with no door (hole into the gap): {}", hole_failures.len());
        for ((name, shape, rotation, side), f) in &hole_failures {
            println!("  {name:?} {shape} rot={rotation} {side} x{} e.g. {}", f.count, f.example);
        }

        println!("\n-- 3. groups with more slots than candidates: {}", short_groups.len());
        for ((key, count, names, random_capacity), f) in &short_groups {
            println!("  {key}: {count} slots, {names} distinct names (planner), {random_capacity} random-path candidates; {} layouts e.g. {}", f.count, f.example);
        }
        println!("  layouts affected: {} of {} (random path deals RoomData::dummy(), a stone placeholder, for the extra slots; the planner skips the layout)", short_layouts.len(), layouts.len());

        assert!(empty_rooms.is_empty() && door_failures.is_empty() && wall_doors.is_empty() && hole_failures.is_empty(), "irregular rooms found, see above");
        let fillable: HashSet<&str> = fillable_layouts(&layouts, &storage).into_iter().collect();
        for layout in &short_layouts {
            assert!(!fillable.contains(layout.trim()), "random runs can still pick {layout}");
        }
    }
}
