//! Ordered placement of a processed ocean-ruin piece.
use super::*;
use native_core::{beardifier::BoundingBox, random::DecorationRandom};
use native_resources::{
    ocean_ruin::{DataMarkerKind, FloorWorld, MarkerWorld, handle_data_marker},
    tree_leaf_update::{ShapeEdgeWorld, TreeBounds, update_shape_at_edges},
};
use native_world::{fluid::FluidState, loaded::BlockState};
use std::collections::BTreeSet;

/// World operations used by Java's template placement. Implementations must
/// use the real generation world's block entities, fluids, ticks and neighbor
/// rules; an error requires discarding the whole unpublished world attempt.
pub trait OceanRuinPlacementWorld:
    FloorWorld + MarkerWorld + ShapeEdgeWorld<State = BlockState>
{
    fn fluid_at(&mut self, position: [i32; 3]) -> Result<Option<FluidState>>;
    fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>>;
    /// Apply the barrier for NBT, the block write with flag 2, and load the
    /// resulting block entity, including any container RNG draw, in that order.
    fn place_template_block(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool>;
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool>;
    /// Invoke the destination block's actual placeLiquid, including its ticks.
    fn place_liquid(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
        fluid: Option<FluidState>,
    ) -> Result<()>;
    fn update_neighbors_at(&mut self, position: [i32; 3], state: &BlockState) -> Result<()>;
    fn mark_block_entity_changed(&mut self, position: [i32; 3]) -> Result<()>;
    fn place_template_entities(
        &mut self,
        entities: &[Tag],
        position: [i32; 3],
        rotation: ocean_ruin::Rotation,
        clip: BoundingBox,
    ) -> Result<()>;
}

fn inside(clip: BoundingBox, pos: [i32; 3]) -> bool {
    (0..3).all(|i| (clip.min[i]..=clip.max[i]).contains(&pos[i]))
}
fn source(fluid: Option<FluidState>) -> bool {
    fluid.is_some_and(|fluid| fluid.source)
}

fn restore_liquids<W: OceanRuinPlacementWorld>(
    world: &mut W,
    mut unresolved: Vec<[i32; 3]>,
    template_sources: &BTreeSet<[i32; 3]>,
) -> Result<()> {
    let directions = [[0, 1, 0], [0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]];
    let mut changed = true;
    while changed && !unresolved.is_empty() {
        changed = false;
        let mut remaining = Vec::new();
        for pos in unresolved {
            let mut fluid = world.fluid_at(pos)?;
            for direction in directions {
                if source(fluid) {
                    break;
                }
                let neighbor = [
                    pos[0].wrapping_add(direction[0]),
                    pos[1].wrapping_add(direction[1]),
                    pos[2].wrapping_add(direction[2]),
                ];
                let candidate = world.fluid_at(neighbor)?;
                if source(candidate) && !template_sources.contains(&neighbor) {
                    fluid = candidate;
                }
            }
            if source(fluid) {
                let state = ShapeEdgeWorld::block(world, pos)?;
                if world.is_liquid_container(&state)? {
                    world.place_liquid(pos, &state, fluid)?;
                    changed = true;
                    continue;
                }
            }
            remaining.push(pos);
        }
        unresolved = remaining;
    }
    Ok(())
}

impl PreparedPiece {
    /// Align and place a whole piece in the same order as Java 26.3. The world
    /// must be an isolated draft: on error it may contain partial writes and
    /// must be discarded. The piece and decoration RNG change only on success;
    /// callers must additionally stage them until their world draft commits.
    pub fn post_process<W: OceanRuinPlacementWorld>(
        &mut self,
        world: &mut W,
        registry: &GeneratedBlockRegistry,
        world_seed: i64,
        temperature: Temperature,
        clip: BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(
            (0..3).all(|i| clip.min[i] <= clip.max[i]),
            "invalid ruin clipping box"
        );
        let mut piece = self.clone();
        let mut next_random = random.clone();
        piece.align_to_floor(world)?;
        let placed = piece.place_aligned(
            world,
            registry,
            world_seed,
            temperature,
            clip,
            &mut next_random,
        )?;
        *self = piece;
        *random = next_random;
        Ok(placed)
    }

    fn place_aligned<W: OceanRuinPlacementWorld>(
        &self,
        world: &mut W,
        registry: &GeneratedBlockRegistry,
        world_seed: i64,
        temperature: Temperature,
        clip: BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        let Some(index) = self.selected_palette_index()? else {
            return Ok(false);
        };
        if self.template.blocks.is_empty() && self.template.entities.is_empty()
            || self.template.size.iter().any(|size| *size < 1)
        {
            return Ok(false);
        }
        ensure!(
            !self.template.palettes[index].iter().any(|entry| matches!(entry.get("id"), Some(Tag::String(id)) if id.matches("minecraft:jigsaw"))),
            "ocean ruin jigsaw final-state processing is unsupported"
        );
        let palette = self.transformed_palette(registry, index)?;
        let blocks = self.process_blocks_with_palette(index, &palette, world_seed, temperature)?;
        let mut unresolved = Vec::new();
        let mut template_sources = BTreeSet::new();
        let mut placed = Vec::new();
        for block in &blocks {
            let pos = block.original.position;
            if !inside(clip, pos) {
                continue;
            }
            let original = &self.template.blocks[block.original.source_index];
            let (state, nbt) = match &block.archaeology {
                Some(replacement) => (&replacement.state, Some(&replacement.nbt)),
                None => {
                    let PreparedPaletteState::Block(state) = &palette[original.state] else {
                        anyhow::bail!("processed marker cannot be placed as a block")
                    };
                    (state, original.nbt.as_ref())
                }
            };
            let previous_fluid = world.fluid_at(pos)?;
            if !world.place_template_block(pos, state, nbt, random)? {
                continue;
            }
            placed.push((pos, nbt.is_some()));
            if source(world.fluid_of_state(state)?) {
                template_sources.insert(pos);
            } else if world.is_liquid_container(state)? {
                world.place_liquid(pos, state, previous_fluid)?;
                if !source(previous_fluid) {
                    unresolved.push(pos);
                }
            }
        }
        restore_liquids(world, unresolved, &template_sources)?;
        if !placed.is_empty() {
            let mut min = [i32::MAX; 3];
            let mut max = [i32::MIN; 3];
            let mut shape = BTreeSet::new();
            for &(pos, _) in &placed {
                for i in 0..3 {
                    min[i] = min[i].min(pos[i]);
                    max[i] = max[i].max(pos[i]);
                }
                shape.insert(pos);
            }
            update_shape_at_edges(world, TreeBounds { min, max }, &shape, 2)?;
            for (pos, has_nbt) in placed {
                let state = ShapeEdgeWorld::block(world, pos)?;
                let updated = crate::worldgen::postprocessing::update_from_neighbor_shapes(
                    world, pos, &state,
                )?;
                if updated != state {
                    ShapeEdgeWorld::set_block(world, pos, &updated, 18)?;
                }
                world.update_neighbors_at(pos, &updated)?;
                if has_nbt {
                    world.mark_block_entity_changed(pos)?;
                }
            }
        }
        if !self.template.entities.is_empty() {
            world.place_template_entities(
                &self.template.entities,
                self.selection.position,
                self.selection.rotation,
                clip,
            )?;
        }
        for marker in self.data_markers_with_palette(index, &palette, clip)? {
            let kind = if marker.metadata.matches("chest") {
                DataMarkerKind::Chest
            } else if marker.metadata.matches("drowned") {
                DataMarkerKind::Drowned
            } else {
                DataMarkerKind::Other
            };
            handle_data_marker(world, random, kind, marker.position, self.selection.large)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct World {
        blocks: BTreeMap<[i32; 3], BlockState>,
        events: Vec<(&'static str, [i32; 3])>,
        fail_shapes: bool,
    }
    fn air() -> BlockState {
        BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        }
    }
    impl FloorWorld for World {
        fn min_y(&self) -> i32 {
            -64
        }
        fn ocean_floor_height(&mut self, _: i32, _: i32) -> Result<i32> {
            Ok(20)
        }
        fn floor_scan_passable(&mut self, _: [i32; 3]) -> Result<bool> {
            Ok(false)
        }
    }
    impl MarkerWorld for World {
        fn fluid_in_water_tag(&mut self, _: [i32; 3]) -> Result<bool> {
            Ok(false)
        }
        fn set_default_chest(&mut self, pos: [i32; 3], _: bool) -> Result<()> {
            self.events.push(("chest", pos));
            Ok(())
        }
        fn chest_entity_exists(&mut self, _: [i32; 3]) -> Result<bool> {
            Ok(true)
        }
        fn set_chest_loot(&mut self, pos: [i32; 3], _: &str, _: i64) -> Result<()> {
            self.events.push(("loot", pos));
            Ok(())
        }
        fn create_and_spawn_drowned(&mut self, pos: [i32; 3]) -> Result<bool> {
            self.events.push(("drowned", pos));
            Ok(false)
        }
        fn sea_level(&mut self) -> Result<i32> {
            Ok(63)
        }
        fn clear_drowned_marker(&mut self, _: [i32; 3], _: bool) -> Result<()> {
            unreachable!()
        }
    }
    impl ShapeEdgeWorld for World {
        type State = BlockState;
        fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
            Ok(self.blocks.get(&pos).cloned().unwrap_or_else(air))
        }
        fn update_shape(
            &mut self,
            pos: [i32; 3],
            state: &BlockState,
            _: [i32; 3],
            _: [i32; 3],
            _: &BlockState,
        ) -> Result<BlockState> {
            if self.fail_shapes {
                anyhow::bail!("injected shape failure")
            }
            self.events.push(("shape", pos));
            Ok(state.clone())
        }
        fn set_block(&mut self, pos: [i32; 3], state: &BlockState, _: i32) -> Result<()> {
            self.blocks.insert(pos, state.clone());
            Ok(())
        }
    }
    impl OceanRuinPlacementWorld for World {
        fn fluid_at(&mut self, pos: [i32; 3]) -> Result<Option<FluidState>> {
            self.fluid_of_state(&self.blocks.get(&pos).cloned().unwrap_or_else(air))
        }
        fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>> {
            FluidState::from_block(state).map_err(anyhow::Error::msg)
        }
        fn place_template_block(
            &mut self,
            pos: [i32; 3],
            state: &BlockState,
            _: Option<&Tag>,
            _: &mut DecorationRandom,
        ) -> Result<bool> {
            self.events.push(("block", pos));
            self.blocks.insert(pos, state.clone());
            Ok(true)
        }
        fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
            Ok(state.name.ends_with("_stairs"))
        }
        fn place_liquid(
            &mut self,
            pos: [i32; 3],
            state: &BlockState,
            fluid: Option<FluidState>,
        ) -> Result<()> {
            self.events.push(("liquid", pos));
            if fluid.is_some_and(|f| f.source && f.kind == native_world::fluid::FluidKind::Water) {
                let mut wet = state.clone();
                wet.properties.insert("waterlogged".into(), "true".into());
                self.blocks.insert(pos, wet);
            }
            Ok(())
        }
        fn update_neighbors_at(&mut self, pos: [i32; 3], _: &BlockState) -> Result<()> {
            self.events.push(("neighbors", pos));
            Ok(())
        }
        fn mark_block_entity_changed(&mut self, pos: [i32; 3]) -> Result<()> {
            self.events.push(("changed", pos));
            Ok(())
        }
        fn place_template_entities(
            &mut self,
            _: &[Tag],
            pos: [i32; 3],
            _: ocean_ruin::Rotation,
            _: BoundingBox,
        ) -> Result<()> {
            self.events.push(("entities", pos));
            Ok(())
        }
    }

    #[test]
    fn flowing_positions_revisit_in_order_but_ignore_template_sources() -> Result<()> {
        let stair = || BlockState {
            name: "minecraft:stone_brick_stairs".into(),
            properties: BTreeMap::from([("waterlogged".into(), "false".into())]),
        };
        let water = || BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        };
        let mut world = World::default();
        for x in 0..=2 {
            world.blocks.insert([x, 0, 0], stair());
        }
        world.blocks.insert([-1, 0, 0], water());
        restore_liquids(
            &mut world,
            vec![[2, 0, 0], [1, 0, 0], [0, 0, 0]],
            &BTreeSet::new(),
        )?;
        assert_eq!(
            world.events,
            vec![
                ("liquid", [0, 0, 0]),
                ("liquid", [1, 0, 0]),
                ("liquid", [2, 0, 0])
            ]
        );
        let mut world = World::default();
        world.blocks.insert([0, 0, 0], stair());
        world.blocks.insert([-1, 0, 0], water());
        world.blocks.insert([0, -1, 0], water());
        restore_liquids(&mut world, vec![[0, 0, 0]], &BTreeSet::from([[-1, 0, 0]]))?;
        assert!(
            world.events.is_empty(),
            "template source and water below must not spread"
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn liquid_restoration_order_matches_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("missing libraries")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let source = temp.path().join("OceanRuinLiquidOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/OceanRuinLiquidOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(cp)
            .arg("OceanRuinLiquidOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java water test failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            String::from_utf8(output.stdout)?
                .lines()
                .any(|line| line == "OK"),
            "missing Java result"
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn piece_places_blocks_before_shapes_and_markers_and_rolls_back_inputs() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let registry = data.vanilla_generated_block_registry()?;
        let mut seen_blocks = 0;
        let mut seen_markers = 0;
        for temperature in [Temperature::Warm, Temperature::Cold] {
            let definition = serde_json::json!({"type":"minecraft:ocean_ruin","biome_temp":if temperature == Temperature::Warm { "warm" } else { "cold" },"large_probability":0.5,"cluster_probability":0.9});
            for seed in 0..8 {
                for mut piece in super::super::prepare_pieces(&mut data, &definition, seed, [0, 0])?
                {
                    let clip = BoundingBox::new([-128, -64, -128], [128, 320, 128]).unwrap();
                    let mut world = World::default();
                    let mut random = DecorationRandom::new(seed);
                    assert!(piece.post_process(
                        &mut world,
                        &registry,
                        seed,
                        temperature,
                        clip,
                        &mut random
                    )?);
                    let writes = world
                        .events
                        .iter()
                        .filter(|(event, _)| *event == "block")
                        .count();
                    let markers = world
                        .events
                        .iter()
                        .filter(|(event, _)| matches!(*event, "chest" | "drowned"))
                        .count();
                    seen_blocks += writes;
                    seen_markers += markers;
                    if let Some(first_shape) =
                        world.events.iter().position(|(event, _)| *event == "shape")
                    {
                        assert!(
                            world.events[first_shape..]
                                .iter()
                                .all(|(event, _)| *event != "block")
                        );
                    }
                    if let Some(first_marker) = world
                        .events
                        .iter()
                        .position(|(event, _)| matches!(*event, "chest" | "drowned"))
                    {
                        assert!(
                            world.events[first_marker..]
                                .iter()
                                .all(|(event, _)| !matches!(
                                    *event,
                                    "shape" | "neighbors" | "block"
                                ))
                        );
                    }
                    assert_eq!(piece.selection.position[1], 20);
                }
            }
        }
        assert!(seen_blocks > 0 && seen_markers > 0);
        let definition = serde_json::json!({"type":"minecraft:ocean_ruin","biome_temp":"warm","large_probability":0.0,"cluster_probability":0.0});
        let mut piece = super::super::prepare_pieces(&mut data, &definition, 0, [0, 0])?.remove(0);
        let original_position = piece.selection.position;
        let mut random = DecorationRandom::new(31415);
        let mut original_random = random.clone();
        let mut world = World {
            fail_shapes: true,
            ..Default::default()
        };
        let clip = BoundingBox::new([-128, -64, -128], [128, 320, 128]).unwrap();
        assert!(
            piece
                .post_process(
                    &mut world,
                    &registry,
                    0,
                    Temperature::Warm,
                    clip,
                    &mut random
                )
                .is_err()
        );
        assert_eq!(piece.selection.position, original_position);
        assert_eq!(random.next_long(), original_random.next_long());
        assert!(world.events.iter().any(|(event, _)| *event == "block"));
        Ok(())
    }
}
