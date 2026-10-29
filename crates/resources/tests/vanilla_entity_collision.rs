//! Compare the runtime entity-class collision classification against Java 26.3.

use native_resources::{entity_collision::EntityCollisionKind, packs::ResourceStack};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.EntityTypes;

public final class EntityCollisionRegistryOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonObject result = new JsonObject();
        for (Field field : EntityTypes.class.getFields()) {
            if (field.getType() != EntityType.class) continue;
            EntityType<?> type = (EntityType<?>) field.get(null);
            String id = BuiltInRegistries.ENTITY_TYPE.getKey(type).toString();
            if (!id.equals("minecraft:" + field.getName().toLowerCase(Locale.ROOT)))
                throw new AssertionError("field does not match registry ID: " + field.getName() + " " + id);
            ParameterizedType generic = (ParameterizedType)field.getGenericType();
            Class<?> entityClass = (Class<?>)generic.getActualTypeArguments()[0];
            Class<?> declaring = entityClass.getMethod("canBeCollidedWith", Entity.class).getDeclaringClass();
            result.addProperty(id, declaring == Entity.class ? "never" : "state");
        }
        if (result.size() != BuiltInRegistries.ENTITY_TYPE.size()) throw new AssertionError("incomplete fields");
        Files.writeString(Path.of(args[0]), new Gson().toJson(result));
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

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "entity oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn entity_type_collision_inheritance_matches_the_installed_java_registry() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut resources = ResourceStack::open(&game).unwrap();
    let started = std::time::Instant::now();
    let registry = resources.vanilla_entity_collision_registry().unwrap();
    let load_time = started.elapsed();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("EntityCollisionRegistryOracle.java");
    let output = temp.path().join("types.json");
    std::fs::write(&source, ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("EntityCollisionRegistryOracle")
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let expected = expected.as_object().unwrap();
    assert_eq!(registry.iter().count(), expected.len());
    for (id, kind) in registry.iter() {
        assert_eq!(
            Some(match kind {
                EntityCollisionKind::Never => "never",
                EntityCollisionKind::RequiresEntityState => "state",
            }),
            expected.get(id).and_then(Value::as_str),
            "{id}"
        );
    }
    println!(
        "Compared {} entity type collision declarations against Java; local initial read: {load_time:?}",
        expected.len(),
    );
}
