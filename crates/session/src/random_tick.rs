//! Java 26.3's per-state random-tick eligibility.
//!
//! A section consumes random block positions if any of its blocks or fluids
//! is randomly ticked, including kinds the session does not yet simulate.

use native_core::leaf::LeafKind;
use native_world::loaded::BlockState;

fn age_below(state: &BlockState, limit: u8) -> bool {
    state
        .properties
        .get("age")
        .and_then(|value| value.parse::<u8>().ok())
        .is_none_or(|age| age < limit)
}

fn copper_weathering(path: &str) -> bool {
    let path = path
        .strip_prefix("exposed_")
        .or_else(|| path.strip_prefix("weathered_"))
        .unwrap_or(path);
    matches!(
        path,
        "copper_block"
            | "copper"
            | "cut_copper"
            | "chiseled_copper"
            | "cut_copper_stairs"
            | "cut_copper_slab"
            | "copper_door"
            | "copper_trapdoor"
            | "copper_grate"
            | "copper_bulb"
            | "copper_bars"
            | "copper_chain"
            | "copper_lantern"
            | "copper_chest"
            | "copper_golem_statue"
            | "lightning_rod"
    )
}

pub(crate) fn is_randomly_ticking(state: &BlockState) -> bool {
    let path = match state.name.split_once(':') {
        Some(("minecraft", path)) => path,
        Some(_) => return false,
        None => state.name.as_str(),
    };
    if LeafKind::from_id(&state.name).is_some() {
        return match (
            state.properties.get("distance").map(String::as_str),
            state.properties.get("persistent").map(String::as_str),
        ) {
            (_, Some("true")) => false,
            (Some(distance), Some("false")) => distance == "7",
            _ => true,
        };
    }
    if copper_weathering(path) {
        return true;
    }
    match path {
        "redstone_ore" | "deepslate_redstone_ore" => {
            state.properties.get("lit").is_none_or(|lit| lit != "false")
        }
        "wheat" | "carrots" | "potatoes" => age_below(state, 7),
        "beetroots" | "nether_wart" | "sweet_berry_bush" => age_below(state, 3),
        "cocoa" => age_below(state, 2),
        "chorus_flower" => age_below(state, 5),
        "kelp" | "weeping_vines" | "twisting_vines" | "cave_vines" => age_below(state, 25),
        "pitcher_crop" => {
            state
                .properties
                .get("half")
                .is_none_or(|half| half != "upper")
                && age_below(state, 4)
        }
        "bamboo" => state
            .properties
            .get("stage")
            .is_none_or(|stage| stage != "1"),
        "grass_block"
        | "mycelium"
        | "farmland"
        | "snow"
        | "lava"
        | "oak_sapling"
        | "spruce_sapling"
        | "birch_sapling"
        | "jungle_sapling"
        | "acacia_sapling"
        | "cherry_sapling"
        | "dark_oak_sapling"
        | "pale_oak_sapling"
        | "poplar_sapling"
        | "mangrove_propagule"
        | "brown_mushroom"
        | "red_mushroom"
        | "ice"
        | "cactus"
        | "sugar_cane"
        | "nether_portal"
        | "pumpkin_stem"
        | "melon_stem"
        | "vine"
        | "torchflower_crop"
        | "turtle_egg"
        | "dried_ghast"
        | "bamboo_sapling"
        | "warped_nylium"
        | "crimson_nylium"
        | "budding_amethyst"
        | "pointed_dripstone"
        | "sulfur_spike"
        | "open_eyeblossom"
        | "closed_eyeblossom"
        | "potted_open_eyeblossom"
        | "potted_closed_eyeblossom" => true,
        _ => false,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf, process::Command};

    use super::*;

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn every_vanilla_block_state_matches_java_random_tick_eligibility() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                    "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
                )
            })
            .join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("RandomTickStateOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/RandomTickStateOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("RandomTickStateOracle")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let mut checked = 0;
        for line in stdout.lines().filter(|line| line.starts_with("STATE\t")) {
            let mut fields = line.split('\t');
            assert_eq!(fields.next(), Some("STATE"));
            let name = fields.next().unwrap().to_owned();
            let expected = match fields.next() {
                Some("true") => true,
                Some("false") => false,
                other => panic!("invalid Java random-tick result: {other:?}"),
            };
            let properties = fields
                .map(|field| {
                    let (key, value) = field.split_once('=').unwrap();
                    (key.to_owned(), value.to_owned())
                })
                .collect::<BTreeMap<_, _>>();
            let state = BlockState { name, properties };
            assert_eq!(is_randomly_ticking(&state), expected, "{state:?}");
            assert_eq!(
                crate::loaded::random_tick_kind(&state) != 0,
                expected,
                "{state:?}"
            );
            checked += 1;
        }
        assert!(checked > 20_000, "only {checked} Java states were checked");
        eprintln!("Compared random-tick eligibility for {checked} Java block states");
    }
}
