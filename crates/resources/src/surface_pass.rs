//! Apply material rules to a noise-stage chunk. Features, carvers, structures,
//! biome generation are separate.
use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    noise_stage::{NoiseMaterial, NoiseStageChunk},
    packs::{ResourceId, ResourceProvider},
    surface_rules::{SeededSurfaceInputs, SurfacePosition, SurfaceRandom, SurfaceRule},
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

fn state_id(state: &Value) -> Result<ResourceId> {
    let name = state
        .as_str()
        .or_else(|| state.get("id").and_then(Value::as_str))
        .context("block state has no identifier")?;
    ResourceId::parse(name)
}

/// Extend a single column after the seeded pillar height has been computed.
fn extend_badlands_column(
    states: &mut [Arc<Value>],
    kinds: &mut [SurfaceBlockKind],
    updates: &mut [bool],
    min_y: i32,
    start: i32,
    peak: i32,
    default_block: &Arc<Value>,
    default_kind: SurfaceBlockKind,
) -> Result<()> {
    if peak < start || peak < min_y {
        return Ok(());
    }
    let default_id = state_id(default_block)?;
    let water = ResourceId::parse("minecraft:water")?;
    // Above the chunk, the column is air and writes are discarded.
    let limit = ((i64::from(peak) - i64::from(min_y)) as usize).min(states.len() - 1);
    for state in states[..=limit].iter().rev() {
        let id = state_id(state)?;
        if id == default_id {
            break;
        }
        if id == water {
            return Ok(());
        }
    }
    for i in (0..=limit).rev() {
        if kinds[i] != SurfaceBlockKind::Air {
            break;
        }
        states[i] = Arc::clone(default_block);
        kinds[i] = default_kind;
        if default_kind == SurfaceBlockKind::Fluid {
            updates[i] = true;
        }
    }
    Ok(())
}

/// Apply the frozen-ocean extension, including random draws outside the chunk.
fn extend_iceberg_column(
    states: &mut [Arc<Value>],
    kinds: &mut [SurfaceBlockKind],
    updates: &mut [bool],
    min_y: i32,
    start: i32,
    min_surface: i32,
    sea: i32,
    top: f64,
    bottom: f64,
    random: &mut SurfaceRandom,
    snow: &(Arc<Value>, SurfaceBlockKind),
    ice: &(Arc<Value>, SurfaceBlockKind),
) -> Result<()> {
    let snow_limit = 2 + random.next_int_bound(4);
    let snow_height = sea.wrapping_add(18).wrapping_add(random.next_int_bound(10));
    let mut snow_count = 0;
    let top = top as i32;
    let bottom = bottom as i32;
    let first = start.max(top.wrapping_add(1));
    if first < min_surface {
        return Ok(());
    }
    ensure!(
        i64::from(first) - i64::from(min_surface) <= 1_048_576,
        "iceberg vertical range is too large"
    );
    let water = ResourceId::parse("minecraft:water")?;
    for y in (min_surface..=first).rev() {
        let index = usize::try_from(i64::from(y) - i64::from(min_y))
            .ok()
            .filter(|&i| i < states.len());
        let is_air = index.is_none_or(|i| kinds[i] == SurfaceBlockKind::Air);
        let placed = if is_air && y < top {
            random.next_double() > 0.01
        } else if let Some(i) = index {
            state_id(&states[i])? == water && y > bottom && y < sea && random.next_double() > 0.15
        } else {
            false
        };
        if placed {
            let choice = if snow_count <= snow_limit && y > snow_height {
                snow_count += 1;
                snow
            } else {
                ice
            };
            if let Some(i) = index {
                states[i] = Arc::clone(&choice.0);
                kinds[i] = choice.1;
                if choice.1 == SurfaceBlockKind::Fluid {
                    updates[i] = true;
                }
            }
        }
    }
    Ok(())
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
    let mut fluid_updates = chunk.fluid_updates().to_vec();
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
            if top_biome.namespace == "minecraft" && top_biome.path == "eroded_badlands" {
                if let Some(peak) = inputs.badlands_peak(world_x, world_z)? {
                    let range = column * h..(column + 1) * h;
                    let default_kind = if let Some(kind) = cache.get(default_block.as_ref()) {
                        *kind
                    } else {
                        let kind = classify(&default_block)?;
                        cache.insert((*default_block).clone(), kind);
                        kind
                    };
                    extend_badlands_column(
                        &mut states[range.clone()],
                        &mut kinds[range.clone()],
                        &mut fluid_updates[range.clone()],
                        min_y,
                        start,
                        peak,
                        &default_block,
                        default_kind,
                    )?;
                    heights[column] = kinds[range]
                        .iter()
                        .rposition(|kind| *kind != SurfaceBlockKind::Air)
                        .map_or(min_y - 1, |index| min_y + index as i32);
                }
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
                    if kind == SurfaceBlockKind::Fluid {
                        fluid_updates[index] = true;
                    }
                }
                Ok(())
            })?;
            if top_biome.namespace == "minecraft"
                && matches!(
                    top_biome.path.as_str(),
                    "frozen_ocean" | "deep_frozen_ocean"
                )
            {
                if let Some((top, bottom)) =
                    inputs.iceberg_bounds(&top_biome, world_x, world_z, sea_level)?
                {
                    let mut material = |name: &str| -> Result<(Arc<Value>, SurfaceBlockKind)> {
                        let value = Arc::new(json!(name));
                        let kind = if let Some(kind) = cache.get(value.as_ref()) {
                            *kind
                        } else {
                            let kind = classify(&value)?;
                            cache.insert((*value).clone(), kind);
                            kind
                        };
                        Ok((value, kind))
                    };
                    let snow = material("minecraft:snow_block")?;
                    let ice = material("minecraft:packed_ice")?;
                    let mut random = inputs.iceberg_random(world_x, world_z);
                    extend_iceberg_column(
                        &mut states[range.clone()],
                        &mut kinds[range.clone()],
                        &mut fluid_updates[range.clone()],
                        min_y,
                        start,
                        min_surface_level,
                        sea_level,
                        top,
                        bottom,
                        &mut random,
                        &snow,
                        &ice,
                    )?;
                }
            }
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
        fluid_updates,
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
    fn java_bin() -> PathBuf {
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin")
    }
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
        let java = java_bin();
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

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn badlands_pillars_match_vanilla_columns() {
        use crate::{
            density::WorldgenRandom,
            packs::ResourceStack,
            surface_rules::{BadlandsPillars, SurfaceNoise},
        };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut stack = ResourceStack::open(&game).unwrap();
        let pillars = BadlandsPillars::default();
        let mut definitions = serde_json::Map::new();
        for id in pillars.noise_dependencies() {
            let name = format!("{}:{}", id.namespace, id.path);
            let path = format!("data/{}/worldgen/noise/{}.json", id.namespace, id.path);
            definitions.insert(
                name,
                serde_json::from_slice(&stack.read_builtin_data(&path).unwrap().unwrap()).unwrap(),
            );
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("BadlandsExtensionOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/BadlandsExtensionOracle.java"),
        )
        .unwrap();
        let cp = classpath(&game, temp.path());
        let java = java_bin();
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let input = temp.path().join("noise.json");
        std::fs::write(&input, serde_json::to_vec(&definitions).unwrap()).unwrap();
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("BadlandsExtensionOracle")
            .arg(&input));
        let mut noise = None;
        let mut samples = 0;
        let mut changed = [0; 3];
        for line in output.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            match fields.first().copied() {
                Some("S") => {
                    let algorithm = if fields[1] == "0" {
                        WorldgenRandom::Legacy
                    } else {
                        WorldgenRandom::Xoroshiro
                    };
                    let seed = fields[2].parse().unwrap();
                    noise = Some(
                        SurfaceNoise::load(
                            &mut stack,
                            seed,
                            algorithm,
                            pillars.noise_dependencies(),
                        )
                        .unwrap(),
                    );
                }
                Some("P") => {
                    let n: i32 = fields[1].parse().unwrap();
                    let x: i32 = fields[2].parse().unwrap();
                    let z: i32 = fields[3].parse().unwrap();
                    let variant: i32 = fields[4].parse().unwrap();
                    let top = 48 + n % 14;
                    let air = Arc::new(json!("minecraft:air"));
                    let stone = Arc::new(json!("minecraft:stone"));
                    let water = Arc::new(json!("minecraft:water"));
                    let dirt = Arc::new(json!("minecraft:dirt"));
                    let mut states: Vec<_> = (0..128)
                        .map(|i| Arc::clone(if i - 32 <= top { &stone } else { &air }))
                        .collect();
                    let mut kinds: Vec<_> = (0..128)
                        .map(|i| {
                            if i - 32 <= top {
                                SurfaceBlockKind::Solid
                            } else {
                                SurfaceBlockKind::Air
                            }
                        })
                        .collect();
                    if variant == 1 {
                        states[(top + 33) as usize] = water;
                        kinds[(top + 33) as usize] = SurfaceBlockKind::Fluid;
                    }
                    if variant == 2 {
                        states[(top + 38) as usize] = dirt;
                        kinds[(top + 38) as usize] = SurfaceBlockKind::Solid;
                    }
                    let before = states.clone();
                    let mut updates = vec![false; 128];
                    if let Some(peak) = pillars.peak(noise.as_mut().unwrap(), x, z).unwrap() {
                        let start = top
                            + if variant == 1 {
                                2
                            } else if variant == 2 {
                                7
                            } else {
                                1
                            };
                        extend_badlands_column(
                            &mut states,
                            &mut kinds,
                            &mut updates,
                            -32,
                            start,
                            peak,
                            &stone,
                            SurfaceBlockKind::Solid,
                        )
                        .unwrap();
                    }
                    let actual: String = states
                        .iter()
                        .map(|state| match state.as_str().unwrap() {
                            "minecraft:air" => 'a',
                            "minecraft:stone" => 's',
                            "minecraft:water" => 'w',
                            _ => 'd',
                        })
                        .collect();
                    assert_eq!(actual, fields[5], "badlands column {line}");
                    changed[variant as usize] += usize::from(states != before);
                    assert!(updates.iter().all(|flag| !flag));
                    samples += 1;
                }
                _ => {}
            }
        }
        assert_eq!(samples, 2 * 3 * 300 * 3);
        assert!(
            changed[0] > 0 && changed[2] > 0,
            "fixtures must exercise actual pillars: {changed:?}"
        );
        assert_eq!(changed[1], 0, "water must prevent pillar generation");
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn frozen_ocean_icebergs_match_vanilla_columns() {
        use crate::{
            biome_climate::{BiomeClimate, TemperatureModifier},
            density::WorldgenRandom,
            packs::ResourceStack,
            surface_rules::{FrozenOceanIcebergs, SurfaceNoise, SurfaceRandomFactory},
        };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut stack = ResourceStack::open(&game).unwrap();
        let icebergs = FrozenOceanIcebergs::default();
        let mut definitions = serde_json::Map::new();
        for id in icebergs.noise_dependencies() {
            let name = format!("{}:{}", id.namespace, id.path);
            let path = format!("data/{}/worldgen/noise/{}.json", id.namespace, id.path);
            definitions.insert(
                name,
                serde_json::from_slice(&stack.read_builtin_data(&path).unwrap().unwrap()).unwrap(),
            );
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("FrozenOceanExtensionOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/FrozenOceanExtensionOracle.java"),
        )
        .unwrap();
        let cp = classpath(&game, temp.path());
        let java = java_bin();
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let input = temp.path().join("noise.json");
        std::fs::write(&input, serde_json::to_vec(&definitions).unwrap()).unwrap();
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("FrozenOceanExtensionOracle")
            .arg(&input));
        let mut noise = None;
        let mut random = None;
        let mut samples = 0;
        let mut changed = 0;
        let mut snow_count = 0;
        let mut ice_count = 0;
        for line in output.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            match fields.first().copied() {
                Some("S") => {
                    let algorithm = if fields[1] == "0" {
                        WorldgenRandom::Legacy
                    } else {
                        WorldgenRandom::Xoroshiro
                    };
                    let seed = fields[2].parse().unwrap();
                    noise = Some(
                        SurfaceNoise::load(
                            &mut stack,
                            seed,
                            algorithm,
                            icebergs.noise_dependencies(),
                        )
                        .unwrap(),
                    );
                    random = Some(SurfaceRandomFactory::new(seed, algorithm));
                }
                Some("P") => {
                    let n: i32 = fields[1].parse().unwrap();
                    let x: i32 = fields[2].parse().unwrap();
                    let z: i32 = fields[3].parse().unwrap();
                    let variant: i32 = fields[4].parse().unwrap();
                    let climate = BiomeClimate {
                        has_precipitation: false,
                        temperature: match n % 3 {
                            0 => 0.0,
                            1 => 0.8,
                            _ => 0.1,
                        },
                        modifier: if n & 1 != 0 {
                            TemperatureModifier::Frozen
                        } else {
                            TemperatureModifier::None
                        },
                    };
                    let melts = climate.temperature_at(x, 63, z, 63) > 0.1;
                    assert_eq!(melts, fields[5] == "true", "iceberg melt condition {line}");
                    let air = Arc::new(json!("minecraft:air"));
                    let stone = Arc::new(json!("minecraft:stone"));
                    let water = Arc::new(json!("minecraft:water"));
                    let dirt = Arc::new(json!("minecraft:dirt"));
                    let snow = (
                        Arc::new(json!("minecraft:snow_block")),
                        SurfaceBlockKind::Solid,
                    );
                    let ice = (
                        Arc::new(json!("minecraft:packed_ice")),
                        SurfaceBlockKind::Solid,
                    );
                    let floor = 45 + n % 4;
                    let mut states: Vec<_> = (0..160)
                        .map(|i| {
                            let y = i - 32;
                            Arc::clone(if y <= floor {
                                &stone
                            } else if y < 63 {
                                &water
                            } else {
                                &air
                            })
                        })
                        .collect();
                    let mut kinds: Vec<_> = (0..160)
                        .map(|i| {
                            let y = i - 32;
                            if y <= floor {
                                SurfaceBlockKind::Solid
                            } else if y < 63 {
                                SurfaceBlockKind::Fluid
                            } else {
                                SurfaceBlockKind::Air
                            }
                        })
                        .collect();
                    if variant == 1 {
                        states[107] = dirt;
                        kinds[107] = SurfaceBlockKind::Solid;
                    }
                    if variant == 2 {
                        for i in [87, 92] {
                            states[i] = Arc::clone(&air);
                            kinds[i] = SurfaceBlockKind::Air;
                        }
                    }
                    let before = states.clone();
                    let mut updates = vec![false; 160];
                    if let Some((top, bottom)) = icebergs
                        .bounds(noise.as_mut().unwrap(), x, z, 63, melts)
                        .unwrap()
                    {
                        let mut random = random.as_ref().unwrap().root_at(x, 0, z);
                        extend_iceberg_column(
                            &mut states,
                            &mut kinds,
                            &mut updates,
                            -32,
                            if variant == 1 { 76 } else { 63 },
                            28 + n % 7,
                            63,
                            top,
                            bottom,
                            &mut random,
                            &snow,
                            &ice,
                        )
                        .unwrap();
                    }
                    let actual: String = states
                        .iter()
                        .map(|state| match state.as_str().unwrap() {
                            "minecraft:air" => 'a',
                            "minecraft:stone" => 's',
                            "minecraft:water" => 'w',
                            "minecraft:dirt" => 'd',
                            "minecraft:snow_block" => 'n',
                            _ => 'i',
                        })
                        .collect();
                    assert_eq!(actual, fields[6], "frozen ocean column {line}");
                    changed += usize::from(states != before);
                    snow_count += actual.matches('n').count();
                    ice_count += actual.matches('i').count();
                    assert!(updates.iter().all(|flag| !flag));
                    samples += 1;
                }
                _ => {}
            }
        }
        assert_eq!(samples, 2 * 3 * 1000 * 3);
        assert!(
            changed > 0 && snow_count > 0 && ice_count > 0,
            "fixtures must exercise ice and snow: {changed}, {snow_count}, {ice_count}"
        );
    }
}
