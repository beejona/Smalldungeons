//! Run times, recorded by the server and shown in the Rustclear mod.
//!
//! A run finishes when the blood door opens with S+ score (300 or more). It counts only when the
//! dungeon was dealt fully random (no custom settings, no chosen layout, not practice) and
//! nothing during the run disqualified it (a score-changing debug command). Times are the
//! server's own tick count from the dungeon start, so a client can't report one.
//!
//! Runs are kept in `leaderboard.json` in the working directory and ranked per party size.
//! The mod asks for a board on `rustclear:board` and gets one back on the same channel.

use crate::dungeon::dungeon_state::DungeonState;
use crate::dungeon::settings::{decode_payload, encode_payload};
use crate::net::protocol::play::clientbound::CustomPayload;
use crate::server::player::player::Player;
use crate::server::server::Server;
use serde::{Deserialize, Serialize};

/// 1.8 caps channel names at 20 characters, so not `rustclear:leaderboard`.
pub const LEADERBOARD_CHANNEL: &str = "rustclear:board";
pub const LEADERBOARD_FILE: &str = "leaderboard.json";
pub const LEADERBOARD_PROTOCOL_VERSION: u32 = 1;
/// The score a run needs when blood opens.
pub const S_PLUS: u32 = 300;
/// Entries sent per board.
const TOP: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Everyone in the dungeon when it finished, sorted.
    pub players: Vec<String>,
    pub ticks: u64,
    pub score: u32,
    pub secrets_found: u32,
    pub total_secrets: u32,
    /// Layout string and RNG seed: `cargo run -- <layout>` plus the seed rebuilds the dungeon.
    pub layout: String,
    pub seed: u64,
    /// Unix seconds.
    pub finished_at: u64,
}

impl Run {
    pub fn category(&self) -> String {
        category_for(self.players.len())
    }
}

pub fn category_for(party_size: usize) -> String {
    match party_size {
        1 => "solo".to_string(),
        n => format!("{n} players"),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Leaderboard {
    pub runs: Vec<Run>,
}

impl Leaderboard {
    pub fn load() -> anyhow::Result<Leaderboard> {
        match std::fs::read_to_string(LEADERBOARD_FILE) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Leaderboard::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes to a temporary file first, so a crash mid-write can't lose the board.
    pub fn save(&self) -> anyhow::Result<()> {
        let tmp = format!("{LEADERBOARD_FILE}.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, LEADERBOARD_FILE)?;
        Ok(())
    }

    /// Categories that have runs, solo first, then by party size.
    pub fn categories(&self) -> Vec<String> {
        let mut sizes: Vec<usize> = self.runs.iter().map(|r| r.players.len()).collect();
        sizes.sort();
        sizes.dedup();
        sizes.into_iter().map(category_for).collect()
    }

    /// One category's runs, fastest first: only each party's best (the same players in any
    /// order count as one party), ties broken by who got there first.
    pub fn ranked(&self, category: &str) -> Vec<&Run> {
        let mut runs: Vec<&Run> = self.runs.iter().filter(|r| r.category() == category).collect();
        runs.sort_by_key(|r| (r.ticks, r.finished_at));
        let mut seen: Vec<&Vec<String>> = Vec::new();
        runs.retain(|r| {
            if seen.contains(&&r.players) {
                return false;
            }
            seen.push(&r.players);
            true
        });
        runs
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub rank: usize,
    pub players: Vec<String>,
    pub ticks: u64,
    pub score: u32,
    pub secrets_found: u32,
    pub total_secrets: u32,
    pub finished_at: u64,
}

impl Entry {
    fn new(rank: usize, run: &Run) -> Entry {
        Entry {
            rank,
            players: run.players.clone(),
            ticks: run.ticks,
            score: run.score,
            secrets_found: run.secrets_found,
            total_secrets: run.total_secrets,
            finished_at: run.finished_at,
        }
    }
}

/// What the mod sends on `rustclear:board`.
#[derive(Debug, Deserialize)]
pub struct BoardRequest {
    pub version: u32,
    /// `None`: the requesting player's party size now.
    #[serde(default)]
    pub category: Option<String>,
}

/// What the server sends back, and pushes to everyone with the mod when a run is recorded.
#[derive(Debug, Serialize)]
pub struct BoardMessage {
    pub version: u32,
    pub category: String,
    /// Every category with runs, for tabs.
    pub categories: Vec<String>,
    /// The top runs, fastest first.
    pub entries: Vec<Entry>,
    /// The requesting player's best run in this category, when they have one.
    pub personal_best: Option<Entry>,
    /// Whether the dungeon being played now counts, and why not when it doesn't.
    pub current_run_ranked: bool,
    pub current_run_reason: Option<String>,
    /// A run that was just recorded (only in the push after a finish).
    pub just_finished: Option<Entry>,
}

/// Why the current dungeon can't count, or `None` when it can.
pub fn unranked_reason(server: &Server) -> Option<String> {
    let dungeon = &server.dungeon;
    if server.practice_mode || dungeon.practice_room {
        Some("practice mode".to_string())
    } else if dungeon.custom_layout {
        Some("a chosen layout".to_string())
    } else if !dungeon.counts_for_leaderboard() {
        Some("custom dungeon settings".to_string())
    } else {
        dungeon.run_disqualified.clone()
    }
}

/// Formats ticks as m:ss.cc (20 ticks a second).
pub fn format_ticks(ticks: u64) -> String {
    let centis = ticks * 5;
    format!("{}:{:02}.{:02}", centis / 6000, (centis / 100) % 60, centis % 100)
}

fn board_for(server: &Server, category: String, player_name: &str, just_finished: Option<Entry>) -> BoardMessage {
    let board = &server.leaderboard;
    let ranked = board.ranked(&category);
    let personal_best = ranked.iter().position(|r| r.players.iter().any(|p| p == player_name))
        .map(|i| Entry::new(i + 1, ranked[i]));
    let reason = unranked_reason(server);
    BoardMessage {
        version: LEADERBOARD_PROTOCOL_VERSION,
        categories: board.categories(),
        entries: ranked.iter().take(TOP).enumerate().map(|(i, r)| Entry::new(i + 1, r)).collect(),
        personal_best,
        current_run_ranked: reason.is_none(),
        current_run_reason: reason,
        just_finished,
        category,
    }
}

fn send_board(player: &mut Player, board: &BoardMessage) {
    if let Ok(json) = serde_json::to_string(board) {
        let data = encode_payload(&json);
        player.write_packet(&CustomPayload { channel: LEADERBOARD_CHANNEL.into(), data: &data });
    }
}

/// Handles a `rustclear:board` request from `player`.
pub fn handle_request(player: &mut Player, payload: &[u8]) {
    let request: BoardRequest = match decode_payload(payload).and_then(|s| Ok(serde_json::from_str(&s)?)) {
        Ok(request) => request,
        Err(e) => {
            eprintln!("rustclear leaderboard from {}: {e}", player.profile.username);
            return;
        }
    };
    if request.version != LEADERBOARD_PROTOCOL_VERSION {
        player.send_message(&format!(
            "\u{a7}cYour Rustclear mod's leaderboard speaks version {}, this server speaks {LEADERBOARD_PROTOCOL_VERSION}. Update the mod.",
            request.version
        ));
        return;
    }
    let server = player.server_mut();
    let category = request.category.unwrap_or_else(|| category_for(server.world.players.len()));
    let board = board_for(server, category, &player.profile.username.clone(), None);
    send_board(player, &board);
    player.flush_packets();
}

/// Called when the blood door opens: records the run when it counts, and tells everyone.
pub fn on_blood_open(server: &mut Server) {
    let DungeonState::Started { current_ticks } = server.dungeon.state else { return };
    if server.dungeon.run_recorded {
        return;
    }
    server.dungeon.run_recorded = true;

    server.dungeon.score.sync_exploration(&server.dungeon.rooms);
    server.dungeon.score.elapsed_ticks = current_ticks;
    let score = server.dungeon.score.total();
    let time = format_ticks(current_ticks);

    if let Some(reason) = unranked_reason(server) {
        broadcast(server, &format!("\u{a7}7Blood opened at \u{a7}f{time}\u{a7}7 with \u{a7}f{score}\u{a7}7 score. Not ranked: {reason}."));
        return;
    }
    if score < S_PLUS {
        broadcast(server, &format!(
            "\u{a7}7Blood opened at \u{a7}f{time}\u{a7}7 with \u{a7}c{score}\u{a7}7 score. Runs need S+ ({S_PLUS}) when blood opens to count."
        ));
        return;
    }

    let mut players: Vec<String> = server.world.players.values().map(|p| p.profile.username.clone()).collect();
    players.sort();
    let run = Run {
        players,
        ticks: current_ticks,
        score,
        secrets_found: server.dungeon.score.secrets_found,
        total_secrets: server.dungeon.score.total_secrets,
        layout: server.dungeon.layout.clone(),
        seed: server.dungeon.seed,
        finished_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    };
    let category = run.category();
    server.leaderboard.runs.push(run.clone());
    if let Err(e) = server.leaderboard.save() {
        eprintln!("leaderboard: couldn't save {LEADERBOARD_FILE}: {e}");
    }

    let ranked = server.leaderboard.ranked(&category);
    let rank = ranked.iter().position(|r| **r == run);
    let rank_text = match rank {
        Some(i) => format!("#{} {category}", i + 1),
        // Slower than this party's own best: it's kept, but the best is what's ranked.
        None => format!("not a personal best ({category})"),
    };
    broadcast(server, &format!("\u{a7}a\u{a7}lS+ CLEAR! \u{a7}r\u{a7}aBlood opened at \u{a7}f{time}\u{a7}a with \u{a7}f{score}\u{a7}a score: {rank_text}."));
    println!("Leaderboard: {} in {time} ({score} score), {rank_text}", run.players.join(", "));

    let just_finished = rank.map(|i| Entry::new(i + 1, &run));
    let names: Vec<(crate::server::player::player::ClientId, String, bool)> = server.world.players.iter()
        .map(|(id, p)| (*id, p.profile.username.clone(), p.rustclear_mod.is_some()))
        .collect();
    for (id, name, has_mod) in names {
        if !has_mod {
            continue;
        }
        let board = board_for(server, category.clone(), &name, just_finished.clone());
        if let Some(player) = server.world.players.get_mut(&id) {
            send_board(player, &board);
        }
    }
}

fn broadcast(server: &mut Server, message: &str) {
    for player in server.world.players.values_mut() {
        player.send_message(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(players: &[&str], ticks: u64, finished_at: u64) -> Run {
        Run {
            players: players.iter().map(|p| p.to_string()).collect(),
            ticks,
            score: 300,
            secrets_found: 50,
            total_secrets: 50,
            layout: String::new(),
            seed: 0,
            finished_at,
        }
    }

    #[test]
    fn formats_ticks_as_minutes_seconds_hundredths() {
        assert_eq!(format_ticks(0), "0:00.00");
        assert_eq!(format_ticks(1), "0:00.05");
        assert_eq!(format_ticks(20 * 61 + 3), "1:01.15");
        assert_eq!(format_ticks(20 * 60 * 12), "12:00.00");
    }

    #[test]
    fn ranks_each_party_once_fastest_first() {
        let board = Leaderboard {
            runs: vec![
                run(&["a"], 3000, 1),
                run(&["b"], 2500, 2),
                run(&["a"], 2400, 3),
                run(&["a", "b"], 2000, 4),
                run(&["c"], 2500, 1),
            ],
        };
        let solo: Vec<(&str, u64)> = board.ranked("solo").iter().map(|r| (r.players[0].as_str(), r.ticks)).collect();
        // c ties b and got there first; a's slower run is dropped.
        assert_eq!(solo, vec![("a", 2400), ("c", 2500), ("b", 2500)]);
        assert_eq!(board.ranked("2 players").len(), 1);
        assert_eq!(board.categories(), vec!["solo".to_string(), "2 players".to_string()]);
    }

    #[test]
    fn board_file_round_trips() {
        let board = Leaderboard { runs: vec![run(&["a"], 2400, 3)] };
        let text = serde_json::to_string(&board).unwrap();
        let back: Leaderboard = serde_json::from_str(&text).unwrap();
        assert_eq!(back.runs, board.runs);
    }
}
