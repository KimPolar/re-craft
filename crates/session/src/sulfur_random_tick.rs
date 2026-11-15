//! The verified no-growth branches of Java 26.3 sulfur-spike random ticks.

use anyhow::{Result, bail, ensure};
use glam::IVec3;
use native_core::random::LegacyRandom;
use native_world::loaded::BlockState;

const GROWTH_PROBABILITY: f32 = 0.011_377_778;

pub(crate) trait SulfurWorld {
    fn spike_is_tagged(&mut self) -> Result<bool>;
    fn sulfur_above(&mut self, position: IVec3) -> Result<bool>;
}

/// Consume the same level RNG as Java when a spike provably cannot grow.
/// Possible growth remains an error until placement and callbacks are handled.
pub(crate) fn random_tick(
    world: &mut impl SulfurWorld,
    state: &BlockState,
    position: IVec3,
    random: &mut LegacyRandom,
) -> Result<()> {
    ensure!(
        state.name == "minecraft:sulfur_spike"
            && state.properties.len() == 3
            && matches!(
                state.properties.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            )
            && matches!(
                state
                    .properties
                    .get("vertical_direction")
                    .map(String::as_str),
                Some("up" | "down")
            )
            && matches!(
                state.properties.get("thickness").map(String::as_str),
                Some("tip" | "tip_merge" | "frustum" | "middle" | "base")
            ),
        "invalid sulfur spike state"
    );
    if random.next_float() >= GROWTH_PROBABILITY
        || state.properties["vertical_direction"] == "up"
        || !world.spike_is_tagged()?
        || !world.sulfur_above(position)?
    {
        return Ok(());
    }
    bail!("sulfur spike growth at {position:?} is not yet implemented")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    struct World {
        tagged: Option<bool>,
        sulfur: Option<bool>,
    }

    impl SulfurWorld for World {
        fn spike_is_tagged(&mut self) -> Result<bool> {
            self.tagged
                .ok_or_else(|| anyhow::anyhow!("tag unavailable"))
        }
        fn sulfur_above(&mut self, _: IVec3) -> Result<bool> {
            self.sulfur
                .ok_or_else(|| anyhow::anyhow!("support unavailable"))
        }
    }

    fn spike(direction: &str, thickness: &str, wet: bool) -> BlockState {
        BlockState {
            name: "minecraft:sulfur_spike".into(),
            properties: BTreeMap::from([
                ("vertical_direction".into(), direction.into()),
                ("thickness".into(), thickness.into()),
                ("waterlogged".into(), wet.to_string()),
            ]),
        }
    }

    #[test]
    fn random_tick_only_queries_needed_data_and_rejects_possible_growth() {
        let seed = (0..100_000_i64)
            .find(|seed| LegacyRandom::new(*seed * 1_000_003).next_float() < GROWTH_PROBABILITY)
            .unwrap()
            * 1_000_003;
        for (direction, tagged, sulfur, succeeds) in [
            ("up", None, None, true),
            ("down", Some(false), None, true),
            ("down", Some(true), Some(false), true),
            ("down", Some(true), Some(true), false),
            ("down", None, None, false),
            ("down", Some(true), None, false),
        ] {
            let mut random = LegacyRandom::new(seed);
            let mut expected = random.clone();
            expected.next_float();
            let result = random_tick(
                &mut World { tagged, sulfur },
                &spike(direction, "tip", false),
                IVec3::ZERO,
                &mut random,
            );
            assert_eq!(
                result.is_ok(),
                succeeds,
                "{direction} {tagged:?} {sulfur:?}"
            );
            assert_eq!(random.next_int(), expected.next_int());
        }
        let mut invalid = spike("down", "tip", false);
        invalid
            .properties
            .insert("waterlogged".into(), "invalid".into());
        assert!(
            random_tick(
                &mut World {
                    tagged: None,
                    sulfur: None
                },
                &invalid,
                IVec3::ZERO,
                &mut LegacyRandom::new(seed),
            )
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn no_growth_branches_match_java_26_3() -> Result<()> {
        use anyhow::{Context, ensure};
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "unexpected Minecraft version");
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"
            )).join("bin");
        let source = temp.path().join("SulfurSpikeRandomOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/SulfurSpikeRandomOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let checked = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("SulfurSpikeRandomOracle")
            .output()?;
        ensure!(
            checked.status.success(),
            "Java execution failed: {}",
            String::from_utf8_lossy(&checked.stderr)
        );
        let stdout = String::from_utf8(checked.stdout)?;
        let mut cases = 0;
        for line in stdout.lines().filter(|line| line.starts_with("TICK\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            ensure!(fields.len() == 8, "invalid oracle row");
            let seed: i64 = fields[1].parse()?;
            let wet: bool = fields[4].parse()?;
            let tagged: bool = fields[5].parse()?;
            let support: u8 = fields[6].parse()?;
            let expected: i32 = fields[7].parse()?;
            let mut random = LegacyRandom::new(seed);
            random_tick(
                &mut World {
                    tagged: Some(tagged),
                    sulfur: Some(support == 3),
                },
                &spike(fields[2], fields[3], wet),
                IVec3::ZERO,
                &mut random,
            )?;
            assert_eq!(random.next_int(), expected, "{line}");
            cases += 1;
        }
        assert!(cases > 20_000, "only {cases} Java cases");
        eprintln!("Compared {cases} sulfur-spike no-growth ticks with Java 26.3");
        Ok(())
    }
}
