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
}
