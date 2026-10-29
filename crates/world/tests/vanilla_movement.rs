//! Compare block collision and step-up resolution with the local 26.3 Entity.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use glam::DVec3;
use native_world::{
    collision::block_shape,
    loaded::BlockState,
    movement::{BoundingBox, CollisionShape, resolve_movement},
};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.*;

public class MovementOracle {
    static final class TestLevel extends ClientLevel {
        List<VoxelShape> shapes;
        List<Boolean> fullBlocks;
        WorldBorder border;
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public WorldBorder getWorldBorder() { return border; }
        @Override public List<VoxelShape> getEntityCollisions(Entity e, AABB box) { return List.of(); }
        @Override public Iterable<VoxelShape> getBlockCollisions(Entity e, AABB box) {
            VoxelShape query = Shapes.create(box);
            List<VoxelShape> result = new ArrayList<>();
            for (int i = 0; i < shapes.size(); i++) {
                VoxelShape shape = shapes.get(i);
                if (fullBlocks.get(i) ? shape.bounds().intersects(box) : Shapes.joinIsNotEmpty(shape, query, BooleanOp.AND)) result.add(shape);
            }
            return result;
        }
    }
    static final class TestEntity extends Entity {
        boolean grounded;
        float step;
        TestEntity() { super(null, null); }
        void setup(TestLevel level, AABB box, boolean onGround, float stepHeight) {
            setLevel(level); setBoundingBox(box); grounded = onGround; step = stepHeight;
        }
        @Override public boolean onGround() { return grounded; }
        @Override public float maxUpStep() { return step; }
        @Override protected void defineSynchedData(net.minecraft.network.syncher.SynchedEntityData.Builder builder) {}
        @Override protected void readAdditionalSaveData(net.minecraft.world.level.storage.ValueInput value) {}
        @Override protected void addAdditionalSaveData(net.minecraft.world.level.storage.ValueOutput value) {}
        @Override public boolean hurtServer(net.minecraft.server.level.ServerLevel level, net.minecraft.world.damagesource.DamageSource source, float amount) { return false; }
    }
    static AABB box(JsonArray v) {
        return new AABB(v.get(0).getAsDouble(), v.get(1).getAsDouble(), v.get(2).getAsDouble(), v.get(3).getAsDouble(), v.get(4).getAsDouble(), v.get(5).getAsDouble());
    }
    @SuppressWarnings({"unchecked", "rawtypes"})
    static BlockState property(BlockState state, String name, String value) {
        Property property = state.getBlock().getStateDefinition().getProperty(name);
        return state.setValue(property, (Comparable) property.getValue(value).orElseThrow());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) f.get(null);
        TestLevel level = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        level.border = new WorldBorder() { @Override public boolean isInsideCloseToBorder(Entity e, AABB b) { return false; } };
        TestEntity entity = (TestEntity) unsafe.allocateInstance(TestEntity.class);
        Method collide = Entity.class.getDeclaredMethod("collide", Vec3.class); collide.setAccessible(true);
        JsonArray input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray output = new JsonArray();
        for (var entry : input) {
            var item = entry.getAsJsonObject();
            level.shapes = new ArrayList<>(); level.fullBlocks = new ArrayList<>();
            for (var group : item.getAsJsonArray("shapes")) {
                if (group.isJsonObject()) {
                    var spec = group.getAsJsonObject();
                    BlockState state = BuiltInRegistries.BLOCK.getValue(Identifier.parse(spec.get("id").getAsString())).defaultBlockState();
                    for (var prop : spec.getAsJsonObject("properties").entrySet()) state = property(state, prop.getKey(), prop.getValue().getAsString());
                    var p = spec.getAsJsonArray("pos");
                    var pos = new BlockPos(p.get(0).getAsInt(), p.get(1).getAsInt(), p.get(2).getAsInt());
                    VoxelShape shape = state.getCollisionShape(EmptyBlockGetter.INSTANCE, pos);
                    level.fullBlocks.add(shape == Shapes.block()); level.shapes.add(shape.move(pos));
                } else {
                    VoxelShape shape = Shapes.empty();
                    for (var b : group.getAsJsonArray()) shape = Shapes.or(shape, Shapes.create(box(b.getAsJsonArray())));
                    level.shapes.add(shape);
                    AABB b = group.getAsJsonArray().size() == 1 ? box(group.getAsJsonArray().get(0).getAsJsonArray()) : null;
                    level.fullBlocks.add(b != null && b.maxX - b.minX == 1.0 && b.maxY - b.minY == 1.0 && b.maxZ - b.minZ == 1.0 && b.minX == Math.floor(b.minX) && b.minY == Math.floor(b.minY) && b.minZ == Math.floor(b.minZ));
                }
            }
            var m = item.getAsJsonArray("movement");
            Vec3 movement = new Vec3(m.get(0).getAsDouble(), m.get(1).getAsDouble(), m.get(2).getAsDouble());
            entity.setup(level, box(item.getAsJsonArray("bounds")), item.get("on_ground").getAsBoolean(), item.get("step").getAsFloat());
            Vec3 result = (Vec3) collide.invoke(entity, movement);
            JsonArray vector = new JsonArray(); vector.add(result.x); vector.add(result.y); vector.add(result.z); output.add(vector);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
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
        "movement oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn bounds(min: [f64; 3], max: [f64; 3]) -> BoundingBox {
    BoundingBox {
        min: min.into(),
        max: max.into(),
    }
}

fn values(b: BoundingBox) -> [f64; 6] {
    [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z]
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn swept_box_contacts_match_java_aabb() {
    const JAVA: &str = r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;
public class SweptContactOracle {
  static AABB box(JsonArray a) { return new AABB(a.get(0).getAsDouble(),a.get(1).getAsDouble(),a.get(2).getAsDouble(),a.get(3).getAsDouble(),a.get(4).getAsDouble(),a.get(5).getAsDouble()); }
  public static void main(String[] args) throws Exception {
    var rows=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var out=new JsonArray();
    for(var v:rows) { var row=v.getAsJsonObject(); var d=row.getAsJsonArray("delta");
      out.add(box(row.getAsJsonArray("box")).collidedAlongVector(new Vec3(d.get(0).getAsDouble(),d.get(1).getAsDouble(),d.get(2).getAsDouble()),List.of(box(row.getAsJsonArray("target"))))); }
    Files.writeString(Path.of(args[1]),new Gson().toJson(out));
  }
}
"#;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut cases = Vec::new();
    let target = bounds([0.0, 0.0, 0.0], [1.0, 0.875, 1.0]);
    for x in [-1.5, -0.125, -0.1249999, 0.5, 1.1249999, 1.125, 2.0] {
        for y in [-0.25, 0.0, 0.8749999, 0.875, 1.2] {
            for dx in [-2.5, -0.2, 0.0, 0.2, 2.5] {
                let box_ = bounds([x - 0.125, y, 0.375], [x + 0.125, y + 0.25, 0.625]);
                let delta = DVec3::new(dx, 0.0, 0.0);
                cases.push((box_, delta, target));
            }
        }
    }
    let mut seed = 0x9e3779b97f4a7c15_u64;
    let mut sample = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((seed >> 32) as u32 as f64 / f64::from(u32::MAX)) * 6.0 - 3.0
    };
    for _ in 0..600 {
        let center = DVec3::new(sample(), sample(), sample());
        let box_ = BoundingBox {
            min: center - DVec3::splat(0.125),
            max: center + DVec3::splat(0.125),
        };
        let delta = DVec3::new(sample(), sample(), sample());
        cases.push((box_, delta, target));
    }
    let input: Vec<_> = cases.iter().map(|(box_, delta, target)| json!({"box":values(*box_), "delta":delta.to_array(), "target":values(*target)})).collect();
    let source = temp.path().join("SweptContactOracle.java");
    let input_path = temp.path().join("input.json");
    let output_path = temp.path().join("output.json");
    std::fs::write(&source, JAVA).unwrap();
    std::fs::write(&input_path, serde_json::to_vec(&input).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SweptContactOracle")
        .arg(&input_path)
        .arg(&output_path));
    let expected: Vec<bool> = serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (index, (box_, delta, target)) in cases.into_iter().enumerate() {
        assert_eq!(
            box_.swept_contact_time(delta, target).is_some(),
            expected[index],
            "case {index}: {box_:?} {delta:?}"
        );
    }
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
fn block_collision_and_step_resolution_match_vanilla_entity() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut seed = 0x5329_1ca3_fca4_068bu64;
    let mut rand = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 32) as u32
    };
    let mut input = Vec::new();
    let mut scenarios = Vec::new();
    let block_states = [
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
    ];
    for i in 0..8192 {
        let coordinate = |v: u32| f64::from(v % 25) / 8.0 - 1.0;
        let pos = DVec3::new(
            coordinate(rand()),
            f64::from(rand() % 13) / 8.0,
            coordinate(rand()),
        );
        let player = BoundingBox {
            min: pos + DVec3::new(-0.3, 0.0, -0.3),
            max: pos + DVec3::new(0.3, 1.8, 0.3),
        };
        let movement = DVec3::new(
            f64::from(rand() % 17) / 10.0 - 0.8,
            f64::from(rand() % 9) / 10.0 - 0.4,
            f64::from(rand() % 17) / 10.0 - 0.8,
        );
        let step = if i % 7 == 0 { 0.0 } else { 0.6 };
        let on_ground = i % 3 != 0;
        let floor = vec![bounds([-3.0, -1.0, -3.0], [4.0, 0.0, 4.0])];
        let mut groups = vec![json!(floor.iter().map(|b| values(*b)).collect::<Vec<_>>())];
        let mut shapes = vec![CollisionShape::new(floor)];
        for _ in 0..5 {
            let x = f64::from(rand() % 5) - 1.0;
            let y = if i < 4096 {
                f64::from(rand() % 3) * 0.5
            } else {
                f64::from(rand() % 3)
            };
            let z = f64::from(rand() % 5) - 1.0;
            if i < 4096 {
                let height = if rand() % 2 == 0 { 0.5 } else { 1.0 };
                let mut boxes = vec![bounds([x, y, z], [x + 1.0, y + height, z + 1.0])];
                if height == 0.5 && rand() % 2 == 0 {
                    boxes.push(bounds([x, y + 0.5, z], [x + 0.5, y + 1.0, z + 1.0]));
                }
                groups.push(json!(boxes.iter().map(|b| values(*b)).collect::<Vec<_>>()));
                shapes.push(CollisionShape::new(boxes));
            } else {
                let state = &block_states[rand() as usize % block_states.len()];
                let offset = DVec3::new(x, y, z);
                let boxes = block_shape(state)
                    .unwrap()
                    .iter()
                    .map(|b| BoundingBox {
                        min: offset + b.min.as_dvec3(),
                        max: offset + b.max.as_dvec3(),
                    })
                    .collect();
                groups.push(json!({"id":state.name,"properties":state.properties,"pos":[x as i32,y as i32,z as i32]}));
                shapes.push(CollisionShape::new(boxes));
            }
        }
        input.push(json!({"bounds":values(player),"movement":movement.to_array(),"on_ground":on_ground,"step":step,"shapes":groups }));
        scenarios.push((player, movement, on_ground, step, shapes));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("MovementOracle.java");
    let input_path = temp.path().join("input.json");
    let output_path = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input_path, serde_json::to_vec(&input).unwrap()).unwrap();
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
        .arg("MovementOracle")
        .arg(&input_path)
        .arg(&output_path));
    let results: Value = serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    assert_eq!(results.as_array().unwrap().len(), scenarios.len());
    let mut failures = Vec::new();
    let mut changed = [0usize; 2];
    let mut stepped = [0usize; 2];
    for (i, ((player, movement, on_ground, step, shapes), expected)) in scenarios
        .iter()
        .zip(results.as_array().unwrap())
        .enumerate()
    {
        let actual = resolve_movement(*player, *movement, *on_ground, *step, shapes);
        if actual != *movement {
            changed[i / 4096] += 1;
        }
        if actual.y > 0.0 && actual.y > movement.y {
            stepped[i / 4096] += 1;
        }
        let e = expected.as_array().unwrap();
        let expected = DVec3::new(
            e[0].as_f64().unwrap(),
            e[1].as_f64().unwrap(),
            e[2].as_f64().unwrap(),
        );
        if (actual - expected).abs().max_element() > 1.0e-6 && failures.len() < 20 {
            failures.push(format!(
                "case {i}: {}, native {actual:?}, Java {expected:?}",
                input[i]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "movement mismatches (first 20):\n{}",
        failures.join("\n")
    );
    assert!(
        changed.iter().all(|count| *count > 100) && stepped.iter().all(|count| *count > 10),
        "insufficient collisions or steps: {changed:?}, {stepped:?}"
    );
    println!(
        "Compared {} movement and step-up cases with Minecraft 26.3 Entity; collisions {changed:?}, upward results {stepped:?}",
        scenarios.len()
    );
}
