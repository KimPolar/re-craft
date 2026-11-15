//! Random ticks for grass blocks and mycelium.
//!
//! The caller supplies the active registry, tags, light, and synchronous block
//! updates. Unknown inputs must not be treated as an empty or transparent block.

use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_core::random::LegacyRandom;
use native_world::{fluid::FluidState, loaded::BlockState};
use std::collections::BTreeMap;

/// The live world must observe each write before the next candidate is read.
pub trait SpreadingWorld {
    fn has_dirt(&mut self) -> Result<bool>;
    fn block(&mut self, pos: IVec3) -> Result<BlockState>;
    fn brightness(&mut self, pos: IVec3) -> Result<u8>;
    fn water_tagged(&mut self, state: &BlockState) -> Result<bool>;
    fn snowy(&mut self, state: &BlockState) -> Result<bool>;
    /// Neighbor callbacks can consume the same random source as this tick.
    fn set_block(&mut self, pos: IVec3, state: BlockState, random: &mut LegacyRandom)
    -> Result<()>;
}

/// Java first accepts one layer of snow, then rejects a full fluid, then
/// checks the light dampening between the soil and the block above.
pub fn can_stay_alive(soil: &BlockState, above: &BlockState) -> Option<bool> {
    if above.name == "minecraft:snow"
        && above.properties.len() == 1
        && above.properties.get("layers").map(String::as_str) == Some("1")
    {
        return Some(true);
    }
    let fluid = FluidState::from_block(above).ok()?;
    if fluid.is_some_and(|fluid| fluid.amount == 8) {
        // A waterlogged property on an unrecognized or malformed block does
        // not prove that Java would expose a full fluid state there.
        native_world::block_light::light_dampening_into(soil, above, [0, 1, 0])?;
        return Some(false);
    }
    Some(native_world::block_light::light_dampening_into(soil, above, [0, 1, 0])? < 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_fluid_requires_a_supported_block_state() {
        let soil = BlockState {
            name: "minecraft:grass_block".into(),
            properties: BTreeMap::from([("snowy".into(), "false".into())]),
        };
        let water = BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        };
        let malformed = BlockState {
            name: "minecraft:dirt".into(),
            properties: BTreeMap::from([("waterlogged".into(), "true".into())]),
        };
        assert_eq!(can_stay_alive(&soil, &water), Some(false));
        assert_eq!(can_stay_alive(&soil, &malformed), None);
    }
}

fn offset(pos: IVec3, x: i32, y: i32, z: i32) -> Result<IVec3> {
    Ok(IVec3::new(
        pos.x.checked_add(x).context("soil x coordinate overflow")?,
        pos.y.checked_add(y).context("soil y coordinate overflow")?,
        pos.z.checked_add(z).context("soil z coordinate overflow")?,
    ))
}

/// Execute the four Java candidate attempts in order, using the level RNG.
/// An error may follow earlier synchronous writes; callers must retain those
/// writes and refuse to treat the incomplete tick as a verified save.
pub fn random_tick(
    world: &mut impl SpreadingWorld,
    position: IVec3,
    source: &BlockState,
    random: &mut LegacyRandom,
) -> Result<()> {
    ensure!(
        matches!(
            source.name.as_str(),
            "minecraft:grass_block" | "minecraft:mycelium"
        ) && source.properties.len() == 1
            && matches!(
                source.properties.get("snowy").map(String::as_str),
                Some("true" | "false")
            ),
        "unsupported spreading soil state"
    );
    if !world.has_dirt()? {
        return Ok(());
    }
    let above = offset(position, 0, 1, 0)?;
    let cover = world.block(above)?;
    if !can_stay_alive(source, &cover)
        .with_context(|| format!("unsupported soil cover {} at {above:?}", cover.name))?
    {
        world.set_block(
            position,
            BlockState {
                name: "minecraft:dirt".into(),
                properties: BTreeMap::new(),
            },
            random,
        )?;
        return Ok(());
    }
    if world.brightness(above)? < 9 {
        return Ok(());
    }
    for _ in 0..4 {
        let x = random.next_int_bound(3).expect("positive bound") - 1;
        let y = random.next_int_bound(5).expect("positive bound") - 3;
        let z = random.next_int_bound(3).expect("positive bound") - 1;
        let target = offset(position, x, y, z)?;
        let current = world.block(target)?;
        if current.name != "minecraft:dirt" {
            continue;
        }
        ensure!(current.properties.is_empty(), "unsupported dirt state");
        let above_position = offset(target, 0, 1, 0)?;
        let above = world.block(above_position)?;
        if can_stay_alive(source, &above).with_context(|| {
            format!(
                "unsupported soil cover {} at {above_position:?}",
                above.name
            )
        })? && !world.water_tagged(&above)?
        {
            let snowy = world.snowy(&above)?;
            world.set_block(
                target,
                BlockState {
                    name: source.name.clone(),
                    properties: BTreeMap::from([("snowy".into(), snowy.to_string())]),
                },
                random,
            )?;
        }
    }
    Ok(())
}
