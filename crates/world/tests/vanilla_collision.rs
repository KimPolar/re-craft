//! Differential check of every state for which native collision is implemented.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_world::{
    collision::{Aabb, block_outline, block_shape, slab_stair_solid, spawn_collision_shape},
    loaded::BlockState,
};

const ORACLE: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.context.DirectionalPlaceContext;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.FallingBlock;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.StairBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.phys.shapes.CollisionContext;
public class CollisionOracle {
    static class World extends ClientLevel {
        BlockState state;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public BlockState getBlockState(BlockPos pos) { return state; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Items.STONE_BUTTON.builtInRegistryHolder().bindComponents(net.minecraft.core.component.DataComponentMap.builder().set(net.minecraft.core.component.DataComponents.MAX_STACK_SIZE, 64).build());
        ItemStack button = new ItemStack(Items.STONE_BUTTON);
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world = (World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
        JsonArray all = new JsonArray();
        for (var state : Block.BLOCK_STATE_REGISTRY) {
            JsonObject record = new JsonObject();
            record.addProperty("id", BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
            if (state.getBlock() instanceof net.minecraft.world.level.block.ChestBlock) {
                record.addProperty("chestDirection", net.minecraft.world.level.block.ChestBlock.getConnectedDirection(state).getName());
                record.addProperty("chestOrder", net.minecraft.world.level.block.ChestBlock.getBlockType(state).name());
            }
            record.addProperty("solid", state.isSolid());
            record.addProperty("slab_stair", state.getBlock() instanceof SlabBlock || state.getBlock() instanceof StairBlock);
            record.addProperty("replaceable", state.canBeReplaced());
            record.addProperty("fallingFree", FallingBlock.isFree(state));
            world.state = state;
            record.addProperty("landingReplaceable", state.canBeReplaced(new DirectionalPlaceContext(world, BlockPos.ZERO, Direction.DOWN, ItemStack.EMPTY, Direction.UP)));
            record.addProperty("buttonReplaceable", state.canBeReplaced(new DirectionalPlaceContext(world, BlockPos.ZERO, Direction.DOWN, button, Direction.UP)));
            if (java.util.Set.of("minecraft:short_grass", "minecraft:fern", "minecraft:dead_bush").contains(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString())) {
                world.state = state;
                record.addProperty("fallingReplaceable", state.canBeReplaced(new DirectionalPlaceContext(world, BlockPos.ZERO, Direction.DOWN, ItemStack.EMPTY, Direction.UP)));
            }
            JsonObject properties = new JsonObject();
            state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
            record.add("properties", properties);
            boolean dynamic = state.getBlock().hasDynamicShape();
            record.addProperty("dynamic", dynamic);
            if (!dynamic) {
                {
                    JsonArray sturdy = new JsonArray();
                    for (var direction : new Direction[]{Direction.DOWN,Direction.UP,Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST})
                        sturdy.add(state.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,direction));
                    record.add("sturdy", sturdy);
                }
                JsonArray boxes = new JsonArray();
                for (var box : state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO).toAabbs()) {
                    JsonArray coords = new JsonArray();
                    for (double value : new double[] {box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ}) coords.add(value);
                    boxes.add(coords);
                }
                record.add("boxes", boxes);
                JsonArray spawnBoxes = new JsonArray();
                for (var box : state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, CollisionContext.emptyWithFluidCollisions()).toAabbs()) {
                    JsonArray coords = new JsonArray();
                    for (double value : new double[] {box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ}) coords.add(value);
                    spawnBoxes.add(coords);
                }
                record.add("spawnBoxes", spawnBoxes);
                JsonArray outline = new JsonArray();
                for (var box : state.getShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO).toAabbs()) {
                    JsonArray coords = new JsonArray();
                    for (double value : new double[] {box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ}) coords.add(value);
                    outline.add(coords);
                }
                record.add("outline", outline);
            }
            all.add(record);
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(all));
    }
}
"#;

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to Java 25 JDK"))
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "collision oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn soil_expansion_teleport_matches_java_shapes() {
    use glam::{DVec3, IVec3};
    use native_world::movement::{BoundingBox, soil_expansion_push};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SoilPushOracle.java");
    std::fs::write(&source, r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;
public class SoilPushOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var pos=new BlockPos(10,64,-12);
    var dirt=Blocks.DIRT.defaultBlockState().getCollisionShape(EmptyBlockGetter.INSTANCE,pos);
    var source=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var out=new JsonArray();
    for(var block:List.of(Blocks.DIRT_PATH,Blocks.FARMLAND)) {
      var old=block.defaultBlockState().getCollisionShape(EmptyBlockGetter.INSTANCE,pos);
      var added=Shapes.joinUnoptimized(old,dirt,BooleanOp.ONLY_SECOND).move(pos);
      var rows=new JsonArray();
      for(var entry:source) {
        var a=entry.getAsJsonArray();
        var box=new AABB(a.get(0).getAsDouble(),a.get(1).getAsDouble(),a.get(2).getAsDouble(),a.get(3).getAsDouble(),a.get(4).getAsDouble(),a.get(5).getAsDouble());
        if(!box.intersects(added.bounds())) rows.add(JsonNull.INSTANCE);
        else rows.add(1.0+Shapes.collide(Direction.Axis.Y,box.move(0,1,0),List.of(added),-1.0));
      }
      out.add(rows);
    }
    Files.writeString(Path.of(args[1]),out.toString());
  }
}
"#).unwrap();
    let block = IVec3::new(10, 64, -12);
    let mut boxes = Vec::new();
    for x in [-0.31, -0.125, 0.0, 0.5, 1.0, 1.125] {
        for z in [-0.31, 0.0, 0.5, 1.0] {
            for y in [-0.5, 0.0, 0.9, 0.9375, 0.93750001, 0.96875, 0.99999999, 1.0] {
                for (width, height) in [(0.25, 0.25), (0.6, 1.8), (0.98, 0.98)] {
                    let min = block.as_dvec3() + DVec3::new(x, y, z);
                    boxes.push(BoundingBox {
                        min,
                        max: min + DVec3::new(width, height, width),
                    });
                }
            }
        }
    }
    let input = temp.path().join("boxes.json");
    let output = temp.path().join("results.json");
    let values: Vec<_> = boxes
        .iter()
        .map(|b| [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z])
        .collect();
    std::fs::write(&input, serde_json::to_vec(&values).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SoilPushOracle")
        .arg(&input)
        .arg(&output));
    let rows: Vec<Vec<Option<f64>>> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for block_rows in rows {
        assert_eq!(block_rows.len(), boxes.len());
        for (index, (bounds, expected)) in boxes.iter().zip(block_rows).enumerate() {
            let actual = soil_expansion_push(*bounds, block);
            match (actual, expected) {
                (None, None) => {}
                (Some(a), Some(b)) if (a - b).abs() < 1.0e-10 => {}
                _ => panic!("case {index}: {bounds:?} native {actual:?}, Java {expected:?}"),
            }
        }
    }
    println!(
        "Compared {} soil expansion boxes against both Java block shapes",
        boxes.len()
    );
}

fn same_volume(actual: &[Aabb], expected: &serde_json::Value) -> bool {
    let a: Vec<[f64; 6]> = actual
        .iter()
        .map(|box_| {
            [
                box_.min.x as f64,
                box_.min.y as f64,
                box_.min.z as f64,
                box_.max.x as f64,
                box_.max.y as f64,
                box_.max.z as f64,
            ]
        })
        .collect();
    let b: Vec<[f64; 6]> = expected
        .as_array()
        .unwrap()
        .iter()
        .map(|box_| {
            let values = box_.as_array().unwrap();
            std::array::from_fn(|i| values[i].as_f64().unwrap())
        })
        .collect();
    if a.is_empty() || b.is_empty() {
        return a.is_empty() && b.is_empty();
    }
    let cuts: [Vec<f64>; 3] = std::array::from_fn(|axis| {
        let mut values: Vec<f64> = a
            .iter()
            .chain(&b)
            .flat_map(|box_| [box_[axis], box_[axis + 3]])
            .collect();
        values.sort_by(f64::total_cmp);
        values.dedup_by(|x, y| (*x - *y).abs() < 0.000001);
        values
    });
    let intervals: [Vec<f64>; 3] = std::array::from_fn(|axis| {
        cuts[axis]
            .windows(2)
            .map(|pair| (pair[0] + pair[1]) / 2.0)
            .collect()
    });
    let inside = |boxes: &[[f64; 6]], point: [f64; 3]| {
        boxes
            .iter()
            .any(|box_| (0..3).all(|axis| point[axis] > box_[axis] && point[axis] < box_[axis + 3]))
    };
    intervals[0].iter().all(|&x| {
        intervals[1].iter().all(|&y| {
            intervals[2].iter().all(|&z| {
                let point = [x, y, z];
                inside(&a, point) == inside(&b, point)
            })
        })
    })
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn implemented_collision_shapes_match_vanilla_states() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = verified.vanilla_block_ids().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("CollisionOracle.java");
    let output = temp.path().join("collision.json");
    std::fs::write(&source, ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("CollisionOracle")
        .arg(&output));
    let records: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut checked = 0usize;
    let mut outlines_checked = 0usize;
    let mut button_states_checked = 0usize;
    let mut lever_states_checked = 0usize;
    let mut single_chest_states_checked = 0usize;
    let mut double_chest_states_checked = 0usize;
    let mut sturdy_checked = 0usize;
    let mut fire_checked = 0usize;
    let mut plant_checked = 0usize;
    let mut liquid_checked = 0usize;
    let mut soil_cover_checked = 0usize;
    let mut partial_support_checked = 0usize;
    let mut snow_checked = 0usize;
    let mut failures = Vec::new();
    for entry in records.as_array().unwrap() {
        let id = entry["id"].as_str().unwrap();
        if matches!(id, "minecraft:water" | "minecraft:lava") {
            assert_eq!(entry["fallingFree"].as_bool(), Some(true), "{id}");
            assert_eq!(entry["landingReplaceable"].as_bool(), Some(true), "{id}");
            liquid_checked += 1;
        }
        if matches!(id, "minecraft:fire" | "minecraft:soul_fire") {
            assert_eq!(entry["replaceable"].as_bool(), Some(true), "{id}");
            fire_checked += 1;
        }
        if matches!(
            id,
            "minecraft:short_grass" | "minecraft:fern" | "minecraft:dead_bush"
        ) {
            assert_eq!(entry["fallingReplaceable"].as_bool(), Some(true), "{id}");
            plant_checked += 1;
        }
        let properties = entry["properties"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().unwrap().to_owned()))
            .collect::<BTreeMap<_, _>>();
        let state = BlockState {
            name: id.to_owned(),
            properties,
        };
        if id == "minecraft:snow" {
            let layers = native_world::collision::snow_layers(&state).unwrap();
            assert_eq!(entry["fallingFree"].as_bool(), Some(true), "{state:?}");
            assert_eq!(
                entry["landingReplaceable"].as_bool(),
                Some(layers == 1),
                "{state:?}"
            );
            assert_eq!(
                entry["buttonReplaceable"].as_bool(),
                Some(layers == 1),
                "{state:?}"
            );
            snow_checked += 1;
        }
        if id == "minecraft:lever" {
            assert!(
                block_shape(&state).is_some() && block_outline(&state).is_some(),
                "{state:?}"
            );
            lever_states_checked += 1;
        }
        if id.ends_with("_slab") || id.ends_with("_stairs") {
            assert!(
                registry.contains(id),
                "slab or stair missing from local registry: {id}"
            );
            assert_eq!(entry["slab_stair"].as_bool(), Some(true), "{id}");
            assert_eq!(
                slab_stair_solid(&state),
                entry["solid"].as_bool(),
                "{state:?}"
            );
            soil_cover_checked += 1;
        }
        let Some(actual) = block_shape(&state) else {
            continue;
        };
        if matches!(id, "minecraft:chest" | "minecraft:trapped_chest")
            && state
                .properties
                .get("type")
                .is_some_and(|value| value == "single")
        {
            single_chest_states_checked += 1;
        }
        if matches!(id, "minecraft:chest" | "minecraft:trapped_chest")
            && matches!(
                state.properties.get("type").map(String::as_str),
                Some("left" | "right")
            )
        {
            double_chest_states_checked += 1;
            let direction = native_world::collision::chest_connection(&state).unwrap();
            let expected = match entry["chestDirection"].as_str().unwrap() {
                "north" => glam::IVec3::NEG_Z,
                "south" => glam::IVec3::Z,
                "west" => glam::IVec3::NEG_X,
                "east" => glam::IVec3::X,
                other => panic!("invalid chest direction {other}"),
            };
            assert_eq!(direction, expected, "{state:?}");
            assert_eq!(
                entry["chestOrder"].as_str(),
                Some(if state.properties["type"] == "right" {
                    "FIRST"
                } else {
                    "SECOND"
                }),
                "{state:?}"
            );
        }
        let short_id = id.strip_prefix("minecraft:").unwrap();
        if matches!(
            short_id,
            "dirt_path"
                | "farmland"
                | "straw_bed"
                | "honey_block"
                | "flower_pot"
                | "glass_pane"
                | "iron_bars"
        ) || short_id.starts_with("potted_")
            || short_id.ends_with("_leaves")
            || short_id.ends_with("_fence")
            || short_id.ends_with("_bed")
            || short_id.ends_with("_stained_glass_pane")
        {
            assert_eq!(
                entry["buttonReplaceable"].as_bool(),
                Some(false),
                "{state:?}"
            );
            assert!(!entry["dynamic"].as_bool().unwrap(), "{state:?}");
            partial_support_checked += 1;
        }
        if id.ends_with("_button") {
            assert!(
                block_outline(&state).is_some(),
                "missing button outline: {state:?}"
            );
            button_states_checked += 1;
        }
        checked += 1;
        if !actual.is_empty() && id != "minecraft:snow" {
            assert_eq!(
                entry["landingReplaceable"].as_bool(),
                Some(false),
                "landing target {id} {:?}",
                state.properties
            );
            assert_eq!(
                entry["fallingFree"].as_bool(),
                Some(false),
                "nonempty collision {id} {:?}",
                state.properties
            );
        }
        if entry["dynamic"].as_bool().unwrap() {
            if failures.len() < 20 {
                failures.push(format!("{id} has a dynamic collision shape"));
            }
            continue;
        }
        let expected = &entry["boxes"];
        if let Some(sturdy) = entry["sturdy"].as_array() {
            for (direction, expected) in [
                glam::IVec3::NEG_Y,
                glam::IVec3::Y,
                glam::IVec3::NEG_Z,
                glam::IVec3::Z,
                glam::IVec3::NEG_X,
                glam::IVec3::X,
            ]
            .into_iter()
            .zip(sturdy)
            {
                let Some(native) = native_world::collision::button_support_face(&state, direction)
                else {
                    continue;
                };
                if native != expected.as_bool().unwrap() && failures.len() < 20 {
                    failures.push(format!(
                        "{id} {:?} sturdy {direction:?}: {native} vs {expected}",
                        state.properties
                    ));
                }
                sturdy_checked += 1;
            }
        }
        if !same_volume(&actual, expected) && failures.len() < 20 {
            failures.push(format!(
                "{id} {:?}: native {actual:?}, Java {expected:?}",
                state.properties
            ));
        }
        let spawn_shape = spawn_collision_shape(&state).unwrap();
        if !same_volume(&spawn_shape, &entry["spawnBoxes"]) && failures.len() < 20 {
            failures.push(format!(
                "{id} {:?}: spawn context {spawn_shape:?}, Java {}",
                state.properties, entry["spawnBoxes"]
            ));
        }
        if let Some(outline) = block_outline(&state) {
            outlines_checked += 1;
            if !same_volume(&outline, &entry["outline"]) && failures.len() < 20 {
                failures.push(format!(
                    "{id} {:?}: native outline {outline:?}, Java {}",
                    state.properties, entry["outline"]
                ));
            }
        }
    }
    assert!(checked > 1000, "only {checked} states were checked");
    assert_eq!(snow_checked, 8);
    assert!(
        soil_cover_checked > 1000,
        "only {soil_cover_checked} slab and stair states"
    );
    assert!(
        sturdy_checked > 1000,
        "slab and stair support states missing"
    );
    assert!(
        partial_support_checked > 1000,
        "additional support families missing"
    );
    assert!(
        button_states_checked >= 24,
        "button states missing from Java oracle"
    );
    assert_eq!(
        lever_states_checked, 24,
        "lever states missing from Java oracle"
    );
    assert_eq!(
        single_chest_states_checked, 16,
        "single chest states missing from Java oracle"
    );
    assert_eq!(
        double_chest_states_checked, 32,
        "double chest states missing from Java oracle"
    );
    assert!(fire_checked >= 2, "fire states missing from Java oracle");
    assert!(plant_checked >= 3, "plant states missing from Java oracle");
    assert!(
        liquid_checked >= 32,
        "liquid states missing from Java oracle"
    );
    assert!(
        failures.is_empty(),
        "collision mismatches (first 20):\n{}",
        failures.join("\n")
    );
    println!(
        "Compared {checked} collision and {outlines_checked} targeting shapes, including {button_states_checked} button states, {sturdy_checked} support faces, {partial_support_checked} extra support states and {soil_cover_checked} slab/stair solidity states, against Minecraft 26.3"
    );
}
