use super::*;
use crate::worldgen::GeneratedMonsterRoomEntity;
use native_persistence::nbt::{self, Tag};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3"]
fn generated_rails_and_chains_are_checked_without_fabricating_block_entities() {
    let game = native_resources::default_minecraft_dir().unwrap();
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_generated_block_registry().unwrap();
    let (mut terrain, _, _, _) = super::tests::fixture();
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut states = Vec::new();
    for name in ["rail", "powered_rail", "detector_rail", "activator_rail"] {
        let id = format!("minecraft:{name}");
        let mut properties = registry.default_properties(&id).unwrap();
        properties.insert("waterlogged".into(), "true".into());
        properties.insert("shape".into(), "ascending_east".into());
        states.push(BlockState {
            name: id,
            properties,
        });
    }
    let mut chain = registry.default_properties("minecraft:iron_chain").unwrap();
    chain.insert("waterlogged".into(), "true".into());
    states.push(BlockState {
        name: "minecraft:iron_chain".into(),
        properties: chain,
    });
    {
        let world = Arc::make_mut(&mut terrain.world);
        let ids: Vec<_> = states
            .iter()
            .cloned()
            .map(|state| world.intern_state(state))
            .collect();
        let chunk = Arc::make_mut(world.chunks.get_mut(&chunk_pos).unwrap());
        let blocks = Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
            .blocks
            .as_mut()
            .unwrap();
        for (x, id) in ids.into_iter().enumerate() {
            blocks[x + 8 * 256] = id;
        }
    }
    let Tag::List { values, .. } = terrain
        .generated_block_entity_data(chunk_pos, &registry)
        .unwrap()
    else {
        panic!("missing block entity list")
    };
    assert!(values.is_empty());
    states[0]
        .properties
        .insert("shape".into(), "invalid".into());
    {
        let world = Arc::make_mut(&mut terrain.world);
        let id = world.intern_state(states.remove(0));
        let chunk = Arc::make_mut(world.chunks.get_mut(&chunk_pos).unwrap());
        Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
            .blocks
            .as_mut()
            .unwrap()[8 * 256] = id;
    }
    assert!(
        terrain
            .generated_block_entity_data(chunk_pos, &registry)
            .is_err()
    );
}

#[test]
fn pending_chest_resolution_is_transactional_and_preserves_state() {
    let (mut terrain, tags, motion, _) = super::tests::fixture();
    let position = [9, 8, 9];
    let chest = BlockState {
        name: "minecraft:chest".into(),
        properties: BTreeMap::from([
            ("facing".into(), "east".into()),
            ("type".into(), "left".into()),
            ("waterlogged".into(), "true".into()),
        ]),
    };
    let world = Arc::make_mut(&mut terrain.world);
    let id = world.intern_state(chest.clone());
    let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
    Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
        .blocks
        .as_mut()
        .unwrap()[(9 + 9 * 16 + 8 * 256) as usize] = id;
    Arc::make_mut(&mut terrain.generated_monster_entities).insert(
        position,
        Some(GeneratedMonsterRoomEntity::PendingChest {
            position,
            state: chest.clone(),
        }),
    );
    Arc::make_mut(
        terrain
            .generated_pending_order
            .entry(ChunkPos { x: 0, z: 0 })
            .or_default(),
    )
    .insert(position);
    let Tag::List { values, .. } = terrain
        .generated_block_entity_data_with(ChunkPos { x: 0, z: 0 }, |state| {
            Some(state.name == "minecraft:chest")
        })
        .unwrap()
    else {
        panic!("missing pending entity list")
    };
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].get("id"), Some(&Tag::String("DUMMY".into())));
    assert_eq!(values[0].get("x"), Some(&Tag::Int(position[0])));
    assert_eq!(values[0].get("y"), Some(&Tag::Int(position[1])));
    assert_eq!(values[0].get("z"), Some(&Tag::Int(position[2])));
    let writable = terrain.snapshot().chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
    assert!(!region.resolve_pending_chest([9, 9, 9]).unwrap());
    assert!(region.resolve_pending_chest(position).unwrap());
    assert!(region.resolve_pending_chest(position).unwrap());
    region.finish().unwrap();
    let entity = terrain.generated_monster_entities().next().unwrap();
    let GeneratedMonsterRoomEntity::EmptyChest(empty) = entity else {
        panic!("chest was not resolved")
    };
    assert_eq!(empty.state, chest);
    assert!(
        matches!(empty.block_entity().get("Items"), Some(Tag::List { values, .. }) if values.is_empty())
    );
    let data = terrain
        .generated_block_entity_data_with(ChunkPos { x: 0, z: 0 }, |state| {
            Some(state.name == "minecraft:chest")
        })
        .unwrap();
    let Tag::List { values, .. } = data else {
        panic!("missing block entity list")
    };
    assert_eq!(values.len(), 1);

    let (mut terrain, tags, motion, _) = super::tests::fixture();
    Arc::make_mut(&mut terrain.generated_monster_entities).insert(
        position,
        Some(GeneratedMonsterRoomEntity::PendingChest {
            position,
            state: chest,
        }),
    );
    let before = terrain.snapshot();
    let writable = before.chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
    assert!(region.resolve_pending_chest(position).is_err());
    assert!(region.finish().is_err());
    assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    assert!(matches!(
        terrain.generated_monster_entities().next(),
        Some(GeneratedMonsterRoomEntity::PendingChest { .. })
    ));
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn pending_chests_resolve_with_java_nbt_and_order() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
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
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("PendingChestOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/PendingChestOracle.java"),
    )
    .unwrap();
    let run = |command: &mut Command| {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("PendingChestOracle")
        .arg(temp.path()));

    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let generated = resources.vanilla_generated_block_registry().unwrap();
    let registry = resources.vanilla_block_ids().unwrap();
    let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
    let (mut terrain, tags, _, _) = super::tests::fixture();
    let mut records = Vec::new();
    let mut java_order = None;
    let mut java_middle_order = None;
    for line in output.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.first() == Some(&"P") {
            let i: usize = words[1].parse().unwrap();
            let pos = [
                words[5].parse().unwrap(),
                words[6].parse().unwrap(),
                words[7].parse().unwrap(),
            ];
            let json = serde_json::json!({"id":"minecraft:chest","properties":{
                "facing":words[2],"type":words[3],"waterlogged":words[4]
            }});
            let state = super::super::resolve_generated_terrain_state(&json, &generated).unwrap();
            let expected = nbt::read(&std::fs::read(temp.path().join(format!("{i}.nbt"))).unwrap())
                .unwrap()
                .value;
            records.push((pos, state, expected));
        } else if matches!(words.first(), Some(&"O") | Some(&"M")) {
            let parsed = Some(
                words[1..]
                    .iter()
                    .map(|entry| {
                        let values: Vec<i32> =
                            entry.split(',').map(|v| v.parse().unwrap()).collect();
                        [values[0], values[1], values[2]]
                    })
                    .collect::<Vec<_>>(),
            );
            if words[0] == "O" {
                java_order = parsed;
            } else {
                java_middle_order = parsed;
            }
        }
    }
    assert_eq!(records.len(), 24);
    let center = ChunkPos { x: 0, z: 0 };
    {
        let world = Arc::make_mut(&mut terrain.world);
        let ids: Vec<_> = records
            .iter()
            .map(|(pos, state, _)| (*pos, world.intern_state(state.clone())))
            .collect();
        let chunk = Arc::make_mut(world.chunks.get_mut(&center).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        let blocks = section.blocks.as_mut().unwrap();
        for (p, id) in ids {
            blocks[(p[0] + p[2] * 16 + p[1] * 256) as usize] = id;
        }
    }
    for (pos, state, _) in &records {
        Arc::make_mut(&mut terrain.generated_monster_entities).insert(
            *pos,
            Some(GeneratedMonsterRoomEntity::PendingChest {
                position: *pos,
                state: state.clone(),
            }),
        );
        Arc::make_mut(terrain.generated_pending_order.entry(center).or_default()).insert(*pos);
    }
    let writable = terrain.snapshot().chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
    assert!(!region.resolve_pending_chest([0, 9, 0]).unwrap());
    for (pos, _, _) in &records[..12] {
        assert!(region.resolve_pending_chest(*pos).unwrap());
    }
    assert!(region.resolve_pending_chest(records[0].0).unwrap());
    region.finish().unwrap();
    let Tag::List { values: middle, .. } = terrain
        .generated_block_entity_data(center, &generated)
        .unwrap()
    else {
        panic!("missing intermediate entity list")
    };
    let actual_middle: Vec<_> = middle
        .iter()
        .map(|tag| {
            let number = |name| match tag.get(name) {
                Some(Tag::Int(v)) => *v,
                _ => panic!("missing coordinate"),
            };
            [number("x"), number("y"), number("z")]
        })
        .collect();
    assert_eq!(actual_middle, java_middle_order.unwrap());
    for (p, actual) in actual_middle.iter().zip(&middle) {
        let i = records.iter().position(|(pos, _, _)| pos == p).unwrap();
        let expected =
            nbt::read(&std::fs::read(temp.path().join(format!("middle-{i}.nbt"))).unwrap())
                .unwrap()
                .value;
        let (Tag::Compound(actual_fields), Tag::Compound(expected_fields)) = (actual, &expected)
        else {
            panic!("invalid intermediate NBT at {p:?}")
        };
        let actual_names: BTreeSet<_> = actual_fields
            .iter()
            .map(|(name, _)| name.to_string_lossy())
            .collect();
        let expected_names: BTreeSet<_> = expected_fields
            .iter()
            .map(|(name, _)| name.to_string_lossy())
            .collect();
        assert_eq!(
            actual_names, expected_names,
            "intermediate NBT fields at {p:?}"
        );
        for field in actual_names {
            assert_eq!(
                actual.get(&field),
                expected.get(&field),
                "field {field} at {p:?}"
            );
        }
    }
    let writable = terrain.snapshot().chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
    for (pos, _, _) in &records[12..] {
        assert!(region.resolve_pending_chest(*pos).unwrap());
    }
    region.finish().unwrap();
    for (pos, expected_state, expected) in &records {
        let entity = terrain
            .generated_monster_entities()
            .find(|entity| {
                matches!(entity,
            GeneratedMonsterRoomEntity::EmptyChest(chest) if chest.position == *pos)
            })
            .unwrap();
        let GeneratedMonsterRoomEntity::EmptyChest(chest) = entity else {
            unreachable!()
        };
        assert_eq!(&chest.state, expected_state);
        let actual = chest.block_entity();
        let (Tag::Compound(actual_fields), Tag::Compound(expected_fields)) = (&actual, expected)
        else {
            panic!("invalid chest NBT at {pos:?}")
        };
        let actual_names: BTreeSet<_> = actual_fields
            .iter()
            .map(|(name, _)| name.to_string_lossy())
            .collect();
        let expected_names: BTreeSet<_> = expected_fields
            .iter()
            .map(|(name, _)| name.to_string_lossy())
            .collect();
        assert_eq!(actual_names, expected_names, "NBT fields at {pos:?}");
        for field in ["id", "x", "y", "z", "Items", "components"] {
            assert_eq!(
                actual.get(field),
                expected.get(field),
                "field {field} at {pos:?}"
            );
        }
    }
    let Tag::List { values, .. } = terrain
        .generated_block_entity_data(center, &generated)
        .unwrap()
    else {
        panic!("missing entity list")
    };
    let ordered: Vec<_> = values
        .iter()
        .map(|tag| {
            let number = |name| match tag.get(name) {
                Some(Tag::Int(v)) => *v,
                _ => panic!("missing coordinate"),
            };
            [number("x"), number("y"), number("z")]
        })
        .collect();
    assert_eq!(ordered, java_order.unwrap());

    let special = records
        .iter()
        .find(|(_, state, _)| {
            state.properties.get("type").map(String::as_str) == Some("left")
                && state.properties.get("waterlogged").map(String::as_str) == Some("true")
        })
        .unwrap();
    let writable = terrain.snapshot().chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
    native_resources::monster_room::MonsterRoomWorld::set_chest_loot(&mut region, special.0, 37)
        .unwrap();
    region.finish().unwrap();
    let entity = terrain.generated_monster_entities().find(|entity| matches!(entity, GeneratedMonsterRoomEntity::Chest(chest) if chest.position == special.0)).unwrap();
    let GeneratedMonsterRoomEntity::Chest(chest) = entity else {
        unreachable!()
    };
    assert_eq!(chest.loot_seed, 37);
    let converted = chest.block_state();
    let properties: BTreeMap<_, _> = converted
        .properties
        .iter()
        .map(|(k, v)| (k.to_string_lossy(), v.to_string_lossy()))
        .collect();
    assert_eq!(properties, special.1.properties);
    assert!(
        terrain
            .generated_block_entity_data(center, &generated)
            .is_ok()
    );
}
