//! Compare shape clipping with the local 26.3 VoxelShape implementation.

use glam::{DVec3, IVec3, Vec3};
use native_world::{
    collision::block_outline,
    loaded::BlockState,
    raycast::{clip_shape, raycast_shapes},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.phys.Vec3;
public class RayOracle {
    @SuppressWarnings({"unchecked", "rawtypes"})
    static BlockState property(BlockState state, String name, String value) {
        Property property = state.getBlock().getStateDefinition().getProperty(name);
        return state.setValue(property, (Comparable)property.getValue(value).orElseThrow());
    }
    static Vec3 vec(JsonArray a) { return new Vec3(a.get(0).getAsDouble(), a.get(1).getAsDouble(), a.get(2).getAsDouble()); }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (var item : cases) {
            var test = item.getAsJsonObject();
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(test.get("id").getAsString()));
            BlockState state = block.defaultBlockState();
            for (var entry : test.getAsJsonObject("properties").entrySet()) state = property(state, entry.getKey(), entry.getValue().getAsString());
            var p = test.getAsJsonArray("cell");
            var pos = new BlockPos(p.get(0).getAsInt(), p.get(1).getAsInt(), p.get(2).getAsInt());
            var hit = state.getShape(EmptyBlockGetter.INSTANCE, pos).clip(vec(test.getAsJsonArray("start")), vec(test.getAsJsonArray("end")), pos);
            if (hit == null) { results.add(JsonNull.INSTANCE); continue; }
            var record = new JsonObject();
            var face = new JsonArray();
            face.add(hit.getDirection().getStepX()); face.add(hit.getDirection().getStepY()); face.add(hit.getDirection().getStepZ());
            var point = new JsonArray();
            point.add(hit.getLocation().x); point.add(hit.getLocation().y); point.add(hit.getLocation().z);
            record.add("face", face); record.add("point", point); record.addProperty("inside", hit.isInside()); results.add(record);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(results));
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
    let manifest: Value =
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
        "ray oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn outline_segment_clipping_matches_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let cases = [
        state("minecraft:stone", &[]),
        state("minecraft:oak_slab", &[("type", "bottom")]),
        state("minecraft:oak_slab", &[("type", "top")]),
        state(
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "straight"),
            ],
        ),
        state(
            "minecraft:oak_stairs",
            &[("facing", "east"), ("half", "top"), ("shape", "inner_left")],
        ),
        state(
            "minecraft:oak_stairs",
            &[
                ("facing", "south"),
                ("half", "bottom"),
                ("shape", "outer_right"),
            ],
        ),
        state(
            "minecraft:oak_fence",
            &[
                ("north", "true"),
                ("south", "false"),
                ("east", "true"),
                ("west", "false"),
            ],
        ),
        state(
            "minecraft:glass_pane",
            &[
                ("north", "true"),
                ("south", "true"),
                ("east", "false"),
                ("west", "false"),
            ],
        ),
        state("minecraft:flower_pot", &[]),
        state("minecraft:short_grass", &[]),
        state("minecraft:oak_sapling", &[]),
        state("minecraft:torch", &[]),
        state("minecraft:wall_torch", &[("facing", "north")]),
        state(
            "minecraft:stone_button",
            &[("face", "wall"), ("facing", "north"), ("powered", "false")],
        ),
        state(
            "minecraft:stone_button",
            &[("face", "floor"), ("facing", "east"), ("powered", "true")],
        ),
        state(
            "minecraft:oak_button",
            &[
                ("face", "ceiling"),
                ("facing", "west"),
                ("powered", "false"),
            ],
        ),
        state(
            "minecraft:vine",
            &[
                ("up", "true"),
                ("north", "true"),
                ("south", "false"),
                ("east", "false"),
                ("west", "true"),
            ],
        ),
    ];
    let mut inputs = Vec::new();
    let mut seed = 0x4a63_7661_3236_3301u64;
    let mut rand = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 32) % 41) as f64 / 10.0 - 1.5
    };
    for index in 0..4096 {
        let state = &cases[index % cases.len()];
        let cell = if (index / cases.len()).is_multiple_of(2) {
            IVec3::ZERO
        } else {
            IVec3::new(-3, 4, 2)
        };
        let base = cell.as_dvec3();
        let start = base + DVec3::new(rand(), rand(), rand());
        let end = base + DVec3::new(rand(), rand(), rand());
        inputs.push(json!({"id":state.name,"properties":state.properties,"cell":[cell.x,cell.y,cell.z],"start":start.to_array(),"end":end.to_array()}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RayOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&inputs).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("RayOracle")
        .arg(&input)
        .arg(&output));
    let results: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut hits = 0usize;
    let mut failures = Vec::new();
    for (index, (input, expected)) in inputs.iter().zip(results.as_array().unwrap()).enumerate() {
        let state = &cases[index % cases.len()];
        let arr = |key: &str| {
            let v = input[key].as_array().unwrap();
            DVec3::new(
                v[0].as_f64().unwrap(),
                v[1].as_f64().unwrap(),
                v[2].as_f64().unwrap(),
            )
        };
        let cell = arr("cell").as_ivec3();
        let actual = clip_shape(
            &block_outline(state).unwrap(),
            cell,
            arr("start"),
            arr("end"),
        );
        let mismatch = if expected.is_null() {
            actual.is_some()
        } else if let Some(actual) = actual {
            hits += 1;
            let face = expected["face"].as_array().unwrap();
            let point = expected["point"].as_array().unwrap();
            let expected_face = IVec3::new(
                face[0].as_i64().unwrap() as i32,
                face[1].as_i64().unwrap() as i32,
                face[2].as_i64().unwrap() as i32,
            );
            let expected_point = DVec3::new(
                point[0].as_f64().unwrap(),
                point[1].as_f64().unwrap(),
                point[2].as_f64().unwrap(),
            );
            actual.face != expected_face
                || actual.inside != expected["inside"].as_bool().unwrap()
                || (actual.location - expected_point).abs().max_element() > 1.0e-6
        } else {
            true
        };
        if mismatch && failures.len() < 20 {
            failures.push(format!(
                "case {index}: {input}, native {actual:?}, Java {expected}"
            ));
        }
    }
    assert!(hits > 100, "only {hits} rays hit a shape");
    assert!(
        failures.is_empty(),
        "ray mismatches (first 20):\n{}",
        failures.join("\n")
    );
    println!(
        "Compared {} rays, including {hits} hits, against Minecraft 26.3",
        inputs.len()
    );
}

const WORLD_ORACLE: &str = r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.ClipContext;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.phys.HitResult;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.CollisionContext;
public class WorldRayOracle implements BlockGetter {
    final Map<BlockPos, BlockState> blocks = new HashMap<>();
    public int getHeight() { return 384; }
    public int getMinY() { return -64; }
    public BlockEntity getBlockEntity(BlockPos p) { return null; }
    public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p, Blocks.AIR.defaultBlockState()); }
    public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
    @SuppressWarnings({"unchecked", "rawtypes"})
    static BlockState property(BlockState state, String name, String value) {
        Property property = state.getBlock().getStateDefinition().getProperty(name);
        return state.setValue(property, (Comparable)property.getValue(value).orElseThrow());
    }
    static Vec3 vec(JsonArray a) { return new Vec3(a.get(0).getAsDouble(), a.get(1).getAsDouble(), a.get(2).getAsDouble()); }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonObject input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
        WorldRayOracle world = new WorldRayOracle();
        for (var entry : input.getAsJsonArray("blocks")) {
            var item = entry.getAsJsonObject();
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(item.get("id").getAsString()));
            BlockState state = block.defaultBlockState();
            for (var property : item.getAsJsonObject("properties").entrySet()) state = property(state, property.getKey(), property.getValue().getAsString());
            var p = item.getAsJsonArray("pos");
            world.blocks.put(new BlockPos(p.get(0).getAsInt(), p.get(1).getAsInt(), p.get(2).getAsInt()), state);
        }
        JsonArray output = new JsonArray();
        for (var entry : input.getAsJsonArray("rays")) {
            var ray = entry.getAsJsonObject();
            var context = new ClipContext(vec(ray.getAsJsonArray("start")), vec(ray.getAsJsonArray("end")), ClipContext.Block.OUTLINE, ClipContext.Fluid.NONE, CollisionContext.empty());
            var hit = world.clip(context);
            if (hit.getType() != HitResult.Type.BLOCK) { output.add(JsonNull.INSTANCE); continue; }
            var result = new JsonObject(); var pos = new JsonArray(); var face = new JsonArray(); var point = new JsonArray();
            pos.add(hit.getBlockPos().getX()); pos.add(hit.getBlockPos().getY()); pos.add(hit.getBlockPos().getZ());
            face.add(hit.getDirection().getStepX()); face.add(hit.getDirection().getStepY()); face.add(hit.getDirection().getStepZ());
            point.add(hit.getLocation().x); point.add(hit.getLocation().y); point.add(hit.getLocation().z);
            result.add("pos", pos); result.add("face", face); result.add("point", point); result.addProperty("inside", hit.isInside()); output.add(result);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
"#;

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn world_traversal_and_hit_faces_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let blocks = [
        (IVec3::ZERO, state("minecraft:stone", &[])),
        (
            IVec3::new(0, 0, 1),
            state("minecraft:oak_slab", &[("type", "bottom")]),
        ),
        (
            IVec3::new(1, 0, 1),
            state(
                "minecraft:oak_stairs",
                &[
                    ("facing", "north"),
                    ("half", "bottom"),
                    ("shape", "inner_left"),
                ],
            ),
        ),
        (
            IVec3::new(-1, 0, 0),
            state(
                "minecraft:oak_fence",
                &[
                    ("north", "true"),
                    ("south", "false"),
                    ("east", "true"),
                    ("west", "false"),
                ],
            ),
        ),
        (
            IVec3::new(1, 1, -1),
            state(
                "minecraft:glass_pane",
                &[
                    ("north", "true"),
                    ("south", "true"),
                    ("east", "false"),
                    ("west", "false"),
                ],
            ),
        ),
        (IVec3::new(0, 1, 0), state("minecraft:flower_pot", &[])),
        (
            IVec3::new(-2, -1, 1),
            state("minecraft:oak_slab", &[("type", "top")]),
        ),
    ];
    let outlines: HashMap<IVec3, Vec<_>> = blocks
        .iter()
        .map(|(pos, state)| (*pos, block_outline(state).unwrap()))
        .collect();
    let data: Vec<_> = blocks.iter().map(|(pos,state)| json!({"id":state.name,"properties":state.properties,"pos":[pos.x,pos.y,pos.z]})).collect();
    let mut seed = 0x7a8b_9263_d1f4_e507u64;
    let mut rand = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 32) % 49) as f32 / 8.0 - 3.0
    };
    let mut rays = Vec::new();
    for _ in 0..4096 {
        let start = Vec3::new(rand(), rand(), rand());
        let target = Vec3::new(rand(), rand(), rand());
        let delta = target - start;
        if delta.length_squared() < 0.01 {
            continue;
        }
        let distance = delta.length();
        let direction = delta;
        let end = start.as_dvec3() + direction.normalize().as_dvec3() * f64::from(distance);
        rays.push(json!({"start":start.to_array(),"end":end.to_array(),"dir":direction.to_array(),"distance":distance}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("WorldRayOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, WORLD_ORACLE).unwrap();
    std::fs::write(
        &input,
        serde_json::to_vec(&json!({"blocks":data,"rays":rays})).unwrap(),
    )
    .unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("WorldRayOracle")
        .arg(&input)
        .arg(&output));
    let results: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut hits = 0usize;
    let mut failures = Vec::new();
    for (index, (ray, expected)) in rays.iter().zip(results.as_array().unwrap()).enumerate() {
        let f32vec = |key: &str| {
            let v = ray[key].as_array().unwrap();
            Vec3::new(
                v[0].as_f64().unwrap() as f32,
                v[1].as_f64().unwrap() as f32,
                v[2].as_f64().unwrap() as f32,
            )
        };
        let actual = raycast_shapes(
            f32vec("start"),
            f32vec("dir"),
            ray["distance"].as_f64().unwrap() as f32,
            |pos| Some(outlines.get(&pos).map(Vec::as_slice).unwrap_or(&[])),
        );
        let mismatch = if expected.is_null() {
            actual.is_some()
        } else if let Some(actual) = actual {
            hits += 1;
            let v = |key: &str| {
                let a = expected[key].as_array().unwrap();
                IVec3::new(
                    a[0].as_i64().unwrap() as i32,
                    a[1].as_i64().unwrap() as i32,
                    a[2].as_i64().unwrap() as i32,
                )
            };
            let point = expected["point"].as_array().unwrap();
            let p = DVec3::new(
                point[0].as_f64().unwrap(),
                point[1].as_f64().unwrap(),
                point[2].as_f64().unwrap(),
            );
            actual.position != v("pos")
                || actual.face != v("face")
                || actual.inside != expected["inside"].as_bool().unwrap()
                || (actual.location - p).abs().max_element() > 1.0e-5
        } else {
            true
        };
        if mismatch && failures.len() < 20 {
            failures.push(format!(
                "case {index}: {ray}, native {actual:?}, Java {expected}"
            ));
        }
    }
    assert!(hits > 100, "only {hits} rays hit");
    assert!(
        failures.is_empty(),
        "world ray mismatches (first 20):\n{}",
        failures.join("\n")
    );
    println!(
        "Compared {} world rays, including {hits} hits, against Minecraft 26.3",
        rays.len()
    );
}
