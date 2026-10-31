//! Apply material rules to a noise-stage chunk. Features, carvers, structures,
//! biome generation and special badlands/iceberg extensions are separate.
use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use crate::{
    noise_stage::{NoiseMaterial, NoiseStageChunk},
    packs::{ResourceId, ResourceProvider},
    surface_rules::{SeededSurfaceInputs, SurfacePosition, SurfaceRule},
};

/// Classification must use the active block registry, including fluid states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceBlockKind {
    Air,
    Fluid,
    Solid,
}

/// Intermediate result, still requiring the remaining generation stages.
pub struct SurfaceStageChunk {
    pub chunk_position: [i32; 2],
    pub min_y: i32,
    pub height: i32,
    /// Block state encodings in Z, X, Y order.
    pub states: Vec<Arc<Value>>,
    pub fluid_updates: Vec<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ColumnPosition {
    y: i32,
    above: i32,
    below: i32,
    water: i32,
}

/// Visits solid blocks from top to bottom with Java's depth and fluid bookkeeping.
fn scan_column(
    kinds: &[SurfaceBlockKind],
    min_y: i32,
    mut visit: impl FnMut(ColumnPosition) -> Result<()>,
) -> Result<()> {
    let mut above = 0i32;
    let mut water = i32::MIN;
    let mut bottom = i32::MAX;
    for index in (0..kinds.len()).rev() {
        let y = min_y + index as i32;
        match kinds[index] {
            SurfaceBlockKind::Air => {
                above = 0;
                water = i32::MIN;
            }
            SurfaceBlockKind::Fluid => {
                if water == i32::MIN {
                    water = y + 1;
                }
            }
            SurfaceBlockKind::Solid => {
                if bottom >= y {
                    let mut next = index;
                    while next > 0 && kinds[next - 1] == SurfaceBlockKind::Solid {
                        next -= 1;
                    }
                    bottom = min_y + next as i32;
                }
                above += 1;
                visit(ColumnPosition {
                    y,
                    above,
                    below: y - bottom + 1,
                    water,
                })?;
            }
        }
    }
    Ok(())
}

/// Apply a rule to noise-stage blocks using already sampled biome identities.
/// `preliminary` is the chunk surface-level density in Z, X order; its volume
/// sampling must match the noise generator. Unknown block classifications fail.
/// Eroded badlands and frozen oceans need extra vanilla terrain extensions and
/// are rejected until those extensions are implemented.
pub fn apply_surface<P: ResourceProvider>(
    chunk: &NoiseStageChunk,
    rule: &SurfaceRule,
    inputs: &mut SeededSurfaceInputs<'_, P>,
    preliminary: &[f32],
    sea_level: i32,
    mut classify: impl FnMut(&Value) -> Result<SurfaceBlockKind>,
    mut biome_at: impl FnMut(i32, i32, i32) -> Result<ResourceId>,
) -> Result<SurfaceStageChunk> {
    let [chunk_x, chunk_z] = chunk.chunk_position();
    let origin_x = chunk_x.checked_mul(16).context("chunk X overflow")?;
    let origin_z = chunk_z.checked_mul(16).context("chunk Z overflow")?;
    let min_y = chunk.min_y();
    let height = chunk.height();
    ensure!(
        height > 0 && preliminary.len() == 256,
        "invalid surface volume"
    );
    let top_y = min_y
        .checked_add(height - 1)
        .context("surface height overflow")?;
    let default_block = Arc::new(chunk.default_block().clone());
    let default_fluid = Arc::new(chunk.default_fluid().clone());
    let air = Arc::new(json!("minecraft:air"));
    let lava = Arc::new(json!("minecraft:lava"));
    let mut cache = HashMap::<Value, SurfaceBlockKind>::new();
    let mut kinds = Vec::with_capacity(chunk.materials().len());
    let mut states = Vec::with_capacity(chunk.materials().len());
    for material in chunk.materials() {
        let state = match material {
            NoiseMaterial::DefaultBlock => &default_block,
            NoiseMaterial::DefaultFluid => &default_fluid,
            NoiseMaterial::Air => &air,
            NoiseMaterial::Lava => &lava,
        };
        let kind = if let Some(kind) = cache.get(state.as_ref()) {
            *kind
        } else {
            let kind = classify(state)?;
            cache.insert((**state).clone(), kind);
            kind
        };
        states.push(Arc::clone(state));
        kinds.push(kind);
    }
    let h = height as usize;
    ensure!(states.len() == 256 * h, "invalid noise chunk buffer");
    let mut heights = [min_y - 1; 256];
    for (column, col) in kinds.chunks_exact(h).enumerate() {
        if let Some(index) = col.iter().rposition(|kind| *kind != SurfaceBlockKind::Air) {
            heights[column] = min_y + index as i32;
        }
    }
    // Java visits X before Z and computes each gradient from the current heightmap.
    let mut original_kinds = Vec::with_capacity(h);
    for x in 0usize..16 {
        for z in 0usize..16 {
            let column = z * 16 + x;
            let world_x = origin_x.checked_add(x as i32).context("block X overflow")?;
            let world_z = origin_z.checked_add(z as i32).context("block Z overflow")?;
            let start = heights[column] + 1;
            let top_biome = biome_at(world_x, start, world_z)?;
            if top_biome.namespace == "minecraft"
                && matches!(
                    top_biome.path.as_str(),
                    "eroded_badlands" | "frozen_ocean" | "deep_frozen_ocean"
                )
            {
                bail!(
                    "surface extension for {}:{} is not implemented",
                    top_biome.namespace,
                    top_biome.path
                );
            }
            let gradient_x =
                heights[z * 16 + (x + 1).min(15)] - heights[z * 16 + x.saturating_sub(1)];
            let gradient_z =
                heights[(z + 1).min(15) * 16 + x] - heights[z.saturating_sub(1) * 16 + x];
            let surface_depth = inputs.surface_depth(world_x, world_z)?;
            let secondary = inputs.surface_secondary(world_x, world_z)?;
            let value = preliminary[column];
            // Mth.floor(float) uses a saturating float-to-int cast followed by a wrapping decrement.
            let integral = value as i32;
            let floor = if value < integral as f32 {
                integral.wrapping_sub(1)
            } else {
                integral
            };
            let min_surface_level = floor.wrapping_add(surface_depth).wrapping_sub(8);
            let range = column * h..(column + 1) * h;
            original_kinds.clear();
            original_kinds.extend_from_slice(&kinds[range.clone()]);
            scan_column(&original_kinds, min_y, |step| {
                inputs.set_biome(biome_at(world_x, step.y, world_z)?)?;
                let p = SurfacePosition {
                    x: world_x,
                    y: step.y,
                    z: world_z,
                    min_y,
                    height,
                    sea_level,
                    stone_above: step.above,
                    stone_below: step.below,
                    surface_depth,
                    surface_secondary: secondary,
                    water_height: step.water,
                    min_surface_level,
                    gradient_x,
                    gradient_z,
                };
                if let Some(state) = rule.evaluate(&p, inputs)? {
                    let kind = if let Some(kind) = cache.get(state.as_ref()) {
                        *kind
                    } else {
                        let kind = classify(&state)?;
                        cache.insert((*state).clone(), kind);
                        kind
                    };
                    let index = column * h + (step.y - min_y) as usize;
                    states[index] = state;
                    kinds[index] = kind;
                }
                Ok(())
            })?;
            heights[column] = kinds[range]
                .iter()
                .rposition(|kind| *kind != SurfaceBlockKind::Air)
                .map_or(min_y - 1, |index| min_y + index as i32);
            debug_assert!(heights[column] <= top_y);
        }
    }
    Ok(SurfaceStageChunk {
        chunk_position: [chunk_x, chunk_z],
        min_y,
        height,
        states,
        fluid_updates: chunk.fluid_updates().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsString,
        path::{Path, PathBuf},
        process::Command,
    };
    #[test]
    fn column_depths_and_water_reset_across_air_and_caves() {
        use SurfaceBlockKind::{Air as A, Fluid as F, Solid as S};
        let mut trace = Vec::new();
        scan_column(&[S, S, A, S, F, F, A, S, S, S], -4, |p| {
            trace.push(p);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            trace,
            vec![
                ColumnPosition {
                    y: 5,
                    above: 1,
                    below: 3,
                    water: i32::MIN
                },
                ColumnPosition {
                    y: 4,
                    above: 2,
                    below: 2,
                    water: i32::MIN
                },
                ColumnPosition {
                    y: 3,
                    above: 3,
                    below: 1,
                    water: i32::MIN
                },
                ColumnPosition {
                    y: -1,
                    above: 1,
                    below: 1,
                    water: 2
                },
                ColumnPosition {
                    y: -3,
                    above: 1,
                    below: 2,
                    water: i32::MIN
                },
                ColumnPosition {
                    y: -4,
                    above: 2,
                    below: 1,
                    water: i32::MIN
                },
            ]
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn column_scan_matches_vanilla_material_system() {
        fn run(command: &mut Command) -> String {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }
        fn classpath(game: &Path, classes: &Path) -> OsString {
            let version = game.join("versions/26.3");
            let manifest: Value =
                serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
            assert_eq!(manifest["id"], "26.3");
            let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
            for library in manifest["libraries"].as_array().unwrap() {
                if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                    let path = game.join("libraries").join(path);
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
            std::env::join_paths(paths).unwrap()
        }
        fn kind(x: i32, y: i32, z: i32) -> SurfaceBlockKind {
            let top = 8 + (x * 3 + z * 7) % 20;
            if y > top {
                if y < 15 {
                    SurfaceBlockKind::Fluid
                } else {
                    SurfaceBlockKind::Air
                }
            } else if y > 1 && (y + x * 5 + z * 3) % 11 == 0 {
                SurfaceBlockKind::Air
            } else if y > 1 && (y + x * 7 + z * 5) % 13 == 0 {
                SurfaceBlockKind::Fluid
            } else {
                SurfaceBlockKind::Solid
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("SurfaceColumnOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/SurfaceColumnOracle.java"),
        )
        .unwrap();
        let cp = classpath(&game, temp.path());
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SurfaceColumnOracle"));
        let mut actual = Vec::new();
        for x in 0..16 {
            for z in 0..16 {
                let kinds: Vec<_> = (0..32).map(|y| kind(x, y, z)).collect();
                let height = |x, z| {
                    (0..32)
                        .rev()
                        .find(|y| kind(x, *y, z) != SurfaceBlockKind::Air)
                        .unwrap_or(-1)
                };
                let gx = height((x + 1).min(15), z) - height((x - 1).max(0), z);
                let gz = height(x, (z + 1).min(15)) - height(x, (z - 1).max(0));
                scan_column(&kinds, 0, |p| {
                    actual.push(format!(
                        "P {x} {} {z} {} {} {} {gx} {gz}",
                        p.y, p.above, p.below, p.water
                    ));
                    Ok(())
                })
                .unwrap();
            }
        }
        let expected: Vec<_> = output
            .lines()
            .filter(|line| line.starts_with("P "))
            .collect();
        assert!(expected.len() > 4000);
        assert_eq!(
            actual.iter().map(String::as_str).collect::<Vec<_>>(),
            expected
        );
    }
}
