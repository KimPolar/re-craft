//! Resumable 26.3 on-ground mob placement queries.
//!
//! The world supplies the selected entity's heightmap, block predicates,
//! active tags and border. Missing or unsupported facts are returned as
//! errors without advancing the current query.
use anyhow::{Context, Result};
use glam::IVec3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MobSpawnHeightmap {
    WorldSurface,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

pub trait MobPlacementWorld {
    /// Java LevelReader.getHeight: the first available Y for this entity's
    /// registered spawn heightmap, not the highest occupied Y.
    fn height(&mut self, x: i32, z: i32) -> Result<i32>;
    fn is_air(&mut self, pos: IVec3) -> Result<bool>;
    fn pathfindable_land(&mut self, pos: IVec3) -> Result<bool>;
    fn within_border(&mut self, pos: IVec3) -> Result<bool>;
    fn valid_spawn_support(&mut self, pos: IVec3) -> Result<bool>;
    fn full_collision(&mut self, pos: IVec3) -> Result<bool>;
    fn signal_source(&mut self, pos: IVec3) -> Result<bool>;
    fn fluid_empty(&mut self, pos: IVec3) -> Result<bool>;
    fn prevents_mob_spawning(&mut self, pos: IVec3) -> Result<bool>;
    fn dangerous_to_entity(&mut self, pos: IVec3) -> Result<bool>;
}

fn below(pos: IVec3) -> Result<IVec3> {
    Ok(IVec3::new(
        pos.x,
        pos.y.checked_sub(1).context("mob spawn Y underflow")?,
        pos.z,
    ))
}
fn above(pos: IVec3) -> Result<IVec3> {
    Ok(IVec3::new(
        pos.x,
        pos.y.checked_add(1).context("mob spawn Y overflow")?,
        pos.z,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TopPhase {
    Height,
    FindAir,
    FindFloor,
    Adjust,
    Done,
}

/// `NaturalSpawner.getTopNonCollidingPos` for ON_GROUND placement.
pub struct TopMobSpawnPosition {
    pos: IVec3,
    min_y: i32,
    has_ceiling: bool,
    phase: TopPhase,
}

impl TopMobSpawnPosition {
    pub fn new(x: i32, z: i32, min_y: i32, has_ceiling: bool) -> Self {
        Self {
            pos: IVec3::new(x, 0, z),
            min_y,
            has_ceiling,
            phase: TopPhase::Height,
        }
    }
    /// Each budget unit performs at most one world query. An error preserves
    /// the position and phase so the same input can be supplied and retried.
    pub fn poll(
        &mut self,
        world: &mut impl MobPlacementWorld,
        budget: usize,
    ) -> Result<Option<IVec3>> {
        for _ in 0..budget {
            match self.phase {
                TopPhase::Height => {
                    self.pos.y = world.height(self.pos.x, self.pos.z)?;
                    self.phase = if self.has_ceiling {
                        TopPhase::FindAir
                    } else {
                        TopPhase::Adjust
                    };
                }
                TopPhase::FindAir => {
                    let candidate = below(self.pos)?;
                    let air = world.is_air(candidate)?;
                    self.pos = candidate;
                    if air {
                        self.phase = TopPhase::FindFloor;
                    }
                }
                TopPhase::FindFloor => {
                    let candidate = below(self.pos)?;
                    let air = world.is_air(candidate)?;
                    self.pos = candidate;
                    if !air || candidate.y <= self.min_y {
                        self.phase = TopPhase::Adjust;
                    }
                }
                TopPhase::Adjust => {
                    let candidate = below(self.pos)?;
                    if world.pathfindable_land(candidate)? {
                        self.pos = candidate;
                    }
                    self.phase = TopPhase::Done;
                }
                TopPhase::Done => break,
            }
        }
        Ok((self.phase == TopPhase::Done).then_some(self.pos))
    }
}

/// Java's `NaturalSpawner.isValidEmptySpawnBlock`, preserving its short-circuit
/// order so unknown later facts are not needed after a definite rejection.
pub fn valid_empty_spawn_block(world: &mut impl MobPlacementWorld, pos: IVec3) -> Result<bool> {
    if world.full_collision(pos)?
        || world.signal_source(pos)?
        || !world.fluid_empty(pos)?
        || world.prevents_mob_spawning(pos)?
        || world.dangerous_to_entity(pos)?
    {
        return Ok(false);
    }
    Ok(true)
}

/// `SpawnPlacementTypes.ON_GROUND.isSpawnPositionOk` for a known entity type.
pub fn on_ground_spawn_position_ok(world: &mut impl MobPlacementWorld, pos: IVec3) -> Result<bool> {
    if !world.within_border(pos)? {
        return Ok(false);
    }
    if !world.valid_spawn_support(below(pos)?)? {
        return Ok(false);
    }
    if !valid_empty_spawn_block(world, pos)? {
        return Ok(false);
    }
    valid_empty_spawn_block(world, above(pos)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use std::collections::BTreeSet;

    struct World {
        solid: BTreeSet<i32>,
        path: BTreeSet<i32>,
        unavailable: Option<i32>,
        border: bool,
        support: bool,
        full: bool,
        later_unknown: bool,
    }
    impl MobPlacementWorld for World {
        fn height(&mut self, _: i32, _: i32) -> Result<i32> {
            Ok(12)
        }
        fn is_air(&mut self, pos: IVec3) -> Result<bool> {
            if self.unavailable == Some(pos.y) {
                bail!("missing block");
            }
            Ok(!self.solid.contains(&pos.y))
        }
        fn pathfindable_land(&mut self, pos: IVec3) -> Result<bool> {
            Ok(self.path.contains(&pos.y))
        }
        fn within_border(&mut self, _: IVec3) -> Result<bool> {
            Ok(self.border)
        }
        fn valid_spawn_support(&mut self, _: IVec3) -> Result<bool> {
            Ok(self.support)
        }
        fn full_collision(&mut self, _: IVec3) -> Result<bool> {
            Ok(self.full)
        }
        fn signal_source(&mut self, _: IVec3) -> Result<bool> {
            if self.later_unknown {
                bail!("missing signal");
            }
            Ok(false)
        }
        fn fluid_empty(&mut self, _: IVec3) -> Result<bool> {
            Ok(true)
        }
        fn prevents_mob_spawning(&mut self, _: IVec3) -> Result<bool> {
            Ok(false)
        }
        fn dangerous_to_entity(&mut self, _: IVec3) -> Result<bool> {
            Ok(false)
        }
    }
    fn world() -> World {
        World {
            solid: BTreeSet::from([7, 10, 11]),
            path: BTreeSet::from([6]),
            unavailable: None,
            border: true,
            support: true,
            full: false,
            later_unknown: false,
        }
    }
    #[test]
    fn ceiling_scan_can_pause_and_retry_without_skipping() {
        let mut world = world();
        let mut search = TopMobSpawnPosition::new(-2, 3, 0, true);
        assert_eq!(search.poll(&mut world, 1).unwrap(), None);
        world.unavailable = Some(10);
        assert_eq!(search.poll(&mut world, 1).unwrap(), None);
        assert!(search.poll(&mut world, 1).is_err());
        assert!(search.poll(&mut world, 1).is_err());
        world.unavailable = None;
        while search.poll(&mut world, 1).unwrap().is_none() {}
        assert_eq!(
            search.poll(&mut world, 0).unwrap(),
            Some(IVec3::new(-2, 6, 3))
        );
    }
    #[test]
    fn ground_checks_short_circuit_unknown_later_data() {
        let mut world = world();
        world.full = true;
        world.later_unknown = true;
        assert!(!on_ground_spawn_position_ok(&mut world, IVec3::new(1, 10, 1)).unwrap());
        world.full = false;
        assert!(on_ground_spawn_position_ok(&mut world, IVec3::new(1, 10, 1)).is_err());
        world.border = false;
        assert!(!on_ground_spawn_position_ok(&mut world, IVec3::new(1, 10, 1)).unwrap());
    }
}
