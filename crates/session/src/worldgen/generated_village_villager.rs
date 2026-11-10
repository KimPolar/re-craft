//! Saved drafts for the canonical 26.3 village villager templates.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::{Tag, TagType};
use native_resources::{
    structure_transform::Rotation,
    worldgen_region_random::{RegionRandom, finalize_mob_base},
};

use super::template_entities::{template_entity_location, template_entity_yaw};

fn exact_fields(tag: &Tag, names: &[&str]) -> bool {
    matches!(tag, Tag::Compound(fields) if fields.len() == names.len()
        && names.iter().all(|key| fields.iter().filter(|(name, _)| name.matches(key)).count() == 1))
}

fn compound(fields: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

fn list(element_type: TagType, values: Vec<Tag>) -> Tag {
    Tag::List {
        element_type,
        values,
    }
}

/// Load and finalize a known vanilla village template resident. The caller
/// must use the same region RNG across all structures in the decoration pass.
/// Unverified fields are rejected and failures leave that RNG unchanged.
pub(super) fn draft_village_template_villager(
    entity: &Tag,
    origin: [i32; 3],
    rotation: Rotation,
    pivot: [i32; 3],
    clip: BoundingBox,
    random: &mut RegionRandom,
    uuid: impl FnOnce() -> Result<[i32; 4]>,
) -> Result<Option<([i32; 3], Tag)>> {
    let Some((anchor, position)) = template_entity_location(entity, origin, rotation, pivot, clip)?
    else {
        return Ok(None);
    };
    let data = entity.get("nbt").context("villager NBT missing")?;
    ensure!(matches!(data, Tag::Compound(_)), "invalid villager NBT");
    const COMMON: &[&str] = &[
        "id",
        "UUID",
        "Pos",
        "Motion",
        "Rotation",
        "fall_distance",
        "Fire",
        "Air",
        "OnGround",
        "Invulnerable",
        "PortalCooldown",
        "AbsorptionAmount",
        "ArmorItems",
        "HandItems",
        "DeathTime",
        "Dimension",
        "FallFlying",
        "Health",
        "HurtByTimestamp",
        "HurtTime",
        "Age",
        "ForcedAge",
        "Gossips",
        "Inventory",
        "CanPickUpLoot",
        "LeftHanded",
        "PersistenceRequired",
        "VillagerData",
        "Xp",
        "attributes",
    ];
    let age = match data.get("Age") {
        Some(Tag::Int(value @ (0 | -21359))) => *value,
        _ => anyhow::bail!("unsupported villager age"),
    };
    let baby = age < 0;
    let mut keys = COMMON.to_vec();
    if baby {
        keys.extend(["FoodLevel", "LastRestock"]);
    } else {
        keys.extend(["food_level", "lastRestock", "Leashed"]);
    }
    ensure!(
        exact_fields(data, &keys),
        "unsupported village villager template fields"
    );
    ensure!(
        matches!(data.get("id"), Some(Tag::String(id)) if id.matches("minecraft:villager")),
        "invalid villager ID"
    );
    ensure!(
        matches!(data.get("UUID"), Some(Tag::IntArray(v)) if v.len() == 4),
        "invalid villager UUID"
    );
    ensure!(
        matches!(data.get("Pos"), Some(Tag::List { element_type: TagType::Double, values }) if values.len() == 3 && values.iter().all(|v| matches!(v, Tag::Double(x) if x.is_finite()))),
        "invalid villager source position"
    );
    let (yaw, pitch) = match data.get("Rotation") {
        Some(Tag::List {
            element_type: TagType::Float,
            values,
        }) => match values.as_slice() {
            [Tag::Float(yaw), Tag::Float(pitch)] if yaw.is_finite() && pitch.is_finite() => {
                (*yaw, *pitch)
            }
            _ => anyhow::bail!("invalid villager rotation"),
        },
        _ => anyhow::bail!("invalid villager rotation"),
    };
    let expected_motion = -0.0784000015258789_f64;
    ensure!(
        matches!(data.get("Motion"), Some(Tag::List { element_type: TagType::Double, values }) if matches!(values.as_slice(), [Tag::Double(x), Tag::Double(y), Tag::Double(z)] if x.to_bits() == 0 && y.to_bits() == expected_motion.to_bits() && z.to_bits() == 0)),
        "unsupported villager motion"
    );
    for (key, value) in [
        ("fall_distance", Tag::Double(0.0)),
        ("Fire", Tag::Short(-1)),
        ("Air", Tag::Short(300)),
        ("OnGround", Tag::Byte(1)),
        ("Invulnerable", Tag::Byte(0)),
        ("PortalCooldown", Tag::Int(0)),
        ("AbsorptionAmount", Tag::Float(0.0)),
        ("DeathTime", Tag::Short(0)),
        ("Dimension", Tag::Int(0)),
        ("FallFlying", Tag::Byte(0)),
        ("Health", Tag::Float(20.0)),
        ("HurtByTimestamp", Tag::Int(0)),
        ("HurtTime", Tag::Short(0)),
        ("ForcedAge", Tag::Int(0)),
        ("CanPickUpLoot", Tag::Byte(1)),
        ("LeftHanded", Tag::Byte(0)),
        ("PersistenceRequired", Tag::Byte(0)),
        ("Xp", Tag::Int(0)),
    ] {
        ensure!(
            data.get(key) == Some(&value),
            "unsupported villager state: {key}"
        );
    }
    if baby {
        ensure!(
            data.get("FoodLevel") == Some(&Tag::Byte(0))
                && data.get("LastRestock") == Some(&Tag::Long(0)),
            "unsupported baby villager state"
        );
    } else {
        ensure!(
            data.get("food_level") == Some(&Tag::Byte(0))
                && data.get("lastRestock") == Some(&Tag::Long(0))
                && data.get("Leashed") == Some(&Tag::Byte(0)),
            "unsupported adult villager state"
        );
    }
    for key in ["attributes", "Gossips", "Inventory"] {
        ensure!(
            matches!(data.get(key), Some(Tag::List { values, .. }) if values.is_empty()),
            "unsupported villager {key}"
        );
    }
    let empty = Tag::Compound(vec![]);
    for (key, count) in [("ArmorItems", 4), ("HandItems", 2)] {
        ensure!(
            matches!(data.get(key), Some(Tag::List { element_type: TagType::Compound, values }) if values.len() == count && values.iter().all(|item| item == &empty)),
            "unsupported villager {key}"
        );
    }
    let identity = data
        .get("VillagerData")
        .context("villager identity missing")?;
    ensure!(
        exact_fields(identity, &["type", "profession", "level"])
            && identity.get("level") == Some(&Tag::Int(1)),
        "unsupported villager identity"
    );
    let allowed_types = [
        "minecraft:plains",
        "minecraft:desert",
        "minecraft:savanna",
        "minecraft:snow",
        "minecraft:taiga",
    ];
    ensure!(
        matches!(identity.get("type"), Some(Tag::String(value)) if allowed_types.iter().any(|id| value.matches(id))),
        "unsupported villager type"
    );
    let allowed_professions = ["minecraft:none", "minecraft:nitwit"];
    ensure!(
        matches!(identity.get("profession"), Some(Tag::String(value)) if allowed_professions.iter().any(|id| value.matches(id))),
        "unsupported villager profession"
    );
    ensure!(
        !baby
            || matches!(identity.get("profession"), Some(Tag::String(value)) if value.matches("minecraft:none")),
        "unsupported baby villager profession"
    );
    let Some(Tag::String(kind)) = identity.get("type") else {
        unreachable!()
    };
    let kind = String::from_utf16(&kind.0).context("invalid villager type")?;
    let mut villager_type = native_resources::villager_type::VillagerTypeState {
        kind: native_resources::packs::ResourceId::parse(&kind)?,
        finalized: native_resources::villager_type::finalized_after_load(false, true),
    };
    villager_type.finalize(
        || anyhow::bail!("finalized villager unexpectedly requested its biome"),
        |_| anyhow::bail!("finalized villager unexpectedly requested type lookup"),
    )?;

    let mut next = random.clone();
    let initialization = finalize_mob_base(&mut next, false);
    let uuid = uuid()?;
    let modifier = compound(vec![
        ("id", Tag::String("minecraft:random_spawn_bonus".into())),
        (
            "amount",
            Tag::Double(
                initialization
                    .follow_range_bonus
                    .context("villager follow range bonus missing")?,
            ),
        ),
        ("operation", Tag::String("add_multiplied_base".into())),
    ]);
    let attribute = compound(vec![
        ("id", Tag::String("minecraft:follow_range".into())),
        ("base", Tag::Double(16.0)),
        ("modifiers", list(TagType::Compound, vec![modifier])),
    ]);
    let saved = compound(vec![
        ("id", Tag::String("minecraft:villager".into())),
        ("UUID", Tag::IntArray(uuid.to_vec())),
        (
            "Pos",
            list(
                TagType::Double,
                position.into_iter().map(Tag::Double).collect(),
            ),
        ),
        ("Motion", data.get("Motion").unwrap().clone()),
        (
            "Rotation",
            list(
                TagType::Float,
                vec![
                    Tag::Float(template_entity_yaw(yaw, rotation)),
                    Tag::Float(pitch % 360.0),
                ],
            ),
        ),
        ("fall_distance", Tag::Double(0.0)),
        ("Fire", Tag::Short(-1)),
        ("Air", Tag::Short(300)),
        ("OnGround", Tag::Byte(1)),
        ("Invulnerable", Tag::Byte(0)),
        ("PortalCooldown", Tag::Int(0)),
        ("Health", Tag::Float(20.0)),
        ("HurtTime", Tag::Short(0)),
        ("DeathTime", Tag::Short(0)),
        ("AbsorptionAmount", Tag::Float(0.0)),
        ("current_impulse_context_reset_grace_time", Tag::Int(0)),
        ("attributes", list(TagType::Compound, vec![attribute])),
        ("FallFlying", Tag::Byte(0)),
        ("Brain", compound(vec![("memories", Tag::Compound(vec![]))])),
        ("CanPickUpLoot", Tag::Byte(1)),
        ("PersistenceRequired", Tag::Byte(0)),
        (
            "LeftHanded",
            Tag::Byte(i8::from(initialization.left_handed)),
        ),
        ("Age", Tag::Int(age)),
        ("ForcedAge", Tag::Int(0)),
        ("AgeLocked", Tag::Byte(0)),
        ("VillagerData", identity.clone()),
        ("VillagerDataFinalized", Tag::Byte(1)),
        ("FoodLevel", Tag::Byte(0)),
        ("Gossips", list(TagType::End, vec![])),
        ("Xp", Tag::Int(0)),
        ("LastRestock", Tag::Long(0)),
        ("LastGossipDecay", Tag::Long(0)),
        ("RestocksToday", Tag::Int(0)),
        ("Inventory", list(TagType::End, vec![])),
    ]);
    *random = next;
    Ok(Some((anchor, saved)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(tag: Tag) -> Tag {
        match tag {
            Tag::Compound(fields) => {
                let mut fields: Vec<_> = fields
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect();
                fields.sort_by(|(a, _), (b, _)| a.0.cmp(&b.0));
                Tag::Compound(fields)
            }
            Tag::List {
                element_type,
                values,
            } => Tag::List {
                element_type,
                values: values.into_iter().map(canonical).collect(),
            },
            other => other,
        }
    }

    fn identical(a: &Tag, b: &Tag) -> bool {
        match (a, b) {
            (Tag::Float(a), Tag::Float(b)) => a.to_bits() == b.to_bits(),
            (Tag::Double(a), Tag::Double(b)) => a.to_bits() == b.to_bits(),
            (Tag::Compound(a), Tag::Compound(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b)
                        .all(|((ak, av), (bk, bv))| ak == bk && identical(av, bv))
            }
            (
                Tag::List {
                    element_type: at,
                    values: a,
                },
                Tag::List {
                    element_type: bt,
                    values: b,
                },
            ) => at == bt && a.len() == b.len() && a.iter().zip(b).all(|(a, b)| identical(a, b)),
            _ => a == b,
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn village_template_villagers_match_java_nbt_and_region_random() -> Result<()> {
        use native_core::random::{LegacyRandom, XoroshiroRandom};
        use native_persistence::nbt;
        use serde_json::Value;
        use std::{collections::BTreeMap, path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(metadata["id"] == "26.3", "wrong Minecraft version");
        let dir = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), dir.path().to_path_buf()];
        for library in metadata["libraries"]
            .as_array()
            .context("missing libraries")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = dir.path().join("GeneratedVillageVillagerOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GeneratedVillageVillagerOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("GeneratedVillageVillagerOracle")
            .arg(version.join("26.3.jar"))
            .arg(dir.path())
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout)?;
        ensure!(
            output.lines().any(|line| line == "VILLAGERS 15"),
            "unexpected villager template count"
        );
        let mut cases = BTreeMap::new();
        for line in output.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            if let ["CASE", entity, rotation, algorithm, seed, next] = fields.as_slice() {
                ensure!(
                    cases
                        .insert(
                            (
                                entity.parse::<usize>()?,
                                rotation.parse::<usize>()?,
                                algorithm.parse::<usize>()?,
                                seed.parse::<usize>()?
                            ),
                            next.parse::<i64>()?
                        )
                        .is_none(),
                    "duplicate Java case"
                );
            }
        }
        let rotations = [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::Counterclockwise90,
        ];
        let seeds = [0, 38, 8, -1];
        let clip = BoundingBox::new([-1024; 3], [1024; 3]).map_err(anyhow::Error::msg)?;
        let mut left_handed = [0; 2];
        for entity in 0..15 {
            let source = nbt::read(&std::fs::read(
                dir.path().join(format!("source-{entity}.nbt")),
            )?)?
            .value;
            for (rotation_index, rotation) in rotations.into_iter().enumerate() {
                for algorithm in 0..2 {
                    for (seed_index, seed) in seeds.into_iter().enumerate() {
                        let mut random = if algorithm == 0 {
                            RegionRandom::Legacy(LegacyRandom::new(seed))
                        } else {
                            RegionRandom::Xoroshiro(XoroshiroRandom::new(seed))
                        };
                        let (_, actual) = draft_village_template_villager(
                            &source,
                            [-32, 80, 48],
                            rotation,
                            [0; 3],
                            clip,
                            &mut random,
                            || Ok([0, 1, 0, 2]),
                        )?
                        .context("villager clipped")?;
                        let expected = nbt::read(&std::fs::read(dir.path().join(format!(
                            "saved-{entity}-{rotation_index}-{algorithm}-{seed_index}.nbt"
                        )))?)?
                        .value;
                        if actual.get("LeftHanded") == Some(&Tag::Byte(1)) {
                            left_handed[algorithm] += 1;
                        }
                        let actual = canonical(actual);
                        let expected = canonical(expected);
                        assert!(
                            identical(&actual, &expected),
                            "villager {entity}, rotation {rotation:?}, algorithm {algorithm}, seed {seed}: {actual:?} != {expected:?}"
                        );
                        assert_eq!(
                            random.next_long(),
                            *cases
                                .get(&(entity, rotation_index, algorithm, seed_index))
                                .context("Java random case missing")?
                        );
                    }
                }
            }
            let mut unknown = source.clone();
            let Tag::Compound(outer) = &mut unknown else {
                unreachable!()
            };
            let (_, Tag::Compound(fields)) = outer
                .iter_mut()
                .find(|(name, _)| name.matches("nbt"))
                .context("NBT missing")?
            else {
                unreachable!()
            };
            fields.push(("unknown".into(), Tag::Int(1)));
            let mut random = RegionRandom::Legacy(LegacyRandom::new(42));
            let mut before = random.clone();
            assert!(
                draft_village_template_villager(
                    &unknown,
                    [0; 3],
                    Rotation::None,
                    [0; 3],
                    clip,
                    &mut random,
                    || anyhow::bail!("UUID unexpectedly requested")
                )
                .is_err()
            );
            assert_eq!(random.next_long(), before.next_long());
            let mut random = RegionRandom::Legacy(LegacyRandom::new(42));
            let mut before = random.clone();
            assert!(
                draft_village_template_villager(
                    &source,
                    [0; 3],
                    Rotation::None,
                    [0; 3],
                    clip,
                    &mut random,
                    || anyhow::bail!("UUID failed")
                )
                .is_err()
            );
            assert_eq!(random.next_long(), before.next_long());
        }
        assert_eq!(cases.len(), 15 * 4 * 2 * seeds.len());
        assert!(left_handed.iter().all(|count| *count > 0));
        eprintln!(
            "validated {} villager cases, {:?} left-handed by RNG",
            cases.len(),
            left_handed
        );
        Ok(())
    }
}
