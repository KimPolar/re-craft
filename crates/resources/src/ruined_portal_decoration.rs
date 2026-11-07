//! Java 26.3 ruined-portal terrain decoration after template placement.
use anyhow::{Context, Result, ensure};
use native_core::{beardifier::BoundingBox, random::DecorationRandom};

use crate::{ruined_portal::VerticalPlacement, structure_start::Heightmap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortalBlock {
    Netherrack,
    Magma,
    PersistentJungleLeaves,
    /// Direction of the supporting face of the vine.
    Vine([i32; 3]),
}

/// Implement against an unpublished world draft; discard it on any error.
pub trait PortalDecorationWorld {
    type State;
    fn block(&mut self, position: [i32; 3]) -> Result<Self::State>;
    fn id<'a>(&self, state: &'a Self::State) -> &'a str;
    fn is_air(&self, state: &Self::State) -> bool;
    /// Resolve the active `minecraft:features_cannot_replace` block tag.
    fn is_protected(&self, state: &Self::State) -> Result<bool>;
    fn height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32>;
    fn collision_face_full(&mut self, position: [i32; 3], direction: [i32; 3]) -> Result<bool>;
    /// Match `setBlockAndUpdate`, including observable neighbor updates.
    fn set_block(&mut self, position: [i32; 3], block: PortalBlock) -> Result<()>;
}

#[derive(Clone, Copy, Debug)]
pub struct PortalDecoration {
    pub bounds: BoundingBox,
    pub placement: VerticalPlacement,
    pub cold: bool,
    pub overgrown: bool,
    pub vines: bool,
}

impl PortalDecoration {
    /// Run after template placement. Random commits only on success; a caller
    /// must discard its world draft on failure.
    pub fn apply(
        &self,
        world: &mut impl PortalDecorationWorld,
        random: &mut DecorationRandom,
    ) -> Result<()> {
        let bounds = self.bounds;
        ensure!(
            (0..3).all(|i| bounds.min[i] <= bounds.max[i]),
            "invalid portal bounds"
        );
        let span_x = bounds.max[0]
            .checked_sub(bounds.min[0])
            .and_then(|v| v.checked_add(1))
            .context("portal X span overflow")?;
        let span_z = bounds.max[2]
            .checked_sub(bounds.min[2])
            .and_then(|v| v.checked_add(1))
            .context("portal Z span overflow")?;
        let center_x = bounds.min[0]
            .checked_add(span_x / 2)
            .context("portal center overflow")?;
        let center_z = bounds.min[2]
            .checked_add(span_z / 2)
            .context("portal center overflow")?;
        let min_x = center_x.checked_sub(14).context("portal spread overflow")?;
        let max_x = center_x.checked_add(14).context("portal spread overflow")?;
        let min_z = center_z.checked_sub(14).context("portal spread overflow")?;
        let max_z = center_z.checked_add(14).context("portal spread overflow")?;
        bounds.min[1]
            .checked_sub(12)
            .context("portal drip overflow")?;
        bounds.max[1]
            .checked_add(1)
            .context("portal vine overflow")?;
        let mut next = random.clone();
        let combined_span = span_x.checked_add(span_z).context("portal span overflow")?;
        let offset = next
            .next_int_bound((8 - combined_span / 4).max(1))
            .context("invalid portal spread")?;
        const CHANCE: [f32; 14] = [
            1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.9, 0.9, 0.8, 0.7, 0.6, 0.4, 0.2,
        ];
        let surface_placement = matches!(
            self.placement,
            VerticalPlacement::OnLandSurface | VerticalPlacement::OnOceanFloor
        );
        let heightmap = if self.placement == VerticalPlacement::OnOceanFloor {
            Heightmap::OceanFloor
        } else {
            Heightmap::WorldSurface
        };
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                let distance = (x - center_x).abs() + (z - center_z).abs() + offset;
                if distance >= CHANCE.len() as i32
                    || next.next_double() >= f64::from(CHANCE[distance as usize])
                {
                    continue;
                }
                let surface = world
                    .height(x, z, heightmap)?
                    .checked_sub(1)
                    .context("portal surface overflow")?;
                let y = if surface_placement {
                    surface
                } else {
                    bounds.min[1].min(surface)
                };
                if (i64::from(y) - i64::from(bounds.min[1])).abs() > 3 {
                    continue;
                }
                let pos = [x, y, z];
                if !self.replaceable(world, pos)? {
                    continue;
                }
                self.place_netherrack(world, &mut next, pos)?;
                if self.overgrown {
                    self.leaves(world, &mut next, pos)?;
                }
                self.drip(world, &mut next, [x, y - 1, z])?;
            }
        }
        for x in bounds.min[0].saturating_add(1)..bounds.max[0] {
            for z in bounds.min[2].saturating_add(1)..bounds.max[2] {
                let pos = [x, bounds.min[1], z];
                let state = world.block(pos)?;
                if world.id(&state) == "minecraft:netherrack" {
                    self.drip(world, &mut next, [x, pos[1] - 1, z])?;
                }
            }
        }
        if self.vines || self.overgrown {
            // BlockPos.betweenClosed iterates X first, then Y, then Z.
            for z in bounds.min[2]..=bounds.max[2] {
                for y in bounds.min[1]..=bounds.max[1] {
                    for x in bounds.min[0]..=bounds.max[0] {
                        let pos = [x, y, z];
                        if self.vines {
                            self.vine(world, &mut next, pos)?;
                        }
                        if self.overgrown {
                            self.leaves(world, &mut next, pos)?;
                        }
                    }
                }
            }
        }
        *random = next;
        Ok(())
    }

    fn replaceable(&self, world: &mut impl PortalDecorationWorld, pos: [i32; 3]) -> Result<bool> {
        let state = world.block(pos)?;
        let id = world.id(&state);
        if id == "minecraft:air" || id == "minecraft:obsidian" {
            return Ok(false);
        }
        if world.is_protected(&state)? {
            return Ok(false);
        }
        Ok(self.placement == VerticalPlacement::InNether || id != "minecraft:lava")
    }

    fn place_netherrack(
        &self,
        world: &mut impl PortalDecorationWorld,
        random: &mut DecorationRandom,
        pos: [i32; 3],
    ) -> Result<()> {
        if self.replaceable(world, pos)? {
            let block = if !self.cold && random.next_float() < 0.07 {
                PortalBlock::Magma
            } else {
                PortalBlock::Netherrack
            };
            world.set_block(pos, block)?;
        }
        Ok(())
    }

    fn drip(
        &self,
        world: &mut impl PortalDecorationWorld,
        random: &mut DecorationRandom,
        mut pos: [i32; 3],
    ) -> Result<()> {
        self.place_netherrack(world, random, pos)?;
        for _ in 0..8 {
            if random.next_float() >= 0.5 {
                break;
            }
            pos[1] = pos[1].checked_sub(1).context("portal drip overflow")?;
            self.place_netherrack(world, random, pos)?;
        }
        Ok(())
    }

    fn leaves(
        &self,
        world: &mut impl PortalDecorationWorld,
        random: &mut DecorationRandom,
        pos: [i32; 3],
    ) -> Result<()> {
        if random.next_float() >= 0.5 {
            return Ok(());
        }
        let state = world.block(pos)?;
        if world.id(&state) != "minecraft:netherrack" {
            return Ok(());
        }
        let above = [
            pos[0],
            pos[1].checked_add(1).context("portal leaf overflow")?,
            pos[2],
        ];
        let state = world.block(above)?;
        if world.is_air(&state) {
            world.set_block(above, PortalBlock::PersistentJungleLeaves)?;
        }
        Ok(())
    }

    fn vine(
        &self,
        world: &mut impl PortalDecorationWorld,
        random: &mut DecorationRandom,
        pos: [i32; 3],
    ) -> Result<()> {
        let state = world.block(pos)?;
        if world.is_air(&state) || world.id(&state) == "minecraft:vine" {
            return Ok(());
        }
        let dirs = [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]];
        let direction = dirs[random.next_int_bound(4).unwrap() as usize];
        let neighbor = std::array::from_fn(|i| pos[i].wrapping_add(direction[i]));
        let state = world.block(neighbor)?;
        if world.is_air(&state) && world.collision_face_full(pos, direction)? {
            world.set_block(neighbor, PortalBlock::Vine(direction.map(|n| -n)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MissingHeight;
    impl PortalDecorationWorld for MissingHeight {
        type State = ();
        fn block(&mut self, _: [i32; 3]) -> Result<()> {
            unreachable!()
        }
        fn id<'a>(&self, _: &'a ()) -> &'a str {
            unreachable!()
        }
        fn is_air(&self, _: &()) -> bool {
            unreachable!()
        }
        fn is_protected(&self, _: &()) -> Result<bool> {
            unreachable!()
        }
        fn height(&mut self, _: i32, _: i32, _: Heightmap) -> Result<i32> {
            anyhow::bail!("height unavailable")
        }
        fn collision_face_full(&mut self, _: [i32; 3], _: [i32; 3]) -> Result<bool> {
            unreachable!()
        }
        fn set_block(&mut self, _: [i32; 3], _: PortalBlock) -> Result<()> {
            unreachable!()
        }
    }

    #[test]
    fn failure_does_not_commit_random() {
        let decoration = PortalDecoration {
            bounds: BoundingBox {
                min: [0, 64, 0],
                max: [6, 68, 6],
            },
            placement: VerticalPlacement::OnLandSurface,
            cold: false,
            overgrown: false,
            vines: false,
        };
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        assert!(decoration.apply(&mut MissingHeight, &mut random).is_err());
        assert_eq!(random.next_long(), expected.next_long());
    }
}
