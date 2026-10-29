//! Compare visual interpolation against the locally installed Java camera.
//! The oracle creates synthetic entities and does not open a saved world.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use glam::DVec3;
use native_session::camera::CameraMotion;
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.Camera;
import net.minecraft.client.CameraType;
import net.minecraft.client.Minecraft;
import net.minecraft.client.Options;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.Holder;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.attribute.EnvironmentAttributeProbe;
import net.minecraft.world.damagesource.DamageSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.world.level.storage.ValueOutput;
import net.minecraft.world.phys.Vec3;

public class CameraOracle {
    static class FakeMinecraft extends Minecraft {
        Entity cameraEntity;
        FakeMinecraft() { super(null); }
        @Override public Entity getCameraEntity() { return cameraEntity; }
    }
    static class TestLevel extends ClientLevel {
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public int getNextEntityId() { return 1; }
    }
    static class TestEntity extends Entity {
        TestEntity(Level level) { super(EntityTypes.ARMOR_STAND, level); }
        @Override protected void defineSynchedData(SynchedEntityData.Builder builder) {}
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) { return false; }
        @Override protected void readAdditionalSaveData(ValueInput input) {}
        @Override protected void addAdditionalSaveData(ValueOutput output) {}
    }
    static class NoopProbe extends EnvironmentAttributeProbe {
        @Override public void tick(Level level, Vec3 position) {}
    }
    static class TestPlayer extends LocalPlayer {
        Abilities abilities;
        double speed;
        TestPlayer() { super(null, null, null, null, null, null, false, null, null); }
        @Override public Abilities getAbilities() { return abilities; }
        @Override public double getAttributeValue(Holder<Attribute> ignored) { return speed; }
        @Override public boolean isUsingItem() { return false; }
    }
    static Field field(Class<?> type, String name) throws Exception {
        Field field = type.getDeclaredField(name); field.setAccessible(true); return field;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var minecraft = (FakeMinecraft) unsafe.allocateInstance(FakeMinecraft.class);
        var options = (Options) unsafe.allocateInstance(Options.class);
        options.setCameraType(CameraType.FIRST_PERSON);
        var effect = (OptionInstance<?>) unsafe.allocateInstance(OptionInstance.class);
        field(OptionInstance.class, "value").set(effect, Double.valueOf(1.0));
        field(Options.class, "fovEffectScale").set(options, effect);
        field(Minecraft.class, "options").set(minecraft, options);
        var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        player.abilities = new Abilities();
        minecraft.cameraEntity = player;
        var level = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        var cameraMinecraft = field(Camera.class, "minecraft");
        var cameraProbe = field(Camera.class, "attributeProbe");
        var eye = field(Camera.class, "eyeHeight");
        var oldEye = field(Camera.class, "eyeHeightOld");
        var fovModifier = field(Camera.class, "fovModifier");
        var oldFovModifier = field(Camera.class, "oldFovModifier");
        var entityEye = field(Entity.class, "eyeHeight");
        Method align = Camera.class.getDeclaredMethod("alignWithEntity", float.class);
        align.setAccessible(true);
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var results = new JsonArray();
        for (var scenario : input) {
            var object = scenario.getAsJsonObject();
            var start = object.getAsJsonArray("start");
            var entity = new TestEntity(level);
            entity.setPos(start.get(0).getAsDouble(), start.get(1).getAsDouble(), start.get(2).getAsDouble());
            var camera = new Camera();
            cameraMinecraft.set(camera, minecraft);
            cameraProbe.set(camera, new NoopProbe());
            camera.setLevel(level); camera.setEntity(entity);
            eye.setFloat(camera, object.get("eye").getAsFloat());
            oldEye.setFloat(camera, object.get("eye").getAsFloat());
            fovModifier.setFloat(camera, 1.0f);
            oldFovModifier.setFloat(camera, 1.0f);
            for (var stepElement : object.getAsJsonArray("steps")) {
                var step = stepElement.getAsJsonObject();
                var position = step.getAsJsonArray("position");
                entity.xo = entity.getX(); entity.yo = entity.getY(); entity.zo = entity.getZ();
                entity.setPos(position.get(0).getAsDouble(), position.get(1).getAsDouble(), position.get(2).getAsDouble());
                entityEye.setFloat(entity, step.get("eye").getAsFloat());
                player.speed = step.get("sprinting").getAsBoolean() ? 0.1 * 1.3 : 0.1;
                camera.tick();
                for (var partial : step.getAsJsonArray("partials")) {
                    align.invoke(camera, partial.getAsFloat());
                    var pos = camera.position(); var result = new JsonArray();
                    result.add(pos.x); result.add(pos.y); result.add(pos.z);
                    result.add(net.minecraft.util.Mth.lerp(partial.getAsFloat(), oldFovModifier.getFloat(camera), fovModifier.getFloat(camera)));
                    results.add(result);
                }
            }
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
        "camera oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn position_and_eye_height_match_vanilla_camera() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("CameraOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));

    let mut scenarios = Vec::new();
    let mut expected = Vec::new();
    let partials = [0.0_f32, 0.125, 0.333, 0.5, 0.875, 1.0];
    for scenario in 0..20 {
        let start = if scenario % 2 == 0 {
            DVec3::new(16_000_000.123_456_79, 70.125, -16_000_000.987_654_32)
        } else {
            DVec3::new(-3.25, -12.5, 7.125)
        };
        let initial_eye = 1.62_f32;
        let mut camera = CameraMotion::new(start, initial_eye);
        let mut steps = Vec::new();
        for tick in 0..30 {
            let i = f64::from(tick + 1);
            let position = start + DVec3::new(i * 0.03125, i.sin() * 0.25, -i * 0.0175);
            let target_eye = match (scenario + tick / 3) % 3 {
                0 => 1.62_f32,
                1 => 1.27_f32,
                _ => 0.4_f32,
            };
            let sprinting = (scenario + tick / 4) % 3 == 0;
            camera.tick(position, target_eye, sprinting);
            expected.extend(
                partials
                    .iter()
                    .map(|&partial| (camera.eye(partial), camera.fov_modifier(partial))),
            );
            steps.push(json!({ "position": position.to_array(), "eye": target_eye, "partials": partials, "sprinting": sprinting }));
        }
        scenarios.push(json!({ "start": start.to_array(), "eye": initial_eye, "steps": steps }));
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&scenarios).unwrap()).unwrap();
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("CameraOracle")
        .arg(&input)
        .arg(&output));
    let actual: Vec<[f64; 4]> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, (expected, expected_fov))) in actual.into_iter().zip(expected).enumerate()
    {
        let actual_fov = actual[3] as f32;
        let actual = DVec3::new(actual[0], actual[1], actual[2]);
        assert!(
            (actual - expected).abs().max_element() < 1.0e-8,
            "case {index}: Java {actual:?}, Rust {expected:?}"
        );
        assert!(
            (actual_fov - expected_fov).abs() < 1.0e-6,
            "case {index}: Java FOV {actual_fov}, Rust FOV {expected_fov}"
        );
    }
}

const BOB_ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.entity.ClientAvatarState;
import net.minecraft.client.renderer.GameRenderer;
import net.minecraft.client.renderer.state.level.CameraRenderState;
import net.minecraft.client.renderer.state.level.CameraEntityRenderState;
import net.minecraft.util.Mth;
import net.minecraft.world.phys.Vec3;

public class BobOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var renderer = (GameRenderer) unsafe.allocateInstance(GameRenderer.class);
        Method bobView = GameRenderer.class.getDeclaredMethod("bobView", CameraRenderState.class, PoseStack.class);
        bobView.setAccessible(true);
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var scenario : input) {
            var avatar = new ClientAvatarState();
            for (var value : scenario.getAsJsonArray()) {
                var step = value.getAsJsonObject();
                var v = step.getAsJsonArray("velocity");
                var d = step.getAsJsonArray("displacement");
                var velocity = new Vec3(v.get(0).getAsDouble(), 0, v.get(1).getAsDouble());
                avatar.tick(Vec3.ZERO, velocity);
                float target = step.get("grounded").getAsBoolean() ? Math.min(0.1f, (float)velocity.horizontalDistance()) : 0.0f;
                avatar.updateBob(target);
                avatar.addWalkDistance(Mth.length(d.get(0).getAsFloat(), d.get(1).getAsFloat()) * 0.6f);
                for (var p : step.getAsJsonArray("partials")) {
                    float partial = p.getAsFloat();
                    var camera = new CameraRenderState();
                    camera.entityRenderState = new CameraEntityRenderState();
                    camera.entityRenderState.isPlayer = true;
                    camera.entityRenderState.bob = avatar.getInterpolatedBob(partial);
                    camera.entityRenderState.backwardsInterpolatedWalkDistance = avatar.getBackwardsInterpolatedWalkDistance(partial);
                    var pose = new PoseStack();
                    bobView.invoke(renderer, camera, pose);
                    float[] matrix = pose.last().pose().get(new float[16]);
                    var row = new JsonArray();
                    for (float component : matrix) row.add(component);
                    output.add(row);
                }
            }
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
"#;

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn walking_camera_matrix_matches_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("BobOracle.java");
    std::fs::write(&source, BOB_ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let partials = [0.0_f32, 0.125, 0.333, 0.5, 0.875, 1.0];
    let mut scenarios = Vec::new();
    let mut expected = Vec::new();
    for scenario in 0..20 {
        let mut camera = CameraMotion::new(DVec3::ZERO, 1.62);
        let mut steps = Vec::new();
        for tick in 0..30 {
            let t = f64::from(tick);
            let velocity = DVec3::new((t * 0.3).sin() * 0.15, 0.0, (t * 0.11).cos() * 0.08);
            let displacement = DVec3::new((t * 0.25).sin() * 0.2, 0.0, (t * 0.17).cos() * 0.13);
            let grounded = (scenario + tick / 3) % 4 != 0;
            camera.begin_bob_tick(velocity, grounded);
            camera.finish_bob_tick(displacement);
            expected.extend(
                partials
                    .iter()
                    .map(|&partial| camera.bob_transform(partial).to_cols_array()),
            );
            steps.push(json!({"velocity": [velocity.x, velocity.z], "displacement": [displacement.x, displacement.z], "grounded": grounded, "partials": partials}));
        }
        scenarios.push(steps);
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&scenarios).unwrap()).unwrap();
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("BobOracle")
        .arg(&input)
        .arg(&output));
    let actual: Vec<[f32; 16]> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.into_iter().zip(expected).enumerate() {
        for component in 0..16 {
            assert!(
                (actual[component] - expected[component]).abs() < 0.00001,
                "case {index}, component {component}: Java {}, Rust {}",
                actual[component],
                expected[component]
            );
        }
    }
}
