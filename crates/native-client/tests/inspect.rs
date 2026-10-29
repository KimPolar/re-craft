#![cfg(debug_assertions)]

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
use native_persistence::{
    anvil,
    nbt::{self, NamedTag, Tag, TagType},
};

fn create_world(world: &Path) -> (Vec<u8>, PathBuf, Vec<u8>) {
    create_world_with_block(world, "minecraft:stone")
}

fn create_world_with_block(world: &Path, block_name: &str) -> (Vec<u8>, PathBuf, Vec<u8>) {
    let level = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "Data".into(),
            Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
        )]),
    };
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&nbt::write(&level).unwrap()).unwrap();
    let level_bytes = gzip.finish().unwrap();
    std::fs::write(world.join("level.dat"), &level_bytes).unwrap();

    let block = Tag::String(block_name.into());
    let states = Tag::Compound(vec![(
        "palette".into(),
        Tag::List {
            element_type: TagType::String,
            values: vec![block],
        },
    )]);
    let section = Tag::Compound(vec![
        ("Y".into(), Tag::Byte(-4)),
        ("block_states".into(), states),
    ]);
    let chunk = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            ("xPos".into(), Tag::Int(-1)),
            ("zPos".into(), Tag::Int(-1)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            (
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![section],
                },
            ),
        ]),
    };
    let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
    zlib.write_all(&nbt::write(&chunk).unwrap()).unwrap();
    let compressed = zlib.finish().unwrap();
    let region_path = anvil::overworld_region_path(world, -1, -1);
    std::fs::create_dir_all(region_path.parent().unwrap()).unwrap();
    let mut region = vec![0u8; 3 * 4096];
    region[1023 * 4..1024 * 4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
    region[8192..8196].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
    region[8196] = 2;
    region[8197..8197 + compressed.len()].copy_from_slice(&compressed);
    std::fs::write(&region_path, &region).unwrap();

    (level_bytes, region_path, region)
}

fn create_fluid_world(world: &Path) -> (Vec<u8>, PathBuf, Vec<u8>) {
    let (level_bytes, region_path, _) = create_world(world);
    let block = |name: &str, properties: &[(&str, &str)]| {
        if properties.is_empty() {
            return Tag::Compound(vec![("".into(), Tag::String(name.into()))]);
        }
        Tag::Compound(vec![
            ("id".into(), Tag::String(name.into())),
            (
                "properties".into(),
                Tag::Compound(
                    properties
                        .iter()
                        .map(|(key, value)| ((*key).into(), Tag::String((*value).into())))
                        .collect(),
                ),
            ),
        ])
    };
    let palette = vec![
        block("minecraft:air", &[]),
        block("minecraft:stone", &[]),
        block("minecraft:water", &[("level", "0")]),
        block("minecraft:water", &[("level", "5")]),
        block("minecraft:lava", &[("level", "0")]),
        block(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "true")],
        ),
    ];
    let mut indices = [0u64; 4096];
    for z in 0..16 {
        for x in 0..16 {
            indices[x + z * 16] = 1;
            indices[x + z * 16 + 256] = if x < 7 {
                2
            } else if x == 7 {
                3
            } else {
                4
            };
        }
    }
    indices[2 + 2 * 16 + 256] = 5;
    let words: Vec<i64> = indices
        .chunks(16)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u64, |word, (index, value)| word | (value << (index * 4))) as i64
        })
        .collect();
    let states = Tag::Compound(vec![
        (
            "palette".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: palette,
            },
        ),
        ("data".into(), Tag::LongArray(words)),
    ]);
    let section = Tag::Compound(vec![
        ("Y".into(), Tag::Byte(-4)),
        ("block_states".into(), states),
        (
            "biomes".into(),
            Tag::Compound(vec![(
                "palette".into(),
                Tag::List {
                    element_type: TagType::String,
                    values: vec![Tag::String("minecraft:cherry_grove".into())],
                },
            )]),
        ),
    ]);
    let chunk = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            ("xPos".into(), Tag::Int(-1)),
            ("zPos".into(), Tag::Int(-1)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            (
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![section],
                },
            ),
        ]),
    };
    let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
    zlib.write_all(&nbt::write(&chunk).unwrap()).unwrap();
    let compressed = zlib.finish().unwrap();
    let sectors = (compressed.len() + 5).div_ceil(4096);
    let mut region = vec![0u8; (2 + sectors) * 4096];
    region[1023 * 4..1024 * 4].copy_from_slice(&((2u32 << 8) | sectors as u32).to_be_bytes());
    region[8192..8196].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
    region[8196] = 2;
    region[8197..8197 + compressed.len()].copy_from_slice(&compressed);
    std::fs::write(&region_path, &region).unwrap();
    (level_bytes, region_path, region)
}

fn create_play_world(world: &Path) -> (Vec<u8>, PathBuf, Vec<u8>, PathBuf, Vec<u8>) {
    let (_, region_path, region) = create_world(world);
    let level = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "Data".into(),
            Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "singleplayer_uuid".into(),
                    Tag::IntArray(vec![
                        0x00112233,
                        0x44556677,
                        0x8899aabbu32 as i32,
                        0xccddeeffu32 as i32,
                    ]),
                ),
            ]),
        )]),
    };
    let player = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            (
                "Dimension".into(),
                Tag::String("minecraft:overworld".into()),
            ),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![
                        Tag::Double(-7.500000123456789),
                        Tag::Double(-47.0),
                        Tag::Double(-7.5),
                    ],
                },
            ),
            (
                "Motion".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![Tag::Double(0.008), Tag::Double(0.0), Tag::Double(-0.004)],
                },
            ),
            ("OnGround".into(), Tag::Byte(0)),
            ("fall_distance".into(), Tag::Double(2.5)),
            ("Health".into(), Tag::Float(13.5)),
            ("foodLevel".into(), Tag::Int(6)),
            ("foodSaturationLevel".into(), Tag::Float(1.0)),
            ("playerGameType".into(), Tag::Int(1)),
            (
                "Inventory".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("Slot".into(), Tag::Byte(0)),
                        ("id".into(), Tag::String("minecraft:oak_log".into())),
                        ("count".into(), Tag::Int(64)),
                    ])],
                },
            ),
            (
                "Rotation".into(),
                Tag::List {
                    element_type: TagType::Float,
                    values: vec![Tag::Float(0.0), Tag::Float(89.0)],
                },
            ),
        ]),
    };
    let gzip = |document: &NamedTag| {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(document).unwrap()).unwrap();
        encoder.finish().unwrap()
    };
    let level_bytes = gzip(&level);
    std::fs::write(world.join("level.dat"), &level_bytes).unwrap();
    let player_path = world.join("players/data/00112233-4455-6677-8899-aabbccddeeff.dat");
    std::fs::create_dir_all(player_path.parent().unwrap()).unwrap();
    let player_bytes = gzip(&player);
    std::fs::write(&player_path, &player_bytes).unwrap();
    (level_bytes, region_path, region, player_path, player_bytes)
}

#[test]
fn inspection_reads_a_world_without_modifying_it() {
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path();
    let (level_bytes, region_path, region) = create_world(world);

    let output = Command::new(env!("CARGO_BIN_EXE_native-client"))
        .arg("--inspect-world")
        .arg(world)
        .args(["--chunk", "-1", "-1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 block sections, 1 palette entries")
    );
    let area = Command::new(env!("CARGO_BIN_EXE_native-client"))
        .arg("--inspect-world")
        .arg(world)
        .args(["--chunk", "-1", "-1", "--radius", "1"])
        .output()
        .unwrap();
    assert!(
        area.status.success(),
        "{}",
        String::from_utf8_lossy(&area.stderr)
    );
    assert!(String::from_utf8_lossy(&area.stdout).contains("1 chunks (1 full), 8 missing"));
    let nether = Command::new(env!("CARGO_BIN_EXE_native-client"))
        .arg("--inspect-world")
        .arg(world)
        .args(["--dimension", "minecraft:the_nether", "--chunk", "0", "0"])
        .output()
        .unwrap();
    assert!(
        nether.status.success(),
        "{}",
        String::from_utf8_lossy(&nether.stderr)
    );
    assert!(String::from_utf8_lossy(&nether.stdout).contains("0 chunks (0 full), 1 missing"));
    assert_eq!(std::fs::read(world.join("level.dat")).unwrap(), level_bytes);
    assert_eq!(std::fs::read(region_path).unwrap(), region);
}

#[test]
#[ignore = "requires Minecraft 26.3, a usable Windows graphics device, and an interactive desktop"]
fn viewer_renders_a_world_without_modifying_it() {
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path();
    let (level_bytes, region_path, region) = create_world(world);
    let output = Command::new(env!("CARGO_BIN_EXE_native-client"))
        .arg("--view-world")
        .arg(world)
        .args(["--chunk", "-1", "-1", "--radius", "0", "--smoke-test"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("1 loaded chunks"), "{text}");
    assert!(text.contains("World viewer smoke test passed"), "{text}");
    assert_eq!(std::fs::read(world.join("level.dat")).unwrap(), level_bytes);
    assert_eq!(std::fs::read(region_path).unwrap(), region);
}

#[test]
#[ignore = "requires Minecraft 26.3, a prepared block report, and an interactive graphics device"]
fn viewer_resolves_default_state_using_a_verified_block_report() {
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path();
    let (level_bytes, region_path, region) =
        create_world_with_block(world, "minecraft:grass_block");
    let report = std::env::var_os("MINECRAFT_BLOCK_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local/block-registry")
        });
    let output = Command::new(env!("CARGO_BIN_EXE_native-client"))
        .arg("--view-world")
        .arg(world)
        .arg("--block-report")
        .arg(report)
        .args(["--chunk", "-1", "-1", "--radius", "0", "--smoke-test"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stdout.contains("Loaded default states for"), "{stdout}");
    assert!(
        stdout.contains("World viewer smoke test passed"),
        "{stdout}"
    );
    assert!(!stderr.contains("preview issues"), "{stderr}");
    assert_eq!(std::fs::read(world.join("level.dat")).unwrap(), level_bytes);
    assert_eq!(std::fs::read(region_path).unwrap(), region);
}

#[test]
#[ignore = "requires Minecraft 26.3, a usable Windows graphics device, and an interactive desktop"]
fn experimental_world_play_updates_mesh_without_writing_world_files() {
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path();
    let (level, region_path, region, player_path, player) = create_play_world(world);
    let output = Command::new(env!("CARGO_BIN_EXE_native-client"))
        .arg("--play-world")
        .arg(world)
        .args(["--radius", "0", "--smoke-test"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("World play mesh updated: revision 2"),
        "{stdout}"
    );
    assert!(stdout.contains("World texture atlas resized:"), "{stdout}");
    assert!(stdout.contains("World play smoke test passed"), "{stdout}");
    assert_eq!(std::fs::read(world.join("level.dat")).unwrap(), level);
    assert_eq!(std::fs::read(region_path).unwrap(), region);
    assert_eq!(std::fs::read(player_path).unwrap(), player);
    assert!(!world.join(".minecraft-native-backups").exists());
}

#[test]
#[ignore = "requires Minecraft 26.3, a usable Windows graphics device, and an interactive desktop"]
fn viewer_renders_water_lava_and_waterlogged_blocks_without_modifying_world() {
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path();
    let (level_bytes, region_path, region) = create_fluid_world(world);
    let pack = world.join("datapacks/color_override");
    let biome_dir = pack.join("data/minecraft/worldgen/biome");
    std::fs::create_dir_all(&biome_dir).unwrap();
    std::fs::write(
        pack.join("pack.mcmeta"),
        br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
    )
    .unwrap();
    let biome = br##"{"has_precipitation":true,"temperature":0.5,"downfall":0.8,"effects":{"water_color":"#123456"},"carvers":[],"features":[]}"##;
    std::fs::write(biome_dir.join("cherry_grove.json"), biome).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_native-client"))
        .arg("--view-world")
        .arg(world)
        .args(["--chunk", "-1", "-1", "--radius", "0", "--smoke-test"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stdout
        .lines()
        .find(|line| line.contains("translucent quads"))
        .unwrap();
    let count: u32 = line
        .split("translucent quads")
        .next()
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap()
        .parse()
        .unwrap();
    assert!(count > 0, "{stdout}");
    assert!(
        stdout.contains("World viewer smoke test passed"),
        "{stdout}"
    );
    assert!(!stderr.contains("preview issues"), "{stderr}");
    assert_eq!(std::fs::read(world.join("level.dat")).unwrap(), level_bytes);
    assert_eq!(std::fs::read(region_path).unwrap(), region);
    assert_eq!(
        std::fs::read(biome_dir.join("cherry_grove.json")).unwrap(),
        biome
    );
}
