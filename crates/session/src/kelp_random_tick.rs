//! Java 26.3 kelp random-tick decisions before neighbor callbacks are applied.

use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_core::random::LegacyRandom;
use native_world::loaded::BlockState;

const GROW_PER_TICK_PROBABILITY: f64 = 0.14;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KelpPart {
    Head(u8),
    Body,
}

pub(crate) fn part(state: &BlockState) -> Option<KelpPart> {
    match state.name.as_str() {
        "minecraft:kelp" if state.properties.len() == 1 => {
            let value = state.properties.get("age")?;
            let age = value.parse::<u8>().ok()?;
            (age <= 25 && *value == age.to_string()).then_some(KelpPart::Head(age))
        }
        "minecraft:kelp_plant" if state.properties.is_empty() => Some(KelpPart::Body),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct KelpGrowth {
    pub position: IVec3,
    pub age: u8,
}

/// Draw the level random only when Java would, and inspect the upper block
/// only after the probability check. The caller must apply the returned head
/// together with its shape, fluid, light and neighbor effects.
pub(crate) fn random_tick(
    state: &BlockState,
    position: IVec3,
    random: &mut LegacyRandom,
    read_above: impl FnOnce(IVec3) -> Result<BlockState>,
) -> Result<Option<KelpGrowth>> {
    let Some(KelpPart::Head(age)) = part(state) else {
        anyhow::bail!("invalid kelp head state")
    };
    if age == 25 || random.next_double() >= GROW_PER_TICK_PROBABILITY {
        return Ok(None);
    }
    let above = IVec3::new(
        position.x,
        position
            .y
            .checked_add(1)
            .context("kelp position overflow")?,
        position.z,
    );
    let target = read_above(above)?;
    if target.name != "minecraft:water" {
        return Ok(None);
    }
    let level = target
        .properties
        .get("level")
        .context("water level is missing")?;
    let parsed: u8 = level.parse().context("invalid water level")?;
    ensure!(
        target.properties.len() == 1 && parsed <= 15 && *level == parsed.to_string(),
        "invalid water state"
    );
    Ok(Some(KelpGrowth {
        position: above,
        age: age + 1,
    }))
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, collections::BTreeMap};

    use super::*;

    fn head(age: u8) -> BlockState {
        BlockState {
            name: "minecraft:kelp".into(),
            properties: BTreeMap::from([("age".into(), age.to_string())]),
        }
    }

    fn block(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    #[test]
    fn random_tick_only_reads_above_when_growth_is_attempted() {
        for seed in 0..512_i64 {
            let seed = seed.wrapping_mul(1_000_003).wrapping_sub(17);
            for age in [0, 7, 24, 25] {
                let mut random = LegacyRandom::new(seed);
                let mut expected = random.clone();
                let attempted = age < 25 && expected.next_double() < GROW_PER_TICK_PROBABILITY;
                let read = Cell::new(false);
                let growth = random_tick(&head(age), IVec3::ZERO, &mut random, |position| {
                    assert_eq!(position, IVec3::Y);
                    read.set(true);
                    Ok(block("water", &[("level", "0")]))
                })
                .unwrap();
                assert_eq!(read.get(), attempted);
                assert_eq!(
                    growth.map(|growth| growth.age),
                    attempted.then_some(age + 1)
                );
                assert_eq!(random.next_int(), expected.next_int());
            }
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn decisions_and_random_state_match_java_26_3() -> Result<()> {
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(metadata["id"] == "26.3", "unexpected Minecraft version");
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"]
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
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                    "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
                )
            })
            .join("bin");
        let source = temp.path().join("KelpRandomTickOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/KelpRandomTickOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("KelpRandomTickOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java execution failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout)?;
        let mut cases = 0;
        for line in stdout.lines().filter(|line| line.starts_with("K ")) {
            let fields: Vec<_> = line.split_whitespace().collect();
            ensure!(fields.len() == 8, "invalid Java kelp row: {line}");
            let seed: i64 = fields[1].parse()?;
            let age: u8 = fields[2].parse()?;
            let kind: u8 = fields[3].parse()?;
            let expected_reads: usize = fields[4].parse()?;
            let expected_age: i16 = fields[5].parse()?;
            let expected_level_random_reads: usize = fields[6].parse()?;
            let expected_next: i32 = fields[7].parse()?;
            let above = match kind {
                0..=4 => block(
                    "water",
                    &[("level", ["0", "1", "7", "8", "15"][kind as usize])],
                ),
                5 => block("air", &[]),
                6 => block("stone", &[]),
                7 => block("kelp_plant", &[]),
                _ => anyhow::bail!("unknown Java kelp target"),
            };
            let reads = Cell::new(0);
            let mut random = LegacyRandom::new(seed);
            let growth = random_tick(&head(age), IVec3::ZERO, &mut random, |position| {
                assert_eq!(position, IVec3::Y);
                reads.set(reads.get() + 1);
                Ok(above)
            })?;
            assert_eq!(reads.get(), expected_reads, "{line}");
            assert_eq!(
                growth.map_or(-1, |growth| i16::from(growth.age)),
                expected_age,
                "{line}"
            );
            assert_eq!(
                usize::from(growth.is_some()),
                expected_level_random_reads,
                "{line}"
            );
            assert_eq!(random.next_int(), expected_next, "{line}");
            cases += 1;
        }
        ensure!(cases == 16_384, "unexpected Java kelp case count: {cases}");
        eprintln!("Compared {cases} kelp random ticks with Java 26.3");
        Ok(())
    }
}
