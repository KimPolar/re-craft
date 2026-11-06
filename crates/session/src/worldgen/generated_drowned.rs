//! NBT drafts for freshly initialized structure-spawned drowned.
use anyhow::{Context, Result, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::{
    drowned_spawn::{ChickenJockey, HalloweenHelmet, MainHandRoll, StructureDrownedInitialization},
    worldgen_region_random::{AnimalBaseInitialization, MobBaseInitialization},
};

fn compound(fields: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}
fn modifier(id: &str, amount: f64, operation: &str) -> Tag {
    compound(vec![
        ("id", Tag::String(id.into())),
        ("amount", Tag::Double(amount)),
        ("operation", Tag::String(operation.into())),
    ])
}
fn attribute(id: &str, base: f64, modifiers: Vec<Tag>) -> Tag {
    let mut fields = vec![("id", Tag::String(id.into())), ("base", Tag::Double(base))];
    if !modifiers.is_empty() {
        fields.push((
            "modifiers",
            Tag::List {
                element_type: TagType::Compound,
                values: modifiers,
            },
        ));
    }
    compound(fields)
}
fn item(id: &str) -> Tag {
    compound(vec![("id", Tag::String(id.into())), ("count", Tag::Int(1))])
}

/// Snapshot the baby drowned when a newly created chicken is added during
/// Zombie.finalizeSpawn, before equipment and zombie attribute rolls. The
/// chicken is at the same position, so its rider's saved X/Z are unchanged.
pub fn draft_drowned_at_chicken_spawn(
    uuid: [i32; 4],
    block_position: [i32; 3],
    mob: MobBaseInitialization,
    can_pick_up_loot: bool,
) -> Result<Tag> {
    let follow = mob
        .follow_range_bonus
        .context("missing drowned follow-range bonus")?;
    ensure!(follow.is_finite(), "invalid drowned follow-range bonus");
    let position = [
        f64::from(block_position[0]) + 0.5,
        f64::from(block_position[1]),
        f64::from(block_position[2]) + 0.5,
    ];
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        0.0,
        "minecraft:drowned",
        20.0,
        AnimalBaseInitialization {
            set_baby_age: false,
            follow_range_bonus: Some(follow),
            left_handed: mob.left_handed,
        },
    )?
    else {
        unreachable!("fresh mob base is a compound")
    };
    fields.retain(|(key, _)| {
        !["Age", "ForcedAge", "AgeLocked", "InLove", "attributes"]
            .iter()
            .any(|name| key.matches(name))
    });
    for (key, value) in &mut fields {
        if key.matches("PersistenceRequired") {
            *value = Tag::Byte(1);
        }
        if key.matches("CanPickUpLoot") {
            *value = Tag::Byte(i8::from(can_pick_up_loot));
        }
    }
    fields.extend([
        (
            "attributes".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![
                    attribute(
                        "minecraft:follow_range",
                        35.0,
                        vec![modifier(
                            "minecraft:random_spawn_bonus",
                            follow,
                            "add_multiplied_base",
                        )],
                    ),
                    attribute("minecraft:movement_speed", f64::from(0.23_f32), vec![]),
                ],
            },
        ),
        ("IsBaby".into(), Tag::Byte(1)),
        ("CanBreakDoors".into(), Tag::Byte(0)),
        ("InWaterTime".into(), Tag::Int(-1)),
        ("DrownedConversionTime".into(), Tag::Int(-1)),
    ]);
    Ok(Tag::Compound(fields))
}

/// Build the saved compound for an unmounted freshly spawned structure
/// drowned. `main_hand` must be the final Java item-stack NBT after any active
/// enchantment provider ran. Mounted entities need their vehicle's passenger
/// serialization and are rejected until that path is available.
pub fn draft_fresh_structure_drowned(
    uuid: [i32; 4],
    block_position: [i32; 3],
    initialization: &StructureDrownedInitialization,
    main_hand: Option<Tag>,
) -> Result<Tag> {
    ensure!(
        initialization.chicken_jockey == ChickenJockey::None
            && !initialization.post.spawned_nautilus,
        "mounted drowned serialization is unsupported"
    );
    let expected_hand = match initialization.main_hand {
        MainHandRoll::Unchanged => None,
        MainHandRoll::Trident => Some("minecraft:trident"),
        MainHandRoll::FishingRod => Some("minecraft:fishing_rod"),
    };
    match (&main_hand, expected_hand) {
        (None, None) => {}
        (Some(tag), Some(id)) => {
            ensure!(
                matches!(tag, Tag::Compound(_)),
                "drowned equipment is not a compound"
            );
            ensure!(
                matches!(tag.get("id"), Some(Tag::String(actual)) if actual.matches(id)),
                "drowned main-hand item mismatch"
            );
            ensure!(
                tag.get("count") == Some(&Tag::Int(1)),
                "invalid drowned main-hand count"
            );
        }
        _ => anyhow::bail!("missing or unexpected drowned main-hand item"),
    }
    let stats = initialization.attributes;
    let follow = initialization
        .mob
        .follow_range_bonus
        .context("missing drowned follow-range bonus")?;
    ensure!(
        follow.is_finite()
            && stats.reinforcement_base.is_finite()
            && stats.knockback_bonus.is_finite(),
        "invalid drowned attributes"
    );
    ensure!(
        stats.zombie_follow_range_bonus.is_none_or(f64::is_finite),
        "invalid drowned follow-range modifier"
    );
    ensure!(
        stats.leader_reinforcement_bonus.is_some() == stats.leader_health_bonus.is_some(),
        "inconsistent leader modifiers"
    );
    ensure!(
        stats.leader_reinforcement_bonus.is_none_or(f64::is_finite)
            && stats
                .leader_health_bonus
                .is_none_or(|v| v.is_finite() && (1.0..4.0).contains(&v)),
        "invalid drowned leader modifiers"
    );
    ensure!(
        stats.leader_health_bonus.is_none() || initialization.can_break_doors,
        "leader drowned must break doors"
    );
    let health = stats
        .leader_health_bonus
        .map_or(20.0_f32, |bonus| (20.0 * (1.0 + bonus)) as f32);
    let position = [
        f64::from(block_position[0]) + 0.5,
        f64::from(block_position[1]),
        f64::from(block_position[2]) + 0.5,
    ];
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        0.0,
        "minecraft:drowned",
        health,
        AnimalBaseInitialization {
            set_baby_age: false,
            follow_range_bonus: Some(follow),
            left_handed: initialization.mob.left_handed,
        },
    )?
    else {
        unreachable!("fresh mob base is a compound")
    };
    fields.retain(|(key, _)| {
        !["Age", "ForcedAge", "AgeLocked", "InLove", "attributes"]
            .iter()
            .any(|name| key.matches(name))
    });
    for (key, value) in &mut fields {
        if key.matches("PersistenceRequired") {
            *value = Tag::Byte(1);
        }
        if key.matches("CanPickUpLoot") {
            *value = Tag::Byte(i8::from(initialization.can_pick_up_loot));
        }
    }
    let mut reinforcement_modifiers = Vec::new();
    if let Some(value) = stats.leader_reinforcement_bonus {
        reinforcement_modifiers.push(modifier(
            "minecraft:leader_zombie_bonus",
            value,
            "add_value",
        ));
    }
    let mut attributes = vec![
        attribute(
            "minecraft:spawn_reinforcements",
            stats.reinforcement_base,
            reinforcement_modifiers,
        ),
        attribute(
            "minecraft:knockback_resistance",
            0.0,
            vec![modifier(
                "minecraft:random_spawn_bonus",
                stats.knockback_bonus,
                "add_value",
            )],
        ),
    ];
    let mut follow_modifiers = vec![modifier(
        "minecraft:random_spawn_bonus",
        follow,
        "add_multiplied_base",
    )];
    if let Some(value) = stats.zombie_follow_range_bonus {
        follow_modifiers.push(modifier(
            "minecraft:zombie_random_spawn_bonus",
            value,
            "add_multiplied_total",
        ));
    }
    attributes.push(attribute("minecraft:follow_range", 35.0, follow_modifiers));
    if let Some(value) = stats.leader_health_bonus {
        attributes.push(attribute(
            "minecraft:max_health",
            20.0,
            vec![modifier(
                "minecraft:leader_zombie_bonus",
                value,
                "add_multiplied_total",
            )],
        ));
    }
    if initialization.baby {
        // setBaby creates the movement attribute; its speed modifier is transient.
        attributes.push(attribute(
            "minecraft:movement_speed",
            f64::from(0.23_f32),
            vec![],
        ));
    }
    fields.extend([
        (
            "attributes".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: attributes,
            },
        ),
        ("IsBaby".into(), Tag::Byte(i8::from(initialization.baby))),
        (
            "CanBreakDoors".into(),
            Tag::Byte(i8::from(initialization.can_break_doors)),
        ),
        ("InWaterTime".into(), Tag::Int(-1)),
        ("DrownedConversionTime".into(), Tag::Int(-1)),
    ]);
    let mut equipment = Vec::new();
    let mut drop_chances = Vec::new();
    if let Some(stack) = main_hand {
        equipment.push(("mainhand".into(), stack));
    }
    if let Some(helmet) = initialization.helmet {
        let id = match helmet {
            HalloweenHelmet::CarvedPumpkin => "minecraft:carved_pumpkin",
            HalloweenHelmet::JackOLantern => "minecraft:jack_o_lantern",
        };
        equipment.push(("head".into(), item(id)));
        drop_chances.push(("head".into(), Tag::Float(0.0)));
    }
    if initialization.post.give_nautilus_shell {
        equipment.push(("offhand".into(), item("minecraft:nautilus_shell")));
        drop_chances.push(("offhand".into(), Tag::Float(2.0)));
    }
    if !equipment.is_empty() {
        fields.push(("equipment".into(), Tag::Compound(equipment)));
    }
    if !drop_chances.is_empty() {
        fields.push(("drop_chances".into(), Tag::Compound(drop_chances)));
    }
    Ok(Tag::Compound(fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::random::{LegacyRandom, XoroshiroRandom};
    use native_persistence::nbt::{self, NamedTag};
    use native_resources::{
        drowned_spawn::{NautilusJockeyWorld, StructureDrownedWorld, initialize_structure_drowned},
        worldgen_region_random::RegionRandom,
    };

    struct World;
    impl NautilusJockeyWorld for World {
        fn more_frequent_drowned_biome(&mut self) -> Result<bool> {
            Ok(true)
        }
        fn spawn_zombie_nautilus(&mut self, _: &mut RegionRandom, _: bool) -> Result<bool> {
            anyhow::bail!("unexpected nautilus")
        }
    }
    impl StructureDrownedWorld for World {
        fn try_existing_chicken(&mut self) -> Result<bool> {
            Ok(false)
        }
        fn spawn_chicken_jockey(&mut self, _: &mut RegionRandom) -> Result<bool> {
            anyhow::bail!("chicken")
        }
        fn enchant_weapon(&mut self, _: &mut RegionRandom, _: MainHandRoll, _: f32) -> Result<()> {
            anyhow::bail!("enchant")
        }
    }

    #[test]
    fn rejects_incomplete_equipment_and_mounted_drowned() {
        let mut region = RegionRandom::Legacy(LegacyRandom::new(-12_345));
        let mut entity = LegacyRandom::new(3_329);
        let mut init =
            initialize_structure_drowned(&mut World, &mut region, &mut entity, 1.0, false).unwrap();
        let hand = match init.main_hand {
            MainHandRoll::Unchanged => None,
            MainHandRoll::Trident => Some(item("minecraft:trident")),
            MainHandRoll::FishingRod => Some(item("minecraft:fishing_rod")),
        };
        assert!(
            draft_fresh_structure_drowned([0, 1, 0, 2], [3, 10, -2], &init, hand.clone()).is_ok()
        );
        assert!(
            draft_fresh_structure_drowned(
                [0, 1, 0, 2],
                [3, 10, -2],
                &init,
                Some(item("minecraft:stone"))
            )
            .is_err()
        );
        init.chicken_jockey = ChickenJockey::Existing;
        assert!(
            draft_fresh_structure_drowned([0, 1, 0, 2], [3, 10, -2], &init, hand.clone()).is_err()
        );
        init.chicken_jockey = ChickenJockey::None;
        init.post.spawned_nautilus = true;
        assert!(draft_fresh_structure_drowned([0, 1, 0, 2], [3, 10, -2], &init, hand).is_err());
    }

    #[test]
    fn preserves_final_main_hand_component_patch() {
        for seed in 0..10_000 {
            let mut region = RegionRandom::Legacy(LegacyRandom::new(seed));
            let mut entity = LegacyRandom::new(seed + 1);
            let Ok(init) =
                initialize_structure_drowned(&mut World, &mut region, &mut entity, 0.0, false)
            else {
                continue;
            };
            let id = match init.main_hand {
                MainHandRoll::Unchanged => continue,
                MainHandRoll::Trident => "minecraft:trident",
                MainHandRoll::FishingRod => "minecraft:fishing_rod",
            };
            let components = Tag::Compound(vec![(
                "minecraft:custom_data".into(),
                Tag::Compound(vec![("kept".into(), Tag::Int(19))]),
            )]);
            let stack = compound(vec![
                ("id", Tag::String(id.into())),
                ("count", Tag::Int(1)),
                ("components", components),
            ]);
            let saved = draft_fresh_structure_drowned(
                [0, 1, 0, 2],
                [3, 10, -2],
                &init,
                Some(stack.clone()),
            )
            .unwrap();
            assert_eq!(
                saved.get("equipment").and_then(|v| v.get("mainhand")),
                Some(&stack)
            );
            return;
        }
        panic!("no armed drowned found in deterministic sample");
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn structure_drowned_nbt_matches_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let shared = temp.path().join("StructureDrownedOracle.java");
        let source = temp.path().join("GeneratedStructureDrownedOracle.java");
        std::fs::write(
            &shared,
            include_str!("../../../resources/tests/java/StructureDrownedOracle.java"),
        )?;
        std::fs::write(
            &source,
            include_str!("../../tests/java/GeneratedStructureDrownedOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&shared)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let date = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GeneratedStructureDrownedOracle")
            .arg("--halloween")
            .output()?;
        ensure!(
            date.status.success(),
            "Java date probe failed: {}",
            String::from_utf8_lossy(&date.stderr)
        );
        let date = String::from_utf8(date.stdout)?;
        let halloween: bool = date
            .lines()
            .find_map(|line| line.strip_prefix("H\t"))
            .context("missing date")?
            .parse()?;
        let factors = [0.0, 0.25, 0.5, 1.0];
        let mut cases = Vec::new();
        let mut babies = 0;
        let mut leaders = 0;
        let mut weapons = 0;
        let mut shells = 0;
        for i in 0..1200 {
            let region_seed = i64::from(i) * 671_231 - 12_345;
            let entity_seed = i64::from(i) * 98_317 + 3_329;
            let mut region = if i % 2 == 0 {
                RegionRandom::Legacy(LegacyRandom::new(region_seed))
            } else {
                RegionRandom::Xoroshiro(XoroshiroRandom::new(region_seed))
            };
            let mut entity = LegacyRandom::new(entity_seed);
            let special = factors[(i as usize / 8) % 4];
            let Ok(init) = initialize_structure_drowned(
                &mut World,
                &mut region,
                &mut entity,
                special,
                halloween,
            ) else {
                continue;
            };
            let hand = match init.main_hand {
                MainHandRoll::Unchanged => None,
                MainHandRoll::Trident => Some(item("minecraft:trident")),
                MainHandRoll::FishingRod => Some(item("minecraft:fishing_rod")),
            };
            let tag = draft_fresh_structure_drowned([0, 1, 0, 2], [3, 10, -2], &init, hand)?;
            let path = temp.path().join(format!("drowned-{i}.nbt"));
            std::fs::write(
                &path,
                nbt::write(&NamedTag {
                    name: "".into(),
                    value: tag,
                })?,
            )?;
            cases.push(serde_json::json!({"path":path,"legacy":i%2==0,"region_seed":region_seed,"entity_seed":entity_seed,"special":special,"next_region":region.next_long(),"next_entity":entity.next_long()}));
            babies += usize::from(init.baby);
            leaders += usize::from(init.attributes.leader_health_bonus.is_some());
            weapons += usize::from(init.main_hand != MainHandRoll::Unchanged);
            shells += usize::from(init.post.give_nautilus_shell);
        }
        ensure!(
            cases.len() > 1000 && babies > 0 && leaders > 0 && weapons > 0 && shells > 0,
            "insufficient case coverage"
        );
        let mut snapshots = 0;
        for i in 0..200_000 {
            let region_seed = i64::from(i) * 671_231 - 12_345;
            let entity_seed = i64::from(i) * 98_317 + 3_329;
            let mut region = if i % 2 == 0 {
                RegionRandom::Legacy(LegacyRandom::new(region_seed))
            } else {
                RegionRandom::Xoroshiro(XoroshiroRandom::new(region_seed))
            };
            let special = factors[(i as usize / 8) % 4];
            let mob =
                native_resources::worldgen_region_random::finalize_mob_base(&mut region, false);
            let can_pick_up_loot = region.next_float() < 0.55_f32 * special;
            if region.next_float() >= 0.05_f32
                || f64::from(region.next_float()) < 0.05_f64
                || f64::from(region.next_float()) >= 0.05_f64
            {
                continue;
            }
            let tag =
                draft_drowned_at_chicken_spawn([0, 1, 0, 2], [3, 10, -2], mob, can_pick_up_loot)?;
            let path = temp.path().join(format!("chicken-rider-{i}.nbt"));
            std::fs::write(
                &path,
                nbt::write(&NamedTag {
                    name: "".into(),
                    value: tag,
                })?,
            )?;
            let mut entity = LegacyRandom::new(entity_seed);
            cases.push(serde_json::json!({"path":path,"legacy":i%2==0,"region_seed":region_seed,"entity_seed":entity_seed,"special":special,"next_region":region.next_long(),"next_entity":entity.next_long(),"chicken_snapshot":true}));
            snapshots += 1;
            if snapshots == 128 {
                break;
            }
        }
        ensure!(snapshots == 128, "insufficient chicken snapshot cases");
        let input = temp.path().join("cases.json");
        std::fs::write(&input, serde_json::to_vec(&cases)?)?;
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(cp)
            .arg("GeneratedStructureDrownedOracle")
            .arg(input)
            .output()?;
        ensure!(
            output.status.success(),
            "Java NBT comparison failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            String::from_utf8(output.stdout)?
                .lines()
                .any(|line| line == format!("OK\t{}", cases.len())),
            "missing oracle result"
        );
        println!(
            "verified {} drowned NBT records ({babies} babies, {leaders} leaders, {weapons} weapons, {shells} shells)",
            cases.len()
        );
        Ok(())
    }
}
