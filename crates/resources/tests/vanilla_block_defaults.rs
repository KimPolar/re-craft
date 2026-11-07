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
import net.minecraft.world.level.block.state.BlockState;
import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
public class BlockDefaultsOracle {
    public static void main(String[] args) throws Exception {
        PrintStream output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (var block : BuiltInRegistries.BLOCK) {
            var values = block.defaultBlockState().getValues()
                .map(value -> value.property().getName() + "=" + value.valueName()).sorted().toList();
            output.println(BuiltInRegistries.BLOCK.getKey(block) + "\t" + String.join("\t", values));
            output.println("ENTITY\t" + BuiltInRegistries.BLOCK.getKey(block) + "\t" + block.defaultBlockState().hasBlockEntity());
            var isAir = block.defaultBlockState().isAir();
            if (block.getStateDefinition().getPossibleStates().stream().anyMatch(state -> state.isAir() != isAir))
                throw new AssertionError("air status varies within block " + block);
            output.println("AIR\t" + BuiltInRegistries.BLOCK.getKey(block) + "\t" + isAir);
        }
        if (args.length > 0) {
            var digits=new JsonArray();
            for(int c=0;c<=0xffff;c++) {
                int digit=Character.digit((char)c,10);
                if(digit>=0) { var row=new JsonArray();row.add(c);row.add(digit);digits.add(row); }
            }
            output.println("DIGITS\t"+digits);
            var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
            for (var input : cases) {
                var decoded=BlockState.CODEC.parse(JsonOps.INSTANCE,input).result().orElse(null);
                var row=new JsonObject(); row.addProperty("success",decoded!=null);
                if(decoded!=null) {
                    row.addProperty("id",BuiltInRegistries.BLOCK.getKey(decoded.getBlock()).toString());
                    var properties=new JsonObject();
                    decoded.getValues().forEach(value -> properties.addProperty(value.property().getName(),value.valueName()));
                    row.add("properties",properties);
                }
                output.println("CODEC\t"+row);
            }
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

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn generated_block_defaults_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_minecraft_dir().unwrap());
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_generated_block_registry().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let pack = temp.path().join("pack");
    let blockstates = pack.join("assets/minecraft/blockstates");
    std::fs::create_dir_all(&blockstates).unwrap();
    std::fs::write(
        blockstates.join("stone.json"),
        b"not a vanilla block registry",
    )
    .unwrap();
    resources.push_pack(&pack).unwrap();
    let with_pack = resources.vanilla_generated_block_registry().unwrap();
    assert_eq!(
        registry.ids().collect::<Vec<_>>(),
        with_pack.ids().collect::<Vec<_>>()
    );
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
    let mut cases = vec![
        serde_json::json!("minecraft:glow_lichen"),
        serde_json::json!({"id":"minecraft:glow_lichen","properties":{"down":"true","north":"true","waterlogged":"true"}}),
        serde_json::json!({"id":"minecraft:glow_lichen","properties":{"up":"invalid","east":"true"}}),
        serde_json::json!("minecraft:stone"),
        serde_json::json!("minecraft:spawner"),
        serde_json::json!("minecraft:oak_fence"),
        serde_json::json!("minecraft:nether_brick_fence"),
        serde_json::json!("minecraft:spruce_stairs"),
        serde_json::json!("minecraft:stone_stairs"),
        serde_json::json!("minecraft:purple_glazed_terracotta"),
        serde_json::json!("minecraft:cauldron"),
        serde_json::json!("minecraft:potted_red_mushroom"),
        serde_json::json!("minecraft:chest"),
        serde_json::json!("minecraft:trapped_chest"),
        serde_json::json!("minecraft:bee_nest"),
        serde_json::json!("minecraft:beehive"),
        serde_json::json!({"id":"minecraft:chest","properties":{"facing":"up","type":"invalid","waterlogged":"invalid"}}),
        serde_json::json!({"id":"minecraft:bee_nest","properties":{"facing":"down","honey_level":"6"}}),
        serde_json::json!("minecraft:diamond_ore"),
        serde_json::json!("minecraft:deepslate_diamond_ore"),
        serde_json::json!("minecraft:coal_ore"),
        serde_json::json!("minecraft:brown_mushroom_block"),
        serde_json::json!("minecraft:red_mushroom_block"),
        serde_json::json!("minecraft:mushroom_stem"),
        serde_json::json!("minecraft:leaf_litter"),
        serde_json::json!("minecraft:cave_vines_plant"),
        serde_json::json!("minecraft:cave_vines"),
        serde_json::json!("minecraft:small_dripleaf"),
        serde_json::json!("minecraft:seagrass"),
        serde_json::json!("minecraft:tall_seagrass"),
        serde_json::json!({"id":"minecraft:tall_seagrass","properties":{"half":"upper"}}),
        serde_json::json!({"id":"minecraft:tall_seagrass","properties":{"half":"invalid"}}),
        serde_json::json!("minecraft:vine"),
        serde_json::json!({"id":"minecraft:vine","properties":{"up":"true","north":"true","east":"true"}}),
        serde_json::json!({"id":"minecraft:vine","properties":{"up":"invalid","south":"false","waterlogged":"true"}}),
        serde_json::json!("minecraft:azalea_leaves"),
        serde_json::json!("minecraft:flowering_azalea_leaves"),
        serde_json::json!({"id":"minecraft:azalea_leaves","properties":{"distance":"3","persistent":"true","waterlogged":"true"}}),
        serde_json::json!({"id":"minecraft:flowering_azalea_leaves","properties":{"distance":"8","persistent":"invalid"}}),
        serde_json::json!("minecraft:hanging_roots"),
        serde_json::json!({"id":"minecraft:hanging_roots","properties":{"waterlogged":"true"}}),
        serde_json::json!({"id":"minecraft:hanging_roots","properties":{"waterlogged":"invalid"}}),
        serde_json::json!("minecraft:big_dripleaf"),
        serde_json::json!("minecraft:big_dripleaf_stem"),
        serde_json::json!({"id":"minecraft:small_dripleaf","properties":{"facing":"east","half":"upper","waterlogged":"true"}}),
        serde_json::json!({"id":"minecraft:small_dripleaf","properties":{"facing":"up","half":"invalid","waterlogged":"invalid"}}),
        serde_json::json!({"id":"minecraft:big_dripleaf","properties":{"facing":"west","tilt":"partial","waterlogged":"true"}}),
        serde_json::json!({"id":"minecraft:big_dripleaf","properties":{"tilt":"invalid"}}),
        serde_json::json!({"id":"minecraft:big_dripleaf_stem","properties":{"facing":"south","waterlogged":"true"}}),
        serde_json::json!({"id":"minecraft:cave_vines_plant","properties":{"berries":"true"}}),
        serde_json::json!({"id":"minecraft:cave_vines","properties":{"berries":"true","age":"25"}}),
        serde_json::json!({"id":"minecraft:cave_vines","properties":{"berries":"invalid","age":"26"}}),
        serde_json::json!({"id":"minecraft:leaf_litter","properties":{"facing":"west","segment_amount":"4"}}),
        serde_json::json!({"id":"minecraft:leaf_litter","properties":{"facing":"up","segment_amount":"5"}}),
        serde_json::json!({"id":"minecraft:brown_mushroom_block","properties":{"down":"false","west":"false","up":"invalid"}}),
        serde_json::json!("minecraft:redstone_ore"),
        serde_json::json!("minecraft:deepslate_redstone_ore"),
        serde_json::json!({"id":"minecraft:redstone_ore","properties":{"lit":"true"}}),
        serde_json::json!({"id":"minecraft:deepslate_redstone_ore","properties":{"lit":"invalid"}}),
        serde_json::json!("minecraft:tall_grass"),
        serde_json::json!({"id":"minecraft:large_fern","properties":{"half":"upper"}}),
        serde_json::json!({"id":"minecraft:sunflower","properties":{"half":"invalid"}}),
        serde_json::json!("minecraft:sweet_berry_bush"),
        serde_json::json!({"id":"minecraft:sweet_berry_bush","properties":{"age":"3"}}),
        serde_json::json!({"id":"minecraft:sweet_berry_bush","properties":{"age":"4"}}),
        serde_json::json!({"id":"minecraft:stone", "properties":{"unknown":"x"}}),
        serde_json::json!("minecraft:water"),
        serde_json::json!({"id":"minecraft:water", "properties":"invalid"}),
        serde_json::json!({"id":"minecraft:water", "properties":null}),
        serde_json::json!({"id":"minecraft:water", "properties":{"level":7}}),
        serde_json::json!({"id":"minecraft:water", "properties":{"level":"07"}}),
        serde_json::json!({"id":"minecraft:water", "properties":{"level":"+7"}}),
        serde_json::json!({"id":"minecraft:water", "properties":{"level":"-0"}}),
        serde_json::json!({"id":"minecraft:water", "properties":{"level":"\u{0667}"}}),
        serde_json::json!({"id":"minecraft:grass_block", "properties":{"snowy":true}}),
        serde_json::json!({"id":"minecraft:grass_block", "properties":{"snowy":"true","extra":7}}),
        serde_json::json!({"id":"minecraft:grass_block", "properties":{"snowy":"true","extra":null}}),
        serde_json::json!({"id":"minecraft:grass_block", "properties":{"snowy":"true","extra":{}}}),
        serde_json::json!({"id":"example:unknown"}),
    ];
    for axis in ["x", "y", "z", "invalid"] {
        for waterlogged in ["true", "false"] {
            cases.push(serde_json::json!({"id":"minecraft:iron_chain", "properties":{"axis":axis,"waterlogged":waterlogged}}));
        }
    }
    for id in [
        "minecraft:rail",
        "minecraft:powered_rail",
        "minecraft:detector_rail",
        "minecraft:activator_rail",
    ] {
        let powered = id != "minecraft:rail";
        for shape in [
            "north_south",
            "east_west",
            "ascending_east",
            "ascending_west",
            "ascending_north",
            "ascending_south",
            "south_east",
            "south_west",
            "north_west",
            "north_east",
        ] {
            for waterlogged in ["true", "false"] {
                for power in if powered {
                    &["true", "false"][..]
                } else {
                    &["false"][..]
                } {
                    let mut props = serde_json::Map::new();
                    props.insert("shape".into(), serde_json::json!(shape));
                    props.insert("waterlogged".into(), serde_json::json!(waterlogged));
                    if powered {
                        props.insert("powered".into(), serde_json::json!(power));
                    }
                    cases.push(serde_json::json!({"id": id, "properties": props}));
                }
            }
        }
    }
    for id in ["minecraft:oak_fence", "minecraft:nether_brick_fence"] {
        for bits in 0..32 {
            let mut properties = serde_json::Map::new();
            for (bit, name) in ["north", "south", "west", "east", "waterlogged"]
                .into_iter()
                .enumerate()
            {
                properties.insert(
                    name.into(),
                    serde_json::json!((bits & (1 << bit) != 0).to_string()),
                );
            }
            cases.push(serde_json::json!({"id":id,"properties":properties}));
        }
    }
    for id in ["minecraft:spruce_stairs", "minecraft:stone_stairs"] {
        for facing in ["north", "south", "west", "east"] {
            for half in ["bottom", "top"] {
                for shape in [
                    "straight",
                    "inner_left",
                    "inner_right",
                    "outer_left",
                    "outer_right",
                ] {
                    for waterlogged in ["true", "false"] {
                        cases.push(serde_json::json!({"id":id,"properties":{"facing":facing,"half":half,"shape":shape,"waterlogged":waterlogged}}));
                    }
                }
            }
        }
    }
    for id in ["minecraft:bee_nest", "minecraft:beehive"] {
        for facing in ["north", "south", "east", "west"] {
            for honey in 0..=5 {
                cases.push(serde_json::json!({"id":id,"properties":{"facing":facing,"honey_level":honey.to_string()}}));
            }
        }
    }
    for id in ["minecraft:chest", "minecraft:trapped_chest"] {
        for facing in ["north", "south", "east", "west"] {
            for chest_type in ["single", "left", "right"] {
                for waterlogged in ["true", "false"] {
                    cases.push(serde_json::json!({"id":id,"properties":{"facing":facing,"type":chest_type,"waterlogged":waterlogged}}));
                }
            }
        }
    }
    for id in ["minecraft:water", "minecraft:lava"] {
        for level in -1..=16 {
            cases.push(serde_json::json!({"id":id, "properties":{"level":level.to_string()}}));
        }
    }
    for id in ["minecraft:sugar_cane", "minecraft:cactus"] {
        cases.push(serde_json::json!(id));
        for age in -1..=16 {
            cases.push(serde_json::json!({"id":id,"properties":{"age":age.to_string()}}));
        }
    }
    for id in [
        "minecraft:oak_sapling",
        "minecraft:spruce_sapling",
        "minecraft:birch_sapling",
        "minecraft:jungle_sapling",
        "minecraft:acacia_sapling",
        "minecraft:dark_oak_sapling",
        "minecraft:cherry_sapling",
        "minecraft:pale_oak_sapling",
    ] {
        cases.push(serde_json::json!(id));
        for stage in -1..=2 {
            cases.push(serde_json::json!({"id":id,"properties":{"stage":stage.to_string()}}));
        }
    }
    for id in [
        "minecraft:oak_leaves",
        "minecraft:spruce_leaves",
        "minecraft:birch_leaves",
        "minecraft:jungle_leaves",
        "minecraft:acacia_leaves",
        "minecraft:dark_oak_leaves",
        "minecraft:cherry_leaves",
        "minecraft:pale_oak_leaves",
    ] {
        cases.push(serde_json::json!(id));
        for distance in 0..=8 {
            cases.push(serde_json::json!({"id":id,"properties":{"distance":distance.to_string(),"persistent":"true","waterlogged":"true"}}));
        }
    }
    for snowy in ["true", "false", "yes", "TRUE"] {
        cases.push(serde_json::json!({"id":"minecraft:grass_block", "properties":{"snowy":snowy,"extra":"ignored"}}));
    }
    for axis in ["x", "y", "z", "bad"] {
        cases.push(serde_json::json!({"id":"minecraft:deepslate", "properties":{"axis":axis}}));
    }
    let input = temp.path().join("states.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BlockDefaultsOracle")
        .arg(&input));
    let expected: BTreeMap<_, _> = output
        .lines()
        .filter(|line| line.starts_with("minecraft:"))
        .map(|line| {
            let mut parts = line.split('\t');
            (
                parts.next().unwrap().to_owned(),
                parts
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let entity_expected: BTreeMap<_, _> = output
        .lines()
        .filter_map(|line| line.strip_prefix("ENTITY\t"))
        .map(|line| {
            let (id, value) = line.split_once('\t').unwrap();
            (id.to_owned(), value.parse::<bool>().unwrap())
        })
        .collect();
    let air_expected: BTreeMap<_, _> = output
        .lines()
        .filter_map(|line| line.strip_prefix("AIR\t"))
        .map(|line| {
            let (id, value) = line.split_once('\t').unwrap();
            (id.to_owned(), value.parse::<bool>().unwrap())
        })
        .collect();
    let mut unsupported_states = 0;
    for id in registry.registered_ids() {
        let is_air = air_expected
            .get(id)
            .unwrap_or_else(|| panic!("unregistered Java block: {id}"));
        assert_eq!(
            *is_air,
            matches!(
                id,
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            ),
            "Java air classification: {id}"
        );
        assert!(
            with_pack.is_registered(id),
            "resource pack changed block registrations: {id}"
        );
        unsupported_states += usize::from(!registry.contains(id));
    }
    assert!(
        unsupported_states > 0,
        "expected verified blocks with unsupported state schemas"
    );
    assert!(!registry.is_registered("minecraft:powder_snow_bucket"));
    assert!(!registry.is_registered("example:unknown"));
    assert!(
        registry.len() >= 100,
        "block registry is unexpectedly small"
    );
    assert!(registry.contains("minecraft:stone"));
    assert!(registry.contains("minecraft:dirt"));
    assert!(registry.contains("minecraft:air"));
    assert!(registry.contains("minecraft:sand"));
    assert!(registry.contains("minecraft:powder_snow"));
    assert!(!registry.contains("minecraft:powder_snow_bucket"));
    assert!(registry.contains("minecraft:grass_block"));
    assert!(registry.contains("minecraft:water"));
    assert!(registry.contains("minecraft:lava"));
    assert!(registry.contains("minecraft:iron_chain"));
    assert_eq!(
        registry.has_block_entity("minecraft:iron_chain"),
        Some(false)
    );
    for id in [
        "minecraft:rail",
        "minecraft:powered_rail",
        "minecraft:detector_rail",
        "minecraft:activator_rail",
    ] {
        assert!(registry.contains(id), "missing rail state schema: {id}");
        assert_eq!(
            registry.has_block_entity(id),
            Some(false),
            "rail block entity: {id}"
        );
    }
    assert!(registry.contains("minecraft:deepslate"));
    for id in [
        "minecraft:bee_nest",
        "minecraft:beehive",
        "minecraft:chest",
        "minecraft:trapped_chest",
        "minecraft:spawner",
    ] {
        assert!(registry.contains(id), "missing block entity state {id}");
    }
    println!("verified {} generated block defaults", registry.len());
    for id in registry.ids() {
        assert_eq!(
            registry.has_block_entity(id),
            Some(entity_expected[id]),
            "Java block entity: {id}"
        );
        let expected = expected
            .get(id)
            .unwrap_or_else(|| panic!("Java block missing: {id}"));
        let properties = registry
            .default_properties(id)
            .unwrap()
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>();
        assert_eq!(&properties, expected, "Java default state: {id}");
    }
    let rows: Vec<serde_json::Value> = output
        .lines()
        .filter_map(|line| line.strip_prefix("CODEC\t"))
        .map(|row| serde_json::from_str(row).unwrap())
        .collect();
    assert_eq!(rows.len(), cases.len());
    for (case, row) in cases.iter().zip(rows) {
        let actual = registry.resolve(case);
        assert_eq!(actual.is_ok(), row["success"].as_bool().unwrap(), "{case}");
        if let Ok((id, properties)) = actual {
            assert_eq!(id, row["id"].as_str().unwrap(), "{case}");
            assert_eq!(
                serde_json::to_value(properties).unwrap(),
                row["properties"],
                "{case}"
            );
        }
    }
    let digits = output
        .lines()
        .find_map(|line| line.strip_prefix("DIGITS\t"))
        .unwrap();
    let digits: Vec<[u32; 2]> = serde_json::from_str(digits).unwrap();
    let expected_digits: BTreeMap<_, _> = digits
        .into_iter()
        .map(|[code, digit]| (code, digit))
        .collect();
    for code in 0..=0xffff {
        let Some(ch) = char::from_u32(code) else {
            continue;
        };
        for prefix in ["0", "1"] {
            let input = serde_json::json!({"id":"minecraft:water", "properties":{"level":format!("{prefix}{ch}")}});
            let (_, properties) = registry.resolve(&input).unwrap();
            let expected = expected_digits
                .get(&code)
                .map(|digit| if prefix == "1" { 10 + digit } else { *digit })
                .filter(|digit| *digit <= 15)
                .unwrap_or(0)
                .to_string();
            assert_eq!(
                properties["level"], expected,
                "Java digit U+{code:04X} with prefix {prefix}"
            );
        }
    }
}
