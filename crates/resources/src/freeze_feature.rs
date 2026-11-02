//! The 26.3 `freeze_top_layer` feature.
use crate::biome_climate::{BiomeClimate, Precipitation};
use anyhow::Result;

/// Supplies the exact generation context; unavailable light, biome or block
/// information must return an error rather than guessing.
pub trait FreezeWorld {
    type State;
    fn motion_height(&mut self, x: i32, z: i32) -> Result<i32>;
    fn biome(&mut self, pos: [i32; 3]) -> Result<BiomeClimate>;
    fn sea_level(&self) -> Result<i32>;
    fn inside_build_height(&self, y: i32) -> bool;
    fn block_light(&mut self, pos: [i32; 3]) -> Result<u8>;
    fn block(&mut self, pos: [i32; 3]) -> Result<Self::State>;
    fn freezable_water(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
    fn air_or_snow(&mut self, state: &Self::State) -> Result<bool>;
    fn snow_survives(&mut self, pos: [i32; 3]) -> Result<bool>;
    fn put_ice(&mut self, pos: [i32; 3]) -> Result<()>;
    fn put_snow(&mut self, pos: [i32; 3]) -> Result<()>;
    /// Apply the `snowy` property only when present on the existing state.
    fn mark_snowy(&mut self, pos: [i32; 3]) -> Result<()>;
}

/// Run on the 16x16 area starting at the supplied feature origin.
pub fn freeze_top_layer(world: &mut impl FreezeWorld, origin: [i32; 3]) -> Result<bool> {
    for dx in 0..16 {
        for dz in 0..16 {
            let x = origin[0].wrapping_add(dx);
            let z = origin[2].wrapping_add(dz);
            let y = world.motion_height(x, z)?;
            let above = [x, y, z];
            let below = [x, y.wrapping_sub(1), z];
            let biome = world.biome(above)?;
            let sea = world.sea_level()?;
            if biome.cold_enough_to_snow(x, below[1], z, sea)
                && world.inside_build_height(below[1])
                && world.block_light(below)? < 10
            {
                let state = world.block(below)?;
                if world.freezable_water(below, &state)? {
                    world.put_ice(below)?;
                }
            }
            if biome.precipitation_at(x, y, z, sea) == Precipitation::Snow
                && world.inside_build_height(y)
                && world.block_light(above)? < 10
            {
                let state = world.block(above)?;
                if world.air_or_snow(&state)? && world.snow_survives(above)? {
                    world.put_snow(above)?;
                    world.mark_snowy(below)?;
                }
            }
        }
    }
    Ok(true)
}
