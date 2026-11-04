//! Bubble-column state propagation as performed by Java's `updateColumn`.
//!
//! The caller supplies live block reads and writes: a write may trigger nested
//! neighbor updates before the next block is read. Callers that cannot yet
//! reproduce those callbacks must stage or defer the update instead.

use anyhow::{Context, Result};
use glam::{DVec3, IVec3};
use native_resources::fluid_tags::BubbleColumnTags;
use native_world::{fluid::FluidState, loaded::BlockState};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EntityContact {
    pub position: IVec3,
    pub drag_down: bool,
    pub surface: bool,
}

/// Player bubble-column velocity after one inside-block callback.
pub fn player_velocity(mut velocity: DVec3, drag_down: bool, surface: bool, flying: bool) -> DVec3 {
    if flying {
        return velocity;
    }
    velocity.y = match (drag_down, surface) {
        (true, true) => (velocity.y - 0.03).max(-0.9),
        (true, false) => (velocity.y - 0.03).max(-0.3),
        (false, true) => (velocity.y + 0.1).min(1.8),
        (false, false) => (velocity.y + 0.06).min(0.7),
    };
    velocity
}

pub trait ColumnWorld {
    fn block(&mut self, position: IVec3) -> Result<BlockState>;
    /// Match `LevelAccessor.setBlock(..., UPDATE_CLIENTS)`, including callbacks.
    fn set_block(&mut self, position: IVec3, state: &BlockState) -> Result<bool>;
}

/// A bubble column can replace an existing column or a full source of a
/// tagged liquid block. Waterlogged blocks are not liquid blocks.
pub fn can_occupy(state: &BlockState, tags: &BubbleColumnTags) -> Result<bool> {
    if state.name == "minecraft:bubble_column" {
        return Ok(true);
    }
    if !matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
        return Ok(false);
    }
    let fluid = FluidState::from_block(state)
        .map_err(anyhow::Error::msg)?
        .context("liquid block has no fluid state")?;
    Ok(tags.can_occupy_fluid(&state.name) && fluid.source && fluid.amount >= 8)
}

/// Determine the state copied up the entire run. A support present in both
/// block tags pushes upward, because Java checks the upward tag first.
pub fn column_state(
    support: &BlockState,
    current: &BlockState,
    tags: &BubbleColumnTags,
) -> BlockState {
    if support.name == "minecraft:bubble_column" {
        return support.clone();
    }
    let drag = if tags.pushes_up(&support.name) {
        Some(false)
    } else if tags.drags_down(&support.name) {
        Some(true)
    } else {
        None
    };
    if let Some(drag) = drag {
        BlockState {
            name: "minecraft:bubble_column".into(),
            properties: BTreeMap::from([("drag".into(), drag.to_string())]),
        }
    } else if current.name == "minecraft:bubble_column" {
        BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        }
    } else {
        current.clone()
    }
}

/// Update from `start` upward, in Java's read/write order. The first write's
/// result is ignored; later unchanged or failed writes terminate propagation.
pub fn update_column(
    world: &mut impl ColumnWorld,
    start: IVec3,
    tags: &BubbleColumnTags,
) -> Result<()> {
    let current = world.block(start)?;
    if !can_occupy(&current, tags)? {
        return Ok(());
    }
    let below = IVec3::new(
        start.x,
        start
            .y
            .checked_sub(1)
            .context("bubble support coordinate overflow")?,
        start.z,
    );
    let support = world.block(below)?;
    propagate_column(world, start, &support, &current, tags)
}

/// Execute a liquid block's scheduled callback using the state captured when
/// the callback was requested. A preceding fluid tick may already have
/// changed the live block, so Java reads the support and then the live block.
pub fn tick_liquid_block(
    world: &mut impl ColumnWorld,
    start: IVec3,
    original: &BlockState,
    tags: &BubbleColumnTags,
) -> Result<()> {
    anyhow::ensure!(
        matches!(original.name.as_str(), "minecraft:water" | "minecraft:lava"),
        "liquid block tick requires a liquid block state"
    );
    if !can_occupy(original, tags)? {
        return Ok(());
    }
    let below = IVec3::new(start.x, start.y.wrapping_sub(1), start.z);
    let support = world.block(below)?;
    let current = world.block(start)?;
    if !can_occupy(&current, tags)? {
        return Ok(());
    }
    propagate_column(world, start, &support, &current, tags)
}

fn propagate_column(
    world: &mut impl ColumnWorld,
    start: IVec3,
    support: &BlockState,
    current: &BlockState,
    tags: &BubbleColumnTags,
) -> Result<()> {
    let next = column_state(support, current, tags);
    world.set_block(start, &next)?;
    let mut position = start;
    loop {
        position.y = position.y.wrapping_add(1);
        let current = world.block(position)?;
        if !can_occupy(&current, tags)? || !world.set_block(position, &next)? {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    fn cell(id: u64) -> BlockState {
        match id {
            0 => state("air", &[]),
            1 => state("water", &[("level", "0")]),
            2 => state("water", &[("level", "1")]),
            3 => state("bubble_column", &[("drag", "true")]),
            4 => state("bubble_column", &[("drag", "false")]),
            5 => state("lava", &[("level", "0")]),
            6 => state("oak_slab", &[("type", "bottom"), ("waterlogged", "true")]),
            7 => state("lava", &[("level", "1")]),
            _ => panic!("unknown fixture block"),
        }
    }

    fn description(state: &BlockState) -> String {
        match state.name.as_str() {
            "minecraft:bubble_column" => format!("{}/{}", state.name, state.properties["drag"]),
            "minecraft:water" | "minecraft:lava" => {
                format!("{}/{}", state.name, state.properties["level"])
            }
            _ => state.name.clone(),
        }
    }

    struct TestWorld {
        blocks: HashMap<IVec3, BlockState>,
        writes: Vec<serde_json::Value>,
    }
    impl ColumnWorld for TestWorld {
        fn block(&mut self, pos: IVec3) -> Result<BlockState> {
            Ok(self.blocks.get(&pos).cloned().unwrap_or_else(|| cell(0)))
        }
        fn set_block(&mut self, pos: IVec3, next: &BlockState) -> Result<bool> {
            let changed = self.block(pos)? != *next;
            self.writes
                .push(serde_json::json!([pos.y, description(next), changed]));
            if changed {
                self.blocks.insert(pos, next.clone());
            }
            Ok(changed)
        }
    }

    struct TracingWorld {
        blocks: HashMap<IVec3, BlockState>,
        reads: Vec<i32>,
        writes: Vec<serde_json::Value>,
    }

    impl ColumnWorld for TracingWorld {
        fn block(&mut self, pos: IVec3) -> Result<BlockState> {
            self.reads.push(pos.y);
            Ok(self.blocks.get(&pos).cloned().unwrap_or_else(|| cell(0)))
        }

        fn set_block(&mut self, pos: IVec3, next: &BlockState) -> Result<bool> {
            let changed = self.blocks.get(&pos).cloned().unwrap_or_else(|| cell(0)) != *next;
            self.writes
                .push(serde_json::json!([pos.y, description(next), changed]));
            if changed {
                self.blocks.insert(pos, next.clone());
            }
            Ok(changed)
        }
    }

    #[test]
    fn liquid_tick_checks_original_then_reads_support_before_live_block() {
        let start = IVec3::new(8, 10, 8);
        let tags = BubbleColumnTags::with_fluids(
            BTreeSet::new(),
            BTreeSet::from(["minecraft:stone".into()]),
            BTreeSet::from(["minecraft:water".into()]),
        );
        let mut world = TracingWorld {
            blocks: HashMap::from([(start - IVec3::Y, state("stone", &[])), (start, cell(0))]),
            reads: Vec::new(),
            writes: Vec::new(),
        };
        tick_liquid_block(&mut world, start, &cell(1), &tags).unwrap();
        assert_eq!(world.reads, [9, 10]);
        assert!(world.writes.is_empty());
        world.reads.clear();
        world.blocks.insert(start, cell(1));
        tick_liquid_block(&mut world, start, &cell(2), &tags).unwrap();
        assert!(world.reads.is_empty());
        assert!(world.writes.is_empty());
        assert!(tick_liquid_block(&mut world, start, &cell(3), &tags).is_err());
    }

    #[test]
    fn postprocessing_uses_captured_liquid_after_fluid_changes_block() {
        use crate::worldgen::postprocessing::{PostprocessingBlocks, PostprocessingWorld};
        use native_core::postprocessing::PostProcessing;
        use native_world::loaded::ChunkPos;
        use std::sync::Arc;

        struct GenerationWorld {
            world: TracingWorld,
            tags: BubbleColumnTags,
            cleared: bool,
        }
        impl PostprocessingWorld for GenerationWorld {
            type State = BlockState;
            type Fluid = ();
            fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
                self.world.block(IVec3::from_array(pos))
            }
            fn fluid(&mut self, _: &BlockState) -> Result<Option<()>> {
                Ok(Some(()))
            }
            fn tick_fluid(&mut self, pos: [i32; 3], _: &BlockState, _: &()) -> Result<()> {
                self.world.blocks.insert(IVec3::from_array(pos), cell(0));
                Ok(())
            }
            fn is_liquid_block(&mut self, state: &BlockState) -> Result<bool> {
                Ok(state.name == "minecraft:water")
            }
            fn tick_liquid_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
                super::tick_liquid_block(&mut self.world, IVec3::from_array(pos), state, &self.tags)
            }
            fn update_neighbor_shapes(
                &mut self,
                _: [i32; 3],
                _: &BlockState,
            ) -> Result<BlockState> {
                unreachable!()
            }
            fn set_block(&mut self, _: [i32; 3], _: &BlockState, _: i32) -> Result<()> {
                unreachable!()
            }
            fn clear_processed_section(&mut self, _: usize) -> Result<()> {
                self.cleared = true;
                Ok(())
            }
        }
        let start = IVec3::new(8, 10, 8);
        let tags = BubbleColumnTags::with_fluids(
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::from(["minecraft:water".into()]),
        );
        let mut world = GenerationWorld {
            world: TracingWorld {
                blocks: HashMap::from([(start - IVec3::Y, state("stone", &[])), (start, cell(1))]),
                reads: Vec::new(),
                writes: Vec::new(),
            },
            tags,
            cleared: false,
        };
        let mut marks = PostProcessing::new(0, 16).unwrap();
        assert!(marks.mark(start.x, start.y, start.z));
        let mut task = PostprocessingBlocks::new(Arc::new(marks), ChunkPos { x: 0, z: 0 });
        assert!(task.poll(&mut world, 1).unwrap());
        assert_eq!(world.world.reads, [10, 9, 10]);
        assert!(world.world.writes.is_empty());
        assert!(world.cleared);
    }

    #[test]
    fn liquid_tick_wraps_vertical_coordinates_like_java() {
        let start = IVec3::new(8, i32::MAX, 8);
        let tags = BubbleColumnTags::with_fluids(
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::from(["minecraft:water".into()]),
        );
        let mut world = TracingWorld {
            blocks: HashMap::from([(start, cell(1))]),
            reads: Vec::new(),
            writes: Vec::new(),
        };
        tick_liquid_block(&mut world, start, &cell(1), &tags).unwrap();
        assert_eq!(world.reads, [i32::MAX - 1, i32::MAX, i32::MIN]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn liquid_tick_matches_java_with_original_and_live_states() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("LiquidBlockTickOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/LiquidBlockTickOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("LiquidBlockTickOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 2016);
        let start = IVec3::new(8, 10, 8);
        for row in rows {
            let support = row["support"].as_u64().unwrap();
            let mut down = BTreeSet::new();
            let mut up = BTreeSet::new();
            let mut fluids = BTreeSet::new();
            if support == 1 || support == 3 {
                down.insert("minecraft:stone".into());
            }
            if support == 2 || support == 3 {
                up.insert("minecraft:stone".into());
            }
            if row["water"].as_bool().unwrap() {
                fluids.insert("minecraft:water".into());
            }
            if row["lava"].as_bool().unwrap() {
                fluids.insert("minecraft:lava".into());
            }
            let tags = BubbleColumnTags::with_fluids(down, up, fluids);
            let mut world = TracingWorld {
                blocks: HashMap::from([
                    (
                        start - IVec3::Y,
                        match support {
                            4 => cell(3),
                            5 => cell(4),
                            _ => state("stone", &[]),
                        },
                    ),
                    (start, cell(row["live"].as_u64().unwrap())),
                    (start + IVec3::Y, cell(row["above"].as_u64().unwrap())),
                ]),
                reads: Vec::new(),
                writes: Vec::new(),
            };
            tick_liquid_block(
                &mut world,
                start,
                &cell(row["original"].as_u64().unwrap()),
                &tags,
            )
            .unwrap();
            assert_eq!(serde_json::json!(world.reads), row["reads"], "{row}");
            assert_eq!(serde_json::json!(world.writes), row["writes"], "{row}");
            assert_eq!(
                row["next"].as_i64().unwrap(),
                i64::from(native_core::random::LegacyRandom::new(1234).next_int()),
                "{row}"
            );
        }
    }

    #[test]
    fn propagation_stops_at_first_unchanged_upper_block() {
        let start = IVec3::new(8, 10, 8);
        let tags = BubbleColumnTags::with_fluids(
            BTreeSet::from(["minecraft:stone".into()]),
            BTreeSet::from(["minecraft:stone".into()]),
            BTreeSet::from(["minecraft:water".into()]),
        );
        let mut world = TestWorld {
            blocks: HashMap::from([
                (start - IVec3::Y, state("stone", &[])),
                (start, cell(1)),
                (start + IVec3::Y, cell(4)),
                (start + IVec3::Y * 2, cell(1)),
            ]),
            writes: Vec::new(),
        };
        update_column(&mut world, start, &tags).unwrap();
        assert_eq!(
            world.writes,
            [
                serde_json::json!([10, "minecraft:bubble_column/false", true]),
                serde_json::json!([11, "minecraft:bubble_column/false", false]),
            ]
        );
        assert_eq!(world.block(start + IVec3::Y * 2).unwrap(), cell(1));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn update_column_matches_java_for_custom_tags_and_mixed_columns() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BubbleColumnUpdateOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/BubbleColumnUpdateOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("BubbleColumnUpdateOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 8232);
        let start = IVec3::new(8, 10, 8);
        for row in rows {
            let support = row["support"].as_u64().unwrap();
            let mut down = BTreeSet::new();
            let mut up = BTreeSet::new();
            let mut fluids = BTreeSet::new();
            if support == 1 || support == 3 {
                down.insert("minecraft:stone".into());
            }
            if support == 2 || support == 3 {
                up.insert("minecraft:stone".into());
            }
            if row["water"].as_bool().unwrap() {
                fluids.insert("minecraft:water".into());
            }
            if row["lava"].as_bool().unwrap() {
                fluids.insert("minecraft:lava".into());
            }
            let tags = BubbleColumnTags::with_fluids(down, up, fluids);
            let mut world = TestWorld {
                blocks: HashMap::from([
                    (
                        start - IVec3::Y,
                        match support {
                            4 => cell(3),
                            5 => cell(4),
                            _ => state("stone", &[]),
                        },
                    ),
                    (start, cell(row["a"].as_u64().unwrap())),
                    (start + IVec3::Y, cell(row["b"].as_u64().unwrap())),
                    (start + IVec3::Y * 2, cell(row["c"].as_u64().unwrap())),
                ]),
                writes: Vec::new(),
            };
            update_column(&mut world, start, &tags).unwrap();
            assert_eq!(
                world.writes,
                row["writes"].as_array().unwrap().to_vec(),
                "{row}"
            );
        }
    }
}
