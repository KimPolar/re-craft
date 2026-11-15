//! The random fire attempts made by a lava block and its fluid in Java 26.3.

use anyhow::{Result, ensure};
use glam::{DVec3, IVec3};
use native_core::{leaf::LeafKind, random::LegacyRandom};

pub(crate) const MAX_CHUNK_COORDINATE: i32 = 2_097_061;

pub(crate) fn valid_horizontal(position: IVec3) -> bool {
    position.x.div_euclid(16).abs() <= MAX_CHUNK_COORDINATE
        && position.z.div_euclid(16).abs() <= MAX_CHUNK_COORDINATE
}

/// The world operations used by `LavaFluid.randomTick`. Implementations must
/// use the active block tags and keep callbacks on the same level RNG.
pub(crate) trait LavaFireWorld {
    type State;

    fn can_spread_fire(&mut self, position: IVec3) -> Result<bool>;
    fn is_loaded(&mut self, position: IVec3) -> Result<bool>;
    fn block(&mut self, position: IVec3) -> Result<Self::State>;
    fn is_air(&mut self, state: &Self::State) -> Result<bool>;
    fn blocks_spread(&mut self, state: &Self::State) -> Result<bool>;
    fn is_flammable(&mut self, position: IVec3) -> Result<bool>;
    fn set_fire(
        &mut self,
        position: IVec3,
        state_position: IVec3,
        random: &mut LegacyRandom,
    ) -> Result<()>;
}

fn offset(position: IVec3, x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(
        position.x.wrapping_add(x),
        position.y.wrapping_add(y),
        position.z.wrapping_add(z),
    )
}

fn step(random: &mut LegacyRandom) -> i32 {
    random.next_int_bound(3).expect("positive bound") - 1
}

/// Java's `ChunkMap.playerIsCloseEnoughTo`, plus the unlimited game rule.
pub(crate) fn can_spread_fire(
    radius: i32,
    spectator: bool,
    player: DVec3,
    position: IVec3,
) -> Result<bool> {
    ensure!(radius >= -1, "fire spread radius must be at least -1");
    if radius == -1 {
        return Ok(true);
    }
    if spectator {
        return Ok(false);
    }
    let delta = player - position.as_dvec3();
    let distance = (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt();
    Ok(distance < f64::from(radius))
}

/// Execute one fluid random tick. A lava block normally invokes this twice:
/// once through its block state and once through its original fluid state.
pub(crate) fn random_tick(
    world: &mut impl LavaFireWorld,
    position: IVec3,
    random: &mut LegacyRandom,
) -> Result<()> {
    if !world.can_spread_fire(position)? {
        return Ok(());
    }
    let attempts = random.next_int_bound(3).expect("positive bound");
    if attempts > 0 {
        let mut current = position;
        for _ in 0..attempts {
            current = offset(current, step(random), 1, step(random));
            if !world.is_loaded(current)? {
                return Ok(());
            }
            let state = world.block(current)?;
            if world.is_air(&state)? {
                // Direction.values(): DOWN, UP, NORTH, SOUTH, WEST, EAST.
                for (x, y, z) in [
                    (0, -1, 0),
                    (0, 1, 0),
                    (0, 0, -1),
                    (0, 0, 1),
                    (-1, 0, 0),
                    (1, 0, 0),
                ] {
                    if world.is_flammable(offset(current, x, y, z))? {
                        world.set_fire(current, current, random)?;
                        return Ok(());
                    }
                }
            } else if world.blocks_spread(&state)? {
                return Ok(());
            }
        }
    } else {
        for _ in 0..3 {
            let current = offset(position, step(random), 0, step(random));
            if !world.is_loaded(current)? {
                return Ok(());
            }
            let above = offset(current, 0, 1, 0);
            let state = world.block(above)?;
            if world.is_air(&state)? && world.is_flammable(current)? {
                // Java computes the fire state at `current`, but places it above.
                world.set_fire(above, current, random)?;
            }
        }
    }
    Ok(())
}

/// Java's block property, independent of data-pack fire-spread tags. The
/// caller must first validate the ID against the active 26.3 block registry.
pub(crate) fn ignited_by_lava(id: &str) -> Option<bool> {
    let path = id.strip_prefix("minecraft:")?;
    if LeafKind::from_id(id).is_some() {
        return Some(true);
    }
    let woods = [
        "oak", "spruce", "birch", "jungle", "acacia", "cherry", "dark_oak", "pale_oak", "mangrove",
        "poplar", "bamboo",
    ];
    for wood in woods {
        if let Some(kind) = path
            .strip_prefix(wood)
            .and_then(|rest| rest.strip_prefix('_'))
        {
            if matches!(
                kind,
                "planks"
                    | "stairs"
                    | "slab"
                    | "fence"
                    | "fence_gate"
                    | "door"
                    | "trapdoor"
                    | "pressure_plate"
                    | "sign"
                    | "wall_sign"
                    | "hanging_sign"
                    | "wall_hanging_sign"
                    | "shelf"
            ) || (wood != "bamboo" && matches!(kind, "log" | "wood"))
            {
                return Some(true);
            }
        }
        if wood != "bamboo"
            && path
                .strip_prefix("stripped_")
                .and_then(|rest| rest.strip_prefix(wood))
                .and_then(|rest| rest.strip_prefix('_'))
                .is_some_and(|kind| matches!(kind, "log" | "wood"))
        {
            return Some(true);
        }
    }
    let colors = [
        "white",
        "orange",
        "magenta",
        "light_blue",
        "yellow",
        "lime",
        "pink",
        "gray",
        "light_gray",
        "cyan",
        "purple",
        "blue",
        "brown",
        "green",
        "red",
        "black",
    ];
    for color in colors {
        if path
            .strip_prefix(color)
            .and_then(|rest| rest.strip_prefix('_'))
            .is_some_and(|kind| {
                matches!(
                    kind,
                    "bed"
                        | "wool"
                        | "wool_stairs"
                        | "wool_slab"
                        | "carpet"
                        | "banner"
                        | "wall_banner"
                )
            })
        {
            return Some(true);
        }
    }
    Some(matches!(
        path,
        "bamboo_block"
            | "stripped_bamboo_block"
            | "bamboo_mosaic"
            | "bamboo_mosaic_stairs"
            | "bamboo_mosaic_slab"
            | "crimson_shelf"
            | "warped_shelf"
            | "mangrove_roots"
            | "note_block"
            | "straw_bed"
            | "short_grass"
            | "fern"
            | "dead_bush"
            | "bush"
            | "red_shrub"
            | "short_dry_grass"
            | "tall_dry_grass"
            | "tnt"
            | "bookshelf"
            | "chiseled_bookshelf"
            | "chest"
            | "trapped_chest"
            | "crafting_table"
            | "cactus_flower"
            | "jukebox"
            | "brown_mushroom_block"
            | "red_mushroom_block"
            | "mushroom_stem"
            | "vine"
            | "glow_lichen"
            | "resin_clump"
            | "daylight_detector"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "tall_grass"
            | "large_fern"
            | "pitcher_plant"
            | "bamboo_sapling"
            | "bamboo"
            | "loom"
            | "barrel"
            | "cartography_table"
            | "fletching_table"
            | "lectern"
            | "smithing_table"
            | "campfire"
            | "soul_campfire"
            | "composter"
            | "bee_nest"
            | "beehive"
            | "hanging_roots"
            | "pale_moss_block"
            | "pale_moss_carpet"
            | "pale_hanging_moss"
            | "firefly_bush"
    ))
}

#[cfg(all(test, windows))]
mod tests {
    use std::{collections::HashMap, path::PathBuf, process::Command};

    use super::*;
    use anyhow::{Context, ensure};

    struct World {
        mode: i32,
        enabled: bool,
        blocked: bool,
        changed: HashMap<IVec3, u8>,
        placed: Vec<IVec3>,
    }

    impl World {
        fn present(&self, position: IVec3) -> bool {
            self.mode != 3 || position.x < 9
        }
    }

    impl LavaFireWorld for World {
        type State = u8;

        fn can_spread_fire(&mut self, _: IVec3) -> Result<bool> {
            Ok(self.enabled)
        }
        fn is_loaded(&mut self, position: IVec3) -> Result<bool> {
            Ok((0..16).contains(&position.y) && self.present(position))
        }
        fn block(&mut self, p: IVec3) -> Result<u8> {
            if let Some(&changed) = self.changed.get(&p) {
                return Ok(changed);
            }
            if !(0..16).contains(&p.y) {
                return Ok(0);
            }
            Ok(match self.mode {
                0 => 2,
                1 if p.y > 5 => 0,
                1 if (p.x + p.z).rem_euclid(3) == 0 => 1,
                1 => 2,
                _ => (p.x * 7 + p.y * 11 + p.z * 13).rem_euclid(4) as u8,
            })
        }
        fn is_air(&mut self, state: &u8) -> Result<bool> {
            Ok(*state == 0)
        }
        fn blocks_spread(&mut self, state: &u8) -> Result<bool> {
            Ok(self.blocked && matches!(state, 1 | 2))
        }
        fn is_flammable(&mut self, position: IVec3) -> Result<bool> {
            if (0..16).contains(&position.y) && !self.present(position) {
                return Ok(false);
            }
            Ok(self.block(position)? == 1)
        }
        fn set_fire(&mut self, position: IVec3, _: IVec3, _: &mut LegacyRandom) -> Result<()> {
            self.placed.push(position);
            self.changed.insert(position, 4);
            Ok(())
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn lava_random_attempts_match_java_26_3() -> Result<()> {
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
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                    "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
                )
            })
            .join("bin");
        let source = temp.path().join("LavaFireOracle.java");
        let result = temp.path().join("lava.json");
        std::fs::write(&source, include_str!("../tests/java/LavaFireOracle.java"))?;
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
            .arg("LavaFireOracle")
            .arg(&result)
            .output()?;
        ensure!(
            checked.status.success(),
            "Java execution failed: {} {}",
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&std::fs::read(result)?)?;
        assert_eq!(
            document["max_chunk_coordinate"].as_i64(),
            Some(i64::from(MAX_CHUNK_COORDINATE))
        );
        let ticks = document["ticks"].as_array().context("tick rows missing")?;
        let mut ignitions = 0;
        for row in ticks {
            let mode = row["mode"].as_i64().unwrap() as i32;
            let seed = row["seed"].as_i64().unwrap();
            let position = IVec3::new(8, if mode == 4 { 15 } else { 5 }, 8);
            let mut world = World {
                mode,
                enabled: row["enabled"].as_bool().unwrap(),
                blocked: row["blocked"].as_bool().unwrap(),
                changed: HashMap::from([(position, 5)]),
                placed: Vec::new(),
            };
            let mut random = LegacyRandom::new(seed);
            random_tick(&mut world, position, &mut random)?;
            random_tick(&mut world, position, &mut random)?;
            let expected: Vec<[i32; 3]> = serde_json::from_value(row["placed"].clone())?;
            let actual: Vec<_> = world.placed.iter().map(|p| p.to_array()).collect();
            assert_eq!(actual, expected, "{row}");
            assert_eq!(
                i64::from(random.next_int()),
                row["next"].as_i64().unwrap(),
                "{row}"
            );
            ignitions += expected.len();
        }
        assert!(ignitions > 0, "oracle did not exercise fire placement");
        assert_eq!(ticks.len(), 5 * 2 * 2 * 97);
        for row in document["radius"]
            .as_array()
            .context("radius rows missing")?
        {
            let spectator = row[0].as_bool().unwrap();
            let radius = row[1].as_i64().unwrap() as i32;
            let x = row[2].as_f64().unwrap();
            let expected = row[3].as_bool().unwrap();
            assert_eq!(
                can_spread_fire(radius, spectator, DVec3::new(x, 0., 0.), IVec3::ZERO)?,
                expected,
                "{row}"
            );
        }
        assert!(can_spread_fire(-1, true, DVec3::ZERO, IVec3::ZERO)?);
        eprintln!(
            "Compared {} lava random-tick cases and {} fire-radius cases",
            ticks.len(),
            document["radius"].as_array().unwrap().len()
        );
        Ok(())
    }
}
