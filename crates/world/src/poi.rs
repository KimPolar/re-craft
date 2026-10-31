//! Vanilla 26.3 point-of-interest types for registered block states.
//!
//! Callers must establish that the state is registered and valid before using
//! a negative result as evidence that a block has no POI. This classification
//! does not itself update POI storage or account for villager ticket changes.

use crate::loaded::BlockState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoiType {
    pub id: &'static str,
    pub max_tickets: u8,
    pub valid_range: u8,
}

/// Returns the POI type of a canonical, registered vanilla block state.
pub fn for_state(state: &BlockState) -> Option<PoiType> {
    let id = state.name.strip_prefix("minecraft:")?;
    let (kind, tickets, range) = match id {
        "blast_furnace" => ("minecraft:armorer", 1, 1),
        "smoker" => ("minecraft:butcher", 1, 1),
        "cartography_table" => ("minecraft:cartographer", 1, 1),
        "brewing_stand" => ("minecraft:cleric", 1, 1),
        "composter" => ("minecraft:farmer", 1, 1),
        "barrel" => ("minecraft:fisherman", 1, 1),
        "fletching_table" => ("minecraft:fletcher", 1, 1),
        "cauldron" | "lava_cauldron" | "water_cauldron" | "powder_snow_cauldron" => {
            ("minecraft:leatherworker", 1, 1)
        }
        "lectern" => ("minecraft:librarian", 1, 1),
        "stonecutter" => ("minecraft:mason", 1, 1),
        "loom" => ("minecraft:shepherd", 1, 1),
        "smithing_table" => ("minecraft:toolsmith", 1, 1),
        "grindstone" => ("minecraft:weaponsmith", 1, 1),
        "white_bed" | "orange_bed" | "magenta_bed" | "light_blue_bed" | "yellow_bed"
        | "lime_bed" | "pink_bed" | "gray_bed" | "light_gray_bed" | "cyan_bed" | "purple_bed"
        | "blue_bed" | "brown_bed" | "green_bed" | "red_bed" | "black_bed"
            if state
                .properties
                .get("part")
                .is_some_and(|part| part == "head") =>
        {
            ("minecraft:home", 1, 1)
        }
        "bell" => ("minecraft:meeting", 32, 6),
        "beehive" => ("minecraft:beehive", 0, 1),
        "bee_nest" => ("minecraft:bee_nest", 0, 1),
        "nether_portal" => ("minecraft:nether_portal", 0, 1),
        "lodestone" => ("minecraft:lodestone", 0, 1),
        "test_instance_block" => ("minecraft:test_instance", 0, 1),
        "lightning_rod"
        | "exposed_lightning_rod"
        | "weathered_lightning_rod"
        | "oxidized_lightning_rod"
        | "waxed_lightning_rod"
        | "waxed_exposed_lightning_rod"
        | "waxed_weathered_lightning_rod"
        | "waxed_oxidized_lightning_rod" => ("minecraft:lightning_rod", 0, 1),
        _ => return None,
    };
    Some(PoiType {
        id: kind,
        max_tickets: tickets,
        valid_range: range,
    })
}

/// Type-preserving state changes must keep the existing POI and its tickets.
pub fn change(old: &BlockState, new: &BlockState) -> Option<(Option<PoiType>, Option<PoiType>)> {
    let before = for_state(old);
    let after = for_state(new);
    (before != after).then_some((before, after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        }
    }

    #[test]
    fn same_poi_type_does_not_replace_existing_tickets() {
        let empty = state("cauldron", &[]);
        let water = state("water_cauldron", &[("level", "2")]);
        assert!(change(&empty, &water).is_none());
        let foot = state("red_bed", &[("part", "foot")]);
        let head = state("red_bed", &[("part", "head")]);
        assert_eq!(
            change(&foot, &head).unwrap().1.unwrap().id,
            "minecraft:home"
        );
        assert_eq!(for_state(&state("bell", &[])).unwrap().max_tickets, 32);
        assert_eq!(for_state(&state("bell", &[])).unwrap().valid_range, 6);
        assert!(for_state(&state("straw_bed", &[])).is_none());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn every_registered_state_matches_java_poi_registry() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PoiTypesOracle.java");
        std::fs::write(&source, include_str!("../tests/java/PoiTypesOracle.java")).unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = temp.path().join("states.json");
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PoiTypesOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert!(rows.len() > 10_000);
        for row in rows {
            let state = BlockState {
                name: row["name"].as_str().unwrap().into(),
                properties: serde_json::from_value::<BTreeMap<String, String>>(
                    row["properties"].clone(),
                )
                .unwrap(),
            };
            let actual = for_state(&state);
            let expected = row.get("type").and_then(|value| value.as_str());
            assert_eq!(actual.map(|poi| poi.id), expected, "{state:?}");
            if let Some(poi) = actual {
                assert_eq!(
                    u64::from(poi.max_tickets),
                    row["tickets"].as_u64().unwrap(),
                    "{state:?}"
                );
                assert_eq!(
                    u64::from(poi.valid_range),
                    row["range"].as_u64().unwrap(),
                    "{state:?}"
                );
            }
        }
    }
}
