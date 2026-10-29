//! Compare enchantment attribute calculations with the installed 26.3 client.

use native_resources::{
    enchantments::{EquipmentSlot, OxygenEnchantments},
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.resources.Identifier;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.item.enchantment.LevelBasedValue;
import net.minecraft.world.item.enchantment.effects.EnchantmentAttributeEffect;

public class OxygenOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var values = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var result = new JsonArray();
        for (var input : values) {
            var amount = LevelBasedValue.CODEC.parse(JsonOps.INSTANCE, input).getOrThrow();
            var effect = new EnchantmentAttributeEffect(Identifier.parse("example:oxygen"), Attributes.OXYGEN_BONUS, amount, AttributeModifier.Operation.ADD_VALUE);
            var row = new JsonArray();
            for (int level : new int[]{1, 2, 3, 8, 255}) {
                var modifier = effect.getModifier(level, EquipmentSlot.HEAD);
                var entry = new JsonArray(); entry.add(modifier.id().toString()); entry.add(Float.floatToIntBits((float) modifier.amount())); row.add(entry);
            }
            result.add(row);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(result));
    }
}
"#;

struct Data(Value);
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn list_data(&mut self, _: &str) -> anyhow::Result<Vec<ResourceId>> {
        Ok(vec![ResourceId::parse("example:test")?])
    }
    fn read_data(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(Some(serde_json::to_vec(
            &json!({"slots":["head"],"effects":{"minecraft:attributes":[{"attribute":"minecraft:oxygen_bonus","id":"example:oxygen","operation":"add_value","amount":self.0}]}}),
        )?))
    }
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in metadata["libraries"].as_array().unwrap() {
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
        "Java enchantment oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn enchantment_level_values_and_vanilla_respiration_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut builtin = ResourceStack::open(&game).unwrap();
    let registry = OxygenEnchantments::load(&mut builtin).unwrap();
    let respiration = ResourceId::parse("minecraft:respiration").unwrap();
    for level in 1..=3 {
        let result = registry
            .modifiers(&respiration, level, EquipmentSlot::Head)
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].amount, f64::from(level));
        assert_eq!(
            result[0].id,
            ResourceId::parse("minecraft:enchantment.respiration/head").unwrap()
        );
        assert!(
            registry
                .modifiers(&respiration, level, EquipmentSlot::Chest)
                .unwrap()
                .is_empty()
        );
    }
    let values = vec![
        json!(2.5),
        json!({"type":"minecraft:linear","base":1.25,"per_level_above_first":0.125}),
        json!({"type":"minecraft:levels_squared","added":-2.5}),
        json!({"type":"minecraft:fraction","numerator":{"type":"minecraft:linear","base":3.0,"per_level_above_first":0.25},"denominator":7.0}),
        json!({"type":"minecraft:fraction","numerator":5.0,"denominator":0.0}),
        json!({"type":"minecraft:exponent","base":1.5,"power":{"type":"minecraft:linear","base":1.0,"per_level_above_first":0.125}}),
        json!({"type":"minecraft:clamped","min":2.0,"max":6.0,"value":{"type":"minecraft:linear","base":1.0,"per_level_above_first":1.0}}),
        json!({"type":"minecraft:lookup","values":[3.0,5.0,8.0],"fallback":{"type":"minecraft:levels_squared","added":0.0}}),
    ];
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("OxygenOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&values).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("OxygenOracle")
        .arg(&input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (index, value) in values.into_iter().enumerate() {
        let registry = OxygenEnchantments::load(&mut Data(value)).unwrap();
        for (slot, level) in [1, 2, 3, 8, 255].into_iter().enumerate() {
            let actual = registry
                .modifiers(
                    &ResourceId::parse("example:test").unwrap(),
                    level,
                    EquipmentSlot::Head,
                )
                .unwrap();
            assert_eq!(
                format!("{}:{}", actual[0].id.namespace, actual[0].id.path),
                expected[index][slot][0].as_str().unwrap()
            );
            assert_eq!(
                (actual[0].amount as f32).to_bits() as i32,
                expected[index][slot][1].as_i64().unwrap() as i32,
                "value {index} level {level}"
            );
        }
    }
}
