//! Compare edge restraint with the installed 26.3 Player implementation.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use glam::DVec3;
use native_world::movement::{BoundingBox, back_off_from_edge};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.MoverType;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

public class SneakOracle {
    static final class TestLevel extends ClientLevel {
        List<AABB> boxes;
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public boolean noCollision(Entity e, AABB box) {
            for (AABB shape : boxes) if (shape.intersects(box)) return false;
            return true;
        }
    }
    static final class TestPlayer extends Player {
        boolean ground, shift;
        float step;
        TestPlayer() { super(null, null); }
        void setup(TestLevel level, AABB bounds, boolean grounded, boolean sneaking, float stepHeight, double falling) {
            setLevel(level); setBoundingBox(bounds); ground = grounded; shift = sneaking; step = stepHeight; fallDistance = falling;
        }
        @Override public boolean onGround() { return ground; }
        @Override public boolean isShiftKeyDown() { return shift; }
        @Override public float maxUpStep() { return step; }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
    }
    static AABB box(JsonArray v) { return new AABB(v.get(0).getAsDouble(),v.get(1).getAsDouble(),v.get(2).getAsDouble(),v.get(3).getAsDouble(),v.get(4).getAsDouble(),v.get(5).getAsDouble()); }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) f.get(null);
        TestLevel level = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        TestPlayer player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        Field abilities = Player.class.getDeclaredField("abilities"); abilities.setAccessible(true); abilities.set(player, new Abilities());
        Method edge = Player.class.getDeclaredMethod("maybeBackOffFromEdge", Vec3.class, MoverType.class); edge.setAccessible(true);
        JsonArray input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray output = new JsonArray();
        for (var entry : input) {
            var item = entry.getAsJsonObject(); level.boxes = new ArrayList<>();
            for (var shape : item.getAsJsonArray("shapes")) level.boxes.add(box(shape.getAsJsonArray()));
            player.setup(level, box(item.getAsJsonArray("bounds")), item.get("ground").getAsBoolean(), item.get("shift").getAsBoolean(), item.get("step").getAsFloat(), item.get("fall").getAsDouble());
            JsonArray v = item.getAsJsonArray("movement");
            Vec3 result = (Vec3) edge.invoke(player, new Vec3(v.get(0).getAsDouble(),v.get(1).getAsDouble(),v.get(2).getAsDouble()), MoverType.SELF);
            JsonArray resultVector = new JsonArray(); resultVector.add(result.x); resultVector.add(result.y); resultVector.add(result.z); output.add(resultVector);
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
        "sneak oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn vec3(value: &Value) -> DVec3 {
    DVec3::new(
        value[0].as_f64().unwrap(),
        value[1].as_f64().unwrap(),
        value[2].as_f64().unwrap(),
    )
}

fn bounds(value: &Value) -> BoundingBox {
    let a = value.as_array().unwrap();
    BoundingBox {
        min: DVec3::new(
            a[0].as_f64().unwrap(),
            a[1].as_f64().unwrap(),
            a[2].as_f64().unwrap(),
        ),
        max: DVec3::new(
            a[3].as_f64().unwrap(),
            a[4].as_f64().unwrap(),
            a[5].as_f64().unwrap(),
        ),
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn edge_restraint_matches_vanilla_player() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut scenarios = Vec::new();
    let mut random = 1234567_u32;
    let mut sample = || {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        f64::from(random >> 8) / f64::from(1_u32 << 24)
    };
    for index in 0..8192 {
        let x = sample() * 3.0 - 1.5;
        let z = sample() * 3.0 - 1.5;
        let y = if index % 4 == 0 { sample() * 0.5 } else { 0.0 };
        let half = f64::from(0.3_f32);
        let player = [x - half, y, z - half, x + half, y + 1.5, z + half];
        let shapes = if index % 5 == 0 {
            vec![
                [-1.0, -1.0, -1.0, 0.0, 0.0, 0.0],
                [0.0, -0.5, 0.0, 1.0, 0.0, 1.0],
            ]
        } else {
            vec![
                [0.0, -1.0, 0.0, 1.0, 0.0, 1.0],
                [-1.0, -1.5, 0.0, 0.0, -0.5, 1.0],
            ]
        };
        let movement = [
            sample() * 1.8 - 0.9,
            if index % 7 == 0 { 0.2 } else { -0.1 * sample() },
            sample() * 1.8 - 0.9,
        ];
        scenarios.push(json!({"bounds":player,"shapes":shapes,"movement":movement,"ground":index % 3 != 0,"shift":index % 5 != 0,"fall":sample() * 0.8,"step":if index%11==0 {0.0} else {0.6}}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SneakOracle.java");
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
        .arg("SneakOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(scenarios.len(), expected.len());
    let mut mismatches = Vec::new();
    let mut changed = 0;
    for (index, (input, expected)) in scenarios.iter().zip(&expected).enumerate() {
        let movement = vec3(&input["movement"]);
        let shapes: Vec<_> = input["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .map(bounds)
            .collect();
        let result = back_off_from_edge(
            bounds(&input["bounds"]),
            movement,
            input["ground"].as_bool().unwrap(),
            input["fall"].as_f64().unwrap(),
            input["step"].as_f64().unwrap() as f32,
            input["shift"].as_bool().unwrap(),
            |area| {
                !shapes
                    .iter()
                    .any(|shape| area.min.cmplt(shape.max).all() && area.max.cmpgt(shape.min).all())
            },
        );
        if result != movement {
            changed += 1;
        }
        if (result - vec3(expected)).abs().max_element() > 1.0e-12 && mismatches.len() < 20 {
            mismatches.push(format!(
                "case {index}: native {result:?}, Java {expected}, input {input}"
            ));
        }
    }
    assert!(changed > 100, "expected edge cases, only {changed} changed");
    assert!(
        mismatches.is_empty(),
        "edge mismatches:\n{}",
        mismatches.join("\n")
    );
    println!(
        "Compared {} edge movements with Minecraft 26.3; {changed} restrained",
        scenarios.len()
    );
}
