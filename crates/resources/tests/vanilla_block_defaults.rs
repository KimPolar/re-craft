//! Compare the optional local registry report with the installed vanilla registry.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_resources::{block_registry, default_minecraft_dir};

const ORACLE: &str = r#"
import java.io.PrintStream;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
public class BlockDefaultsOracle {
    public static void main(String[] args) {
        PrintStream output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (var block : BuiltInRegistries.BLOCK) {
            var values = block.defaultBlockState().getValues()
                .map(value -> value.property().getName() + "=" + value.valueName()).sorted().toList();
            output.println(BuiltInRegistries.BLOCK.getKey(block) + "\t" + String.join("\t", values));
        }
    }
}
"#;

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
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
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to Java 25 JDK"))
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java registry oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft 26.3, Java 25 JDK and a prepared block report"]
fn local_block_report_matches_all_vanilla_default_states() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_minecraft_dir().unwrap());
    let report = std::env::var_os("MINECRAFT_BLOCK_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local/block-registry")
        });
    let defaults = block_registry::load(&game, &report).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("BlockDefaultsOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BlockDefaultsOracle"));
    let mut expected = BTreeMap::new();
    for line in output.lines().filter(|line| line.starts_with("minecraft:")) {
        let mut parts = line.split('\t');
        let id = parts.next().unwrap().to_owned();
        let properties = parts
            .filter(|part| !part.is_empty())
            .map(|part| {
                let (name, value) = part.split_once('=').unwrap();
                (name.to_owned(), value.to_owned())
            })
            .collect();
        assert!(expected.insert(id, properties).is_none());
    }
    assert!(!expected.is_empty());
    assert_eq!(defaults, expected);
}
