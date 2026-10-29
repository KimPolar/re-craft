//! Compare the reconstructed runtime IDs against the installed game's registry.
use native_resources::packs::ResourceStack;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn every_numeric_item_id_matches_the_installed_java_registry() {
    const JAVA: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.item.Item;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.component.DataComponents;
import net.minecraft.tags.DamageTypeTags;
public class NumericItemOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registry=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
    var out=new JsonObject();
    var fire=new JsonObject();
    var other=new JsonObject();
    for(var item:BuiltInRegistries.ITEM) out.addProperty(BuiltInRegistries.ITEM.getKey(item).toString(),Item.getId(item));
    for(var item:BuiltInRegistries.ITEM) {
      var resistant=item.components().get(DataComponents.DAMAGE_RESISTANT);
      fire.addProperty(BuiltInRegistries.ITEM.getKey(item).toString(),resistant!=null && resistant.types().unwrapKey().filter(DamageTypeTags.IS_FIRE::equals).isPresent());
      if(resistant!=null && resistant.types().unwrapKey().filter(DamageTypeTags.IS_FIRE::equals).isEmpty())
        other.addProperty(BuiltInRegistries.ITEM.getKey(item).toString(), resistant.types().unwrapKey().map(key -> key.location().toString()).orElse("direct"));
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
    Files.writeString(Path.of(args[1]),new Gson().toJson(fire));
    Files.writeString(Path.of(args[2]),new Gson().toJson(other));
  }
}

"#;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut stack = ResourceStack::open(&game).unwrap();
    let actual = stack.vanilla_item_registry().unwrap();
    let temp = tempfile::tempdir().unwrap();
    // Java classes are not resource-pack assets and must always come from the verified JAR.
    let fake_class = temp.path().join("net/minecraft/world/item/Items.class");
    std::fs::create_dir_all(fake_class.parent().unwrap()).unwrap();
    std::fs::write(&fake_class, b"not a Java class").unwrap();
    stack.push_pack(temp.path()).unwrap();
    let with_pack = stack.vanilla_item_registry().unwrap();
    assert_eq!(actual.iter().count(), with_pack.iter().count());
    for (name, id) in actual.iter() {
        assert_eq!(with_pack.id(name), Some(id));
        assert_eq!(
            with_pack.explicitly_fire_resistant(name),
            actual.explicitly_fire_resistant(name)
        );
    }
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("NumericItemOracle.java");
    let output = temp.path().join("output.json");
    let fire_output = temp.path().join("fire.json");
    let other_output = temp.path().join("other.json");
    std::fs::write(&source, JAVA).unwrap();
    let run = |cmd: &mut Command| {
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "Java oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("NumericItemOracle")
        .arg(&output)
        .arg(&fire_output)
        .arg(&other_output));
    let expected: std::collections::HashMap<String, i32> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(actual.iter().count(), expected.len());
    for (name, id) in expected {
        assert_eq!(actual.id(&name), Some(id), "{name}");
    }
    let fire: std::collections::HashMap<String, bool> =
        serde_json::from_slice(&std::fs::read(fire_output).unwrap()).unwrap();
    for (name, _) in actual.iter() {
        assert_eq!(
            fire.get(name).copied(),
            Some(actual.explicitly_fire_resistant(name)),
            "{name}"
        );
    }
    let extracted = actual
        .iter()
        .filter(|(name, _)| actual.explicitly_fire_resistant(name))
        .count();
    let java_count = fire.values().filter(|value| **value).count();
    eprintln!(
        "recognized fire-resistant prototypes: {extracted}; Java fire-tag prototypes: {java_count}"
    );
    let other: std::collections::HashMap<String, String> =
        serde_json::from_slice(&std::fs::read(other_output).unwrap()).unwrap();
    eprintln!("other default damage resistance types: {other:?}");
    use native_resources::item_registry::DamageResistancePrototype;
    for (name, _) in actual.iter() {
        let expected = if fire[name] {
            DamageResistancePrototype::FireTag
        } else if other.contains_key(name) {
            DamageResistancePrototype::Unknown
        } else {
            DamageResistancePrototype::Absent
        };
        assert_eq!(actual.damage_resistance_prototype(name), expected, "{name}");
    }
    assert_eq!(
        actual.damage_resistance_prototype("example:missing"),
        DamageResistancePrototype::Unknown
    );
}

#[test]
#[ignore = "CPU measurement using the installed Minecraft 26.3 client"]
fn measure_item_registry_reconstruction() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut stack = ResourceStack::open(&game).unwrap();
    let mut times = Vec::new();
    for _ in 0..25 {
        let started = std::time::Instant::now();
        let registry = stack.vanilla_item_registry().unwrap();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(&registry);
        assert!(registry.id("minecraft:diamond_sword").is_some());
        times.push(elapsed);
    }
    times.drain(..5);
    times.sort_by(f64::total_cmp);
    eprintln!(
        "item registry reconstruction: median {:.3} ms, p95 {:.3} ms (verified JAR already open)",
        times[10], times[18]
    );
}
