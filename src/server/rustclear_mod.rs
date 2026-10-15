//! The server half of the Rustclear mod: the hello on join, the settings players send, and the
//! kick for clients without the mod. Message formats are in `dungeon::settings`.

use crate::dungeon::settings::{
    decode_payload, encode_payload, Catalog, ClientSettingsMessage, SettingsAction, StatusMessage,
    HELLO_CHANNEL, PROTOCOL_VERSION, SETTINGS_CHANNEL, STATUS_CHANNEL,
};
use crate::net::internal_packets::NetworkThreadMessage;
use crate::net::protocol::play::clientbound::{CustomPayload, Disconnect};
use crate::server::player::player::Player;
use crate::server::server::Server;
use crate::server::utils::chat_component::chat_component_text::ChatComponentTextBuilder;
use crate::server::utils::color::MCColors;

/// How long a client has to answer the hello before it's kicked (10 seconds).
const MOD_TIMEOUT_TICKS: u64 = 200;

const KICK_MESSAGE: &str = "This server needs the Rustclear mod.\nInstall it, then join again.";

fn write_json(player: &mut Player, channel: &str, json: &str) {
    let data = encode_payload(json);
    player.write_packet(&CustomPayload { channel: channel.into(), data: &data });
}

/// Tells the client which channel we listen on and sends the catalog for the settings screen.
pub fn send_hello(server: &Server, player: &mut Player) {
    // 1.8's REGISTER lists channels separated by NUL; ViaProxy turns it into minecraft:register.
    player.write_packet(&CustomPayload { channel: "REGISTER".into(), data: SETTINGS_CHANNEL.as_bytes() });

    let catalog = Catalog::build(&server.room_data_storage, server.require_mod, &server.dungeon_settings);
    match serde_json::to_string(&catalog) {
        Ok(json) => write_json(player, HELLO_CHANNEL, &json),
        Err(e) => eprintln!("rustclear hello: {e}"),
    }
}

fn send_status(player: &mut Player, status: &StatusMessage) {
    if let Ok(json) = serde_json::to_string(status) {
        write_json(player, STATUS_CHANNEL, &json);
    }
}

fn status_for(server: &Server, ok: bool, message: String) -> StatusMessage {
    StatusMessage {
        ok,
        message,
        ranked: server.dungeon.counts_for_leaderboard(),
        settings: server.dungeon_settings.clone(),
        total_secrets: Some(server.dungeon.rooms.iter().map(|r| r.room_data.secrets as u32).sum()),
    }
}

/// Handles a `rustclear:settings` message from `player`.
pub fn handle_settings(player: &mut Player, payload: &[u8]) {
    let message: ClientSettingsMessage = match decode_payload(payload).and_then(|s| Ok(serde_json::from_str(&s)?)) {
        Ok(message) => message,
        Err(e) => {
            eprintln!("rustclear settings from {}: {e}", player.profile.username);
            return;
        }
    };

    if message.version != PROTOCOL_VERSION {
        player.write_packet(&crate::net::protocol::play::clientbound::Chat {
            component: ChatComponentTextBuilder::new(format!(
                "Your Rustclear mod speaks protocol {}, this server speaks {PROTOCOL_VERSION}. Update the mod.",
                message.version
            )).color(MCColors::Red).build(),
            chat_type: 0,
        });
        return;
    }

    if player.rustclear_mod.is_none() {
        println!("{} has Rustclear mod {}", player.profile.username, message.mod_version);
    }
    player.rustclear_mod = Some(message.mod_version.clone());

    let server = player.server_mut();
    let settings = message.settings.normalized();

    match message.action {
        SettingsAction::Join => {
            // A joiner doesn't overwrite settings someone already chose for this server.
            if server.dungeon_settings.is_fully_random() {
                server.dungeon_settings = settings;
            }
            let status = status_for(server, true, String::new());
            send_status(player, &status);
        }
        SettingsAction::Apply => {
            server.dungeon_settings = settings;
            let text = format!("Next dungeon: {}", server.dungeon_settings.describe());
            let status = status_for(server, true, text.clone());
            send_status(player, &status);
            player.send_message(&format!("\u{a7}a{text}"));
        }
        SettingsAction::Generate => {
            let previous = std::mem::replace(&mut server.dungeon_settings, settings);
            match crate::server::dungeon_switch::switch_dungeon(server) {
                Ok(()) => {
                    let ranked = server.dungeon.counts_for_leaderboard();
                    let secrets: u32 = server.dungeon.rooms.iter().map(|r| r.room_data.secrets as u32).sum();
                    let text = format!(
                        "New dungeon: {secrets} secrets, {}.",
                        if ranked { "counts for the leaderboard" } else { "custom settings, doesn't count for the leaderboard" }
                    );
                    let status = status_for(server, true, text.clone());
                    for (_, other) in server.world.players.iter_mut() {
                        send_status(other, &status);
                        other.send_message(&format!("\u{a7}a{text}"));
                        other.flush_packets();
                    }
                }
                Err(e) => {
                    server.dungeon_settings = previous;
                    let status = status_for(server, false, e.to_string());
                    send_status(player, &status);
                    player.send_message(&format!("\u{a7}c{e}"));
                }
            }
        }
    }
    player.flush_packets();
}

/// Kicks players who haven't answered the hello within `MOD_TIMEOUT_TICKS`, when the mod is
/// required.
pub fn kick_players_without_mod(server: &mut Server) {
    if !server.require_mod {
        return;
    }
    let now = server.world.tick_count;
    for (client_id, player) in server.world.players.iter_mut() {
        if player.rustclear_mod.is_some() || now.saturating_sub(player.joined_tick) < MOD_TIMEOUT_TICKS {
            continue;
        }
        // Only kick once: the player stays in the map until the network thread reports the close.
        player.rustclear_mod = Some(String::from("kicked"));
        println!("Kicking {}: no Rustclear mod", player.profile.username);
        player.write_packet(&Disconnect {
            reason: ChatComponentTextBuilder::new(KICK_MESSAGE).color(MCColors::Red).build(),
        });
        player.flush_packets();
        let _ = server.network_tx.send(NetworkThreadMessage::DisconnectClient { client_id: *client_id });
    }
}
