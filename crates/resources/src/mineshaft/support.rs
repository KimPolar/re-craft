//! Vertical supports used at the ends of mineshaft corridors.
use anyhow::{Result, ensure};
use native_core::beardifier::BoundingBox;

use super::MineshaftVariant;
use crate::structure_piece::StructureFrame;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MineshaftSupportBlock {
    Wood,
    Fence,
    IronChain,
}

/// Each direct write uses WorldGenLevel.setBlock(..., 2). The block-state
/// queries must reflect prior writes and preserve block-entity side effects.
pub trait MineshaftSupportWorld {
    type State;
    fn vertical_bounds(&self) -> (i32, i32);
    fn block_state(&mut self, position: [i32; 3]) -> Result<Self::State>;
    /// Air, liquid blocks, glow lichen, seagrass and tall seagrass.
    fn replaceable_by_structure(&mut self, state: &Self::State) -> Result<bool>;
    fn is_lava(&mut self, state: &Self::State) -> Result<bool>;
    fn supports_column(&mut self, position: [i32; 3], state: &Self::State) -> Result<bool>;
    /// Match Block.canSupportCenter(DOWN), followed by the falling-block check.
    fn supports_hanging_chain(&mut self, position: [i32; 3], state: &Self::State) -> Result<bool>;
    fn set_support(
        &mut self,
        position: [i32; 3],
        block: MineshaftSupportBlock,
        variant: MineshaftVariant,
    ) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MineshaftSupportOutcome {
    OutsideClip,
    NoAnchor,
    Grounded,
    Hanging,
}

/// Scan alternately below and above one local corridor position. Only the
/// initial position is clipped; Java may read and write beyond that clip.
/// Discard the containing generation draft on callback failure.
pub fn place_vertical_support(
    frame: StructureFrame,
    local: [i32; 3],
    variant: MineshaftVariant,
    clip: BoundingBox,
    world: &mut impl MineshaftSupportWorld,
) -> Result<MineshaftSupportOutcome> {
    ensure!(
        (0..3).all(|i| clip.min[i] <= clip.max[i]),
        "invalid support clip"
    );
    let base = frame.world_position(local);
    if !(0..3).all(|i| (clip.min[i]..=clip.max[i]).contains(&base[i])) {
        return Ok(MineshaftSupportOutcome::OutsideClip);
    }
    let (min_y, max_y) = world.vertical_bounds();
    ensure!(min_y <= max_y, "invalid support world bounds");
    let mut below = true;
    let mut above = true;
    let mut distance = 1i32;
    while below || above {
        if below {
            let y = base[1].wrapping_sub(distance);
            let pos = [base[0], y, base[2]];
            let state = world.block_state(pos)?;
            let empty = world.replaceable_by_structure(&state)? && !world.is_lava(&state)?;
            if !empty && world.supports_column(pos, &state)? {
                for y in y.wrapping_add(1)..base[1] {
                    world.set_support(
                        [base[0], y, base[2]],
                        MineshaftSupportBlock::Wood,
                        variant,
                    )?;
                }
                return Ok(MineshaftSupportOutcome::Grounded);
            }
            below = distance <= 20 && empty && y > min_y.wrapping_add(1);
        }
        if above {
            let y = base[1].wrapping_add(distance);
            let pos = [base[0], y, base[2]];
            let state = world.block_state(pos)?;
            let empty = world.replaceable_by_structure(&state)?;
            if !empty && world.supports_hanging_chain(pos, &state)? {
                world.set_support(
                    [base[0], base[1].wrapping_add(1), base[2]],
                    MineshaftSupportBlock::Fence,
                    variant,
                )?;
                for y in base[1].wrapping_add(2)..y {
                    world.set_support(
                        [base[0], y, base[2]],
                        MineshaftSupportBlock::IronChain,
                        variant,
                    )?;
                }
                return Ok(MineshaftSupportOutcome::Hanging);
            }
            above = distance <= 50 && empty && y < max_y;
        }
        distance += 1;
    }
    Ok(MineshaftSupportOutcome::NoAnchor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure_start::HorizontalDirection;

    #[derive(Clone, Copy)]
    enum State {
        Air,
        Water,
        Stone,
    }
    #[derive(Default)]
    struct World {
        reads: Vec<[i32; 3]>,
        writes: Vec<([i32; 3], MineshaftSupportBlock)>,
    }
    impl MineshaftSupportWorld for World {
        type State = State;
        fn vertical_bounds(&self) -> (i32, i32) {
            (-64, 100)
        }
        fn block_state(&mut self, p: [i32; 3]) -> Result<State> {
            self.reads.push(p);
            Ok(if p[1] < 45 {
                State::Water
            } else if p[1] == 96 {
                State::Stone
            } else {
                State::Air
            })
        }
        fn replaceable_by_structure(&mut self, s: &State) -> Result<bool> {
            Ok(matches!(s, State::Air | State::Water))
        }
        fn is_lava(&mut self, _: &State) -> Result<bool> {
            Ok(false)
        }
        fn supports_column(&mut self, _: [i32; 3], s: &State) -> Result<bool> {
            Ok(matches!(s, State::Stone))
        }
        fn supports_hanging_chain(&mut self, _: [i32; 3], s: &State) -> Result<bool> {
            Ok(matches!(s, State::Stone))
        }
        fn set_support(
            &mut self,
            p: [i32; 3],
            block: MineshaftSupportBlock,
            _: MineshaftVariant,
        ) -> Result<()> {
            self.writes.push((p, block));
            Ok(())
        }
    }

    #[test]
    fn water_allows_scan_to_the_last_upper_anchor() {
        let frame = StructureFrame {
            bounds: BoundingBox::new([0, 46, 0], [2, 48, 9]).unwrap(),
            orientation: Some(HorizontalDirection::South),
        };
        let clip = BoundingBox::new([0, 45, 0], [2, 45, 9]).unwrap();
        let mut world = World::default();
        assert_eq!(
            place_vertical_support(
                frame,
                [0, -1, 2],
                MineshaftVariant::Normal,
                clip,
                &mut world
            )
            .unwrap(),
            MineshaftSupportOutcome::Hanging
        );
        assert_eq!(world.reads.len(), 72);
        assert_eq!(world.writes.len(), 50);
        assert_eq!(world.writes[0], ([0, 46, 2], MineshaftSupportBlock::Fence));
        assert_eq!(
            world.writes[49],
            ([0, 95, 2], MineshaftSupportBlock::IronChain)
        );
    }
}
