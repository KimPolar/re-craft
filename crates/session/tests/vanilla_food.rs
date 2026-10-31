//! Compare the food-data algorithm with 26.3; entity damage is mocked.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_persistence::world::Difficulty;
use native_session::survival::{
    FoodEffect, MovementEnvironment, PlayerVitals, SurvivalRules, ordinary_food,
};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.*;
import net.minecraft.world.Difficulty;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.TagKey;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.core.Holder;
import net.minecraft.world.entity.ai.attributes.Attribute;

public class FoodOracle {
    static class TestLevel extends ServerLevel {
        Difficulty difficulty;
        GameRules rules;
        TestLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public Difficulty getDifficulty() { return difficulty; }
        @Override public GameRules getGameRules() { return rules; }
    }
    static class TestDamage extends DamageSources {
        TestDamage() { super(null); }
        @Override public DamageSource starve() { return null; }
    }
    static class TestPlayer extends ServerPlayer {
        TestLevel world; TestDamage damage;
        float health, maximum;
        int effect; float amount;
        boolean swim, eyes, water, grounded, sprinting;
        float movementExhaustion;
        TestPlayer() { super(null, null, null, null); }
        @Override public ServerLevel level() { return world; }
        @Override public boolean isHurt() { return health > 0 && health < maximum; }
        @Override public float getHealth() { return health; }
        @Override public DamageSources damageSources() { return damage; }
        @Override public void heal(float value) { effect = 1; amount = value; if (health > 0) health = Math.min(health + value, maximum); }
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float value) { effect = 2; amount = value; health = Math.max(health - value, 0); return true; }
        @Override public boolean isPassenger() { return false; }
        @Override public boolean isSwimming() { return swim; }
        @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return eyes; }
        @Override public boolean isInWater() { return water; }
        @Override public boolean onClimbable() { return false; }
        @Override public boolean onGround() { return grounded; }
        @Override public boolean isSprinting() { return sprinting; }
        @Override public boolean isFallFlying() { return false; }
        @Override public boolean isCrouching() { return false; }
        @Override public void awardStat(Identifier id, int value) { }
        @Override public void causeFoodExhaustion(float value) { movementExhaustion += value; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return maximum; }
        void regenerate(FoodData data, int tick) { foodData = data; tickCount = tick; super.tickRegeneration(); }
    }
    static Field field(String name) throws Exception { Field f = FoodData.class.getDeclaredField(name); f.setAccessible(true); return f; }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        TestPlayer player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        TestLevel world = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        player.world = world; player.damage = (TestDamage) unsafe.allocateInstance(TestDamage.class);
        Field food = field("foodLevel"), saturation = field("saturationLevel"), exhaustion = field("exhaustionLevel"), timer = field("tickTimer");
        JsonArray scenarios = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (var entry : scenarios) {
            JsonObject s = entry.getAsJsonObject();
            world.difficulty = Difficulty.valueOf(s.get("difficulty").getAsString());
            world.rules = new GameRules(List.of(GameRules.NATURAL_HEALTH_REGENERATION));
            world.rules.set(GameRules.NATURAL_HEALTH_REGENERATION, s.get("regen").getAsBoolean(), null);
            player.health = s.get("health").getAsFloat(); player.maximum = s.get("max").getAsFloat();
            FoodData data = new FoodData(); food.setInt(data, s.get("food").getAsInt());
            saturation.setFloat(data, s.get("saturation").getAsFloat()); exhaustion.setFloat(data, s.get("exhaustion").getAsFloat()); timer.setInt(data, s.get("timer").getAsInt());
            JsonArray ticks = new JsonArray();
            for (int i=0; i<100; i++) {
                player.regenerate(data, i+1); player.effect = 0; player.amount = 0; data.tick(player);
                JsonArray t = new JsonArray(); t.add(food.getInt(data)); t.add(saturation.getFloat(data));
                t.add(exhaustion.getFloat(data)); t.add(timer.getInt(data)); t.add(player.health); t.add(player.effect); t.add(player.amount); ticks.add(t);
            }
            results.add(ticks);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(results));
        JsonObject foods = new JsonObject();
        for (var item : BuiltInRegistries.ITEM) {
            var foodProps = item.components().get(DataComponents.FOOD);
            var consumable = item.components().get(DataComponents.CONSUMABLE);
            if (foodProps == null || consumable == null) continue;
            JsonObject value = new JsonObject();
            value.addProperty("nutrition", foodProps.nutrition()); value.addProperty("saturation", foodProps.saturation());
            value.addProperty("always", foodProps.canAlwaysEat()); value.addProperty("ticks", consumable.consumeTicks());
            value.addProperty("effects", consumable.onConsumeEffects().size());
            value.addProperty("remainder", item.components().has(DataComponents.USE_REMAINDER));
            foods.add(BuiltInRegistries.ITEM.getKey(item).toString(), value);
        }
        Files.writeString(Path.of(args[2]), new Gson().toJson(foods));
        JsonArray movementCases = JsonParser.parseString(Files.readString(Path.of(args[3]))).getAsJsonArray();
        JsonArray movementResults = new JsonArray();
        for (var entry : movementCases) {
            JsonObject c = entry.getAsJsonObject();
            player.swim = c.get("swimming").getAsBoolean(); player.eyes = c.get("eyes").getAsBoolean();
            player.water = c.get("water").getAsBoolean(); player.grounded = c.get("ground").getAsBoolean();
            player.sprinting = c.get("sprinting").getAsBoolean(); player.movementExhaustion = 0;
            player.checkMovementStatistics(c.get("x").getAsDouble(), c.get("y").getAsDouble(), c.get("z").getAsDouble());
            movementResults.add(player.movementExhaustion);
        }
        Files.writeString(Path.of(args[4]), new Gson().toJson(movementResults));
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
        "food oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn food_ticks_match_vanilla_with_mocked_damage() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    let mut state = 17_u32;
    for i in 0..768 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        let difficulty = ["PEACEFUL", "EASY", "NORMAL", "HARD"][i % 4];
        let food = [0, 1, 6, 17, 18, 19, 20][(state as usize >> 4) % 7];
        let saturation = [0.0, 0.1, 0.9, 1.0, 5.0, 6.0, 20.0][(state as usize >> 9) % 7];
        let exhaustion = [0.0, 4.0, 4.1, 5.0, 40.0][(state as usize >> 14) % 5];
        let health = [0.0, 1.0, 9.5, 10.0, 11.0, 19.0, 20.0][(state as usize >> 19) % 7];
        let timer = [0, 9, 10, 78, 79, 80][(state as usize >> 24) % 6];
        cases.push(json!({"difficulty":difficulty,"regen":i % 8 < 4,"food":food,"saturation":saturation,"exhaustion":exhaustion,"health":health,"max":20.0,"timer":timer}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FoodOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    let food_output = temp.path().join("foods.json");
    let movement_input = temp.path().join("movement.json");
    let movement_output = temp.path().join("movement_results.json");
    let mut movement_cases = Vec::new();
    for i in 0..2048 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        let x = (i as f64 % 37.0 - 18.0) * 0.005;
        let y = ((state >> 8) % 25) as f64 * 0.007 - 0.08;
        let z = ((state >> 16) % 91) as f64 * 0.003 - 0.12;
        movement_cases.push(json!({"x":x,"y":y,"z":z,"swimming":i%5==0,"eyes":i%7==0,"water":i%3==0,"ground":i%2==0,"sprinting":i%4<2}));
    }
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    std::fs::write(
        &movement_input,
        serde_json::to_vec(&movement_cases).unwrap(),
    )
    .unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("FoodOracle")
        .arg(&input)
        .arg(&output)
        .arg(&food_output)
        .arg(&movement_input)
        .arg(&movement_output));
    let movement_results: Value =
        serde_json::from_slice(&std::fs::read(movement_output).unwrap()).unwrap();
    for (case, expected) in movement_cases
        .iter()
        .zip(movement_results.as_array().unwrap())
    {
        let mut vitals = PlayerVitals {
            exhaustion: 0.0,
            ..PlayerVitals::default()
        };
        vitals.record_movement(
            native_session::GameMode::Survival,
            glam::DVec3::new(
                case["x"].as_f64().unwrap(),
                case["y"].as_f64().unwrap(),
                case["z"].as_f64().unwrap(),
            ),
            MovementEnvironment {
                on_ground: case["ground"].as_bool().unwrap(),
                sprinting: case["sprinting"].as_bool().unwrap(),
                swimming: case["swimming"].as_bool().unwrap(),
                eyes_in_water: case["eyes"].as_bool().unwrap(),
                in_water: case["water"].as_bool().unwrap(),
            },
        );
        let expected = expected.as_f64().unwrap() as f32;
        assert!(
            (vitals.exhaustion - expected).abs() < 1.0e-6,
            "{case}: {} != {expected}",
            vitals.exhaustion
        );
    }
    let foods: Value = serde_json::from_slice(&std::fs::read(food_output).unwrap()).unwrap();
    let mut tested_foods = 0;
    for (id, properties) in foods.as_object().unwrap() {
        let Some(food) = ordinary_food(id) else {
            continue;
        };
        tested_foods += 1;
        assert_eq!(
            food.nutrition,
            properties["nutrition"].as_i64().unwrap() as i32,
            "{id}"
        );
        assert_eq!(
            food.saturation,
            properties["saturation"].as_f64().unwrap() as f32,
            "{id}"
        );
        assert_eq!(
            food.use_ticks,
            properties["ticks"].as_i64().unwrap() as i32,
            "{id}"
        );
        assert_eq!(properties["always"], false, "{id}");
        assert_eq!(properties["effects"], 0, "{id}");
        assert_eq!(
            properties["remainder"].as_bool().unwrap(),
            matches!(
                id.as_str(),
                "minecraft:mushroom_stew" | "minecraft:rabbit_stew" | "minecraft:beetroot_soup"
            ),
            "{id}"
        );
    }
    assert_eq!(tested_foods, 30);
    let results: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (case, ticks) in cases.iter().zip(results.as_array().unwrap()) {
        let difficulty = match case["difficulty"].as_str().unwrap() {
            "PEACEFUL" => Difficulty::Peaceful,
            "EASY" => Difficulty::Easy,
            "NORMAL" => Difficulty::Normal,
            _ => Difficulty::Hard,
        };
        let rules = SurvivalRules {
            difficulty,
            natural_health_regeneration: case["regen"].as_bool().unwrap(),
            fall_damage: true,
        };
        let mut vitals = PlayerVitals {
            health: case["health"].as_f64().unwrap() as f32,
            food: case["food"].as_i64().unwrap() as i32,
            saturation: case["saturation"].as_f64().unwrap() as f32,
            exhaustion: case["exhaustion"].as_f64().unwrap() as f32,
            food_tick_timer: case["timer"].as_i64().unwrap() as i32,
            ..PlayerVitals::default()
        };
        for (i, expected) in ticks.as_array().unwrap().iter().enumerate() {
            vitals.tick_peaceful_regeneration(rules, (i + 1) as i32);
            let (effect, amount) = match vitals.tick_food(rules) {
                FoodEffect::None => (0, 0.0),
                FoodEffect::Heal(n) => {
                    vitals.heal(n);
                    (1, n)
                }
                FoodEffect::Starve => {
                    vitals.health = (vitals.health - 1.0).max(0.0);
                    (2, 1.0)
                }
            };
            assert_eq!(
                vitals.food,
                expected[0].as_i64().unwrap() as i32,
                "{case} tick {i}"
            );
            for (actual, index) in [
                (vitals.saturation, 1),
                (vitals.exhaustion, 2),
                (vitals.health, 4),
                (amount, 6),
            ] {
                let reference = expected[index].as_f64().unwrap() as f32;
                assert!(
                    (actual - reference).abs() < 1.0e-5,
                    "{case} tick {i}, field {index}: {actual} != {reference}"
                );
            }
            assert_eq!(
                vitals.food_tick_timer,
                expected[3].as_i64().unwrap() as i32,
                "{case} tick {i}"
            );
            assert_eq!(effect, expected[5].as_i64().unwrap(), "{case} tick {i}");
        }
    }
}
