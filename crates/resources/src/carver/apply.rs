//! Apply an already computed carving mask to an intermediate surface chunk.
//! The caller supplies the active block tag, aquifer and surface-rule context.
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::CarvingMask;
use crate::{packs::ResourceId, surface_pass::SurfaceStageChunk};

/// A block and its actual fluid state, resolved with the active block registry.
pub struct CarvedState {
    pub state: Arc<Value>,
    pub has_fluid: bool,
}
/// Aquifers may decline a replacement; fluid updates are only scheduled when
/// both this flag and the replacement's fluid state are nonempty.
pub struct AquiferReplacement {
    pub block: CarvedState,
    pub schedule_fluid_update: bool,
}

pub trait CarvingInputs {
    /// Old-chunk blending's carving filter, or true for fresh terrain.
    fn may_carve(&mut self, local_x: u8, y: i32, local_z: u8) -> Result<bool>;
    fn uncarvable(&mut self, state: &Value) -> Result<bool>;
    fn aquifer(&mut self, x: i32, y: i32, z: i32) -> Result<Option<AquiferReplacement>>;
    /// Evaluate the material rule for dirt just below an exposed grass block.
    /// The chunk already contains the newly carved block, as in Java.
    fn top_material(
        &mut self,
        chunk: &SurfaceStageChunk,
        x: i32,
        y: i32,
        z: i32,
        under_fluid: bool,
    ) -> Result<Option<CarvedState>>;
}

fn is_block(state: &Value, name: &str) -> Result<bool> {
    let id = state
        .as_str()
        .or_else(|| state.get("id").and_then(Value::as_str))
        .context("block state has no identifier")?;
    let id = ResourceId::parse(id)?;
    Ok(id.namespace == "minecraft" && id.path == name)
}

/// Mutates only the supplied intermediate chunk. A failure leaves it partially
/// modified, so callers must discard it instead of saving or displaying it.
pub fn apply_mask(
    chunk: &mut SurfaceStageChunk,
    mask: &CarvingMask,
    inputs: &mut impl CarvingInputs,
) -> Result<()> {
    let height = usize::try_from(chunk.height).context("invalid chunk height")?;
    ensure!(
        height > 0
            && chunk.states.len() == height * 256
            && chunk.fluid_updates.len() == height * 256,
        "invalid surface chunk buffers"
    );
    let end = chunk
        .min_y
        .checked_add(chunk.height)
        .context("chunk height overflow")?;
    ensure!(
        mask.min_y() >= chunk.min_y && mask.max_y() < end,
        "carving mask outside chunk"
    );
    let origin_x = chunk.chunk_position[0]
        .checked_mul(16)
        .context("chunk X overflow")?;
    let origin_z = chunk.chunk_position[1]
        .checked_mul(16)
        .context("chunk Z overflow")?;
    origin_x.checked_add(15).context("block X overflow")?;
    origin_z.checked_add(15).context("block Z overflow")?;
    for (x, z, bottom, top) in mask.segments() {
        let mut has_grass = false;
        let world_x = origin_x + i32::from(x);
        let world_z = origin_z + i32::from(z);
        let column = (usize::from(z) * 16 + usize::from(x)) * height;
        for y in (bottom..=top).rev() {
            if !inputs.may_carve(x, y, z)? {
                continue;
            }
            let index = column + (y - chunk.min_y) as usize;
            let state = &chunk.states[index];
            if inputs.uncarvable(state)? {
                continue;
            }
            if is_block(state, "grass_block")? || is_block(state, "mycelium")? {
                has_grass = true;
            }
            let Some(replacement) = inputs.aquifer(world_x, y, world_z)? else {
                continue;
            };
            let under_fluid = replacement.block.has_fluid;
            chunk.states[index] = replacement.block.state;
            if replacement.schedule_fluid_update && under_fluid {
                chunk.fluid_updates[index] = true;
            }
            if has_grass && y > chunk.min_y && is_block(&chunk.states[index - 1], "dirt")? {
                if let Some(top) =
                    inputs.top_material(chunk, world_x, y - 1, world_z, under_fluid)?
                {
                    chunk.states[index - 1] = top.state;
                    if top.has_fluid {
                        chunk.fluid_updates[index - 1] = true;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn state(name: &str, fluid: bool) -> CarvedState {
        CarvedState {
            state: Arc::new(json!(name)),
            has_fluid: fluid,
        }
    }
    struct Inputs {
        calls: Vec<(i32, bool)>,
    }
    impl CarvingInputs for Inputs {
        fn may_carve(&mut self, _: u8, y: i32, _: u8) -> Result<bool> {
            Ok(y != 5)
        }
        fn uncarvable(&mut self, s: &Value) -> Result<bool> {
            is_block(s, "bedrock")
        }
        fn aquifer(&mut self, _: i32, y: i32, _: i32) -> Result<Option<AquiferReplacement>> {
            if y == 4 {
                return Ok(None);
            }
            let fluid = y == 7;
            Ok(Some(AquiferReplacement {
                block: state(
                    if fluid {
                        "minecraft:water"
                    } else {
                        "minecraft:cave_air"
                    },
                    fluid,
                ),
                schedule_fluid_update: true,
            }))
        }
        fn top_material(
            &mut self,
            _: &SurfaceStageChunk,
            _: i32,
            y: i32,
            _: i32,
            under_fluid: bool,
        ) -> Result<Option<CarvedState>> {
            self.calls.push((y, under_fluid));
            Ok(Some(state(
                if under_fluid {
                    "minecraft:water"
                } else {
                    "minecraft:grass_block"
                },
                under_fluid,
            )))
        }
    }
    #[test]
    fn segments_protection_fluid_and_grass_repair_follow_java_order() {
        let height = 16usize;
        let mut chunk = SurfaceStageChunk {
            chunk_position: [0, 0],
            min_y: 0,
            height: height as i32,
            states: vec![Arc::new(json!("minecraft:stone")); 256 * height],
            fluid_updates: vec![false; 256 * height],
        };
        for (y, name) in [
            (2, "minecraft:dirt"),
            (3, "minecraft:stone"),
            (4, "minecraft:stone"),
            (5, "minecraft:stone"),
            (6, "minecraft:dirt"),
            (7, "minecraft:grass_block"),
            (8, "minecraft:bedrock"),
            (10, "minecraft:dirt"),
            (11, "minecraft:stone"),
        ] {
            chunk.states[y] = Arc::new(json!(name));
        }
        let mut mask = CarvingMask::new(1, 14).unwrap();
        for y in [3, 4, 5, 7, 8, 11] {
            mask.carve(0, y, 0);
        }
        let mut inputs = Inputs { calls: Vec::new() };
        apply_mask(&mut chunk, &mask, &mut inputs).unwrap();
        assert_eq!(inputs.calls, vec![(6, true)]);
        assert_eq!(chunk.states[8].as_str(), Some("minecraft:bedrock"));
        assert_eq!(chunk.states[7].as_str(), Some("minecraft:water"));
        assert_eq!(chunk.states[6].as_str(), Some("minecraft:water"));
        assert_eq!(chunk.states[5].as_str(), Some("minecraft:stone"));
        assert_eq!(chunk.states[4].as_str(), Some("minecraft:stone"));
        assert_eq!(chunk.states[3].as_str(), Some("minecraft:cave_air"));
        assert_eq!(chunk.states[2].as_str(), Some("minecraft:dirt"));
        assert_eq!(chunk.states[10].as_str(), Some("minecraft:dirt"));
        assert!(chunk.fluid_updates[7] && chunk.fluid_updates[6]);
        assert!(!chunk.fluid_updates[3]);
    }
}
