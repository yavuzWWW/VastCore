use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::{damage::DamageType, sound::Sound};
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::{
    Control, MoveControlTrait,
    move_control::{MoveControl, Operation},
};
use crate::entity::ai::pathfinder::{Navigator, node::PathType};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::custom_sound::CustomSound;
use crate::entity::living::LivingEntity;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};
use crate::world::World;

const GUARDIAN_PREY: TargetMatch = TargetMatch::AnyOf(&[
    &EntityType::PLAYER,
    &EntityType::SQUID,
    &EntityType::GLOW_SQUID,
    &EntityType::AXOLOTL,
]);

/// Vanilla `GuardianAttackSelector`: nothing within 3 blocks.
fn outside_melee_range(hunter: &dyn Mob, target: &LivingEntity, _world: &World) -> bool {
    let hunter_pos = hunter.get_mob_entity().living_entity.entity.pos.load();
    target
        .entity
        .pos
        .load()
        .squared_distance_to_vec(&hunter_pos)
        > 9.0
}

pub const GUARDIAN_HUNTS: &[Hunt] =
    &[Hunt::new(1, GUARDIAN_PREY, true).matching(outside_melee_range)];

/// Shared swimming controls for guardian and elder guardian (upstream #4257 and #4253).
pub(super) fn initialize_swimming(mob: &mut MobEntity) -> Arc<AtomicBool> {
    let navigator = mob
        .navigator
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *navigator = Navigator::water_bound(false);
    navigator.set_pathfinding_malus(PathType::Water, 0.0);

    let moving = Arc::new(AtomicBool::new(false));
    *mob.move_control
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(GuardianMoveControl {
        inner: MoveControl::default(),
        moving: moving.clone(),
    });
    moving
}

struct GuardianMoveControl {
    inner: MoveControl,
    moving: Arc<AtomicBool>,
}

impl Control for GuardianMoveControl {}

impl GuardianMoveControl {
    fn set_moving(&self, mob: &dyn Mob, moving: bool) {
        self.moving.store(moving, Ordering::Relaxed);
        mob.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::guardian::DATA_ID_MOVING, moving);
    }
}

impl MoveControlTrait for GuardianMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;
        let active = self.inner.operation == Operation::MoveTo
            && !mob_entity.steered().navigator().is_done();
        let delta = self.get_wanted_position() - entity.pos.load();
        let distance = delta.length();
        if !active || !distance.is_finite() || distance <= f64::EPSILON {
            mob_entity.set_speed(0.0);
            self.set_moving(mob, false);
            return;
        }

        let yaw = MoveControl::rotlerp(
            entity.yaw.load(),
            delta.z.atan2(delta.x).to_degrees() as f32 - 90.0,
            MoveControl::MAX_TURN,
        );
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
        let target_speed = (self.inner.speed_modifier
            * living.get_attribute_value(&Attributes::MOVEMENT_SPEED))
            as f32;
        let speed = (target_speed - living.get_speed()).mul_add(0.125, living.get_speed());
        mob_entity.set_speed(speed);

        let phase =
            f64::from(entity.tick_count.load(Ordering::Relaxed)) + f64::from(entity.entity_id);
        let horizontal = (phase * 0.5).sin() * 0.05;
        let vertical = (phase * 0.75).sin() * 0.05;
        let (sin, cos) = f64::from(yaw).to_radians().sin_cos();
        entity.velocity.store(entity.velocity.load().add_raw(
            horizontal * cos,
            vertical * (sin + cos) * 0.25 + f64::from(speed) * delta.y / distance * 0.1,
            horizontal * sin,
        ));
        self.set_moving(mob, true);
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.inner.set_wanted_position(x, y, z, speed_modifier);
    }

    fn has_wanted(&self) -> bool {
        self.inner.has_wanted()
    }

    fn get_speed_modifier(&self) -> f64 {
        self.inner.get_speed_modifier()
    }

    fn get_wanted_position(&self) -> Vector3<f64> {
        self.inner.get_wanted_position()
    }
}

pub(super) fn travel_in_water(mob: &MobEntity, caller: &dyn EntityBase, moving: bool) {
    let living = &mob.living_entity;
    let entity = &living.entity;
    entity.update_velocity_from_input(living.movement_input.load(), 0.1);
    living.make_move(caller);
    let mut velocity = entity.velocity.load() * 0.9;
    if !moving && mob.get_target().is_none() {
        velocity.y -= 0.005;
    }
    entity.velocity.store(velocity);
}

pub struct GuardianEntity {
    pub mob_entity: MobEntity,
    moving: Arc<AtomicBool>,
}

impl GuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        let moving = initialize_swimming(&mut mob_entity);
        let guardian = Self { mob_entity, moving };
        let mob_arc = Arc::new(guardian);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(7, Box::new(WanderAroundGoal::with_interval(1.0, 80)));
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            let target_selector = &mob_arc.mob_entity.target_selector;
            hunting::apply(target_selector, &mob_arc.mob_entity, GUARDIAN_HUNTS);
        };

        mob_arc
    }
}

impl CustomSound for GuardianEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_water {
            Sound::EntityGuardianHurt
        } else {
            Sound::EntityGuardianHurtLand
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_water {
            Sound::EntityGuardianDeath
        } else {
            Sound::EntityGuardianDeathLand
        })
    }
}

impl Mob for GuardianEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn check_spawn_obstruction(&self, world: &World) -> bool {
        world.is_unobstructed(self.get_entity())
    }

    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        travel_in_water(
            &self.mob_entity,
            caller,
            self.moving.load(Ordering::Relaxed),
        );
    }

    fn on_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if damage_type != DamageType::THORNS
            && !self.moving.load(Ordering::Relaxed)
            && let Some(src) = source
            && let Some(living) = src.get_living_entity()
        {
            let _ = living.damage(src, 2.0, DamageType::THORNS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::{
        pathfinder::{node::Node, path::Path},
        test_support::world,
    };
    use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
    use pumpkin_world::chunk::ChunkData;

    #[tokio::test]
    async fn guardian_and_elder_guardian_accept_water_and_steer_vertically()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let chunk = ChunkData::empty_sync(0, 0);
        for x in 2..=10 {
            for y in 60..=70 {
                for z in 2..=10 {
                    chunk.set_block_absolute_y(
                        x,
                        y,
                        z,
                        pumpkin_data::Block::WATER.default_state.id,
                    );
                }
            }
        }
        world.level.loaded_chunks.insert(Vector2::new(0, 0), chunk);
        for entity_type in [&EntityType::GUARDIAN, &EntityType::ELDER_GUARDIAN] {
            let mob = crate::entity::r#type::from_type(
                entity_type,
                Vector3::new(5.0, 64.0, 5.0),
                &world,
                uuid::Uuid::new_v4(),
            );
            let entity = mob;
            let mob = entity.get_mob().ok_or("missing guardian mob")?;
            let body = mob.get_mob_entity();
            let living = &body.living_entity;
            assert!(mob.check_spawn_obstruction(&world));
            living.entity.touching_water.store(true, Ordering::Relaxed);
            let target = BlockPos::new(5, 67, 7);
            let mut navigator = body
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(!navigator.can_navigate_ground());
            assert!(navigator.move_to_path(
                Some(Path::new(vec![Node::new(target)], target, true)),
                1.0,
                living
            ));
            drop(navigator);

            for tick in 0..20 {
                living.entity.tick_count.store(tick, Ordering::Relaxed);
                let mut control = body
                    .move_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                control.set_wanted_position(5.0, 67.0, 7.0, 1.0);
                control.tick(mob);
                drop(control);
                mob.travel_in_water(entity.as_ref(), 0.08, false);
            }
            let pos = living.entity.pos.load();
            assert!(pos.y > 64.0, "guardian did not ascend: {pos:?}");
            assert!(pos.z > 5.0, "guardian did not swim forward: {pos:?}");
        }
        world.level.shutdown().await;
        Ok(())
    }
}
