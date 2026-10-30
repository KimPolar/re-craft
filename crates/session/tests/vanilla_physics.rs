//! Compare default and sprint movement math with the locally installed 26.3 player.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use glam::DVec3;
use native_session::physics::{MotionState, MovementInput, ground_friction};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.effect.MobEffects;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.ai.attributes.AttributeInstance;
import net.minecraft.world.entity.ai.attributes.AttributeMap;
import net.minecraft.world.entity.ai.attributes.AttributeModifier;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.Vec2;
import net.minecraft.world.phys.Vec3;

public class PhysicsOracle {
    static final class TestLevel extends ClientLevel {
        BlockState ground;
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public boolean isClientSide() { return false; }
        @Override public BlockState getBlockState(BlockPos pos) { return ground; }
    }
    static final class TestPlayer extends Player {
        Vec3 v, p;
        Vec3 moved;
        boolean grounded;
        boolean sprinting;
        boolean horizontal;
        float yaw;
        double wall;
        int delay;
        AttributeInstance speed;
        MobEffectInstance jumpBoost;
        TestPlayer() { super(null, null); }
        void setup(TestLevel world, Vec3 position, Vec3 velocity, boolean ground, float rotation, double wallX) {
            setLevel(world); p = position; v = velocity; grounded = ground; yaw = rotation; wall = wallX; delay = 0;
        }
        @Override public boolean onGround() { return grounded; }
        @Override public Vec3 getDeltaMovement() { return v; }
        @Override public void setDeltaMovement(Vec3 value) { v = value; }
        @Override public void setDeltaMovement(double x, double y, double z) { v = new Vec3(x, y, z); }
        @Override public float getYRot() { return yaw; }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override protected float getBlockJumpFactor() { return 1.0f; }
        @Override protected double getEffectiveGravity() { return 0.08; }
        @Override protected boolean omnidirectionalAirMover() { return false; }
        @Override public boolean shouldDiscardFriction() { return false; }
        @Override public boolean isSprinting() { return sprinting; }
        @Override public boolean onClimbable() { return false; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return attribute == Attributes.MOVEMENT_SPEED ? speed.getValue() : attribute == Attributes.JUMP_STRENGTH ? 0.42 : 1.0; }
        @Override public boolean hasEffect(Holder<MobEffect> effect) { return effect == MobEffects.JUMP_BOOST && jumpBoost != null; }
        @Override public MobEffectInstance getEffect(Holder<MobEffect> effect) { return effect == MobEffects.JUMP_BOOST ? jumpBoost : null; }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return BlockPos.ZERO; }
        @Override public HumanoidArm getMainArm() { return HumanoidArm.RIGHT; }
        @Override public void move(MoverType type, Vec3 requested) {
            double x = requested.x, y = requested.y, z = requested.z;
            if (y < 0.0 && p.y + y < 0.0) y = -p.y;
            if (x > 0.0 && p.x + 0.30000001192092896 + x > wall) x = Math.max(0.0, wall - p.x - 0.30000001192092896);
            p = p.add(x, y, z);
            boolean bx = Math.abs(x - requested.x) >= (double) 1.0e-5f;
            boolean bz = Math.abs(z - requested.z) >= (double) 1.0e-5f;
            moved = new Vec3(x,y,z);
            horizontal = bx || bz;
            boolean by = y != requested.y;
            grounded = by && requested.y < 0.0;
            v = new Vec3(bx ? 0.0 : v.x, by ? 0.0 : v.y, bz ? 0.0 : v.z);
        }
    }
    static final class TestLocal extends LocalPlayer {
        boolean slow, using;
        double sneakSpeed;
        TestLocal() { super(null, null, null, null, null, null, false, null, null); }
        @Override public boolean isMovingSlowly() { return slow; }
        @Override public boolean isUsingItem() { return using; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return attribute == Attributes.SNEAKING_SPEED ? sneakSpeed : 1.0; }
    }
    static Vec3 vec(JsonArray a) { return new Vec3(a.get(0).getAsDouble(), a.get(1).getAsDouble(), a.get(2).getAsDouble()); }
    static JsonArray vec(Vec3 a) { var out = new JsonArray(); out.add(a.x); out.add(a.y); out.add(a.z); return out; }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) f.get(null);
        TestLevel level = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        TestPlayer entity = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        Field abilities = Player.class.getDeclaredField("abilities"); abilities.setAccessible(true); abilities.set(entity, new Abilities());
        entity.speed = new AttributeMap(Player.createAttributes().build()).getInstance(Attributes.MOVEMENT_SPEED);
        Field modifierField = LivingEntity.class.getDeclaredField("SPEED_MODIFIER_SPRINTING"); modifierField.setAccessible(true);
        AttributeModifier modifier = (AttributeModifier) modifierField.get(null);
        Method travel = LivingEntity.class.getDeclaredMethod("travelInAir", Vec3.class); travel.setAccessible(true);
        Method modify = LocalPlayer.class.getDeclaredMethod("modifyInput", Vec2.class); modify.setAccessible(true);
        Method minor = LocalPlayer.class.getDeclaredMethod("isHorizontalCollisionMinor", Vec3.class); minor.setAccessible(true);
        TestLocal local = (TestLocal) unsafe.allocateInstance(TestLocal.class);
        Field useItem = LivingEntity.class.getDeclaredField("useItem"); useItem.setAccessible(true); useItem.set(local, new ItemStack(Items.APPLE));
        local.sneakSpeed = new AttributeMap(Player.createAttributes().build()).getValue(Attributes.SNEAKING_SPEED);
        JsonArray scenarios = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (var entry : scenarios) {
            var scenario = entry.getAsJsonObject();
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(scenario.get("block").getAsString()));
            level.ground = block.defaultBlockState();
            entity.setup(level, vec(scenario.getAsJsonArray("position")), vec(scenario.getAsJsonArray("velocity")), scenario.get("ground").getAsBoolean(), 0, scenario.get("wall").getAsDouble());
            int boost = scenario.get("boost").getAsInt();
            entity.jumpBoost = boost < 0 ? null : new MobEffectInstance(MobEffects.JUMP_BOOST, 1000, boost);
            JsonArray ticks = new JsonArray();
            for (var input : scenario.getAsJsonArray("inputs")) {
                var i = input.getAsJsonObject();
                entity.yaw = i.get("yaw").getAsFloat();
                entity.sprinting = i.get("sprint").getAsBoolean();
                entity.speed.removeModifier(modifier.id());
                if (entity.sprinting) entity.speed.addTransientModifier(modifier);
                if (entity.delay > 0) entity.delay--;
                double x = entity.v.x, y = entity.v.y, z = entity.v.z;
                if (x*x + z*z < 9.0e-6) { x = 0; z = 0; }
                if (Math.abs(y) < 0.003) y = 0;
                entity.v = new Vec3(x,y,z);
                boolean jump = i.get("jump").getAsBoolean();
                if (jump && entity.grounded && entity.delay == 0) { entity.jumpFromGround(); entity.delay = 10; }
                else if (!jump) entity.delay = 0;
                Vec2 key = new Vec2(-i.get("right").getAsFloat(), i.get("forward").getAsFloat()).normalized();
                local.slow = i.get("slow").getAsBoolean();
                local.using = i.get("using").getAsBoolean();
                Vec2 modified = (Vec2) modify.invoke(local, key);
                travel.invoke(entity, new Vec3(modified.x, 0, modified.y));
                local.setYRot(entity.yaw); local.xxa = modified.x; local.zza = modified.y;
                boolean isMinor = entity.horizontal && (boolean) minor.invoke(local, entity.moved);
                JsonObject tick = new JsonObject(); tick.add("position", vec(entity.p)); tick.add("velocity", vec(entity.v)); tick.addProperty("ground", entity.grounded); tick.addProperty("delay", entity.delay); tick.addProperty("horizontal", entity.horizontal); tick.addProperty("minor", isMinor); ticks.add(tick);
            }
            results.add(ticks);
        }
        JsonObject frictions = new JsonObject();
        for (var block : BuiltInRegistries.BLOCK) frictions.addProperty(BuiltInRegistries.BLOCK.getKey(block).toString(), block.getFriction());
        JsonObject output = new JsonObject(); output.add("results", results); output.add("frictions", frictions);
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
        "physics oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn vec(value: &Value) -> DVec3 {
    let v = value.as_array().unwrap();
    DVec3::new(
        v[0].as_f64().unwrap(),
        v[1].as_f64().unwrap(),
        v[2].as_f64().unwrap(),
    )
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn creative_flight_collision_matches_vanilla_air_movement() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("CreativeFlightOracle.java");
    std::fs::write(&source, include_str!("java/CreativeFlightOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = temp.path().join("flight.json");
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("CreativeFlightOracle")
        .arg(&output));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 3 * 3 * 16 * 64);
    let mut motion = MotionState {
        position: DVec3::ZERO,
        velocity: DVec3::ZERO,
        on_ground: false,
        jump_delay: 0,
    };
    for row in &rows {
        if row["tick"].as_u64() == Some(0) {
            let seed = row["seed"].as_u64().unwrap();
            motion.position = DVec3::ZERO;
            motion.velocity = DVec3::new(seed as f64 * 0.001, -0.02, 0.03);
            motion.on_ground = seed % 2 == 0;
            motion.jump_delay = 0;
        }
        motion.tick_flying_with_bounciness(
            MovementInput {
                forward: row["forward"].as_f64().unwrap() as f32,
                right: row["right"].as_f64().unwrap() as f32,
                yaw_degrees: row["yaw"].as_f64().unwrap() as f32,
                jump: row["jump"].as_bool().unwrap(),
                sprinting: row["sprint"].as_bool().unwrap(),
                using_item: false,
                moving_slowly: false,
            },
            row["down"].as_bool().unwrap(),
            row["speed"].as_f64().unwrap() as f32,
            0.6,
            if row["down"].as_bool().unwrap() {
                0.0
            } else {
                row["bounce"].as_f64().unwrap()
            },
            |position, requested, _| {
                let mut actual = requested;
                if actual.y < 0.0 && position.y + actual.y < 0.0 {
                    actual.y = -position.y;
                }
                if actual.y > 0.0 && position.y + actual.y > 2.0 {
                    actual.y = (2.0 - position.y).max(0.0);
                }
                if actual.x > 0.0 && position.x + 0.30000001192092896 + actual.x > 1.0 {
                    actual.x = (1.0 - position.x - 0.30000001192092896).max(0.0);
                }
                (requested, actual)
            },
        );
        assert!(
            (motion.position - vec(&row["position"])).length() < 1e-6,
            "{row}: {:?}",
            motion.position
        );
        assert!(
            (motion.velocity - vec(&row["velocity"])).length() < 1e-6,
            "{row}: {:?}",
            motion.velocity
        );
        assert_eq!(motion.on_ground, row["ground"].as_bool().unwrap(), "{row}");
        assert_eq!(
            u64::from(motion.jump_delay),
            row["delay"].as_u64().unwrap(),
            "{row}"
        );
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn spectator_flight_matches_vanilla_air_movement() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SpectatorFlightOracle.java");
    std::fs::write(&source, include_str!("java/SpectatorFlightOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = temp.path().join("flight.json");
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SpectatorFlightOracle")
        .arg(&output));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 16 * 64);
    let mut motion = MotionState {
        position: DVec3::ZERO,
        velocity: DVec3::ZERO,
        on_ground: false,
        jump_delay: 0,
    };
    for row in &rows {
        if row["tick"].as_u64() == Some(0) {
            motion.position = DVec3::ZERO;
            motion.velocity = DVec3::new(row["seed"].as_f64().unwrap() * 0.001, -0.02, 0.03);
        }
        motion.tick_spectator(
            MovementInput {
                forward: row["forward"].as_f64().unwrap() as f32,
                right: row["right"].as_f64().unwrap() as f32,
                yaw_degrees: row["yaw"].as_f64().unwrap() as f32,
                jump: row["jump"].as_bool().unwrap(),
                sprinting: row["sprint"].as_bool().unwrap(),
                using_item: false,
                moving_slowly: false,
            },
            row["down"].as_bool().unwrap(),
            row["speed"].as_f64().unwrap() as f32,
        );
        let expected_position = vec(&row["position"]);
        let expected_velocity = vec(&row["velocity"]);
        assert!(
            (motion.position - expected_position).length() < 1e-6,
            "{row}: {:?}",
            motion.position
        );
        assert!(
            (motion.velocity - expected_velocity).length() < 1e-6,
            "{row}: {:?}",
            motion.velocity
        );
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn walking_and_sprinting_air_and_ground_motion_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let blocks = [
        "minecraft:stone",
        "minecraft:ice",
        "minecraft:blue_ice",
        "minecraft:slime_block",
    ];
    let angles = [0.0_f32, 45.0, -90.0, 113.0, 180.0, -179.9, 12.5, 270.0];
    let mut scenarios = Vec::new();
    for scenario in 0..128 {
        let s = scenario % 64;
        let mut inputs = Vec::new();
        for t in 0..80 {
            let (forward, right) = match (t / 10 + s) % 6 {
                0 => (1.0, 0.0),
                1 => (0.0, 0.0),
                2 => (1.0, 1.0),
                3 => (0.0, 1.0),
                4 => (-1.0, 0.0),
                _ => (1.0, -1.0),
            };
            inputs.push(json!({"forward":forward,"right":right,"yaw":angles[(s + t/20) % angles.len()],"jump":(35..65).contains(&t),"sprint":(s + t / 7) % 3 == 0,"slow":scenario >= 64 && (s + t / 9) % 3 == 0,"using":(s + t / 11) % 4 == 0}));
        }
        scenarios.push(json!({"position":[0.0, if s%2==0 {0.0} else {2.0}, 0.0],"velocity":[if s%3==0 {0.005} else {0.0},0.0,0.0],"ground":s%2==0,"wall":2.75,"block":blocks[s%blocks.len()],"boost": ([-1, 0, 1, 2, 255][scenario % 5]),"inputs":inputs}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("PhysicsOracle.java");
    let input_path = temp.path().join("input.json");
    let output_path = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input_path, serde_json::to_vec(&scenarios).unwrap()).unwrap();
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
        .arg("PhysicsOracle")
        .arg(&input_path)
        .arg(&output_path));
    let expected: Value = serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    let mut errors = Vec::new();
    for (id, value) in expected["frictions"].as_object().unwrap() {
        let java = value.as_f64().unwrap() as f32;
        if ground_friction(id).to_bits() != java.to_bits() {
            errors.push(format!(
                "{id} friction: native {}, Java {java}",
                ground_friction(id)
            ));
        }
    }
    let results = expected["results"].as_array().unwrap();
    let mut collision_counts = [0, 0];
    assert_eq!(results.len(), scenarios.len());
    for (s, (scenario, results)) in scenarios.iter().zip(results).enumerate() {
        let mut state = MotionState {
            position: vec(&scenario["position"]),
            velocity: vec(&scenario["velocity"]),
            on_ground: scenario["ground"].as_bool().unwrap(),
            jump_delay: 0,
        };
        let inputs = scenario["inputs"].as_array().unwrap();
        assert_eq!(inputs.len(), results.as_array().unwrap().len());
        for (t, (input, result)) in inputs.iter().zip(results.as_array().unwrap()).enumerate() {
            let value = |key: &str| input[key].as_f64().unwrap() as f32;
            let command = MovementInput {
                forward: value("forward"),
                right: value("right"),
                yaw_degrees: value("yaw"),
                jump: input["jump"].as_bool().unwrap(),
                sprinting: input["sprint"].as_bool().unwrap(),
                using_item: input["using"].as_bool().unwrap(),
                moving_slowly: input["slow"].as_bool().unwrap(),
            };
            let boost = scenario["boost"].as_i64().unwrap();
            let (major_collision, _) = state.tick_resolved_with_power(
                command,
                ground_friction(scenario["block"].as_str().unwrap()),
                native_session::physics::jump_power(0.42, 1.0, u8::try_from(boost).ok()),
                |position, requested, _| {
                    let mut actual = requested;
                    if actual.y < 0.0 && position.y + actual.y < 0.0 {
                        actual.y = -position.y;
                    }
                    let wall = scenario["wall"].as_f64().unwrap();
                    if actual.x > 0.0 && position.x + f64::from(0.3_f32) + actual.x > wall {
                        actual.x = (wall - position.x - f64::from(0.3_f32)).max(0.0);
                    }
                    (requested, actual)
                },
            );
            let horizontal = result["horizontal"].as_bool().unwrap();
            let minor = result["minor"].as_bool().unwrap();
            if horizontal {
                collision_counts[usize::from(minor)] += 1;
            }
            if ((state.position - vec(&result["position"]))
                .abs()
                .max_element()
                > 1.0e-6
                || (state.velocity - vec(&result["velocity"]))
                    .abs()
                    .max_element()
                    > 1.0e-6
                || state.on_ground != result["ground"].as_bool().unwrap()
                || state.jump_delay != result["delay"].as_u64().unwrap() as u8
                || major_collision != (horizontal && !minor))
                && errors.len() < 20
            {
                errors.push(format!(
                    "scenario {s}, tick {t}, input {input}: native {state:?}, Java {result}"
                ));
            }
        }
    }
    assert!(
        errors.is_empty(),
        "physics mismatches (first 20):\n{}",
        errors.join("\n")
    );
    assert!(
        collision_counts[0] > 0 && collision_counts[1] > 0,
        "missing major or minor collision cases: {collision_counts:?}"
    );
    println!(
        "Compared {} movement ticks, {} block frictions and {collision_counts:?} major/minor collisions with Minecraft 26.3",
        128 * 80,
        expected["frictions"].as_object().unwrap().len()
    );
}
