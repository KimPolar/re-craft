//! Block portion of the null-entity collision query used by chunk spawning.
use anyhow::{Context, Result, bail, ensure};
use glam::IVec3;
use native_resources::worldgen_mob_geometry::CreatureSpawnGeometry;
use native_world::{
    collision::{Aabb, block_shape},
    loaded::BlockState,
    mob_spawn_blocks::common_mob_spawn_block_facts,
};

const EPSILON: f64 = 1.0e-7;

struct Shape {
    // None is the canonical full cube, which needs no allocation.
    boxes: Option<Vec<Aabb>>,
    large: bool,
    canonical_full: bool,
}

fn shape(state: &BlockState) -> Result<Shape> {
    if let Some(facts) = common_mob_spawn_block_facts(state) {
        if facts.full_collision {
            return Ok(Shape {
                boxes: None,
                large: false,
                canonical_full: true,
            });
        }
        let boxes = block_shape(state)
            .with_context(|| format!("unsupported spawn collision shape {state:?}"))?;
        return Ok(Shape {
            boxes: Some(boxes),
            large: false,
            canonical_full: false,
        });
    }
    let property = |name: &str| {
        matches!(
            state.properties.get(name).map(String::as_str),
            Some("true" | "false")
        )
    };
    if matches!(
        state.name.as_str(),
        "minecraft:oak_leaves"
            | "minecraft:dark_oak_leaves"
            | "minecraft:birch_leaves"
            | "minecraft:azalea_leaves"
            | "minecraft:flowering_azalea_leaves"
    ) && state.properties.len() == 3
        && property("persistent")
        && property("waterlogged")
        && matches!(
            state.properties.get("distance").map(String::as_str),
            Some("1" | "2" | "3" | "4" | "5" | "6" | "7")
        )
    {
        return Ok(Shape {
            boxes: None,
            large: false,
            canonical_full: true,
        });
    }
    if state.name == "minecraft:oak_fence"
        && state.properties.len() == 5
        && ["north", "south", "east", "west", "waterlogged"]
            .iter()
            .all(|p| property(p))
    {
        return Ok(Shape {
            boxes: Some(block_shape(state).context("invalid fence shape")?),
            large: true,
            canonical_full: false,
        });
    }
    if matches!(
        state.name.as_str(),
        "minecraft:oak_slab" | "minecraft:stone_slab"
    ) && state.properties.len() == 2
        && property("waterlogged")
        && matches!(
            state.properties.get("type").map(String::as_str),
            Some("top" | "bottom" | "double")
        )
    {
        let full = state.properties["type"] == "double";
        return Ok(Shape {
            boxes: if full {
                None
            } else {
                Some(block_shape(state).context("invalid slab shape")?)
            },
            large: false,
            canonical_full: full,
        });
    }
    bail!("unsupported spawn collision shape {state:?}")
}

/// Java BlockCollisions uses a one-cell halo and visits X, then Y, then Z.
/// Missing chunks and unsupported shapes are errors, preserving the attempt.
pub(super) fn no_block_collision<'a>(
    area: CreatureSpawnGeometry,
    mut block: impl FnMut(IVec3) -> Result<&'a BlockState>,
) -> Result<bool> {
    let mut start = [0; 3];
    let mut end = [0; 3];
    for axis in 0..3 {
        ensure!(
            area.min[axis].is_finite()
                && area.max[axis].is_finite()
                && area.max[axis] - area.min[axis] > EPSILON,
            "invalid spawn collision box"
        );
        let lo = (area.min[axis] - EPSILON).floor();
        let hi = (area.max[axis] + EPSILON).floor();
        ensure!(
            lo >= f64::from(i32::MIN + 1) && hi <= f64::from(i32::MAX - 1),
            "spawn collision coordinates overflow"
        );
        ensure!(hi - lo <= 32.0, "spawn collision box is too large");
        start[axis] = lo as i32 - 1;
        end[axis] = hi as i32 + 1;
    }
    for z in start[2]..=end[2] {
        for y in start[1]..=end[1] {
            for x in start[0]..=end[0] {
                let edge = usize::from(x == start[0] || x == end[0])
                    + usize::from(y == start[1] || y == end[1])
                    + usize::from(z == start[2] || z == end[2]);
                if edge == 3 {
                    continue;
                }
                let state = block(IVec3::new(x, y, z))?;
                if edge == 2 {
                    ensure!(
                        state.name != "minecraft:moving_piston",
                        "moving piston collision is unsupported"
                    );
                    continue;
                }
                let shape = shape(state)?;
                if edge == 1 && !shape.large {
                    continue;
                }
                let base = [f64::from(x), f64::from(y), f64::from(z)];
                if shape.canonical_full {
                    if (0..3).all(|axis| {
                        area.max[axis] > base[axis] && area.min[axis] < base[axis] + 1.0
                    }) {
                        return Ok(false);
                    }
                    continue;
                }
                for b in shape.boxes.context("non-full collision shape is missing")? {
                    if (0..3).all(|axis| {
                        area.max[axis] > base[axis] + f64::from(b.min[axis]) + EPSILON
                            && area.min[axis] < base[axis] + f64::from(b.max[axis]) - EPSILON
                    }) {
                        return Ok(false);
                    }
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        }
    }
    fn box_at(min: [f64; 3], max: [f64; 3]) -> CreatureSpawnGeometry {
        CreatureSpawnGeometry {
            position: [0.0; 3],
            min,
            max,
        }
    }
    #[test]
    fn full_face_and_fence_overhang_are_distinguished() {
        let air = state("air", &[]);
        let stone = state("stone", &[]);
        let fence = state(
            "oak_fence",
            &[
                ("north", "false"),
                ("south", "false"),
                ("east", "false"),
                ("west", "false"),
                ("waterlogged", "false"),
            ],
        );
        let bounds = box_at([0.4, 1.0, 0.4], [0.6, 1.9, 0.6]);
        for (block, expected) in [(stone, true), (fence, false)] {
            assert_eq!(
                no_block_collision(bounds, |pos| Ok(if pos == IVec3::ZERO {
                    &block
                } else {
                    &air
                }))
                .unwrap(),
                expected
            );
        }
        let unknown = state("unknown", &[]);
        assert!(
            no_block_collision(bounds, |pos| Ok(if pos == IVec3::ZERO {
                &unknown
            } else {
                &air
            }))
            .is_err()
        );
        let solid = state("stone", &[]);
        let spanning = box_at([0.1, 0.1, 0.1], [1.9, 0.9, 0.9]);
        assert!(
            !no_block_collision(spanning, |pos| Ok(match pos.to_array() {
                [0, 0, 0] => &solid,
                [1, 0, 0] => &unknown,
                _ => &air,
            }))
            .unwrap()
        );
        let invalid_leaf = state(
            "oak_leaves",
            &[
                ("distance", "0"),
                ("persistent", "false"),
                ("waterlogged", "false"),
            ],
        );
        assert!(shape(&invalid_leaf).is_err());
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_block_collision_matches_java_iterator() {
        use serde_json::{Value, json};
        use std::{path::PathBuf, process::Command};
        let states = [
            state("stone", &[]),
            state("glass", &[]),
            state("dirt", &[]),
            state("short_grass", &[]),
            state(
                "leaf_litter",
                &[("facing", "north"), ("segment_amount", "2")],
            ),
            state(
                "oak_leaves",
                &[
                    ("distance", "7"),
                    ("persistent", "false"),
                    ("waterlogged", "false"),
                ],
            ),
            state(
                "dark_oak_leaves",
                &[
                    ("distance", "1"),
                    ("persistent", "true"),
                    ("waterlogged", "true"),
                ],
            ),
            state(
                "birch_leaves",
                &[
                    ("distance", "3"),
                    ("persistent", "false"),
                    ("waterlogged", "true"),
                ],
            ),
            state(
                "azalea_leaves",
                &[
                    ("distance", "4"),
                    ("persistent", "true"),
                    ("waterlogged", "false"),
                ],
            ),
            state(
                "flowering_azalea_leaves",
                &[
                    ("distance", "6"),
                    ("persistent", "false"),
                    ("waterlogged", "false"),
                ],
            ),
            state("water", &[("level", "0")]),
            state("farmland", &[("moisture", "0")]),
            state(
                "oak_fence",
                &[
                    ("north", "false"),
                    ("south", "false"),
                    ("east", "false"),
                    ("west", "false"),
                    ("waterlogged", "false"),
                ],
            ),
            state(
                "oak_fence",
                &[
                    ("north", "true"),
                    ("south", "true"),
                    ("east", "false"),
                    ("west", "true"),
                    ("waterlogged", "true"),
                ],
            ),
            state(
                "stone_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ),
            state("stone_slab", &[("type", "top"), ("waterlogged", "false")]),
            state("oak_slab", &[("type", "double"), ("waterlogged", "false")]),
        ];
        let mut cases = Vec::new();
        let mut inputs = Vec::new();
        for block in &states {
            for origin in [[0, 0, 0], [-16, -32, 16]] {
                for axis in 0..3 {
                    for side in [0, 1] {
                        for delta in [-2.0e-7, -5.0e-8, 0.0, 5.0e-8, 2.0e-7] {
                            let mut min = [0.2, 0.2, 0.2];
                            let mut max = [0.8, 0.8, 0.8];
                            if side == 0 {
                                min[axis] = -0.7 + delta;
                                max[axis] = delta;
                            } else {
                                min[axis] = 1.0 + delta;
                                max[axis] = 1.7 + delta;
                            }
                            for i in 0..3 {
                                min[i] += f64::from(origin[i]);
                                max[i] += f64::from(origin[i]);
                            }
                            let cell = IVec3::from_array(origin);
                            cases.push((box_at(min, max), cell, block.clone()));
                            inputs.push(json!({"min":min,"max":max,"blocks":[{"position":origin,"name":block.name,"properties":block.properties}]}));
                        }
                    }
                }
                let min = [
                    f64::from(origin[0]) + 0.4,
                    f64::from(origin[1]) + 1.0,
                    f64::from(origin[2]) + 0.4,
                ];
                let max = [min[0] + 0.2, min[1] + 0.9, min[2] + 0.2];
                cases.push((box_at(min, max), IVec3::from_array(origin), block.clone()));
                inputs.push(json!({"min":min,"max":max,"blocks":[{"position":origin,"name":block.name,"properties":block.properties}]}));
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
        let source = temp.path().join("CreatureBlockCollisionOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/CreatureBlockCollisionOracle.java"),
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
            .arg("CreatureBlockCollisionOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<bool> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(cases.len(), expected.len());
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        };
        for ((bounds, cell, state), expected) in cases.into_iter().zip(expected) {
            let actual =
                no_block_collision(bounds, |pos| Ok(if pos == cell { &state } else { &air }))
                    .unwrap();
            assert_eq!(actual, expected, "{bounds:?} {cell:?} {state:?}");
        }
    }
}
