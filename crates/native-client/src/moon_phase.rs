//! The non-interpolated 26.3 moon phase environment attribute.

pub use native_resources::moon_phase::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    #[test]
    fn phases_apply_dimension_biome_and_timeline_in_order() {
        let mut environment = Environment {
            base: layer(&json!("new_moon")).unwrap(),
            biomes: BTreeMap::from([("test:plains".into(), Some(Some(Phase::FirstQuarter)))]),
            has_biome_layer: true,
            tracks: Vec::new(),
        };
        assert_eq!(
            environment.sample(Some("test:plains"), |_| None),
            Some(Phase::FirstQuarter)
        );
        assert_eq!(environment.sample(None, |_| None), None);
        environment.tracks.push(Track::parse(&json!({"clock":"test:clock", "period_ticks":20}),
            &json!({"keyframes":[{"ticks":0,"value":"full_moon"},{"ticks":10,"value":"new_moon"}]})).unwrap());
        for (tick, expected) in [
            (-1, Phase::NewMoon),
            (0, Phase::FullMoon),
            (9, Phase::FullMoon),
            (10, Phase::NewMoon),
            (19, Phase::NewMoon),
            (20, Phase::FullMoon),
        ] {
            assert_eq!(
                environment.sample(Some("test:plains"), |_| Some(tick)),
                Some(expected)
            );
        }
        assert_eq!(environment.sample(Some("test:plains"), |_| None), None);
        assert!(layer(&json!({"modifier":"add","argument":"new_moon"})).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn phase_timeline_matches_java() {
        use native_resources::packs::{ResourceProvider, ResourceStack};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let timeline: Value = serde_json::from_slice(
            &resources
                .read_data("data/minecraft/timeline/moon.json")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let value = &timeline["tracks"]["minecraft:visual/moon_phase"];
        let track = Track::parse(&timeline, value).unwrap();
        let mut ticks: Vec<i64> = (-1000..=193000).step_by(293).collect();
        for i in -1..=9 {
            for delta in [-1, 0, 1] {
                ticks.push(i * 24000 + delta);
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("MoonPhaseOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/MoonPhaseOracle.java")).unwrap();
        std::fs::write(
            &input,
            serde_json::to_vec(&json!({"track":value,"ticks":ticks})).unwrap(),
        )
        .unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("MoonPhaseOracle")
                .arg(&input)
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let expected: Vec<usize> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), ticks.len() + 4);
        for (&tick, &index) in ticks.iter().zip(&expected) {
            assert_eq!(
                track.sample(|_| Some(tick)),
                Some(Phase::ALL[index]),
                "tick {tick}"
            );
        }
        assert_eq!(&expected[ticks.len()..], &[4, 4, 4, 4]);
    }
}
