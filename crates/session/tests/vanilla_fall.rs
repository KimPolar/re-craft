//! Compare fall damage rounding and attribute multipliers with Java 26.3.

use native_session::fall::calculate_fall_damage;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.core.Holder;
import net.minecraft.tags.TagKey;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.ai.attributes.*;

public class FallOracle {
    static class TestPlayer extends ServerPlayer {
        double safe, multiplier;
        TestPlayer() { super(null, null, null, null); }
        @Override public boolean is(TagKey<EntityType<?>> tag) { return false; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) {
            if (attribute == Attributes.SAFE_FALL_DISTANCE) return safe;
            if (attribute == Attributes.FALL_DAMAGE_MULTIPLIER) return multiplier;
            throw new AssertionError(attribute);
        }
        int damage(double distance, float factor) { return calculateFallDamage(distance, factor); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        TestPlayer player = (TestPlayer)((sun.misc.Unsafe)f.get(null)).allocateInstance(TestPlayer.class);
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (var item : cases) {
            JsonObject c = item.getAsJsonObject();
            player.safe = c.get("safe").getAsDouble(); player.multiplier = c.get("attribute").getAsDouble();
            results.add(player.damage(c.get("distance").getAsDouble(), c.get("block").getAsFloat()));
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(results));
    }
}
"#;

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

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "fall oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn fall_calculation_matches_java_for_rounding_boundaries_and_attributes() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    let mut state = 0x1234_5678_u32;
    for index in 0..8192 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let safe = [-1024.0, -3.25, 0.0, 3.0, 4.0, 7.125, 1024.0][index % 7];
        let block = [0.0_f32, 0.2, 0.5, 1.0, 2.0][(index / 7) % 5];
        let attribute = [0.0, 0.01, 0.5, 1.0, 2.5, 100.0][(index / 35) % 6];
        let distance = if index % 3 == 0 {
            safe + f64::from((state % 25) as i32 - 3)
                + [-0.000_001_01, -0.000_001, -0.000_000_99, 0.0][(index / 210) % 4]
        } else {
            f64::from(state % 100_000) / 1000.0
        };
        cases.push(json!({"safe":safe,"block":block,"attribute":attribute,"distance":distance}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FallOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("FallOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<i32> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (case, result) in cases.iter().zip(expected) {
        assert_eq!(
            calculate_fall_damage(
                case["distance"].as_f64().unwrap(),
                case["safe"].as_f64().unwrap(),
                case["block"].as_f64().unwrap() as f32,
                case["attribute"].as_f64().unwrap()
            ),
            result,
            "{case}"
        );
    }
}
