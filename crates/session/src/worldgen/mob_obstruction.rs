//! Java's inclusive block scan for Mob.checkSpawnObstruction.
use anyhow::{Result, ensure};
use glam::IVec3;
use native_world::{loaded::ChunkPos, movement::BoundingBox};

pub(super) trait FluidScanWorld {
    fn require_chunk(&mut self, pos: ChunkPos) -> Result<()>;
    fn has_fluid(&mut self, pos: IVec3) -> Result<bool>;
}

/// The max position is floored and included even when it is exactly integral.
/// Java scans chunk X, chunk Z, section Y, then block Y, Z, X.
pub(super) fn contains_any_liquid(
    bounds: BoundingBox,
    min_y: i32,
    max_y: i32,
    world: &mut impl FluidScanWorld,
) -> Result<bool> {
    ensure!(min_y < max_y, "invalid world height");
    let mut lo = [0i32; 3];
    let mut hi = [0i32; 3];
    for axis in 0..3 {
        let min = bounds.min[axis];
        let max = bounds.max[axis];
        ensure!(
            min.is_finite() && max.is_finite() && min <= max,
            "invalid entity bounding box"
        );
        let a = min.floor();
        let b = max.floor();
        ensure!(
            a >= f64::from(i32::MIN) && b <= f64::from(i32::MAX),
            "entity bounding box coordinate overflow"
        );
        ensure!(b - a <= 32.0, "entity bounding box is too large");
        lo[axis] = a as i32;
        hi[axis] = b as i32;
    }
    let y0 = lo[1].max(min_y);
    let y1 = hi[1].min(max_y - 1);
    for cx in lo[0].div_euclid(16)..=hi[0].div_euclid(16) {
        for cz in lo[2].div_euclid(16)..=hi[2].div_euclid(16) {
            world.require_chunk(ChunkPos { x: cx, z: cz })?;
            if y0 > y1 {
                continue;
            }
            let section_base = |section: i32| i64::from(section) * 16;
            let x0 = i64::from(lo[0]).max(section_base(cx)) as i32;
            let x1 = i64::from(hi[0]).min(section_base(cx) + 15) as i32;
            let z0 = i64::from(lo[2]).max(section_base(cz)) as i32;
            let z1 = i64::from(hi[2]).min(section_base(cz) + 15) as i32;
            for sy in y0.div_euclid(16)..=y1.div_euclid(16) {
                let from_y = i64::from(y0).max(section_base(sy)) as i32;
                let to_y = i64::from(y1).min(section_base(sy) + 15) as i32;
                for y in from_y..=to_y {
                    for z in z0..=z1 {
                        for x in x0..=x1 {
                            if world.has_fluid(IVec3::new(x, y, z))? {
                                return Ok(true);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use glam::DVec3;

    struct FakeWorld {
        wet: Option<IVec3>,
        missing: Option<ChunkPos>,
        reads: Vec<IVec3>,
    }
    impl FluidScanWorld for FakeWorld {
        fn require_chunk(&mut self, pos: ChunkPos) -> Result<()> {
            if self.missing == Some(pos) {
                bail!("missing chunk");
            }
            Ok(())
        }
        fn has_fluid(&mut self, pos: IVec3) -> Result<bool> {
            self.reads.push(pos);
            Ok(self.wet == Some(pos))
        }
    }
    fn bounds(min: [f64; 3], max: [f64; 3]) -> BoundingBox {
        BoundingBox {
            min: DVec3::from_array(min),
            max: DVec3::from_array(max),
        }
    }
    #[test]
    fn scan_includes_maximum_cells_and_clips_vertical_bounds() {
        let mut world = FakeWorld {
            wet: Some(IVec3::new(1, 0, 0)),
            missing: None,
            reads: Vec::new(),
        };
        assert!(
            !contains_any_liquid(
                bounds([0.1, 0.1, 0.1], [0.999, 0.9, 0.9]),
                -16,
                16,
                &mut world
            )
            .unwrap()
        );
        world.reads.clear();
        assert!(
            contains_any_liquid(
                bounds([0.1, 0.1, 0.1], [1.0, 0.9, 0.9]),
                -16,
                16,
                &mut world
            )
            .unwrap()
        );
        world.reads.clear();
        world.wet = Some(IVec3::new(0, 16, 0));
        assert!(
            !contains_any_liquid(
                bounds([0.1, 15.1, 0.1], [0.9, 16.0, 0.9]),
                -16,
                16,
                &mut world
            )
            .unwrap()
        );
        assert!(world.reads.iter().all(|p| p.y == 15));
        world.missing = Some(ChunkPos { x: 1, z: 0 });
        assert!(
            contains_any_liquid(
                bounds([15.1, 0.1, 0.1], [16.0, 0.9, 0.9]),
                -16,
                16,
                &mut world
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn actual_animal_obstruction_matches_java_worldgen_region() {
        use native_world::{fluid::FluidState, loaded::BlockState};
        use serde_json::{Value, json};
        use std::{path::PathBuf, process::Command};
        fn state(name: &str, props: &[(&str, &str)]) -> BlockState {
            BlockState {
                name: format!("minecraft:{name}"),
                properties: props
                    .iter()
                    .map(|(k, v)| ((*k).into(), (*v).into()))
                    .collect(),
            }
        }
        let states = [
            state("air", &[]),
            state("stone", &[]),
            state("water", &[("level", "0")]),
            state("water", &[("level", "15")]),
            state("lava", &[("level", "0")]),
            state("seagrass", &[]),
            state(
                "oak_leaves",
                &[
                    ("distance", "7"),
                    ("persistent", "false"),
                    ("waterlogged", "false"),
                ],
            ),
            state(
                "oak_leaves",
                &[
                    ("distance", "7"),
                    ("persistent", "false"),
                    ("waterlogged", "true"),
                ],
            ),
            state(
                "small_amethyst_bud",
                &[("facing", "up"), ("waterlogged", "true")],
            ),
            state(
                "small_dripleaf",
                &[
                    ("facing", "north"),
                    ("half", "lower"),
                    ("waterlogged", "true"),
                ],
            ),
            state("short_grass", &[]),
        ];
        let entities = [
            "pig", "cow", "sheep", "chicken", "horse", "donkey", "goat", "rabbit",
        ];
        let mut cases = Vec::new();
        let mut inputs = Vec::new();
        for (index, state) in states.iter().enumerate() {
            for origin in [[0, 0, 0], [-16, -16, 16], [15, 15, -1]] {
                for axis in 0..3 {
                    for delta in [-1.0e-7, 0.0, 1.0e-7] {
                        let mut min = [0.2, 0.2, 0.2];
                        let mut max = [0.8, 0.8, 0.8];
                        max[axis] = 1.0 + delta;
                        for i in 0..3 {
                            min[i] += f64::from(origin[i]);
                            max[i] += f64::from(origin[i]);
                        }
                        let mut position = origin;
                        position[axis] += 1;
                        let bbox = bounds(min, max);
                        let cell = IVec3::from_array(position);
                        cases.push((bbox, cell, state.clone()));
                        inputs.push(json!({"entity":entities[index % entities.len()],"min":min,"max":max,
                        "blocks":[{"position":position,"name":state.name,"properties":state.properties}]}));
                    }
                }
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
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
        let source = temp.path().join("CreatureObstructionOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/CreatureObstructionOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&inputs).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("CreatureObstructionOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<bool> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(cases.len(), expected.len());
        struct TestWorld<'a> {
            cell: IVec3,
            state: &'a BlockState,
            registry: &'a native_resources::block_registry::GeneratedBlockRegistry,
        }
        impl FluidScanWorld for TestWorld<'_> {
            fn require_chunk(&mut self, _: ChunkPos) -> Result<()> {
                Ok(())
            }
            fn has_fluid(&mut self, pos: IVec3) -> Result<bool> {
                if pos != self.cell {
                    return Ok(false);
                }
                anyhow::ensure!(
                    self.registry
                        .is_canonical_state(&self.state.name, &self.state.properties),
                    "unverified test state {:?}",
                    self.state
                );
                Ok(FluidState::from_block(self.state)
                    .map_err(anyhow::Error::msg)?
                    .is_some())
            }
        }
        for ((bounds, cell, state), expected) in cases.iter().zip(expected) {
            let mut world = TestWorld {
                cell: *cell,
                state,
                registry: &registry,
            };
            let actual = contains_any_liquid(*bounds, -16, 16, &mut world).unwrap();
            assert_eq!(!actual, expected, "{bounds:?} {cell:?} {state:?}");
        }
    }
}
