//! Compare saved entity motion with the locally installed Minecraft 26.3.
//! The oracle consumes synthetic NBT only; no world is opened or modified.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use glam::DVec3;
use native_persistence::nbt::{NamedTag, Tag, TagType};
use native_session::player_data::{
    read_respawn_max_health, read_saved_motion, read_saved_position, read_saved_vitals,
};
use serde_json::Value;

const ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.stream.Stream;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.damagesource.DamageSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.entity.ai.attributes.AttributeInstance;
import net.minecraft.world.entity.ai.attributes.AttributeMap;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.world.level.storage.ValueOutput;

public class PlayerMotionOracle {
    static final class TestLevel extends ClientLevel {
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public int getNextEntityId() { return 1; }
    }
    static final class TestEntity extends Entity {
        TestEntity(Level level) { super(EntityTypes.ARMOR_STAND, level); }
        @Override protected void defineSynchedData(SynchedEntityData.Builder builder) {}
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) { return false; }
        @Override protected void readAdditionalSaveData(ValueInput input) {}
        @Override protected void addAdditionalSaveData(ValueOutput output) {}
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var level = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        var lookup = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        var results = new JsonArray();
        for (int i = 0; i < Integer.parseInt(args[1]); i++) {
            var tag = NbtIo.read(Path.of(args[0], i + ".nbt"));
            var entity = new TestEntity(level);
            var input = TagValueInput.create(ProblemReporter.DISCARDING, lookup, tag);
            entity.load(input);
            var food = new FoodData(); food.readAdditionalSaveData(input);
            var attributes = new AttributeMap(Player.createAttributes().build());
            input.read("attributes", AttributeInstance.Packed.LIST_CODEC).ifPresent(attributes::apply);
            float maxHealth = (float) attributes.getValue(Attributes.MAX_HEALTH);
            var respawnAttributes = new AttributeMap(Player.createAttributes().build());
            respawnAttributes.assignBaseValues(attributes);
            Field exhaustion = FoodData.class.getDeclaredField("exhaustionLevel"); exhaustion.setAccessible(true);
            Field timer = FoodData.class.getDeclaredField("tickTimer"); timer.setAccessible(true);
            var result = new JsonObject();
            var motion = entity.getDeltaMovement();
            var vector = new JsonArray(); vector.add(motion.x); vector.add(motion.y); vector.add(motion.z);
            result.add("velocity", vector);
            var position = entity.position();
            var coordinates = new JsonArray(); coordinates.add(position.x); coordinates.add(position.y); coordinates.add(position.z);
            result.add("position", coordinates);
            result.addProperty("on_ground", entity.onGround());
            result.addProperty("air", entity.getAirSupply());
            result.addProperty("fall_distance", entity.fallDistance);
            result.addProperty("food", food.getFoodLevel());
            result.addProperty("saturation", food.getSaturationLevel());
            result.addProperty("exhaustion", exhaustion.getFloat(food));
            result.addProperty("food_timer", timer.getInt(food));
            result.addProperty("max_health", maxHealth);
            result.addProperty("respawn_max_health", (float) respawnAttributes.getValue(Attributes.MAX_HEALTH));
            result.addProperty("health", Math.clamp(input.getFloatOr("Health", maxHealth), 0.0f, maxHealth));
            result.addProperty("absorption", input.getFloatOr("AbsorptionAmount", 0.0f));
            results.add(result);
        }
        Files.writeString(Path.of(args[2]), new Gson().toJson(results));
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

const EXPERIENCE_ORACLE: &str = r#"
import java.nio.file.*;
import java.util.UUID;
import com.google.gson.*;
import com.mojang.authlib.GameProfile;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.GameType;
import net.minecraft.world.entity.player.Player;

public class ExperienceOracle {
    static final class TestLevel extends ClientLevel {
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public int getNextEntityId() { return 1; }
    }
    static final class TestPlayer extends Player {
        TestPlayer(Level level) { super(level, new GameProfile(UUID.randomUUID(), "Oracle")); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var world = (TestLevel) ((sun.misc.Unsafe) f.get(null)).allocateInstance(TestLevel.class);
        var inputs = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var results = new JsonArray();
        for (var value : inputs) {
            var input = value.getAsJsonObject();
            var player = new TestPlayer(world);
            player.experienceProgress = input.get("progress").getAsFloat();
            player.experienceLevel = input.get("level").getAsInt();
            player.totalExperience = input.get("total").getAsInt();
            player.setScore(input.get("score").getAsInt());
            player.giveExperiencePoints(input.get("amount").getAsInt());
            var result = new JsonObject();
            result.addProperty("progress_bits", Float.floatToRawIntBits(player.experienceProgress));
            result.addProperty("level", player.experienceLevel);
            result.addProperty("total", player.totalExperience);
            result.addProperty("score", player.getScore());
            results.add(result);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(results));
    }
}
"#;

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn experience_point_changes_match_vanilla_player() {
    use native_session::experience::Experience;
    use serde_json::json;

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ExperienceOracle.java");
    std::fs::write(&source, EXPERIENCE_ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .current_dir(temp.path())
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for level in [0, 1, 14, 15, 29, 30, 50, 100] {
        for progress in [0.0, 0.25, 0.75, 0.99] {
            for amount in [-100, -7, -1, 0, 1, 3, 7, 17, 73, 100, 2477] {
                let total = if amount == 2477 {
                    i32::MAX - 1
                } else {
                    level * 31
                };
                let original_score = if amount == 2477 {
                    i32::MAX - 1
                } else {
                    level * 41
                };
                inputs.push(json!({"progress":progress,"level":level,"total":total,
                    "score":original_score,"amount":amount}));
                let mut xp = Experience {
                    progress,
                    level,
                    total,
                };
                let mut score = original_score;
                assert!(xp.give_points(&mut score, amount));
                expected.push((xp, score));
            }
        }
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&inputs).unwrap()).unwrap();
    run(Command::new(java_bin().join("java.exe"))
        .current_dir(temp.path())
        .arg("-cp")
        .arg(&cp)
        .arg("ExperienceOracle")
        .arg(&input)
        .arg(&output));
    let actual: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (i, (xp, score)) in expected.iter().enumerate() {
        let result = &actual[i];
        assert_eq!(
            xp.progress.to_bits() as i32,
            result["progress_bits"].as_i64().unwrap() as i32,
            "case {i}: {}",
            inputs[i]
        );
        assert_eq!(
            xp.level,
            result["level"].as_i64().unwrap() as i32,
            "case {i}"
        );
        assert_eq!(
            xp.total,
            result["total"].as_i64().unwrap() as i32,
            "case {i}"
        );
        assert_eq!(*score, result["score"].as_i64().unwrap() as i32, "case {i}");
    }
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "player motion oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn list(values: &[f64]) -> Tag {
    Tag::List {
        element_type: TagType::Double,
        values: values.iter().copied().map(Tag::Double).collect(),
    }
}

fn max_health_attribute(base: f64, modifiers: &[(&str, &str, f64)]) -> Tag {
    Tag::List {
        element_type: TagType::Compound,
        values: vec![Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:max_health".into())),
            ("base".into(), Tag::Double(base)),
            (
                "modifiers".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: modifiers
                        .iter()
                        .map(|(id, operation, amount)| {
                            Tag::Compound(vec![
                                ("id".into(), Tag::String((*id).into())),
                                ("operation".into(), Tag::String((*operation).into())),
                                ("amount".into(), Tag::Double(*amount)),
                            ])
                        })
                        .collect(),
                },
            ),
        ])],
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn saved_motion_matches_vanilla_entity_load() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let cases: Vec<Vec<(&str, Tag)>> = vec![
        vec![],
        vec![
            ("Motion", list(&[0.125, -0.275, 0.375])),
            ("OnGround", Tag::Byte(1)),
            ("Air", Tag::Short(-19)),
            ("fall_distance", Tag::Double(8.5)),
            ("foodLevel", Tag::Int(13)),
            ("foodSaturationLevel", Tag::Float(2.5)),
            ("foodExhaustionLevel", Tag::Float(3.75)),
            ("foodTickTimer", Tag::Int(71)),
            (
                "Pos",
                list(&[16_000_000.123_456_79, 75.987654321, -16_000_000.987_654_32]),
            ),
        ],
        vec![
            ("Motion", list(&[10.0, -10.0, 10.000001])),
            ("OnGround", Tag::Int(256)),
            ("Air", Tag::Float(-1.25)),
            ("fall_distance", Tag::Float(1.25)),
            ("foodLevel", Tag::Double(-0.5)),
            ("foodSaturationLevel", Tag::Int(2)),
            ("foodTickTimer", Tag::Float(-1.25)),
            (
                "Pos",
                list(&[90_000_000.0, -30_000_000.0, f64::NEG_INFINITY]),
            ),
        ],
        vec![
            (
                "Motion",
                list(&[f64::NAN, f64::INFINITY, f64::NEG_INFINITY]),
            ),
            ("OnGround", Tag::Double(-0.5)),
        ],
        vec![("Motion", list(&[1.0, 2.0]))],
        vec![("Motion", list(&[1.0, 2.0, 3.0, 4.0]))],
        vec![
            ("Motion", Tag::ByteArray(vec![255, 2, 3, 4])),
            ("OnGround", Tag::Long(-257)),
        ],
        vec![
            ("Motion", Tag::IntArray(vec![4, -3, 2, 1])),
            ("OnGround", Tag::Float(-0.125)),
            ("Pos", Tag::IntArray(vec![4, -3, 2, 1])),
        ],
        vec![("Motion", Tag::LongArray(vec![12, -2, 10]))],
        vec![
            ("Health", Tag::Float(46.0)),
            ("AbsorptionAmount", Tag::Float(3.0)),
            (
                "attributes",
                max_health_attribute(
                    20.0,
                    &[
                        ("example:a", "add_value", 4.0),
                        ("example:b", "add_multiplied_base", 0.5),
                        ("example:c", "add_multiplied_total", 0.25),
                    ],
                ),
            ),
        ],
        vec![
            ("Health", Tag::Float(21.5)),
            (
                "attributes",
                max_health_attribute(
                    20.0,
                    &[
                        ("example:duplicate", "add_value", 4.0),
                        ("example:duplicate", "add_value", 7.0),
                    ],
                ),
            ),
        ],
        vec![
            ("Motion", Tag::String("bad".into())),
            ("OnGround", Tag::String("true".into())),
            ("fall_distance", Tag::String("bad".into())),
        ],
        vec![(
            "Motion",
            Tag::List {
                element_type: TagType::Float,
                values: vec![Tag::Float(0.1), Tag::Float(-0.2), Tag::Float(0.3)],
            },
        )],
        vec![(
            "Motion",
            Tag::List {
                element_type: TagType::String,
                values: vec![
                    Tag::String("x".into()),
                    Tag::String("y".into()),
                    Tag::String("z".into()),
                ],
            },
        )],
    ];
    let players: Vec<NamedTag> = cases
        .into_iter()
        .map(|fields| NamedTag {
            name: "".into(),
            value: Tag::Compound(fields.into_iter().map(|(k, v)| (k.into(), v)).collect()),
        })
        .collect();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("PlayerMotionOracle.java");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    for (i, player) in players.iter().enumerate() {
        std::fs::write(
            temp.path().join(format!("{i}.nbt")),
            native_persistence::nbt::write(player).unwrap(),
        )
        .unwrap();
    }
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
        .arg("PlayerMotionOracle")
        .arg(temp.path())
        .arg(players.len().to_string())
        .arg(&output));
    let results: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (i, (player, expected)) in players.iter().zip(results.as_array().unwrap()).enumerate() {
        let actual = read_saved_motion(player).unwrap();
        let vector = expected["velocity"].as_array().unwrap();
        let velocity = DVec3::new(
            vector[0].as_f64().unwrap(),
            vector[1].as_f64().unwrap(),
            vector[2].as_f64().unwrap(),
        );
        assert_eq!(actual.velocity, velocity, "case {i}");
        assert_eq!(
            actual.on_ground,
            expected["on_ground"].as_bool().unwrap(),
            "case {i}"
        );
        assert_eq!(
            actual.fall_distance,
            expected["fall_distance"].as_f64().unwrap(),
            "case {i}"
        );
        let vitals = read_saved_vitals(player, false).unwrap();
        assert_eq!(
            read_respawn_max_health(player).unwrap(),
            expected["respawn_max_health"].as_f64().unwrap() as f32,
            "case {i}"
        );
        assert_eq!(
            vitals.max_health,
            expected["max_health"].as_f64().unwrap() as f32,
            "case {i}"
        );
        assert_eq!(
            vitals.health,
            expected["health"].as_f64().unwrap() as f32,
            "case {i}"
        );
        assert_eq!(
            vitals.absorption,
            expected["absorption"].as_f64().unwrap() as f32,
            "case {i}"
        );
        assert_eq!(
            vitals.food,
            expected["food"].as_i64().unwrap() as i32,
            "case {i}"
        );
        assert_eq!(
            vitals.air,
            expected["air"].as_i64().unwrap() as i32,
            "case {i}"
        );
        assert_eq!(
            vitals.saturation,
            expected["saturation"].as_f64().unwrap() as f32,
            "case {i}"
        );
        assert_eq!(
            vitals.exhaustion,
            expected["exhaustion"].as_f64().unwrap() as f32,
            "case {i}"
        );
        assert_eq!(
            vitals.food_tick_timer,
            expected["food_timer"].as_i64().unwrap() as i32,
            "case {i}"
        );
        if let Some(position) = read_saved_position(player).unwrap() {
            let values = expected["position"].as_array().unwrap();
            let expected_position = DVec3::new(
                values[0].as_f64().unwrap(),
                values[1].as_f64().unwrap(),
                values[2].as_f64().unwrap(),
            );
            assert_eq!(position, expected_position, "position case {i}");
        }
    }
    println!(
        "Compared {} synthetic saved entities with Minecraft 26.3",
        players.len()
    );
}

const SAVE_ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.UUID;
import com.google.gson.*;
import com.mojang.authlib.GameProfile;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.damagesource.DamageSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.TagValueOutput;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.world.level.storage.ValueOutput;

public class PlayerSaveOracle {
    static final class TestLevel extends ClientLevel {
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public int getNextEntityId() { return 1; }
    }
    static final class TestEntity extends Entity {
        TestEntity(Level level) { super(EntityTypes.ARMOR_STAND, level); }
        @Override protected void defineSynchedData(SynchedEntityData.Builder builder) {}
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) { return false; }
        @Override protected void readAdditionalSaveData(ValueInput input) {}
        @Override protected void addAdditionalSaveData(ValueOutput output) {}
    }
    static final class TestPlayer extends Player {
        TestPlayer(Level level) { super(level, new GameProfile(UUID.randomUUID(), "Oracle")); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
    }
    static void compare(net.minecraft.nbt.CompoundTag left, net.minecraft.nbt.CompoundTag right, String[] fields) {
        for (String field : fields) {
            if (left.get(field) == null || !left.get(field).equals(right.get(field)))
                throw new AssertionError(field + ": " + left.get(field) + " != " + right.get(field));
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var level = (TestLevel) ((sun.misc.Unsafe) f.get(null)).allocateInstance(TestLevel.class);
        var lookup = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        Field exhaustion = FoodData.class.getDeclaredField("exhaustionLevel"); exhaustion.setAccessible(true);
        Field timer = FoodData.class.getDeclaredField("tickTimer"); timer.setAccessible(true);
        Field onGround = Entity.class.getDeclaredField("onGround"); onGround.setAccessible(true);
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0], "input.json"))).getAsJsonArray();
        String[] motionFields = {"Pos", "Motion", "Rotation", "fall_distance", "OnGround", "Air"};
        String[] foodFields = {"foodLevel", "foodTickTimer", "foodSaturationLevel", "foodExhaustionLevel"};
        String[] xpFields = {"XpP", "XpLevel", "XpTotal", "XpSeed", "Score"};
        Field seed = Player.class.getDeclaredField("enchantmentSeed"); seed.setAccessible(true);
        for (int i = 0; i < cases.size(); i++) {
            var state = cases.get(i).getAsJsonObject();
            var pos = state.getAsJsonArray("pos");
            var vel = state.getAsJsonArray("vel");
            var entity = new TestEntity(level);
            entity.setPos(pos.get(0).getAsDouble(), pos.get(1).getAsDouble(), pos.get(2).getAsDouble());
            entity.setDeltaMovement(vel.get(0).getAsDouble(), vel.get(1).getAsDouble(), vel.get(2).getAsDouble());
            onGround.setBoolean(entity, state.get("ground").getAsBoolean());
            entity.fallDistance = state.get("fall").getAsDouble();
            entity.setYRot(state.get("yaw").getAsFloat());
            entity.setXRot(state.get("pitch").getAsFloat());
            entity.setAirSupply(state.get("air").getAsInt());
            var food = new FoodData();
            food.setFoodLevel(state.get("food").getAsInt());
            food.setSaturation(state.get("saturation").getAsFloat());
            exhaustion.setFloat(food, state.get("exhaustion").getAsFloat());
            timer.setInt(food, state.get("timer").getAsInt());
            var output = TagValueOutput.createWithContext(ProblemReporter.DISCARDING, lookup);
            entity.saveWithoutId(output);
            food.addAdditionalSaveData(output);
            var reference = output.buildResult();
            var player = new TestPlayer(level);
            player.experienceProgress = state.get("xp_progress").getAsFloat();
            player.experienceLevel = state.get("xp_level").getAsInt();
            player.totalExperience = state.get("xp_total").getAsInt();
            player.setScore(state.get("score").getAsInt());
            seed.setInt(player, 55);
            var xpOutput = TagValueOutput.createWithContext(ProblemReporter.DISCARDING, lookup);
            player.saveWithoutId(xpOutput);
            var xpReference = xpOutput.buildResult();
            for (String field : xpFields) reference.put(field, xpReference.get(field));
            NbtIo.write(reference, Path.of(args[0], "java-" + i + ".nbt"));
            var rust = NbtIo.read(Path.of(args[0], "rust-" + i + ".nbt"));
            compare(reference, rust, motionFields);
            compare(reference, rust, foodFields);
            compare(reference, rust, xpFields);
            var loaded = new TestEntity(level);
            var input = TagValueInput.create(ProblemReporter.DISCARDING, lookup, rust);
            loaded.load(input);
            var loadedFood = new FoodData(); loadedFood.readAdditionalSaveData(input);
            var roundTrip = TagValueOutput.createWithContext(ProblemReporter.DISCARDING, lookup);
            loaded.saveWithoutId(roundTrip);
            loadedFood.addAdditionalSaveData(roundTrip);
            compare(reference, roundTrip.buildResult(), motionFields);
            compare(reference, roundTrip.buildResult(), foodFields);
            var loadedPlayer = new TestPlayer(level);
            loadedPlayer.load(input);
            var xpRoundTrip = TagValueOutput.createWithContext(ProblemReporter.DISCARDING, lookup);
            loadedPlayer.saveWithoutId(xpRoundTrip);
            compare(reference, xpRoundTrip.buildResult(), xpFields);
            if (input.getFloatOr("Health", -1) != state.get("health").getAsFloat() ||
                input.getFloatOr("AbsorptionAmount", -1) != state.get("absorption").getAsFloat())
                throw new AssertionError("health or absorption");
            if (rust.getIntOr("unknown_value", -1) != 9182) throw new AssertionError("unknown field lost");
        }
    }
}
"#;

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn drafted_player_state_matches_vanilla_save_and_load() {
    use native_session::{
        GameMode,
        inventory::PlayerInventory,
        player_data::{PlayerSaveState, SavedPlayerMotion, draft_player_state},
        survival::PlayerVitals,
    };
    use serde_json::json;

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("PlayerSaveOracle.java");
    std::fs::write(&source, SAVE_ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .current_dir(temp.path())
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let original = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            ("XpSeed".into(), Tag::Int(55)),
            ("unknown_value".into(), Tag::Int(9182)),
        ]),
    };
    let inventory = PlayerInventory::default();
    let mut inputs = Vec::new();
    let mut drafts = Vec::new();
    for i in 0..128 {
        let n = f64::from(i);
        let position = DVec3::new(
            if i % 2 == 0 {
                16_000_000.123_456_79 + n * 0.125
            } else {
                -0.123_456_789 - n * 0.25
            },
            70.125 + n * 0.0625,
            -16_000_000.987_654_32 + n * 0.015625,
        );
        let velocity = DVec3::new(
            (n * 0.13).sin() * 0.3,
            (n * 0.07).cos() * 0.2,
            (n * 0.11).cos() * 0.35,
        );
        let vitals = PlayerVitals {
            health: 1.0 + (i % 20) as f32,
            absorption: (i % 5) as f32 * 0.5,
            air: (i * 13) % 340 - 20,
            food: i % 21,
            saturation: (i % 11) as f32 * 0.5,
            exhaustion: (i % 9) as f32 * 0.5,
            food_tick_timer: i % 80,
            ..Default::default()
        };
        let state = PlayerSaveState {
            position,
            motion: SavedPlayerMotion {
                velocity,
                on_ground: i % 3 == 0,
                fall_distance: n * 0.0625,
            },
            yaw: -175.0 + i as f32 * 2.5,
            pitch: -70.0 + i as f32,
            dimension: "minecraft:overworld".into(),
            game_mode: GameMode::Survival,
            experience: native_session::experience::Experience {
                progress: (i % 16) as f32 / 16.0,
                level: i % 43,
                total: i * 31,
            },
            score: i * 41,
            vitals,
        };
        let draft = draft_player_state(&original, &state, &inventory).unwrap();
        std::fs::write(
            temp.path().join(format!("rust-{i}.nbt")),
            native_persistence::nbt::write(draft.document()).unwrap(),
        )
        .unwrap();
        inputs.push(json!({"pos": position.to_array(), "vel": velocity.to_array(), "ground": state.motion.on_ground, "fall": state.motion.fall_distance, "yaw": state.yaw, "pitch": state.pitch,
            "food": vitals.food, "saturation": vitals.saturation, "exhaustion": vitals.exhaustion, "timer": vitals.food_tick_timer, "health": vitals.health, "absorption": vitals.absorption, "air": vitals.air,
            "xp_progress": state.experience.progress, "xp_level": state.experience.level, "xp_total": state.experience.total, "score": state.score}));
        drafts.push(draft);
    }
    std::fs::write(
        temp.path().join("input.json"),
        serde_json::to_vec(&inputs).unwrap(),
    )
    .unwrap();
    run(Command::new(java_bin().join("java.exe"))
        .current_dir(temp.path())
        .arg("-cp")
        .arg(&cp)
        .arg("PlayerSaveOracle")
        .arg(temp.path()));
    for (i, draft) in drafts.iter().enumerate() {
        let reference = native_persistence::nbt::read(
            &std::fs::read(temp.path().join(format!("java-{i}.nbt"))).unwrap(),
        )
        .unwrap();
        let saved_experience = native_session::player_data::read_saved_experience(&reference);
        assert_eq!(
            saved_experience.progress,
            inputs[i]["xp_progress"].as_f64().unwrap() as f32
        );
        assert_eq!(
            saved_experience.level,
            inputs[i]["xp_level"].as_i64().unwrap() as i32
        );
        assert_eq!(
            saved_experience.total,
            inputs[i]["xp_total"].as_i64().unwrap() as i32
        );
        for field in [
            "Pos",
            "Motion",
            "Rotation",
            "fall_distance",
            "OnGround",
            "Air",
            "foodLevel",
            "foodTickTimer",
            "foodSaturationLevel",
            "foodExhaustionLevel",
            "XpP",
            "XpLevel",
            "XpTotal",
            "XpSeed",
            "Score",
        ] {
            assert_eq!(
                draft.document().value.get(field),
                reference.value.get(field),
                "case {i}, field {field}"
            );
        }
    }
    println!(
        "Compared {} synthetic player state drafts with Java save and reload",
        drafts.len()
    );
}
