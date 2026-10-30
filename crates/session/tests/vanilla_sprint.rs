//! Exercise the actual 26.3 LocalPlayer sprint input branch with mocked surroundings.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_session::{
    GameMode, PlayerPose,
    physics::{FlightState, SprintState},
};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.MultiPlayerGameMode;
import net.minecraft.client.Options;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.gui.Gui;
import net.minecraft.client.gui.screens.LevelLoadingScreen;
import net.minecraft.client.tutorial.Tutorial;
import net.minecraft.client.player.ClientInput;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.entity.Pose;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.entity.player.Input;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.Vec2;
import net.minecraft.tags.TagKey;
import net.minecraft.world.level.material.Fluid;

public class SprintOracle {
    static class StopAfterFlight extends RuntimeException { }
    static class TestInput extends ClientInput {
        Input next = Input.EMPTY;
        @Override public void tick() {
            keyPresses = next;
            float forward = (next.forward() ? 1.0f : 0.0f) - (next.backward() ? 1.0f : 0.0f);
            moveVector = new Vec2(0.0f, forward).normalized();
        }
    }
    static class TestLocal extends LocalPlayer {
        boolean running, water, under, swimming, grounded, enough, using;
        int flightJumps;
        Abilities testAbilities;
        TestLocal() { super(null, null, null, null, null, null, false, null, null); }
        @Override public Abilities getAbilities() { return testAbilities; }
        @Override public boolean isSprinting() { return running; }
        @Override public boolean isUsingItem() { return using; }
        @Override public void setSprinting(boolean value) { running = value; }
        @Override public boolean isPassenger() { return false; }
        @Override public boolean isSwimming() { return swimming; }
        @Override public boolean isInWater() { return water; }
        @Override public boolean onGround() { return grounded; }
        @Override public boolean isFallFlying() { return false; }
        @Override public boolean onClimbable() { return true; }
        @Override public boolean isEyeInFluid(TagKey<Fluid> fluid) { throw new StopAfterFlight(); }
        @Override public void onUpdateAbilities() { }
        @Override public void jumpFromGround() { flightJumps++; }
        @Override public boolean isUnderWater() { return under; }
        @Override public boolean isInShallowWater() { return water && !under; }
        @Override public boolean isMobilityRestricted() { return false; }
        @Override public boolean isVisuallyCrawling() { return false; }
        @Override public boolean isSleeping() { return false; }
        @Override protected boolean hasEnoughFoodToDoExhaustiveManoeuvres() { return enough; }
        @Override protected boolean canPlayerFitWithinBlocksAndEntitiesWhen(Pose pose) { return true; }
        @Override public float getEffectBlendFactor(Holder<MobEffect> effect, float delta) { return 0.0f; }
        void reset() { running = false; sprintTriggerTime = 0; input = new TestInput(); noPhysics = true; }
        void resetFlight() { reset(); jumpTriggerTime = 0; flightJumps = 0; }
        // aiStep is stopped before Player.aiStep, which decrements this timer.
        void finishPlayerTick() { if (jumpTriggerTime > 0) jumpTriggerTime--; }
    }
    static class TestPose extends Player {
        Pose current;
        boolean shift, swimming, standing, crouching, crawling;
        TestPose() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean isSleeping() { return false; }
        @Override public boolean isSwimming() { return swimming; }
        @Override public boolean isFallFlying() { return false; }
        @Override public boolean isAutoSpinAttack() { return false; }
        @Override public boolean isShiftKeyDown() { return shift; }
        @Override public boolean isSpectator() { return false; }
        @Override public boolean isPassenger() { return false; }
        @Override public void setPose(Pose pose) { current = pose; }
        @Override protected boolean canPlayerFitWithinBlocksAndEntitiesWhen(Pose pose) {
            if (pose == Pose.STANDING) return standing;
            if (pose == Pose.CROUCHING) return crouching;
            if (pose == Pose.SWIMMING) return crawling;
            throw new AssertionError(pose);
        }
    }
    static void set(Class<?> owner, Object target, String name, Object value) throws Exception {
        Field field = owner.getDeclaredField(name); field.setAccessible(true); field.set(target, value);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) field.get(null);
        Minecraft minecraft = (Minecraft) unsafe.allocateInstance(Minecraft.class);
        Gui gui = (Gui) unsafe.allocateInstance(Gui.class);
        set(Gui.class, gui, "screen", unsafe.allocateInstance(LevelLoadingScreen.class));
        set(Minecraft.class, minecraft, "gui", gui);
        set(Minecraft.class, minecraft, "tutorial", unsafe.allocateInstance(Tutorial.class));
        Options options = (Options) unsafe.allocateInstance(Options.class);
        OptionInstance<?> window = (OptionInstance<?>) unsafe.allocateInstance(OptionInstance.class);
        set(OptionInstance.class, window, "value", Integer.valueOf(7));
        set(Options.class, options, "sprintWindow", window);
        set(Minecraft.class, minecraft, "options", options);
        TestLocal player = (TestLocal) unsafe.allocateInstance(TestLocal.class);
        set(LivingEntity.class, player, "useItem", new ItemStack(Items.APPLE));
        set(LocalPlayer.class, player, "minecraft", minecraft);
        player.testAbilities = new Abilities();
        // Stop aiStep just after the sprint branch, before flight and unrelated world work.
        player.testAbilities.mayfly = true;
        JsonArray scenarios = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray output = new JsonArray();
        for (var scenario : scenarios) {
            player.reset();
            JsonArray ticks = new JsonArray();
            for (var value : scenario.getAsJsonArray()) {
                JsonObject step = value.getAsJsonObject();
                ((TestInput) player.input).next = new Input(step.get("forward").getAsBoolean(), step.get("backward").getAsBoolean(), false, false, false, step.get("shift").getAsBoolean(), step.get("key").getAsBoolean());
                player.horizontalCollision = step.get("collision").getAsBoolean();
                player.minorHorizontalCollision = step.get("minor").getAsBoolean();
                player.water = step.get("water").getAsBoolean(); player.under = step.get("under").getAsBoolean();
                player.swimming = step.get("swimming").getAsBoolean(); player.grounded = step.get("ground").getAsBoolean();
                player.enough = step.get("enough").getAsBoolean();
                player.using = step.get("using").getAsBoolean();
                try {
                    player.aiStep();
                    throw new AssertionError("aiStep unexpectedly continued beyond the sprint branch");
                } catch (NullPointerException expected) {
                    if (expected.getMessage() == null || !expected.getMessage().contains("isSpectator")) throw expected;
                }
                ticks.add(player.running);
            }
            output.add(ticks);
        }
        MultiPlayerGameMode gameMode = (MultiPlayerGameMode) unsafe.allocateInstance(MultiPlayerGameMode.class);
        set(Minecraft.class, minecraft, "gameMode", gameMode);
        JsonArray flights = new JsonArray();
        for (int mode = 0; mode < 3; mode++) for (int seed = 0; seed < 64; seed++) {
            set(MultiPlayerGameMode.class, gameMode, "localPlayerMode", mode == 2 ? GameType.SPECTATOR : mode == 1 ? GameType.CREATIVE : GameType.SURVIVAL);
            player.resetFlight(); player.testAbilities = new Abilities(); player.testAbilities.mayfly = mode != 0;
            player.enough = true; player.water = false; player.under = false; player.using = false;
            player.horizontalCollision = false; player.minorHorizontalCollision = false;
            for (int tick = 0; tick < 40; tick++) {
                boolean jump = ((tick + seed) % (2 + seed % 7)) == 0;
                player.swimming = (tick + seed) % 11 == 0;
                player.grounded = (tick + seed) % 3 == 0;
                ((TestInput) player.input).next = new Input(false, false, false, false, jump, false, false);
                int before = player.flightJumps;
                try { player.aiStep(); throw new AssertionError("flight branch did not stop"); }
                catch (StopAfterFlight expected) { }
                player.finishPlayerTick();
                JsonObject row = new JsonObject(); row.addProperty("mode", mode); row.addProperty("seed", seed); row.addProperty("tick", tick);
                row.addProperty("jump", jump); row.addProperty("swimming", player.swimming); row.addProperty("ground", player.grounded);
                row.addProperty("flying", player.testAbilities.flying); row.addProperty("impulse", player.flightJumps != before); flights.add(row);
            }
        }
        TestPose posePlayer = (TestPose) unsafe.allocateInstance(TestPose.class);
        set(Player.class, posePlayer, "abilities", new Abilities());
        Method updatePose = Player.class.getDeclaredMethod("updatePlayerPose"); updatePose.setAccessible(true);
        JsonArray poses = new JsonArray(), dimensions = new JsonArray();
        for (Pose initial : new Pose[]{Pose.STANDING, Pose.CROUCHING, Pose.SWIMMING}) {
            var size = posePlayer.getDefaultDimensions(initial);
            JsonArray dimension = new JsonArray(); dimension.add(size.width()); dimension.add(size.height()); dimension.add(size.eyeHeight()); dimensions.add(dimension);
            for (int bits = 0; bits < 32; bits++) {
                posePlayer.current = initial; posePlayer.shift = (bits & 1) != 0;
                posePlayer.standing = (bits & 2) != 0; posePlayer.crouching = (bits & 4) != 0; posePlayer.crawling = (bits & 8) != 0;
                posePlayer.swimming = (bits & 16) != 0;
                updatePose.invoke(posePlayer); poses.add(posePlayer.current.name());
            }
        }
        JsonObject result = new JsonObject(); result.add("sprints", output); result.add("poses", poses); result.add("dimensions", dimensions); result.add("flights", flights);
        Files.writeString(Path.of(args[1]), new Gson().toJson(result));
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
        "sprint oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn sprint_controls_match_vanilla_local_player() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut scenarios = Vec::new();
    for seed in 0_u32..96 {
        let mut random = seed.wrapping_add(1);
        let mut ticks = Vec::new();
        for tick in 0..80 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let forward = (random >> 27) % 5 >= 2;
            let backward = (random >> 24) % 7 == 0;
            let key = (random >> 11) % 5 == 0 || (tick / 16 + seed) % 4 == 0;
            let collision = (random >> 16) % 11 == 0;
            let minor = collision && (random >> 21) % 3 == 0;
            let shift = (random >> 6) % 5 == 0;
            let water = (random >> 3) % 4 == 0;
            let under = water && (random >> 9) % 2 == 0;
            let swimming = water && (random >> 14) % 3 == 0;
            let ground = (random >> 19) % 2 == 0;
            let enough = (random >> 23) % 4 != 0;
            let using = (random >> 2) % 4 == 0;
            ticks.push(json!({"forward":forward,"backward":backward,"shift":shift,"key":key,"collision":collision,"minor":minor,"water":water,"under":under,"swimming":swimming,"ground":ground,"enough":enough,"using":using}));
        }
        scenarios.push(ticks);
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SprintOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&scenarios).unwrap()).unwrap();
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
        .arg("SprintOracle")
        .arg(&input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut differences = Vec::new();
    for (scenario, (inputs, expected)) in scenarios
        .iter()
        .zip(expected["sprints"].as_array().unwrap())
        .enumerate()
    {
        let mut state = SprintState::default();
        let mut previous_sneak = false;
        for (tick, (input, expected)) in inputs.iter().zip(expected.as_array().unwrap()).enumerate()
        {
            state.pressed = input["key"].as_bool().unwrap();
            state.backward_pressed = input["backward"].as_bool().unwrap();
            state.previous_sneak = previous_sneak;
            state.in_water = input["water"].as_bool().unwrap();
            state.under_water = input["under"].as_bool().unwrap();
            state.swimming = input["swimming"].as_bool().unwrap();
            state.on_ground = input["ground"].as_bool().unwrap();
            state.current_sneak = input["shift"].as_bool().unwrap();
            state.insufficient_food = !input["enough"].as_bool().unwrap();
            state.using_item = input["using"].as_bool().unwrap();
            state.moving_slowly = previous_sneak && !state.swimming;
            state.after_move(
                input["collision"].as_bool().unwrap() && !input["minor"].as_bool().unwrap(),
            );
            let forward =
                i8::from(input["forward"].as_bool().unwrap()) - i8::from(state.backward_pressed);
            let actual = state.update(f32::from(forward));
            previous_sneak = input["shift"].as_bool().unwrap();
            if actual != expected.as_bool().unwrap() && differences.len() < 20 {
                differences.push(format!("scenario {scenario}, tick {tick}, input {input}: native {actual}, Java {expected}"));
            }
        }
    }
    assert!(
        differences.is_empty(),
        "sprint mismatches:\n{}",
        differences.join("\n")
    );
    let flights = expected["flights"].as_array().unwrap();
    assert_eq!(flights.len(), 3 * 64 * 40);
    let mut flight = FlightState::default();
    for row in flights {
        if row["tick"].as_u64() == Some(0) {
            flight = FlightState::default();
        }
        let mode = match row["mode"].as_u64().unwrap() {
            0 => GameMode::Survival,
            1 => GameMode::Creative,
            2 => GameMode::Spectator,
            _ => unreachable!(),
        };
        let impulse = flight.update(
            mode,
            row["jump"].as_bool().unwrap(),
            row["swimming"].as_bool().unwrap(),
            row["ground"].as_bool().unwrap(),
        );
        assert_eq!(flight.flying, row["flying"].as_bool().unwrap(), "{row}");
        assert_eq!(impulse, row["impulse"].as_bool().unwrap(), "{row}");
    }
    let poses = [
        PlayerPose::Standing,
        PlayerPose::Crouching,
        PlayerPose::Crawling,
    ];
    let pose_results = expected["poses"].as_array().unwrap();
    let dimensions = expected["dimensions"].as_array().unwrap();
    for (index, pose) in poses.iter().copied().enumerate() {
        let size = dimensions[index].as_array().unwrap();
        assert_eq!(size[0].as_f64().unwrap() as f32, 0.6_f32);
        assert_eq!(size[1].as_f64().unwrap() as f32, pose.height());
        assert_eq!(size[2].as_f64().unwrap() as f32, pose.eye_height());
        for bits in 0..32 {
            let actual = pose.after_movement(
                bits & 1 != 0,
                bits & 16 != 0,
                bits & 2 != 0,
                bits & 4 != 0,
                bits & 8 != 0,
            );
            let java = pose_results[index * 32 + bits].as_str().unwrap();
            let expected = match java {
                "STANDING" => PlayerPose::Standing,
                "CROUCHING" => PlayerPose::Crouching,
                "SWIMMING" if bits & 8 == 0 => pose,
                "SWIMMING" if bits & 16 != 0 => PlayerPose::Swimming,
                "SWIMMING" => PlayerPose::Crawling,
                _ => panic!("unexpected pose {java}"),
            };
            assert_eq!(actual, expected, "initial pose {pose:?}, flags {bits}");
        }
    }
    println!(
        "Compared {} sprint and {} flight input ticks with 26.3 LocalPlayer.aiStep",
        96 * 80,
        3 * 64 * 40
    );
}
