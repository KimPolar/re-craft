//! Frosted ice scheduled-tick rules. The caller supplies staged world access so
//! block notifications and incomplete chunk/light data can be handled safely.
use anyhow::{Result, ensure};
use glam::IVec3;
use native_core::random::LegacyRandom;

const DIRECTIONS: [IVec3; 6] = [
    IVec3::NEG_Y,
    IVec3::Y,
    IVec3::NEG_Z,
    IVec3::Z,
    IVec3::NEG_X,
    IVec3::X,
];

fn offset(position: IVec3, direction: IVec3) -> IVec3 {
    IVec3::new(
        position.x.wrapping_add(direction.x),
        position.y.wrapping_add(direction.y),
        position.z.wrapping_add(direction.z),
    )
}

pub(crate) trait IceWorld {
    /// None is a known non-frosted-ice state; unavailable data is an error.
    fn ice_age(&self, position: IVec3) -> Result<Option<u8>>;
    /// Java's getBrightness(BLOCK) in the End, getMaxLocalRawBrightness elsewhere.
    fn brightness(&self, position: IVec3) -> Result<u8>;
    fn set_age(&mut self, position: IVec3, age: u8) -> Result<()>;
    /// Melt must apply block notifications before returning, as Java does.
    fn melt(&mut self, position: IVec3) -> Result<()>;
    fn schedule(&mut self, position: IVec3, delay: i32) -> Result<()>;
}

fn fewer_than(world: &impl IceWorld, position: IVec3, limit: usize) -> Result<bool> {
    let mut count = 0;
    for direction in DIRECTIONS {
        if world.ice_age(offset(position, direction))?.is_some() {
            count += 1;
            if count >= limit {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn slightly_melt(world: &mut impl IceWorld, position: IVec3, age: u8) -> Result<bool> {
    ensure!(age <= 3, "invalid frosted ice age");
    if age < 3 {
        world.set_age(position, age + 1)?;
        Ok(false)
    } else {
        world.melt(position)?;
        Ok(true)
    }
}

fn delay(random: &mut LegacyRandom, min: i32, max: i32) -> i32 {
    min + random
        .next_int_bound(max - min + 1)
        .expect("positive range")
}

/// Called when frosted ice is first placed, including Frost Walker placement.
#[cfg(test)]
pub(crate) fn placed(random: &mut LegacyRandom) -> i32 {
    delay(random, 60, 120)
}

/// Execute one scheduled tick. The caller must roll back the world and RNG if
/// a callback fails, since Java's mutation and notifications occur in order.
pub(crate) fn tick(
    world: &mut impl IceWorld,
    position: IVec3,
    age: u8,
    random: &mut LegacyRandom,
) -> Result<()> {
    ensure!(age <= 3, "invalid frosted ice age");
    if (random.next_int_bound(3).expect("positive bound") == 0 || fewer_than(world, position, 4)?)
        && i32::from(world.brightness(position)?) > 11 - i32::from(age) - 1
        && slightly_melt(world, position, age)?
    {
        for direction in DIRECTIONS {
            let neighbor = offset(position, direction);
            if let Some(age) = world.ice_age(neighbor)?
                && !slightly_melt(world, neighbor, age)?
            {
                world.schedule(neighbor, delay(random, 20, 40))?;
            }
        }
    } else {
        world.schedule(position, delay(random, 20, 40))?;
    }
    Ok(())
}

/// A neighbor update from another frosted-ice block can melt an isolated block.
#[cfg(test)]
pub(crate) fn neighbor_changed(
    world: &mut impl IceWorld,
    position: IVec3,
    changed_by_ice: bool,
) -> Result<()> {
    if changed_by_ice && fewer_than(world, position, 2)? {
        world.melt(position)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct World {
        ice: BTreeMap<[i32; 3], u8>,
        light: u8,
        events: Vec<serde_json::Value>,
    }

    struct CascadeWorld {
        ice: BTreeMap<[i32; 3], u8>,
        events: Vec<serde_json::Value>,
    }
    impl IceWorld for CascadeWorld {
        fn ice_age(&self, position: IVec3) -> Result<Option<u8>> {
            Ok(self.ice.get(&position.to_array()).copied())
        }
        fn brightness(&self, _: IVec3) -> Result<u8> {
            Ok(15)
        }
        fn set_age(&mut self, position: IVec3, age: u8) -> Result<()> {
            self.ice.insert(position.to_array(), age);
            self.events.push(serde_json::json!([
                "age", position.x, position.y, position.z, age
            ]));
            Ok(())
        }
        fn melt(&mut self, position: IVec3) -> Result<()> {
            self.ice.remove(&position.to_array());
            self.events.push(serde_json::json!([
                "melt", position.x, position.y, position.z
            ]));
            // Level.updateNeighborsAt uses WEST, EAST, DOWN, UP, NORTH, SOUTH.
            for direction in [
                IVec3::NEG_X,
                IVec3::X,
                IVec3::NEG_Y,
                IVec3::Y,
                IVec3::NEG_Z,
                IVec3::Z,
            ] {
                let neighbor = offset(position, direction);
                if self.ice_age(neighbor)?.is_some() {
                    neighbor_changed(self, neighbor, true)?;
                }
            }
            Ok(())
        }
        fn schedule(&mut self, position: IVec3, delay: i32) -> Result<()> {
            self.events.push(serde_json::json!([
                "schedule", position.x, position.y, position.z, delay
            ]));
            Ok(())
        }
    }
    impl IceWorld for World {
        fn ice_age(&self, position: IVec3) -> Result<Option<u8>> {
            Ok(self.ice.get(&position.to_array()).copied())
        }
        fn brightness(&self, _: IVec3) -> Result<u8> {
            Ok(self.light)
        }
        fn set_age(&mut self, position: IVec3, age: u8) -> Result<()> {
            self.ice.insert(position.to_array(), age);
            self.events
                .push(serde_json::json!(["age", index(position), age]));
            Ok(())
        }
        fn melt(&mut self, position: IVec3) -> Result<()> {
            self.ice.remove(&position.to_array());
            self.events
                .push(serde_json::json!(["melt", index(position)]));
            Ok(())
        }
        fn schedule(&mut self, position: IVec3, delay: i32) -> Result<()> {
            self.events
                .push(serde_json::json!(["schedule", index(position), delay]));
            Ok(())
        }
    }
    fn index(position: IVec3) -> i32 {
        let center = IVec3::new(0, 10, 0);
        if position == center {
            -1
        } else {
            DIRECTIONS
                .iter()
                .position(|dir| center + *dir == position)
                .unwrap() as i32
        }
    }

    #[test]
    fn isolated_neighbor_updates_melt_only_when_caused_by_ice() {
        let center = IVec3::new(0, 10, 0);
        let mut world = World {
            ice: BTreeMap::from([(center.to_array(), 2)]),
            light: 0,
            events: Vec::new(),
        };
        neighbor_changed(&mut world, center, false).unwrap();
        assert!(world.events.is_empty());
        neighbor_changed(&mut world, center, true).unwrap();
        assert_eq!(world.events, vec![serde_json::json!(["melt", -1])]);
        for seed in 0..100 {
            assert!((60..=120).contains(&placed(&mut LegacyRandom::new(seed))));
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn scheduled_ice_tick_matches_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("FrostedIceTickOracle.java");
        let output = temp.path().join("ice.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FrostedIceTickOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("FrostedIceTickOracle")
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
        let output: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let rows = output["ticks"].as_array().unwrap();
        assert_eq!(rows.len(), 16 * 4 * 7 * 6 * 2);
        let center = IVec3::new(0, 10, 0);
        for row in rows {
            let seed = row["seed"].as_i64().unwrap();
            let age = row["age"].as_u64().unwrap() as u8;
            let mask = row["mask"].as_u64().unwrap();
            let mut world = World {
                ice: BTreeMap::from([(center.to_array(), age)]),
                light: if row["end"].as_bool().unwrap() {
                    row["light"].as_u64().unwrap() as u8
                } else {
                    15 - row["light"].as_u64().unwrap() as u8
                },
                events: Vec::new(),
            };
            for (i, direction) in DIRECTIONS.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    world.ice.insert(
                        (center + *direction).to_array(),
                        ((seed + i as i64) % 4) as u8,
                    );
                }
            }
            let mut random = LegacyRandom::new(seed);
            tick(&mut world, center, age, &mut random).unwrap();
            assert_eq!(
                world.events,
                row["events"].as_array().unwrap().to_vec(),
                "{row}"
            );
            assert_eq!(
                random.next_int(),
                row["next"].as_i64().unwrap() as i32,
                "{row}"
            );
        }
        let placements = output["placements"].as_array().unwrap();
        assert_eq!(placements.len(), 128);
        for row in placements {
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let delay = placed(&mut random);
            assert_eq!(
                row["events"],
                serde_json::json!([["schedule", -1, delay]]),
                "{row}"
            );
            assert_eq!(
                random.next_int(),
                row["next"].as_i64().unwrap() as i32,
                "{row}"
            );
        }
        let neighbors = output["neighbors"].as_array().unwrap();
        assert_eq!(neighbors.len(), 64 * 4 * 2);
        for row in neighbors {
            let mask = row["mask"].as_u64().unwrap();
            let age = row["age"].as_u64().unwrap() as u8;
            let mut world = World {
                ice: BTreeMap::from([(center.to_array(), age)]),
                light: 0,
                events: Vec::new(),
            };
            for (i, direction) in DIRECTIONS.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    world.ice.insert((center + *direction).to_array(), 0);
                }
            }
            neighbor_changed(&mut world, center, row["changed_by_ice"].as_bool().unwrap()).unwrap();
            assert_eq!(
                world.events,
                row["events"].as_array().unwrap().to_vec(),
                "{row}"
            );
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn ice_neighbor_cascades_match_java_updater() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("FrostedIceCascadeOracle.java");
        let output = temp.path().join("cascade.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FrostedIceCascadeOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("FrostedIceCascadeOracle")
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
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 32 * 64 * 2);
        crate::loaded::tests::compare_ice_stage_with_java(&rows);
        let center = IVec3::new(0, 10, 0);
        for row in rows {
            let seed = row["seed"].as_i64().unwrap();
            let mask = row["mask"].as_u64().unwrap();
            let outer = row["outer"].as_i64().unwrap() != 0;
            let mut world = CascadeWorld {
                ice: BTreeMap::from([(center.to_array(), 3)]),
                events: Vec::new(),
            };
            for (i, direction) in DIRECTIONS.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    let pos = offset(center, *direction);
                    world
                        .ice
                        .insert(pos.to_array(), ((seed + i as i64) % 4) as u8);
                    if outer {
                        world.ice.insert(
                            offset(pos, *direction).to_array(),
                            ((seed + i as i64 + 1) % 4) as u8,
                        );
                    }
                }
            }
            let mut random = LegacyRandom::new(seed);
            tick(&mut world, center, 3, &mut random).unwrap();
            assert_eq!(
                world.events,
                row["events"].as_array().unwrap().to_vec(),
                "{row}"
            );
            assert_eq!(
                random.next_int(),
                row["next"].as_i64().unwrap() as i32,
                "{row}"
            );
        }
    }
}
