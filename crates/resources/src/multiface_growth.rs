//! Ordered 26.3 multiface feature placement and spreading.
use crate::packs::ResourceId;
use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Down,
    Up,
    North,
    South,
    West,
    East,
}
impl Face {
    pub fn offset(self, p: [i32; 3]) -> [i32; 3] {
        let d = match self {
            Self::Down => [0, -1, 0],
            Self::Up => [0, 1, 0],
            Self::North => [0, 0, -1],
            Self::South => [0, 0, 1],
            Self::West => [-1, 0, 0],
            Self::East => [1, 0, 0],
        };
        [
            p[0].wrapping_add(d[0]),
            p[1].wrapping_add(d[1]),
            p[2].wrapping_add(d[2]),
        ]
    }
    pub fn opposite(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }
    fn axis(self) -> u8 {
        match self {
            Self::Down | Self::Up => 0,
            Self::North | Self::South => 1,
            Self::West | Self::East => 2,
        }
    }
}
const ALL: [Face; 6] = [
    Face::Down,
    Face::Up,
    Face::North,
    Face::South,
    Face::West,
    Face::East,
];
fn shuffle<T>(values: &mut [T], random: &mut DecorationRandom) {
    for n in (2..=values.len()).rev() {
        values.swap(n - 1, random.next_int_bound(n as i32).unwrap() as usize);
    }
}

/// Registry and block-update effects supplied by the active generation world.
pub trait MultifaceWorld {
    type State: Clone;
    fn block(&mut self, pos: [i32; 3]) -> Result<Self::State>;
    fn air_or_water(&mut self, state: &Self::State) -> Result<bool>;
    fn is_growth(&mut self, state: &Self::State, block: &ResourceId) -> Result<bool>;
    fn in_block_set(&mut self, state: &Self::State, set: &BlockSet) -> Result<bool>;
    /// Performs the multiface block's face-support and existing-face checks.
    fn state_for_placement(
        &mut self,
        block: &ResourceId,
        existing: &Self::State,
        pos: [i32; 3],
        face: Face,
    ) -> Result<Option<Self::State>>;
    fn has_face(&mut self, state: &Self::State, face: Face) -> Result<bool>;
    /// The default spreader accepts air, the same growth block and source water.
    fn can_replace_for_spread(&mut self, state: &Self::State, block: &ResourceId) -> Result<bool>;
    /// Initial placement uses setBlockAndUpdate; spreading uses flags=2.
    fn set_growth(&mut self, pos: [i32; 3], state: &Self::State, initial: bool) -> Result<bool>;
    fn mark_postprocessing(&mut self, pos: [i32; 3]) -> Result<()>;
}
#[derive(Clone, Debug)]
pub enum BlockSet {
    Tag(ResourceId),
    Blocks(Vec<ResourceId>),
}
impl BlockSet {
    fn parse(value: &Value) -> Result<Self> {
        if let Some(s) = value.as_str() {
            return if let Some(s) = s.strip_prefix('#') {
                Ok(Self::Tag(ResourceId::parse(s)?))
            } else {
                Ok(Self::Blocks(vec![ResourceId::parse(s)?]))
            };
        }
        let entries = value.as_array().context("invalid multiface support set")?;
        ensure!(entries.len() <= 4096, "too many multiface support blocks");
        Ok(Self::Blocks(
            entries
                .iter()
                .map(|v| ResourceId::parse(v.as_str().context("invalid multiface support block")?))
                .collect::<Result<_>>()?,
        ))
    }
}
pub struct MultifaceGrowth {
    block: ResourceId,
    search_range: i32,
    floor: bool,
    ceiling: bool,
    wall: bool,
    spread: f32,
    support: BlockSet,
}
impl MultifaceGrowth {
    pub fn decode(value: &Value) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing multiface type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "multiface_growth",
            "wrong multiface type"
        );
        let block = ResourceId::parse(
            value
                .get("block")
                .and_then(Value::as_str)
                .context("missing multiface block")?,
        )?;
        let search_range = match value.get("search_range") {
            Some(v) => i32::try_from(v.as_i64().context("invalid multiface search range")?)?,
            None => 10,
        };
        ensure!(
            (1..=64).contains(&search_range),
            "invalid multiface search range"
        );
        let flag = |key: &str| -> Result<bool> {
            match value.get(key) {
                Some(v) => v.as_bool().with_context(|| format!("invalid {key}")),
                None => Ok(false),
            }
        };
        let spread = match value.get("chance_of_spreading") {
            Some(v) => v.as_f64().context("invalid spread chance")? as f32,
            None => 0.5,
        };
        ensure!(
            spread.is_finite() && (0.0..=1.0).contains(&spread),
            "invalid spread chance"
        );
        Ok(Self {
            block,
            search_range,
            floor: flag("can_place_on_floor")?,
            ceiling: flag("can_place_on_ceiling")?,
            wall: flag("can_place_on_wall")?,
            spread,
            support: BlockSet::parse(
                value
                    .get("can_be_placed_on")
                    .context("missing multiface support")?,
            )?,
        })
    }
    pub fn block(&self) -> &ResourceId {
        &self.block
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        match &self.support {
            BlockSet::Tag(id) => BTreeSet::from([id.clone()]),
            _ => BTreeSet::new(),
        }
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut out = BTreeSet::from([self.block.clone()]);
        if let BlockSet::Blocks(v) = &self.support {
            out.extend(v.iter().cloned());
        }
        out
    }
    fn directions(&self) -> ([Face; 6], usize) {
        let mut v = [Face::Down; 6];
        let mut n = 0;
        if self.ceiling {
            v[n] = Face::Up;
            n += 1;
        }
        if self.floor {
            v[n] = Face::Down;
            n += 1;
        }
        if self.wall {
            v[n..n + 4].copy_from_slice(&[Face::North, Face::East, Face::South, Face::West]);
            n += 4;
        }
        (v, n)
    }
    fn place_at<W: MultifaceWorld>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        pos: [i32; 3],
        state: &W::State,
        dirs: &[Face],
    ) -> Result<bool> {
        for &face in dirs {
            let support = world.block(face.offset(pos))?;
            if world.in_block_set(&support, &self.support)? {
                let Some(growth) = world.state_for_placement(&self.block, state, pos, face)? else {
                    return Ok(false);
                };
                world.set_growth(pos, &growth, true)?;
                world.mark_postprocessing(pos)?;
                if random.next_float() < self.spread {
                    self.spread(world, random, pos, &growth, face)?;
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn spread<W: MultifaceWorld>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        pos: [i32; 3],
        source: &W::State,
        face: Face,
    ) -> Result<()> {
        let mut directions = ALL;
        shuffle(&mut directions, random);
        for direction in directions {
            if direction.axis() == face.axis()
                || !world.has_face(source, face)?
                || world.has_face(source, direction)?
            {
                continue;
            }
            let candidates = [
                (pos, direction),
                (direction.offset(pos), face),
                (face.offset(direction.offset(pos)), direction.opposite()),
            ];
            for (target, target_face) in candidates {
                let existing = world.block(target)?;
                if !world.can_replace_for_spread(&existing, &self.block)? {
                    continue;
                }
                if world
                    .state_for_placement(&self.block, &existing, target, target_face)?
                    .is_none()
                {
                    continue;
                }
                // Java resolves the placement twice: candidate predicate, then write.
                let existing = world.block(target)?;
                if let Some(state) =
                    world.state_for_placement(&self.block, &existing, target, target_face)?
                {
                    world.mark_postprocessing(target)?;
                    world.set_growth(target, &state, false)?;
                }
                return Ok(());
            }
        }
        Ok(())
    }
    pub fn place<W: MultifaceWorld>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        let state = world.block(origin)?;
        if !world.air_or_water(&state)? {
            return Ok(false);
        }
        let (mut dirs, count) = self.directions();
        shuffle(&mut dirs[..count], random);
        let state = world.block(origin)?;
        if self.place_at(world, random, origin, &state, &dirs[..count])? {
            return Ok(true);
        }
        for &direction in &dirs[..count] {
            let (valid, valid_count) = self.directions();
            let mut choices = [Face::Down; 6];
            let mut choice_count = 0;
            for &face in &valid[..valid_count] {
                if face != direction.opposite() {
                    choices[choice_count] = face;
                    choice_count += 1;
                }
            }
            shuffle(&mut choices[..choice_count], random);
            for _ in 0..self.search_range {
                // Java 26.3 resets the mutable position to origin + direction
                // each iteration, rather than moving it one step farther.
                let pos = direction.offset(origin);
                let state = world.block(pos)?;
                if !world.air_or_water(&state)? && !world.is_growth(&state, &self.block)? {
                    break;
                }
                if self.place_at(world, random, pos, &state, &choices[..choice_count])? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
