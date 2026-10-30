//! Direct-sky source heights for loaded Java 26.3 columns.
//!
//! This tracks the vertical source boundary only. Diffuse sky-light updates
//! are a separate step and are not performed by this module.
use std::collections::BTreeMap;

use crate::{
    block_light::{
        LightBounds, LightProperties, faces_occlude, light_properties, transparent_light_properties,
    },
    loaded::{BlockPosition, BlockStateId, World},
};

/// Java represents an unobstructed source below the world with this sentinel.
pub const UNBOUNDED_SKY_SOURCE: i32 = i32::MIN;

/// The lowest Y at which this column receives unobstructed direct sky light.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkyColumn {
    x: i32,
    z: i32,
    bounds: LightBounds,
    lowest: i32,
}

impl SkyColumn {
    /// Computes the source from loaded block data. Unknown blocks or sections
    /// return `None` rather than assuming they transmit sky light.
    pub fn load(world: &World, bounds: LightBounds, x: i32, z: i32) -> Option<Self> {
        let lowest = scan(world, bounds, x, z)?;
        Some(Self {
            x,
            z,
            bounds,
            lowest,
        })
    }

    pub fn lowest_source_y(self) -> i32 {
        self.lowest
    }

    /// Update after the block at `y` changes. A change below an existing
    /// obstruction cannot change the source and does not scan the column.
    /// If required data is missing, the previous value is retained.
    pub fn update(&mut self, world: &World, y: i32) -> Option<bool> {
        let range = self.bounds.build_range();
        if !range.contains(&y) {
            return None;
        }
        let previous = if self.lowest == UNBOUNDED_SKY_SOURCE {
            range.start.checked_sub(1)?
        } else {
            self.lowest
        };
        if y.checked_add(1)? < previous {
            return Some(false);
        }
        let mut reader = ColumnReader::new(world, self.bounds, self.x, self.z)?;
        let upper = reader.get(y.checked_add(1)?)?;
        let current = reader.get(y)?;
        if let Some(next) = reader.update_edge(y.checked_add(1)?, previous, upper, current)? {
            self.lowest = next;
            return Some(true);
        }
        let lower = reader.get(y.checked_sub(1)?)?;
        if let Some(next) = reader.update_edge(y, previous, current, lower)? {
            self.lowest = next;
            return Some(true);
        }
        Some(false)
    }
}

struct ColumnReader<'a> {
    world: &'a World,
    range: std::ops::Range<i32>,
    x: i32,
    z: i32,
    cache: BTreeMap<BlockStateId, usize>,
    behaviors: Vec<LightProperties>,
}

impl<'a> ColumnReader<'a> {
    fn new(world: &'a World, bounds: LightBounds, x: i32, z: i32) -> Option<Self> {
        let range = bounds.build_range();
        let chunk = world.chunks.get(
            &BlockPosition {
                x,
                y: range.start,
                z,
            }
            .chunk(),
        )?;
        if chunk.status.as_deref() != Some("minecraft:full") {
            return None;
        }
        Some(Self {
            world,
            range,
            x,
            z,
            cache: BTreeMap::new(),
            behaviors: vec![transparent_light_properties()],
        })
    }

    fn get(&mut self, y: i32) -> Option<usize> {
        if !self.range.contains(&y) {
            return Some(0);
        }
        let id = self.world.block_id(self.x, y, self.z)?;
        if let Some(&index) = self.cache.get(&id) {
            return Some(index);
        }
        let index = self.behaviors.len();
        self.behaviors
            .push(light_properties(self.world.state(id)?)?);
        self.cache.insert(id, index);
        Some(index)
    }

    fn edge_occluded(&self, above: usize, below: usize) -> bool {
        let above = &self.behaviors[above];
        let below = &self.behaviors[below];
        below.opacity > 0
            || ((above.has_faces || below.has_faces)
                && faces_occlude(&above.faces[3], &below.faces[2]))
    }

    fn update_edge(
        &mut self,
        upper_y: i32,
        previous: i32,
        above: usize,
        below: usize,
    ) -> Option<Option<i32>> {
        if self.edge_occluded(above, below) {
            return Some((upper_y > previous).then_some(upper_y));
        }
        if upper_y != previous {
            return Some(None);
        }
        let mut current = below;
        let mut y = upper_y.checked_sub(2)?;
        let last = self.range.start.checked_sub(1)?;
        while y >= last {
            let next = self.get(y)?;
            if self.edge_occluded(current, next) {
                return Some(Some(y.checked_add(1)?));
            }
            current = next;
            y = y.checked_sub(1)?;
        }
        Some(Some(UNBOUNDED_SKY_SOURCE))
    }
}

fn scan(world: &World, bounds: LightBounds, x: i32, z: i32) -> Option<i32> {
    let mut reader = ColumnReader::new(world, bounds, x, z)?;
    let mut above = 0;
    for y in reader.range.clone().rev() {
        let below = reader.get(y)?;
        if reader.edge_occluded(above, below) {
            return y.checked_add(1);
        }
        above = below;
    }
    Some(UNBOUNDED_SKY_SOURCE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, Section};
    use std::sync::Arc;

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        }
    }
    fn fixture() -> World {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air", &[]));
        let position = ChunkPos { x: 0, z: 0 };
        let sections = (0..=1)
            .map(|y| {
                (
                    y,
                    Arc::new(Section {
                        blocks: Some(Box::new([air; BLOCK_COUNT])),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )
            })
            .collect();
        world.chunks.insert(
            position,
            Arc::new(Chunk {
                position,
                status: Some("minecraft:full".into()),
                block_light_correct: true,
                sections,
            }),
        );
        world
    }
    fn put(world: &mut World, y: i32, state: BlockState) {
        let id = world.intern_state(state);
        world.set_block_id(15, y, 15, id).unwrap();
    }

    #[test]
    fn source_tracks_attenuation_partial_faces_and_unavailable_states() {
        let mut world = fixture();
        let mut sky = SkyColumn::load(&world, LightBounds::new(0, 32).unwrap(), 15, 15).unwrap();
        assert_eq!(sky.lowest_source_y(), UNBOUNDED_SKY_SOURCE);
        put(&mut world, 20, state("water", &[("level", "0")]));
        assert_eq!(sky.update(&world, 20), Some(true));
        assert_eq!(sky.lowest_source_y(), 21);
        put(&mut world, 10, state("stone", &[]));
        assert_eq!(sky.update(&world, 10), Some(false));
        put(&mut world, 20, state("air", &[]));
        assert_eq!(sky.update(&world, 20), Some(true));
        assert_eq!(sky.lowest_source_y(), 11);
        put(
            &mut world,
            25,
            state(
                "stone_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ),
        );
        assert_eq!(sky.update(&world, 25), Some(true));
        assert_eq!(sky.lowest_source_y(), 25);
        put(
            &mut world,
            25,
            state("stone_slab", &[("type", "top"), ("waterlogged", "false")]),
        );
        assert_eq!(sky.update(&world, 25), Some(true));
        assert_eq!(sky.lowest_source_y(), 26);
        put(&mut world, 15, state("unrecognized", &[]));
        assert_eq!(sky.update(&world, 15), Some(false));
        put(&mut world, 25, state("air", &[]));
        assert_eq!(sky.update(&world, 25), None);
        assert_eq!(
            sky.lowest_source_y(),
            26,
            "failed scans retain the last known source"
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn changes_match_java_chunk_sky_sources() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SkyColumnOracle.java");
        std::fs::write(&source, include_str!("../tests/java/SkyColumnOracle.java")).unwrap();
        let run = |cmd: &mut Command| {
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let kinds = [
            ("air", state("air", &[])),
            ("stone", state("stone", &[])),
            ("water", state("water", &[("level", "0")])),
            ("glass", state("glass", &[])),
            ("torch", state("torch", &[])),
            ("mushroom", state("brown_mushroom", &[])),
            (
                "slab_bottom",
                state(
                    "stone_slab",
                    &[("type", "bottom"), ("waterlogged", "false")],
                ),
            ),
            (
                "slab_top",
                state("stone_slab", &[("type", "top"), ("waterlogged", "false")]),
            ),
            (
                "slab_double",
                state(
                    "stone_slab",
                    &[("type", "double"), ("waterlogged", "false")],
                ),
            ),
            ("snow_1", state("snow", &[("layers", "1")])),
            ("snow_7", state("snow", &[("layers", "7")])),
            ("snow_8", state("snow", &[("layers", "8")])),
            ("farmland", state("farmland", &[("moisture", "0")])),
            ("dirt_path", state("dirt_path", &[])),
            (
                "stair",
                state(
                    "oak_stairs",
                    &[
                        ("facing", "north"),
                        ("half", "bottom"),
                        ("shape", "straight"),
                        ("waterlogged", "false"),
                    ],
                ),
            ),
            (
                "leaves",
                state(
                    "oak_leaves",
                    &[
                        ("distance", "7"),
                        ("persistent", "false"),
                        ("waterlogged", "false"),
                    ],
                ),
            ),
        ];
        let mut actions = vec![
            (0, 6),
            (0, 0),
            (0, 7),
            (0, 0),
            (31, 1),
            (31, 0),
            (20, 2),
            (10, 1),
            (20, 0),
            (25, 6),
            (25, 7),
            (25, 0),
            (31, 10),
            (31, 11),
            (31, 0),
        ];
        let mut random = 0x9e37_79b9_u32;
        for _ in 0..1024 {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let y = (random >> 16) as i32 % 32;
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            actions.push((y, (random as usize >> 16) % kinds.len()));
        }
        let entries: Vec<_> = actions
            .iter()
            .map(|(y, kind)| serde_json::json!({"y":y,"state":kinds[*kind].0}))
            .collect();
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SkyColumnOracle")
            .arg(&input)
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        assert_eq!(rows.len(), actions.len());
        let mut world = fixture();
        let mut sky = SkyColumn::load(&world, LightBounds::new(0, 32).unwrap(), 15, 15).unwrap();
        for ((y, kind), row) in actions.into_iter().zip(rows) {
            put(&mut world, y, kinds[kind].1.clone());
            assert_eq!(
                sky.update(&world, y),
                row[0].as_bool(),
                "y={y}, state={}",
                kinds[kind].0
            );
            assert_eq!(
                i64::from(sky.lowest_source_y()),
                row[1].as_i64().unwrap(),
                "y={y}, state={}",
                kinds[kind].0
            );
        }

        // Initial chunk scanning has its own boundary behavior and is not
        // necessarily equivalent to applying each block change incrementally.
        let mut scenarios = vec![vec![], vec![(0, 6)], vec![(0, 7)], vec![(31, 2)]];
        for _ in 0..100 {
            let mut blocks = Vec::new();
            for _ in 0..12 {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let y = (random >> 16) as i32 % 32;
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                blocks.push((y, (random as usize >> 16) % kinds.len()));
            }
            scenarios.push(blocks);
        }
        let entries: Vec<Vec<_>> = scenarios
            .iter()
            .map(|blocks| {
                blocks
                    .iter()
                    .map(|(y, kind)| serde_json::json!({"y":y,"state":kinds[*kind].0}))
                    .collect()
            })
            .collect();
        std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SkyColumnOracle")
            .arg(&input)
            .arg(&output)
            .arg("initial"));
        let rows: Vec<i32> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), scenarios.len());
        for (blocks, expected) in scenarios.into_iter().zip(rows) {
            let mut world = fixture();
            for &(y, kind) in &blocks {
                put(&mut world, y, kinds[kind].1.clone());
            }
            assert_eq!(
                SkyColumn::load(&world, LightBounds::new(0, 32).unwrap(), 15, 15)
                    .unwrap()
                    .lowest_source_y(),
                expected,
                "initial blocks: {blocks:?}"
            );
        }
    }
}
