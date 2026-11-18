use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{AxolotlVariantImpl, BucketEntityDataImpl};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        active_target::ActiveTargetGoal, breed::BreedGoal, escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, revenge::RevengeGoal,
        swim::SwimGoal, tempt::TemptGoal, try_find_water::TryFindWaterGoal,
        wander_around::WanderAroundGoal,
    },
    bucketable::{Bucketable, bucket_mob_pickup},
    mob::{
        Mob, MobEntity,
        spawn::{EntitySpawnReason, SpawnGroupData},
    },
    passive::animal::Animal,
    player::Player,
};

use crate::world::World;

const MAX_AIR_SUPPLY: i32 = 6000;
const RARE_VARIANT_CHANCE: u32 = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum AxolotlVariant {
    #[default]
    Lucy = 0,
    Wild = 1,
    Gold = 2,
    Cyan = 3,
    Blue = 4,
}

impl AxolotlVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Wild,
            2 => Self::Gold,
            3 => Self::Cyan,
            4 => Self::Blue,
            _ => Self::Lucy,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub fn random_variant() -> Self {
        Self::from_id(rand::rng().random_range(0..4))
    }
}

const fn offspring_variant(
    parent: AxolotlVariant,
    partner: AxolotlVariant,
    rare: bool,
    inherit_parent: bool,
) -> AxolotlVariant {
    if rare {
        AxolotlVariant::Blue
    } else if inherit_parent {
        parent
    } else {
        partner
    }
}

pub struct AxolotlEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub playing_dead: AtomicBool,
    pub from_bucket: AtomicBool,
    pub play_dead_ticks: AtomicI32,
}

impl AxolotlEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.air_supply.store(MAX_AIR_SUPPLY, Ordering::Relaxed);
        let mob_entity = MobEntity::new(entity);
        let axolotl = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(AxolotlVariant::Lucy.id()),
            playing_dead: AtomicBool::new(false),
            from_bucket: AtomicBool::new(false),
            play_dead_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(axolotl);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(TryFindWaterGoal::new()));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.5));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                Box::new(TemptGoal::with_tag(
                    1.25,
                    &tag::Item::MINECRAFT_AXOLOTL_FOOD,
                    false,
                )),
            );
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(MeleeAttackGoal::new(1.2, false)));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let target_selector = &mob_arc.mob_entity.target_selector;

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::DROWNED, false),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::GUARDIAN, false),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(
                    &mob_arc.mob_entity,
                    &EntityType::ELDER_GUARDIAN,
                    false,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::SQUID, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::GLOW_SQUID, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::COD, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::SALMON, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(
                    &mob_arc.mob_entity,
                    &EntityType::TROPICAL_FISH,
                    false,
                ),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> AxolotlVariant {
        AxolotlVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: AxolotlVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(variant.id()),
        );
    }

    #[must_use]
    pub fn is_playing_dead(&self) -> bool {
        self.playing_dead.load(Ordering::Relaxed)
    }

    pub fn set_playing_dead(&self, playing_dead: bool) {
        self.playing_dead.store(playing_dead, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            playing_dead,
        );
    }

    #[must_use]
    pub fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            from_bucket,
        );
    }
}

impl AgeableMob for AxolotlEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for AxolotlEntity {
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::AXOLOTL,
        ));
        let partner_variant = partner
            .cast_any()
            .downcast_ref::<Self>()
            .map_or_else(|| self.get_variant(), Self::get_variant);
        let mut rng = self.get_random();
        let rare = rng.random_range(0..RARE_VARIANT_CHANCE) == 0;
        baby.set_variant(offspring_variant(
            self.get_variant(),
            partner_variant,
            rare,
            rng.random::<bool>(),
        ));
        baby.mob_entity.set_persistence_required();
        baby
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_AXOLOTL_FOOD)
    }
}

impl Mob for AxolotlEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if reason == EntitySpawnReason::Bucket {
            return group_data;
        }

        let (variants, size) = match group_data {
            Some(SpawnGroupData::Axolotl { variants, size }) => (variants, size),
            _ => (
                [
                    AxolotlVariant::random_variant(),
                    AxolotlVariant::random_variant(),
                ],
                0,
            ),
        };
        self.set_variant(variants[self.get_random().random_range(0..2)]);
        if size >= 2 {
            self.set_age(-24000);
        }
        self.mob_entity.finalize_spawn_base();
        Some(SpawnGroupData::Axolotl {
            variants,
            size: size + 1,
        })
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Variant", self.get_variant().id());
        nbt.put_bool("FromBucket", self.is_from_bucket());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_variant(AxolotlVariant::from_id(nbt.get_int("Variant").unwrap_or(0)));
        self.set_from_bucket(nbt.get_bool("FromBucket").unwrap_or(false));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn check_spawn_obstruction(&self, world: &crate::world::World) -> bool {
        world.is_unobstructed(self.get_entity())
    }

    fn get_walk_target_value(&self, _pos: &BlockPos) -> f32 {
        0.0
    }

    fn as_bucketable(&self) -> Option<&dyn Bucketable> {
        Some(self)
    }

    fn mob_set_variant_name(&self, name: &str) {
        if let Some(variant) = pumpkin_data::entity_variant::AxolotlVariant::from_name(
            name.strip_prefix("minecraft:").unwrap_or(name),
        ) {
            self.set_variant(AxolotlVariant::from_id(variant.id()));
        }
    }

    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        !self.is_from_bucket() && self.get_entity().custom_name.load().is_none()
    }

    fn requires_custom_persistence(&self) -> bool {
        self.is_from_bucket()
    }

    fn mob_max_air_supply(&self) -> i32 {
        MAX_AIR_SUPPLY
    }

    fn base_tick(&self) {
        let living = &self.mob_entity.living_entity;
        if self.mob_entity.is_no_ai() {
            living.tick_air_supply(self);
            return;
        }
        if !living.is_alive() {
            return;
        }
        let entity = &living.entity;
        if entity.is_in_water_or_rain() {
            entity.set_air_supply(MAX_AIR_SUPPLY);
        } else {
            entity.set_air_supply(entity.air_supply.load(Ordering::Relaxed).saturating_sub(1));
            if living.should_take_drowning_damage() {
                entity.set_air_supply(0);
                self.damage(self, 2.0, DamageType::DRY_OUT);
            }
        }
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let play_dead_ticks = self.play_dead_ticks.load(Ordering::Relaxed);
        if play_dead_ticks > 0 {
            let remaining = play_dead_ticks - 1;
            self.play_dead_ticks.store(remaining, Ordering::Relaxed);
            if remaining == 0 {
                self.set_playing_dead(false);
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::axolotl::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(self.get_variant().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            self.is_playing_dead(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            self.is_from_bucket(),
        );
    }

    fn mob_interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        bucket_mob_pickup(player, item_stack, self)
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack)
    }
}

// Use the same capture and release path as fish and tadpoles (upstream #3933 and #3971).
impl Bucketable for AxolotlEntity {
    fn is_from_bucket(&self) -> bool {
        Self::is_from_bucket(self)
    }

    fn set_from_bucket(&self, from_bucket: bool) {
        Self::set_from_bucket(self, from_bucket);
    }

    fn bucket_item(&self) -> &'static Item {
        &Item::AXOLOTL_BUCKET
    }

    fn pickup_sound(&self) -> Sound {
        Sound::ItemBucketFillAxolotl
    }

    fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
        self.save_default_data_to_bucket_tag(bucket);
        let variant = pumpkin_data::entity_variant::AxolotlVariant::by_id(self.get_variant().id());
        bucket.set_data_component(AxolotlVariantImpl {
            value: variant.name().into(),
        });
        let mut nbt = bucket
            .get_data_component::<BucketEntityDataImpl>()
            .and_then(|data| data.nbt.clone())
            .unwrap_or_default();
        nbt.put_int("Age", AgeableMob::get_age(self));
        nbt.put_bool("AgeLocked", self.is_age_locked());
        bucket.set_data_component(BucketEntityDataImpl { nbt: Some(nbt) });
    }

    fn load_from_bucket_tag(&self, nbt: &NbtCompound) {
        self.load_default_data_from_bucket_tag(nbt);
        self.set_age(nbt.get_int("Age").unwrap_or(0));
        self.set_age_locked(nbt.get_bool("AgeLocked").unwrap_or(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn offspring_inherits_parent_variants_except_on_rare_roll() {
        let parent = AxolotlVariant::Cyan;
        let partner = AxolotlVariant::Gold;
        assert_eq!(offspring_variant(parent, partner, false, true), parent);
        assert_eq!(offspring_variant(parent, partner, false, false), partner);
        assert_eq!(
            offspring_variant(parent, partner, true, true),
            AxolotlVariant::Blue
        );
        assert_eq!(
            offspring_variant(AxolotlVariant::Blue, partner, false, true),
            AxolotlVariant::Blue
        );
    }

    #[tokio::test]
    async fn axolotl_groups_share_common_variants_and_bucket_spawn_preserves_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let mut group = None;
        for index in 0..4 {
            let axolotl = AxolotlEntity::new(Entity::new(
                world.clone(),
                Vector3::new(5.5, 64.0, 5.5),
                &EntityType::AXOLOTL,
            ));
            assert_eq!(axolotl.get_variant(), AxolotlVariant::Lucy);
            group = axolotl.finalize_spawn(&world, EntitySpawnReason::Natural, group);
            let Some(SpawnGroupData::Axolotl { variants, size }) = &group else {
                return Err("missing axolotl group".into());
            };
            assert_eq!(*size, index + 1);
            assert!(
                variants
                    .iter()
                    .all(|variant| *variant != AxolotlVariant::Blue)
            );
            assert!(variants.contains(&axolotl.get_variant()));
            assert_eq!(axolotl.get_age() < 0, index >= 2);

            axolotl.set_variant(AxolotlVariant::Blue);
            group = axolotl.finalize_spawn(&world, EntitySpawnReason::Bucket, group);
            assert_eq!(axolotl.get_variant(), AxolotlVariant::Blue);
        }
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn axolotl_rehydrates_in_water_and_loses_air_on_land()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let axolotl = AxolotlEntity::new(Entity::new(
            world.clone(),
            Vector3::new(5.5, 64.0, 5.5),
            &EntityType::AXOLOTL,
        ));
        let entity = axolotl.get_entity();
        entity.touching_water.store(false, Ordering::Relaxed);
        assert_eq!(entity.air_supply.load(Ordering::Relaxed), MAX_AIR_SUPPLY);
        axolotl.base_tick();
        assert_eq!(
            entity.air_supply.load(Ordering::Relaxed),
            MAX_AIR_SUPPLY - 1
        );
        entity.touching_water.store(true, Ordering::Relaxed);
        axolotl.base_tick();
        assert_eq!(entity.air_supply.load(Ordering::Relaxed), MAX_AIR_SUPPLY);
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn axolotl_bucket_preserves_variant_health_and_age()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let source = AxolotlEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::AXOLOTL,
        ));
        source.set_variant(AxolotlVariant::Blue);
        source.set_age(-1200);
        source.set_age_locked(true);
        source.mob_entity.living_entity.set_health(7.0);

        let mut bucket = ItemStack::new(1, &Item::AXOLOTL_BUCKET);
        let bucketable = source.as_bucketable().ok_or("missing Bucketable")?;
        bucketable.save_to_bucket_tag(&mut bucket);
        let variant = bucket
            .get_data_component::<AxolotlVariantImpl>()
            .ok_or("missing axolotl variant")?;
        let data = bucket
            .get_data_component::<BucketEntityDataImpl>()
            .and_then(|data| data.nbt.as_ref())
            .ok_or("missing bucket NBT")?;
        assert_eq!(&*variant.value, "blue");

        let released = AxolotlEntity::new(Entity::new(
            world.clone(),
            Vector3::new(2.5, 64.0, 0.5),
            &EntityType::AXOLOTL,
        ));
        released.set_variant(AxolotlVariant::Gold);
        released.mob_set_variant_name(&variant.value);
        released.load_from_bucket_tag(data);
        released.set_from_bucket(true);
        assert_eq!(released.get_variant(), AxolotlVariant::Blue);
        assert_eq!(released.get_age(), -1200);
        assert!(released.is_age_locked());
        assert!((released.mob_entity.living_entity.health.load() - 7.0).abs() < f32::EPSILON);
        assert!(released.requires_custom_persistence());
        assert!(!released.remove_when_far_away(100_000.0));
        world.level.shutdown().await;
        Ok(())
    }
}
