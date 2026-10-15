//! Hypixel's Dungeonbreaker: a pickaxe that breaks one block per charge, server side.
//!
//! Behaviour as recorded on Hypixel (main server) for the P3 part of the boss, used here for the
//! whole dungeon:
//! - 20 charges, one per block. Charges come back in irregular +2 steps (now and then +1), about
//!   6 a second.
//! - A broken block comes back 221 ticks later (~11 s, as the client sees it). Breaking a 21st
//!   block brings the oldest of the last 20 back 41 ticks later, if that is sooner.
//! - The server decides: a hit either breaks the block for everyone or is refused, and the block
//!   is sent back to the hitting player.
//!
//! Hypixel's rules for what it refuses during clear are not recorded yet. [`refusal`] starts
//! from the P3 list (barrier, bedrock, levers, buttons, pistons, granite, emerald, redstone lamps,
//! gold, anything something is attached to) and adds what a clear obviously needs intact:
//! chests, skulls, spawners, and every block the dungeon handles clicks on (doors, levers,
//! puzzle blocks, secrets).

use crate::net::protocol::play::clientbound::{BlockChange, SetSlot};
use crate::server::block::block_parameter::{LeverOrientation, TorchDirection};
use crate::server::block::block_position::BlockPos;
use crate::server::block::blocks::Blocks;
use crate::server::items::item_stack::ItemStack;
use crate::server::items::Item;
use crate::server::player::inventory::ItemSlot;
use crate::server::player::player::Player;
use crate::server::utils::direction::Direction;
use crate::server::utils::nbt::nbt::{NBTNode, NBT};
use crate::server::utils::nbt::serialize::TAG_COMPOUND_ID;
use crate::server::world::World;
use rand::Rng;
use std::collections::{HashMap, VecDeque};

pub const MAX_CHARGES: u8 = 20;
/// How long a broken block stays broken.
const REGEN_TICKS: u64 = 221;
/// How soon the oldest of the last 20 comes back once a 21st block is broken.
const WINDOW_RESTORE_TICKS: u64 = 41;
/// Farther than this from the eyes, the hit is put back without a message.
const MAX_REACH: f64 = 5.2;
/// One message per this many ticks, shared by every refusal line.
const MESSAGE_THROTTLE_TICKS: u64 = 20;

const THAT_BLOCK: &str = "§cA mystical force prevents you from digging that block!";
const NO_CHARGES: &str = "§cYou don't have enough charges to break this block right now!";

/// A player's own charges and recent breaks.
#[derive(Debug)]
pub struct BreakerState {
    pub charges: u8,
    /// World tick of the next refill step, or `None` while full (or not scheduled yet).
    refill_at: Option<u64>,
    /// The player's last [`MAX_CHARGES`] breaks, oldest first.
    window: VecDeque<BlockPos>,
    last_message_tick: Option<u64>,
}

impl Default for BreakerState {
    fn default() -> Self {
        Self { charges: MAX_CHARGES, refill_at: None, window: VecDeque::new(), last_message_tick: None }
    }
}

/// A block a Dungeonbreaker broke, waiting to come back. Kept on the world, so blocks come back
/// even after the player who broke them leaves.
#[derive(Debug)]
pub struct BrokenBlock {
    pub pos: BlockPos,
    pub block: Blocks,
    pub broken_at: u64,
    pub restore_at: u64,
}

pub fn item_stack(charges: u8) -> ItemStack {
    let enchant = |id: i16, lvl: i16| {
        NBTNode::Compound(HashMap::from([
            ("id".to_string(), NBTNode::Short(id)),
            ("lvl".to_string(), NBTNode::Short(lvl)),
        ]))
    };
    let lore = [
        "§7Speed: §f+20".to_string(),
        String::new(),
        "§6Ability: Dungeon Breaker §e§lDIG".to_string(),
        "§7While in §cThe Catacombs§7, consume §e1§c⸕".to_string(),
        "§7charge to break a block. §320§7 blocks can".to_string(),
        "§7be broken at a time, and re-appear after".to_string(),
        "§a10s§7. §e2§c⸕§7 charges are regenerated each".to_string(),
        "§7second.".to_string(),
        String::new(),
        format!("§7Charges: §e{charges}§7/§e{MAX_CHARGES}§c⸕"),
        String::new(),
        "§8§l* §8Co-op Soulbound §8§l*".to_string(),
        "§c§lSPECIAL DUNGEON PICKAXE".to_string(),
    ];
    ItemStack {
        item: 278, // diamond pickaxe
        stack_size: 1,
        metadata: 0,
        tag_compound: Some(NBT::with_nodes(vec![
            // Efficiency X, Unbreaking X, as Hypixel's carries them (hidden).
            NBT::list("ench", TAG_COMPOUND_ID, vec![enchant(32, 10), enchant(34, 10)]),
            NBT::int("HideFlags", 127),
            NBT::compound("display", vec![
                NBT::string("Name", "§cDungeonbreaker"),
                ("Lore".to_string(), NBTNode::List {
                    type_id: crate::server::utils::nbt::serialize::TAG_STRING_ID,
                    children: lore.into_iter().map(NBTNode::String).collect(),
                }),
            ]),
            NBT::compound("ExtraAttributes", vec![NBT::string("id", "DUNGEONBREAKER")]),
        ])),
    }
}

/// A left click on `pos` with the Dungeonbreaker in hand.
pub fn on_hit(player: &mut Player, pos: BlockPos) {
    let world = player.world_mut();
    let block = world.get_block_at(pos.x, pos.y, pos.z);
    if matches!(block, Blocks::Air) {
        return;
    }
    let now = world.tick_count;

    let why = refusal(player, world, &pos, block);
    if why.is_some() || player.dungeonbreaker.charges == 0 {
        player.write_packet(&BlockChange { block_pos: pos, block_state: block.get_block_state_id() });
        let message = match why {
            Some(Refusal::Message(text)) => Some(text),
            Some(Refusal::Silent) => None,
            None => Some(NO_CHARGES),
        };
        if let Some(text) = message {
            let state = &mut player.dungeonbreaker;
            if state.last_message_tick.is_none_or(|t| now >= t + MESSAGE_THROTTLE_TICKS) {
                state.last_message_tick = Some(now);
                player.send_message(text);
            }
        }
        return;
    }

    player.dungeonbreaker.charges -= 1;
    world.set_block_at(Blocks::Air, pos.x, pos.y, pos.z);
    world.dungeonbreaker_broken.push(BrokenBlock { pos, block, broken_at: now, restore_at: u64::MAX });

    let window = &mut player.dungeonbreaker.window;
    window.push_back(pos);
    if window.len() > MAX_CHARGES as usize {
        let oldest = window.pop_front().unwrap();
        if let Some(entry) = world.dungeonbreaker_broken.iter_mut().find(|b| b.pos == oldest) {
            entry.restore_at = entry.restore_at.min(now + WINDOW_RESTORE_TICKS);
        }
    }
    refresh_lore(player);
}

/// Per player, every tick: charges coming back.
pub fn tick_player(player: &mut Player) {
    let now = player.world_mut().tick_count;
    let state = &mut player.dungeonbreaker;
    if state.charges >= MAX_CHARGES {
        state.refill_at = None;
        return;
    }
    let mut rng = rand::rng();
    // A step every 1-20 ticks (~10 on average); +2 four times in five, else +1: about 6 a second.
    let due = *state.refill_at.get_or_insert_with(|| now + rng.random_range(1..=20));
    if now < due {
        return;
    }
    let step = if rng.random_range(0..5) == 0 { 1 } else { 2 };
    state.charges = (state.charges + step).min(MAX_CHARGES);
    state.refill_at = None;
    refresh_lore(player);
}

/// Per world, every tick: broken blocks coming back. A block only comes back if its spot is
/// still empty, so a door opened or a puzzle block moved in the meantime is left alone.
pub fn tick_world(world: &mut World) {
    if world.dungeonbreaker_broken.is_empty() {
        return;
    }
    let now = world.tick_count;
    let mut due = Vec::new();
    world.dungeonbreaker_broken.retain(|b| {
        let back = now >= b.broken_at + REGEN_TICKS || now >= b.restore_at;
        if back {
            due.push((b.pos, b.block));
        }
        !back
    });
    for (pos, block) in due {
        if matches!(world.get_block_at(pos.x, pos.y, pos.z), Blocks::Air) {
            world.set_block_at(block, pos.x, pos.y, pos.z);
        }
    }
}

/// Re-sends every Dungeonbreaker in the player's inventory with the current charges.
fn refresh_lore(player: &mut Player) {
    let charges = player.dungeonbreaker.charges;
    for slot in 0..player.inventory.items.len() {
        if matches!(player.inventory.items[slot], ItemSlot::Filled(Item::Dungeonbreaker, _)) {
            player.write_packet(&SetSlot { window_id: 0, slot: slot as i16, item_stack: Some(item_stack(charges)) });
        }
    }
}

enum Refusal {
    Message(&'static str),
    Silent,
}

fn refusal(player: &Player, world: &World, pos: &BlockPos, block: Blocks) -> Option<Refusal> {
    // Fluids aren't blocks you can hit, but a stray hit shouldn't turn water into air.
    if matches!(block, Blocks::FlowingWater { .. } | Blocks::StillWater { .. } | Blocks::FlowingLava { .. } | Blocks::Lava { .. }) {
        return Some(Refusal::Silent);
    }
    // Anything the dungeon handles clicks on: doors, levers, chests, puzzle blocks, secrets.
    if world.interactable_blocks.contains_key(pos) {
        return Some(Refusal::Message(THAT_BLOCK));
    }
    let refused = matches!(block,
        Blocks::Barrier | Blocks::Bedrock | Blocks::CommandBlock { .. }
        | Blocks::Lever { .. } | Blocks::StoneButton { .. } | Blocks::WoodenButton { .. }
        | Blocks::Piston { .. } | Blocks::StickyPiston { .. } | Blocks::PistonHead { .. } | Blocks::MovingPiston { .. }
        | Blocks::Stone { variant: 1 | 2 } // granite, polished granite
        | Blocks::EmeraldBlock | Blocks::RedstoneLamp | Blocks::LitRedstoneLamp | Blocks::GoldBlock
        | Blocks::Chest { .. } | Blocks::TrappedChest { .. } | Blocks::EnderChest { .. }
        | Blocks::Skull { .. } | Blocks::MobSpawner
    );
    if refused || holds_something(world, pos) {
        return Some(Refusal::Message(THAT_BLOCK));
    }
    let eye = player.player_eye_position();
    let (dx, dy, dz) = (pos.x as f64 + 0.5 - eye.x, pos.y as f64 + 0.5 - eye.y, pos.z as f64 + 0.5 - eye.z);
    if (dx * dx + dy * dy + dz * dz).sqrt() > MAX_REACH {
        return Some(Refusal::Silent);
    }
    None
}

fn at(world: &World, pos: &BlockPos, d: Direction) -> Blocks {
    let (x, y, z) = d.get_offset();
    world.get_block_at(pos.x + x, pos.y + y, pos.z + z)
}

/// Whether something rests on `pos`, hangs off one of its sides, or hangs under it.
fn holds_something(world: &World, pos: &BlockPos) -> bool {
    let on_top = match at(world, pos, Direction::Up) {
        Blocks::Torch { direction } | Blocks::RedstoneTorch { direction } | Blocks::UnlitRedstoneTorch { direction } => direction == TorchDirection::Up,
        Blocks::Lever { orientation, .. } => matches!(orientation, LeverOrientation::UpX | LeverOrientation::UpZ),
        Blocks::StoneButton { direction, .. } | Blocks::WoodenButton { direction, .. } => direction.direction() == Direction::Up,
        Blocks::Skull { direction, .. } => direction == Direction::Up,
        Blocks::Carpet { .. } | Blocks::StonePressurePlate { .. } | Blocks::WoodenPressurePlate { .. }
        | Blocks::GoldPressurePlate { .. } | Blocks::IronPressurePlate { .. } | Blocks::StandingSign { .. }
        | Blocks::StandingBanner { .. } | Blocks::Rail { .. } | Blocks::PoweredRail { .. } | Blocks::DetectorRail { .. }
        | Blocks::ActivatorRail { .. } | Blocks::Redstone { .. } | Blocks::RedstoneRepeater { .. }
        | Blocks::PoweredRedstoneRepeater { .. } | Blocks::RedstoneComparator { .. } | Blocks::PoweredRedstoneComparator { .. }
        | Blocks::SnowLayer { .. } | Blocks::Fire | Blocks::FlowerPot { .. } | Blocks::Tripwire { .. }
        | Blocks::WoodenDoor { .. } | Blocks::IronDoor { .. } | Blocks::SpruceDoor { .. } | Blocks::BirchDoor { .. }
        | Blocks::JungleDoor { .. } | Blocks::AcaicaDoor { .. } | Blocks::DarkOakDoor { .. }
        | Blocks::Tallgrass { .. } | Blocks::Deadbush { .. } | Blocks::YellowFlower | Blocks::RedFlower { .. }
        | Blocks::BrownMushroom | Blocks::RedMushroom | Blocks::Sapling { .. } | Blocks::DoublePlant { .. } => true,
        _ => false,
    };
    if on_top {
        return true;
    }
    let under = match at(world, pos, Direction::Down) {
        Blocks::Lever { orientation, .. } => matches!(orientation, LeverOrientation::DownX | LeverOrientation::DownZ),
        Blocks::StoneButton { direction, .. } | Blocks::WoodenButton { direction, .. } => direction.direction() == Direction::Down,
        _ => false,
    };
    if under {
        return true;
    }
    for d in [Direction::North, Direction::South, Direction::West, Direction::East] {
        // Wall things face away from the block holding them.
        let held = match at(world, pos, d) {
            Blocks::Torch { direction } | Blocks::RedstoneTorch { direction } | Blocks::UnlitRedstoneTorch { direction } => torch_facing(direction) == Some(d),
            Blocks::Lever { orientation, .. } => lever_facing(orientation) == Some(d),
            Blocks::StoneButton { direction, .. } | Blocks::WoodenButton { direction, .. } => direction.direction() == d,
            Blocks::Ladder { direction } | Blocks::WallSign { direction } | Blocks::WallBanner { direction } => direction == d,
            Blocks::Skull { direction, .. } => direction == d,
            Blocks::TripwireHook { direction, .. } => horizontal_facing(direction as u8) == Some(d),
            // A vine's bits name the sides it hangs on: 1 south, 2 west, 4 north, 8 east.
            Blocks::Vine { metadata } => {
                let bit = match d.opposite() { Direction::South => 1, Direction::West => 2, Direction::North => 4, _ => 8 };
                metadata.get_meta() & bit != 0
            }
            _ => false,
        };
        if held {
            return true;
        }
    }
    false
}

fn torch_facing(direction: TorchDirection) -> Option<Direction> {
    match direction {
        TorchDirection::East => Some(Direction::East),
        TorchDirection::West => Some(Direction::West),
        TorchDirection::South => Some(Direction::South),
        TorchDirection::North => Some(Direction::North),
        TorchDirection::Up => None,
    }
}

fn lever_facing(orientation: LeverOrientation) -> Option<Direction> {
    match orientation {
        LeverOrientation::East => Some(Direction::East),
        LeverOrientation::West => Some(Direction::West),
        LeverOrientation::South => Some(Direction::South),
        LeverOrientation::North => Some(Direction::North),
        _ => None,
    }
}

/// 1.8's horizontal index order: south, west, north, east.
fn horizontal_facing(index: u8) -> Option<Direction> {
    match index {
        0 => Some(Direction::South),
        1 => Some(Direction::West),
        2 => Some(Direction::North),
        3 => Some(Direction::East),
        _ => None,
    }
}
