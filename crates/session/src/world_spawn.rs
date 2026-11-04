//! The shared-spawn search used by Java 26.3 after no player spawn is usable.
//!
//! This module does not load chunks or invent missing collision information.
//! Its caller must supply authoritative heightmaps, block/fluid shapes and
//! entity collisions from the target dimension before using the result.

use anyhow::{Result, ensure};
use glam::IVec3;

/// `Level.getWorldBorderAdjustedRespawnData` keeps a spawn inside the border;
/// outside it, Java uses the border center's column and its heightmap. The
/// boolean indicates that the caller must fetch the center's heightmap.
pub fn border_adjusted_column(
    spawn: IVec3,
    border: crate::respawn::RespawnBorder,
    center_x: f64,
    center_z: f64,
) -> Result<(i32, i32, bool)> {
    ensure!(border.is_valid(), "world border has invalid bounds");
    let x = f64::from(spawn.x);
    let z = f64::from(spawn.z);
    if x >= border.min_x && x < border.max_x && z >= border.min_z && z < border.max_z {
        return Ok((spawn.x, spawn.z, false));
    }
    let x = center_x.floor();
    let z = center_z.floor();
    ensure!(
        x.is_finite()
            && z.is_finite()
            && x >= f64::from(i32::MIN)
            && x <= f64::from(i32::MAX)
            && z >= f64::from(i32::MIN)
            && z <= f64::from(i32::MAX),
        "world border center is out of range"
    );
    Ok((x as i32, z as i32, true))
}

/// Java uses an independently random starting offset, then visits at most 1024
/// candidates. The caller supplies the offset so it can use a random source
/// independent of player and world simulation randomness.
#[derive(Clone, Debug)]
pub struct WorldSpawnCandidates {
    origin: IVec3,
    radius: i32,
    count: i32,
    offset: i32,
    next: i32,
}

impl WorldSpawnCandidates {
    pub fn new(
        origin: IVec3,
        requested_radius: i32,
        border_distance: f64,
        offset: i32,
    ) -> Result<Self> {
        ensure!(
            border_distance.is_finite(),
            "non-finite world-border distance"
        );
        // These bounds also avoid Java's integer-overflow behavior for absurd
        // modded game rules until that behavior can be handled explicitly.
        ensure!(
            border_distance.abs() <= 60_000_000.0,
            "unsupported world-border distance"
        );
        let distance = border_distance.floor() as i32;
        let mut radius = requested_radius.max(0).min(distance);
        if distance <= 1 {
            radius = 1;
        }
        ensure!(
            (0..=1_000_000).contains(&radius),
            "unsupported spawn-search radius"
        );
        let side = i64::from(radius) * 2 + 1;
        let count = (side * side).min(1024) as i32;
        ensure!((0..count).contains(&offset), "invalid spawn-search offset");
        Ok(Self {
            origin,
            radius,
            count,
            offset,
            next: 0,
        })
    }

    pub fn radius(&self) -> i32 {
        self.radius
    }
    pub fn candidate_count(&self) -> i32 {
        self.count
    }
}

impl Iterator for WorldSpawnCandidates {
    type Item = (i32, i32);

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.count {
            return None;
        }
        let coprime = if self.count <= 16 { self.count - 1 } else { 17 };
        let index = (self.offset + coprime * self.next) % self.count;
        self.next += 1;
        let side = self.radius * 2 + 1;
        let x = self
            .origin
            .x
            .wrapping_add(index % side)
            .wrapping_sub(self.radius);
        let z = self
            .origin
            .z
            .wrapping_add(index / side)
            .wrapping_sub(self.radius);
        Some((x, z))
    }
}

/// Heightmap results are Java's highest matching Y, not first-available Y.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnColumnHeights {
    pub motion_blocking: i32,
    pub world_surface: i32,
    pub ocean_floor: i32,
}

/// An incomplete query must return an error instead of pretending to be air.
pub trait SharedSpawnWorld {
    fn min_y(&self) -> i32;
    /// Java's maximum build Y, inclusive.
    fn max_y(&self) -> i32;
    /// A ceiling dimension supplies its generator's spawn height; otherwise
    /// `None` selects the motion-blocking heightmap.
    fn ceiling_spawn_height(&self) -> Option<i32>;
    fn heights(&mut self, x: i32, z: i32) -> Result<SpawnColumnHeights>;
    fn has_fluid(&mut self, pos: IVec3) -> Result<bool>;
    fn full_top_face(&mut self, pos: IVec3) -> Result<bool>;
    /// Must include block, fluid and entity collisions, as Java does for a
    /// standing player with an empty collision context.
    fn clear_player(&mut self, pos: IVec3) -> Result<bool>;
}

/// `PlayerSpawnFinder.getLevelRespawnPos` for one already-loaded column.
pub fn column_spawn<W: SharedSpawnWorld>(world: &mut W, x: i32, z: i32) -> Result<Option<IVec3>> {
    let heights = world.heights(x, z)?;
    let top = world
        .ceiling_spawn_height()
        .unwrap_or(heights.motion_blocking);
    let min_y = world.min_y();
    ensure!(top < i32::MAX - 1, "spawn column height is out of range");
    if top < min_y {
        return Ok(None);
    }
    ensure!(
        i64::from(top) - i64::from(min_y) <= 4096,
        "unsupported spawn column height"
    );
    if heights.world_surface <= top && heights.world_surface > heights.ocean_floor {
        return Ok(None);
    }
    for y in (min_y..=top + 1).rev() {
        let pos = IVec3::new(x, y, z);
        if world.has_fluid(pos)? {
            return Ok(None);
        }
        if world.full_top_face(pos)? {
            return Ok(Some(IVec3::new(x, y + 1, z)));
        }
    }
    Ok(None)
}

/// Incremental form of MinecraftServer's initial global-spawn search. The
/// caller supplies the generator's origin chunk and fallback spawn height
/// (including the WORLD_SURFACE fallback when the generator height is below
/// the dimension minimum). This chooses the world's spawn suggestion, not a
/// collision-checked player position.
pub struct InitialWorldSpawn {
    origin: (i32, i32),
    fallback: IVec3,
    dx: i32,
    dz: i32,
    step_x: i32,
    step_z: i32,
    chunks_visited: u16,
    column: u16,
    result: Option<IVec3>,
}

impl InitialWorldSpawn {
    pub fn new(origin_chunk_x: i32, origin_chunk_z: i32, fallback_y: i32) -> Result<Self> {
        for coord in [origin_chunk_x, origin_chunk_z] {
            coord
                .checked_sub(5)
                .and_then(|v| v.checked_mul(16))
                .ok_or_else(|| anyhow::anyhow!("initial spawn search is out of range"))?;
            coord
                .checked_add(5)
                .and_then(|v| v.checked_mul(16))
                .and_then(|v| v.checked_add(15))
                .ok_or_else(|| anyhow::anyhow!("initial spawn search is out of range"))?;
        }
        Ok(Self {
            origin: (origin_chunk_x, origin_chunk_z),
            fallback: IVec3::new(origin_chunk_x * 16 + 8, fallback_y, origin_chunk_z * 16 + 8),
            dx: 0,
            dz: 0,
            step_x: 0,
            step_z: -1,
            chunks_visited: 0,
            column: 0,
            result: None,
        })
    }

    /// The next column that needs data. An error from the world leaves this
    /// unchanged, so a caller may load the missing chunk and retry.
    pub fn requested_position(&self) -> IVec3 {
        if let Some(pos) = self.result {
            return pos;
        }
        IVec3::new(
            (self.origin.0 + self.dx) * 16 + i32::from(self.column / 16),
            self.fallback.y,
            (self.origin.1 + self.dz) * 16 + i32::from(self.column % 16),
        )
    }

    /// The eventual fallback may depend on the origin chunk's heightmap.
    pub fn fallback_position(&self) -> IVec3 {
        self.fallback
    }

    pub fn poll<W: SharedSpawnWorld>(
        &mut self,
        world: &mut W,
        budget: usize,
    ) -> Result<Option<IVec3>> {
        for _ in 0..budget {
            if self.result.is_some() {
                break;
            }
            let pos = self.requested_position();
            if let Some(found) = column_spawn(world, pos.x, pos.z)? {
                self.result = Some(found);
                break;
            }
            self.column += 1;
            if self.column == 256 {
                self.column = 0;
                self.chunks_visited += 1;
                if self.chunks_visited == 121 {
                    self.result = Some(self.fallback);
                    break;
                }
                if self.dx == self.dz
                    || (self.dx < 0 && self.dx == -self.dz)
                    || (self.dx > 0 && self.dx == 1 - self.dz)
                {
                    (self.step_x, self.step_z) = (-self.step_z, self.step_x);
                }
                self.dx += self.step_x;
                self.dz += self.step_z;
            }
        }
        Ok(self.result)
    }
}

/// `PlayerSpawnFinder.fixupSpawnHeight`, including its downward second pass.
pub fn fixup_spawn_height<W: SharedSpawnWorld>(world: &mut W, mut pos: IVec3) -> Result<IVec3> {
    let min_y = world.min_y();
    let max_y = world.max_y();
    ensure!(
        min_y < max_y && pos.y > i32::MIN && pos.y < i32::MAX,
        "invalid spawn height bounds"
    );
    while !world.clear_player(pos)? && pos.y < max_y {
        pos.y += 1;
    }
    pos.y -= 1;
    while world.clear_player(pos)? && pos.y > min_y {
        pos.y -= 1;
    }
    pos.y += 1;
    Ok(pos)
}

/// Search the world spawn without changing the world. Pass `None` for an
/// adventure world: Java skips the candidates and fixes the suggested height.
pub fn find_shared_spawn<W: SharedSpawnWorld>(
    world: &mut W,
    origin: IVec3,
    candidates: Option<WorldSpawnCandidates>,
) -> Result<IVec3> {
    if let Some(candidates) = candidates {
        ensure!(candidates.origin == origin, "spawn-search origin changed");
        for (x, z) in candidates {
            if let Some(pos) = column_spawn(world, x, z)?
                && world.clear_player(pos)?
            {
                return Ok(pos);
            }
        }
    }
    fixup_spawn_height(world, origin)
}

/// A bounded, resumable form of the same search. Each poll checks at most the
/// requested number of candidates or height positions, allowing the caller to
/// load the next chunk without blocking the render or simulation thread.
/// An error leaves the current candidate intact; an unknown earlier result
/// must never cause the search to advance to a later candidate.
pub struct ResumableWorldSpawn {
    origin: IVec3,
    remaining: Option<WorldSpawnCandidates>,
    current: Option<(i32, i32)>,
    fixing: Option<(IVec3, bool)>, // true: upward pass; false: downward pass
    result: Option<IVec3>,
}

impl ResumableWorldSpawn {
    pub fn new(origin: IVec3, mut candidates: Option<WorldSpawnCandidates>) -> Result<Self> {
        if let Some(candidates) = &candidates {
            ensure!(candidates.origin == origin, "spawn-search origin changed");
        }
        let current = candidates.as_mut().and_then(Iterator::next);
        let fixing = current.is_none().then_some((origin, true));
        Ok(Self {
            origin,
            remaining: candidates,
            current,
            fixing,
            result: None,
        })
    }

    /// The column whose data is needed next, including after a completed
    /// search while its result is waiting to be used.
    pub fn requested_position(&self) -> IVec3 {
        if let Some(result) = self.result {
            result
        } else if let Some((x, z)) = self.current {
            IVec3::new(x, self.origin.y, z)
        } else {
            self.fixing.map_or(self.origin, |(position, _)| position)
        }
    }

    pub fn result(&self) -> Option<IVec3> {
        self.result
    }

    pub fn poll<W: SharedSpawnWorld>(
        &mut self,
        world: &mut W,
        budget: usize,
    ) -> Result<Option<IVec3>> {
        for _ in 0..budget {
            if let Some(result) = self.result {
                return Ok(Some(result));
            }
            if let Some((x, z)) = self.current {
                if let Some(position) = column_spawn(world, x, z)?
                    && world.clear_player(position)?
                {
                    self.result = Some(position);
                    return Ok(Some(position));
                }
                self.current = self.remaining.as_mut().and_then(Iterator::next);
                if self.current.is_none() {
                    self.fixing = Some((self.origin, true));
                }
                continue;
            }
            let (mut position, upward) = self.fixing.expect("unfinished search has a phase");
            let min_y = world.min_y();
            let max_y = world.max_y();
            ensure!(
                min_y < max_y && position.y > i32::MIN && position.y < i32::MAX,
                "invalid spawn height bounds"
            );
            let clear = world.clear_player(position)?;
            if upward && !clear && position.y < max_y {
                position.y += 1;
                self.fixing = Some((position, true));
            } else if upward || (clear && position.y > min_y) {
                position.y -= 1;
                self.fixing = Some((position, false));
            } else {
                position.y += 1;
                self.result = Some(position);
                self.fixing = None;
                return Ok(Some(position));
            }
        }
        Ok(self.result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct InitialWorld {
        trace: Vec<(i32, i32)>,
        target: Option<(i32, i32)>,
        unavailable: Option<(i32, i32)>,
    }
    impl SharedSpawnWorld for InitialWorld {
        fn min_y(&self) -> i32 {
            -4
        }
        fn max_y(&self) -> i32 {
            16
        }
        fn ceiling_spawn_height(&self) -> Option<i32> {
            None
        }
        fn heights(&mut self, x: i32, z: i32) -> Result<SpawnColumnHeights> {
            self.trace.push((x, z));
            ensure!(self.unavailable != Some((x, z)), "column unavailable");
            let height = if self.target == Some((x, z)) { 0 } else { -5 };
            Ok(SpawnColumnHeights {
                motion_blocking: height,
                world_surface: height,
                ocean_floor: height,
            })
        }
        fn has_fluid(&mut self, _: IVec3) -> Result<bool> {
            Ok(false)
        }
        fn full_top_face(&mut self, pos: IVec3) -> Result<bool> {
            Ok(pos.y == 0)
        }
        fn clear_player(&mut self, _: IVec3) -> Result<bool> {
            panic!("initial spawn must not test player collisions")
        }
    }

    #[test]
    fn initial_spawn_visits_columns_in_spiral_and_resumes_on_missing_data() {
        let mut search = InitialWorldSpawn::new(-2, 3, 64).unwrap();
        let mut world = InitialWorld {
            trace: Vec::new(),
            target: Some((-15, 49)),
            unavailable: Some((-16, 49)),
        };
        assert_eq!(search.requested_position(), IVec3::new(-32, 64, 48));
        assert_eq!(search.poll(&mut world, 256).unwrap(), None);
        assert_eq!(search.requested_position(), IVec3::new(-16, 64, 48));
        assert_eq!(search.poll(&mut world, 1).unwrap(), None);
        assert!(search.poll(&mut world, 1).is_err());
        assert_eq!(search.requested_position(), IVec3::new(-16, 64, 49));
        world.unavailable = None;
        assert_eq!(
            search.poll(&mut world, 100).unwrap(),
            Some(IVec3::new(-15, 1, 49))
        );
        assert_eq!(world.trace[0], (-32, 48));
        assert_eq!(world.trace[1], (-32, 49));
        assert_eq!(world.trace[255], (-17, 63));
        assert_eq!(world.trace[256], (-16, 48));
    }

    #[test]
    fn initial_spawn_exhaustion_covers_121_chunks_and_uses_fallback() {
        let mut search = InitialWorldSpawn::new(2, -3, 71).unwrap();
        let mut world = InitialWorld {
            trace: Vec::new(),
            target: None,
            unavailable: None,
        };
        assert_eq!(
            search.poll(&mut world, 121 * 256).unwrap(),
            Some(IVec3::new(40, 71, -40))
        );
        let chunks: Vec<_> = world
            .trace
            .chunks_exact(256)
            .map(|c| (c[0].0.div_euclid(16), c[0].1.div_euclid(16)))
            .collect();
        assert_eq!(
            &chunks[..9],
            &[
                (2, -3),
                (3, -3),
                (3, -2),
                (2, -2),
                (1, -2),
                (1, -3),
                (1, -4),
                (2, -4),
                (3, -4)
            ]
        );
        assert_eq!(chunks.iter().copied().collect::<BTreeSet<_>>().len(), 121);
        assert!(
            chunks
                .iter()
                .all(|&(x, z)| (x - 2).abs() <= 5 && (z + 3).abs() <= 5)
        );
        assert!(InitialWorldSpawn::new(i32::MAX, 0, 0).is_err());
    }

    struct MockWorld {
        blocked: BTreeSet<i32>,
        queried: Vec<IVec3>,
    }
    impl SharedSpawnWorld for MockWorld {
        fn min_y(&self) -> i32 {
            -4
        }
        fn max_y(&self) -> i32 {
            8
        }
        fn ceiling_spawn_height(&self) -> Option<i32> {
            None
        }
        fn heights(&mut self, _: i32, _: i32) -> Result<SpawnColumnHeights> {
            Ok(SpawnColumnHeights {
                motion_blocking: 0,
                world_surface: 0,
                ocean_floor: 0,
            })
        }
        fn has_fluid(&mut self, _: IVec3) -> Result<bool> {
            Ok(false)
        }
        fn full_top_face(&mut self, pos: IVec3) -> Result<bool> {
            Ok(pos.y == 0)
        }
        fn clear_player(&mut self, pos: IVec3) -> Result<bool> {
            self.queried.push(pos);
            Ok(!self.blocked.contains(&pos.y))
        }
    }

    #[test]
    fn candidate_radius_border_offset_and_order_match_java_rules() {
        let origin = IVec3::new(10, 64, -10);
        let choices = WorldSpawnCandidates::new(origin, 1, 100.0, 3).unwrap();
        assert_eq!(choices.radius(), 1);
        assert_eq!(choices.candidate_count(), 9);
        assert_eq!(
            choices.collect::<Vec<_>>(),
            vec![
                (9, -10),
                (11, -11),
                (10, -11),
                (9, -11),
                (11, -9),
                (10, -9),
                (9, -9),
                (11, -10),
                (10, -10)
            ]
        );
        assert_eq!(
            WorldSpawnCandidates::new(origin, 0, 1.99, 0)
                .unwrap()
                .radius(),
            1
        );
        assert_eq!(
            WorldSpawnCandidates::new(origin, 100, 5.9, 0)
                .unwrap()
                .radius(),
            5
        );
        assert_eq!(
            WorldSpawnCandidates::new(origin, 100, 100.0, 0)
                .unwrap()
                .candidate_count(),
            1024
        );
        assert!(WorldSpawnCandidates::new(origin, 0, 100.0, 1).is_err());
        // Java's stride 17 repeats for the 17-by-17 case; preserve that order.
        let repeated = WorldSpawnCandidates::new(origin, 8, 100.0, 0).unwrap();
        assert_eq!(repeated.collect::<BTreeSet<_>>().len(), 17);
    }

    #[test]
    fn height_fixup_searches_up_then_down() {
        let mut world = MockWorld {
            blocked: [-1, 0, 1].into(),
            queried: vec![],
        };
        assert_eq!(
            fixup_spawn_height(&mut world, IVec3::ZERO).unwrap(),
            IVec3::new(0, 2, 0)
        );
        assert_eq!(
            world.queried.iter().map(|p| p.y).collect::<Vec<_>>(),
            vec![0, 1, 2, 1]
        );
        let mut world = MockWorld {
            blocked: [-1].into(),
            queried: vec![],
        };
        assert_eq!(
            fixup_spawn_height(&mut world, IVec3::new(0, 5, 0))
                .unwrap()
                .y,
            0
        );
    }

    #[test]
    fn candidate_search_uses_column_then_falls_back_to_suggested_height() {
        let mut world = MockWorld {
            blocked: [-1, 0].into(),
            queried: vec![],
        };
        let origin = IVec3::new(10, 5, 10);
        let candidates = WorldSpawnCandidates::new(origin, 0, 100.0, 0).unwrap();
        assert_eq!(
            find_shared_spawn(&mut world, origin, Some(candidates)).unwrap(),
            IVec3::new(10, 1, 10)
        );
        assert_eq!(
            find_shared_spawn(&mut world, origin, None).unwrap(),
            IVec3::new(10, 1, 10)
        );
        let mismatch = WorldSpawnCandidates::new(IVec3::ZERO, 0, 100.0, 0).unwrap();
        assert!(find_shared_spawn(&mut world, origin, Some(mismatch)).is_err());
    }

    #[test]
    fn resumable_search_matches_immediate_search_at_multiple_budgets() {
        let origin = IVec3::new(10, 5, 10);
        for mask in 0..64 {
            let blocked: BTreeSet<i32> = (-2..4).filter(|y| mask & (1 << (y + 2)) != 0).collect();
            for candidates in [
                None,
                Some(WorldSpawnCandidates::new(origin, 2, 100.0, 7).unwrap()),
            ] {
                let mut expected_world = MockWorld {
                    blocked: blocked.clone(),
                    queried: vec![],
                };
                let expected =
                    find_shared_spawn(&mut expected_world, origin, candidates.clone()).unwrap();
                for budget in [1, 2, 7] {
                    let mut world = MockWorld {
                        blocked: blocked.clone(),
                        queried: vec![],
                    };
                    let mut search = ResumableWorldSpawn::new(origin, candidates.clone()).unwrap();
                    let result = loop {
                        if let Some(result) = search.poll(&mut world, budget).unwrap() {
                            break result;
                        }
                    };
                    assert_eq!(result, expected, "mask {mask}, budget {budget}");
                    assert_eq!(world.queried, expected_world.queried);
                    assert_eq!(search.requested_position(), expected);
                }
            }
        }
    }

    #[test]
    fn resumable_search_retries_the_same_candidate_after_missing_data() {
        struct Delayed {
            world: MockWorld,
            missing: bool,
        }
        impl SharedSpawnWorld for Delayed {
            fn min_y(&self) -> i32 {
                self.world.min_y()
            }
            fn max_y(&self) -> i32 {
                self.world.max_y()
            }
            fn ceiling_spawn_height(&self) -> Option<i32> {
                None
            }
            fn heights(&mut self, x: i32, z: i32) -> Result<SpawnColumnHeights> {
                ensure!(!self.missing, "chunk not loaded");
                self.world.heights(x, z)
            }
            fn has_fluid(&mut self, pos: IVec3) -> Result<bool> {
                self.world.has_fluid(pos)
            }
            fn full_top_face(&mut self, pos: IVec3) -> Result<bool> {
                self.world.full_top_face(pos)
            }
            fn clear_player(&mut self, pos: IVec3) -> Result<bool> {
                self.world.clear_player(pos)
            }
        }
        let origin = IVec3::new(10, 5, 10);
        let candidates = WorldSpawnCandidates::new(origin, 2, 100.0, 7).unwrap();
        let first = candidates.clone().next().unwrap();
        let mut search = ResumableWorldSpawn::new(origin, Some(candidates)).unwrap();
        let mut world = Delayed {
            world: MockWorld {
                blocked: BTreeSet::new(),
                queried: vec![],
            },
            missing: true,
        };
        assert_eq!(
            search.requested_position(),
            IVec3::new(first.0, origin.y, first.1)
        );
        assert!(search.poll(&mut world, 8).is_err());
        assert_eq!(
            search.requested_position(),
            IVec3::new(first.0, origin.y, first.1)
        );
        world.missing = false;
        assert_eq!(
            search.poll(&mut world, 1).unwrap(),
            Some(IVec3::new(first.0, 1, first.1))
        );
    }
}
