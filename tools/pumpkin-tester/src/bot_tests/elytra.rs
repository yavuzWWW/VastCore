use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use pumpkin_core::{server::Server, world::BlockFlags};
use pumpkin_data::{
    Block, data_component_impl::EquipmentSlot, dimension::Dimension, entity::EntityPose, item::Item,
    item_stack::ItemStack,
};
use pumpkin_protocol::java::server::play::Action;
use pumpkin_util::{
    GameMode,
    math::{position::BlockPos, vector3::Vector3},
};

use crate::bot::{HeadlessBot, MAX_POLL_TICKS};

async fn wait_for(
    server: &Arc<Server>,
    condition: impl Fn() -> bool,
    error: &str,
) -> Result<(), String> {
    for _ in 0..MAX_POLL_TICKS {
        tokio::time::sleep(Duration::from_millis(5)).await;
        server.tick();
        if condition() {
            return Ok(());
        }
    }
    Err(error.into())
}

/// Real Java movement and glide command packets against the test server.
/// Synthetic moves test state synchronization rather than graphical client physics.
#[allow(clippy::too_many_lines)]
pub async fn run_elytra_tests(
    server: &Arc<Server>,
    bot: &mut HeadlessBot,
    summary: &mut Vec<String>,
) -> Result<(), String> {
    let start = Instant::now();
    let world = server.get_world_from_dimension(&Dimension::OVERWORLD);
    let player = world
        .get_player_by_name("TestBot1")
        .ok_or("missing elytra test player")?;
    let entity = &player.living_entity.entity;
    let inventory = player.inventory();
    let chest = inventory
        .equipment_slot_index(&EquipmentSlot::CHEST)
        .ok_or("missing chest equipment slot")?;
    let old_chest = inventory.get_slot(chest);
    let old_slot = inventory.get_selected_slot();
    let old_mode = player.gamemode.load();
    let original_pos = bot.position;
    // Use a known dry, open-air fixture rather than the random spawn terrain.
    let ground = Vector3::new(
        original_pos.x.floor() + 0.5,
        300.0,
        original_pos.z.floor() + 0.5,
    );
    let floor = BlockPos::floored(ground.x, ground.y - 1.0, ground.z);
    let previous_floor = world.get_block_state(&floor).id;
    world.set_block_state(&floor, Block::STONE.default_state.id, BlockFlags::NOTIFY_ALL);
    let air = ground + Vector3::new(0.0, 0.25, 0.0);
    player.set_flying(false);
    let _ = player.set_gamemode(GameMode::Survival);
    inventory.set_slot(chest, ItemStack::new(1, &Item::ELYTRA));

    bot.move_to(ground, true).await?;
    wait_for(
        server,
        || entity.pos.load().squared_distance_to_vec(&ground) < 1.0e-6
            && entity.on_ground.load(Ordering::Relaxed),
        "elytra test player did not reach the dry platform",
    )
    .await?;
    bot.move_to(air, false).await?;
    wait_for(
        server,
        || entity.pos.load().squared_distance_to_vec(&air) < 1.0e-6
            && !entity.on_ground.load(Ordering::Relaxed)
            && player.can_glide(),
        "elytra test player did not reach a valid airborne state",
    )
    .await?;
    bot.send_player_command(entity.entity_id, Action::StartFlyingElytra)
        .await?;
    wait_for(
        server,
        || entity.is_fall_flying() && entity.pose.load() == EntityPose::FallFlying,
        "equipped airborne player failed to start gliding",
    )
    .await?;

    // A later hotbar packet confirms the duplicate glide request was handled.
    bot.send_player_command(entity.entity_id, Action::StartFlyingElytra)
        .await?;
    bot.set_held_item(7).await?;
    wait_for(
        server,
        || inventory.get_selected_slot() == 7,
        "duplicate glide request marker was not received",
    )
    .await?;
    if !entity.is_fall_flying() {
        return Err("duplicate start request stopped an active glide".into());
    }

    // No further movement packet: the ordinary player tick must stop the glide.
    inventory.set_slot(chest, ItemStack::EMPTY.clone());
    wait_for(
        server,
        || !entity.is_fall_flying() && entity.pose.load() != EntityPose::FallFlying,
        "unequipping elytra did not clear gliding and pose",
    )
    .await?;
    bot.send_player_command(entity.entity_id, Action::StartFlyingElytra)
        .await?;
    bot.set_held_item(8).await?;
    wait_for(
        server,
        || inventory.get_selected_slot() == 8,
        "unequipped glide request marker was not received",
    )
    .await?;
    if entity.is_fall_flying() {
        return Err("player started gliding without an equipped glider".into());
    }

    inventory.set_slot(chest, ItemStack::new(1, &Item::ELYTRA));
    bot.send_player_command(entity.entity_id, Action::StartFlyingElytra)
        .await?;
    wait_for(
        server,
        || entity.is_fall_flying(),
        "second glide did not start",
    )
    .await?;
    player.living_entity.fall_distance.store(50.0);
    let health = player.living_entity.health.load();
    bot.move_to(ground, true).await?;
    wait_for(
        server,
        || entity.on_ground.load(Ordering::Relaxed) && !entity.is_fall_flying(),
        "landing did not end flight",
    )
    .await?;
    if entity.pose.load() == EntityPose::FallFlying
        || player.fall_fly_ticks.load(Ordering::Relaxed) != 0
        || player.living_entity.health.load() < health
    {
        return Err("shallow landing left stale flight state or applied fall damage".into());
    }

    bot.move_to(air, false).await?;
    bot.send_player_command(entity.entity_id, Action::StartFlyingElytra)
        .await?;
    wait_for(
        server,
        || entity.is_fall_flying(),
        "third glide did not start",
    )
    .await?;
    let _ = player.set_gamemode(GameMode::Creative);
    player.set_flying(true);
    if entity.is_fall_flying() || entity.pose.load() == EntityPose::FallFlying {
        return Err("creative flight did not clear the elytra state".into());
    }
    player.set_flying(false);
    let _ = player.set_gamemode(old_mode);
    inventory.set_slot(chest, old_chest);
    inventory.set_selected_slot(old_slot);
    player.living_entity.fall_distance.store(0.0);
    bot.move_to(original_pos, true).await?;
    wait_for(
        server,
        || entity.pos.load().squared_distance_to_vec(&original_pos) < 1.0e-6,
        "elytra test player did not return to its original position",
    )
    .await?;
    world.set_block_state(&floor, previous_floor, BlockFlags::NOTIFY_ALL);

    summary.push(format!(
        "[PASS] bot:elytra_state_transitions ({}ms)",
        start.elapsed().as_millis()
    ));
    Ok(())
}
