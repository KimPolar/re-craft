//! Resumable random and group ordering for Java chunk-generation creatures.
//! Terrain, collision, mob construction and entity storage remain the caller's
//! responsibility; an unresolved attempt is never silently treated as failed.

use anyhow::{Result, bail, ensure};
use native_core::random::LegacyRandom;

use crate::{mob_spawns::CreatureSpawns, packs::ResourceId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatureAttempt {
    pub entity: ResourceId,
    pub x: i32,
    pub z: i32,
    pub member: i32,
    pub attempt: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreatureSpawnProgress {
    Pending,
    Attempt(CreatureAttempt),
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptRejection {
    /// Invalid placement follows Java's position random walk.
    Placement,
    /// Collision, spawn rule or entity-construction failure skips the walk.
    CollisionRuleOrConstruction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Group,
    Probe,
    FinishEntity,
    Finished,
}

#[derive(Clone, Debug)]
struct Group {
    entity: ResourceId,
    remaining: i32,
    member: i32,
    attempt: u8,
    x: i32,
    z: i32,
    start_x: i32,
    start_z: i32,
}

pub struct ChunkCreatureSpawner {
    random: LegacyRandom,
    spawns: CreatureSpawns,
    probability: f32,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    group: Option<Group>,
    state: State,
}

impl ChunkCreatureSpawner {
    /// The probability must already include all environment layers and
    /// Java's unit-range sanitization. Coordinates outside representable
    /// chunk bounds are rejected rather than inventing a wrapped region.
    pub fn new(
        world_seed: i64,
        chunk_x: i32,
        chunk_z: i32,
        spawn_mobs: bool,
        probability: f32,
        spawns: CreatureSpawns,
    ) -> Result<Self> {
        if spawn_mobs && !spawns.is_empty() {
            ensure!(
                probability.is_finite() && (0.0..=0.999_999_9_f32).contains(&probability),
                "invalid resolved creature spawn probability"
            );
        }
        let min_x = chunk_x
            .checked_mul(16)
            .ok_or_else(|| anyhow::anyhow!("spawn chunk X overflow"))?;
        let min_z = chunk_z
            .checked_mul(16)
            .ok_or_else(|| anyhow::anyhow!("spawn chunk Z overflow"))?;
        let max_x = min_x
            .checked_add(16)
            .ok_or_else(|| anyhow::anyhow!("spawn chunk X overflow"))?;
        let max_z = min_z
            .checked_add(16)
            .ok_or_else(|| anyhow::anyhow!("spawn chunk Z overflow"))?;
        let mut random = LegacyRandom::new(world_seed);
        random.set_decoration_seed(world_seed, min_x, min_z);
        let state = if !spawn_mobs || spawns.is_empty() {
            State::Finished
        } else {
            State::Group
        };
        Ok(Self {
            random,
            spawns,
            probability,
            min_x,
            min_z,
            max_x,
            max_z,
            group: None,
            state,
        })
    }

    /// Examine at most `group_budget` new groups; existing unresolved probes
    /// are returned unchanged without consuming any more random numbers.
    pub fn next(&mut self, group_budget: usize) -> Result<CreatureSpawnProgress> {
        for _ in 0..group_budget {
            match self.state {
                State::Finished => return Ok(CreatureSpawnProgress::Finished),
                State::FinishEntity => {
                    bail!("finish the created entity before requesting another attempt")
                }
                State::Probe => return Ok(CreatureSpawnProgress::Attempt(self.attempt())),
                State::Group => {
                    if self.random.next_float() >= self.probability {
                        self.state = State::Finished;
                        return Ok(CreatureSpawnProgress::Finished);
                    }
                    let (entity, count) = self
                        .spawns
                        .choose(&mut self.random)
                        .expect("nonempty validated list");
                    let x = self.min_x + self.random.next_int_bound(16).unwrap();
                    let z = self.min_z + self.random.next_int_bound(16).unwrap();
                    if count > 0 {
                        self.group = Some(Group {
                            entity,
                            remaining: count,
                            member: 0,
                            attempt: 0,
                            x,
                            z,
                            start_x: x,
                            start_z: z,
                        });
                        self.state = State::Probe;
                        return Ok(CreatureSpawnProgress::Attempt(self.attempt()));
                    }
                }
            }
        }
        match self.state {
            State::Finished => Ok(CreatureSpawnProgress::Finished),
            State::Probe => Ok(CreatureSpawnProgress::Attempt(self.attempt())),
            State::FinishEntity => {
                bail!("finish the created entity before requesting another attempt")
            }
            State::Group => Ok(CreatureSpawnProgress::Pending),
        }
    }

    fn attempt(&self) -> CreatureAttempt {
        let group = self.group.as_ref().expect("probe has a group");
        CreatureAttempt {
            entity: group.entity.clone(),
            x: group.x,
            z: group.z,
            member: group.member,
            attempt: group.attempt,
        }
    }

    /// Java wanders after an invalid placement or a created mob that failed
    /// its own spawn checks, but not after a collision/rule/construction
    /// failure that skips directly to the next attempt.
    pub fn reject(&mut self, reason: AttemptRejection) -> Result<()> {
        ensure!(self.state == State::Probe, "no creature probe is pending");
        self.finish_attempt(false, reason == AttemptRejection::Placement);
        Ok(())
    }

    /// Once entity construction succeeds Java draws yaw before testing the
    /// mob's own spawn rules. The caller must then report the final outcome.
    pub fn entity_created(&mut self) -> Result<f32> {
        ensure!(self.state == State::Probe, "no creature probe is pending");
        self.state = State::FinishEntity;
        Ok(self.random.next_float() * 360.0_f32)
    }

    /// The current constructed entity remains pending until the caller
    /// reports its post-construction checks and finalization outcome.
    pub fn created_attempt(&self) -> Option<CreatureAttempt> {
        (self.state == State::FinishEntity).then(|| self.attempt())
    }

    pub fn finish_entity(&mut self, accepted: bool) -> Result<()> {
        ensure!(
            self.state == State::FinishEntity,
            "no created creature is pending"
        );
        self.finish_attempt(accepted, true);
        Ok(())
    }

    fn finish_attempt(&mut self, accepted: bool, wander: bool) {
        let group = self.group.as_mut().expect("attempt has a group");
        if wander {
            group.x = group
                .x
                .wrapping_add(self.random.next_int_bound(5).unwrap())
                .wrapping_sub(self.random.next_int_bound(5).unwrap());
            group.z = group
                .z
                .wrapping_add(self.random.next_int_bound(5).unwrap())
                .wrapping_sub(self.random.next_int_bound(5).unwrap());
            while group.x < self.min_x
                || group.x >= self.max_x
                || group.z < self.min_z
                || group.z >= self.max_z
            {
                group.x = group
                    .start_x
                    .wrapping_add(self.random.next_int_bound(5).unwrap())
                    .wrapping_sub(self.random.next_int_bound(5).unwrap());
                group.z = group
                    .start_z
                    .wrapping_add(self.random.next_int_bound(5).unwrap())
                    .wrapping_sub(self.random.next_int_bound(5).unwrap());
            }
        }
        group.attempt += 1;
        if accepted || group.attempt == 4 {
            group.remaining -= 1;
            group.member += 1;
            group.attempt = 0;
            if group.remaining == 0 {
                self.group = None;
                self.state = State::Group;
                return;
            }
        }
        self.state = State::Probe;
    }

    pub fn into_random(self) -> Result<LegacyRandom> {
        ensure!(
            self.state == State::Finished,
            "creature generation is unfinished"
        );
        Ok(self.random)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob_spawns::CreatureSpawnLayer;
    use serde_json::json;

    fn entries(count: i32) -> CreatureSpawns {
        let attributes = json!({"gameplay/natural_mob_spawns": {
            "spawns_by_category":{"creature":[{"type":"minecraft:pig","weight":1,"count":count}]},
            "spawn_costs":{}
        }});
        let mut spawns = CreatureSpawns::default();
        spawns.apply(
            CreatureSpawnLayer::from_attributes(&attributes, |_| Some(false))
                .unwrap()
                .unwrap(),
        );
        spawns
    }
    #[test]
    fn unresolved_attempt_and_entity_creation_preserve_random_order() {
        let mut planner =
            ChunkCreatureSpawner::new(42, 0, 0, true, 0.999_999_9, entries(1)).unwrap();
        let CreatureSpawnProgress::Attempt(first) = planner.next(1).unwrap() else {
            panic!("expected attempt")
        };
        let before = planner.random.state();
        assert_eq!(
            planner.next(0).unwrap(),
            CreatureSpawnProgress::Attempt(first)
        );
        assert_eq!(planner.random.state(), before);
        let mut reference = planner.random.clone();
        let expected_yaw = reference.next_float() * 360.0_f32;
        assert_eq!(
            planner.entity_created().unwrap().to_bits(),
            expected_yaw.to_bits()
        );
        assert_eq!(planner.random.state(), reference.state());
        assert!(planner.next(1).is_err());
        planner.finish_entity(true).unwrap();
        assert!(planner.finish_entity(true).is_err());
    }

    #[test]
    fn empty_lists_and_zero_count_groups_are_bounded() {
        let mut empty =
            ChunkCreatureSpawner::new(42, -1, 1, true, 0.5, CreatureSpawns::default()).unwrap();
        assert_eq!(empty.next(0).unwrap(), CreatureSpawnProgress::Finished);
        let mut zero = ChunkCreatureSpawner::new(42, 0, 0, true, 0.999_999_9, entries(0)).unwrap();
        assert_eq!(zero.next(0).unwrap(), CreatureSpawnProgress::Pending);
        assert_eq!(zero.next(1).unwrap(), CreatureSpawnProgress::Pending);
        assert!(ChunkCreatureSpawner::new(42, 0, 0, true, 1.0, entries(1)).is_err());
        let mut disabled =
            ChunkCreatureSpawner::new(42, 0, 0, false, f32::NAN, entries(1)).unwrap();
        assert_eq!(disabled.next(1).unwrap(), CreatureSpawnProgress::Finished);
        let mut empty =
            ChunkCreatureSpawner::new(42, 0, 0, true, f32::INFINITY, CreatureSpawns::default())
                .unwrap();
        assert_eq!(empty.next(1).unwrap(), CreatureSpawnProgress::Finished);
    }

    #[test]
    fn accepted_and_rejected_created_entities_advance_the_correct_member() {
        let mut planner =
            ChunkCreatureSpawner::new(42, 0, 0, true, 0.999_999_9, entries(2)).unwrap();
        let CreatureSpawnProgress::Attempt(first) = planner.next(1).unwrap() else {
            panic!("expected attempt")
        };
        assert_eq!((first.member, first.attempt), (0, 0));
        planner.entity_created().unwrap();
        planner.finish_entity(false).unwrap();
        let CreatureSpawnProgress::Attempt(second) = planner.next(0).unwrap() else {
            panic!("expected retry")
        };
        assert_eq!((second.member, second.attempt), (0, 1));
        planner.entity_created().unwrap();
        planner.finish_entity(true).unwrap();
        let CreatureSpawnProgress::Attempt(third) = planner.next(0).unwrap() else {
            panic!("expected next member")
        };
        assert_eq!((third.member, third.attempt), (1, 0));
        for n in 0..4 {
            let CreatureSpawnProgress::Attempt(attempt) = planner.next(0).unwrap() else {
                panic!("expected retry")
            };
            assert_eq!((attempt.member, attempt.attempt), (1, n));
            planner
                .reject(AttemptRejection::CollisionRuleOrConstruction)
                .unwrap();
        }
        assert_eq!(planner.state, State::Group);
    }
}
