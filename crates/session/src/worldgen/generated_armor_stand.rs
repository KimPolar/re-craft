//! Template armor stands with the canonical data used by 26.3 taiga armories.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::{Tag, TagType};
use native_resources::structure_transform::Rotation;

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

/// Java reloads the source entity before saving it; unknown data is rejected
/// until its load/save behavior is understood. Armor stands are not mobs and
/// therefore do not consume the world-generation region RNG on placement.
pub(super) fn draft_template_armor_stand(
    entity: &Tag,
    origin: [i32; 3],
    rotation: Rotation,
    pivot: [i32; 3],
    clip: BoundingBox,
    uuid: impl FnOnce() -> Result<[i32; 4]>,
) -> Result<Option<([i32; 3], Tag)>> {
    let Some((anchor, position)) = template_entity_location(entity, origin, rotation, pivot, clip)?
    else {
        return Ok(None);
    };
    let data = entity.get("nbt").context("armor stand NBT missing")?;
    let fields = match data {
        Tag::Compound(fields) => fields,
        _ => anyhow::bail!("invalid armor stand NBT"),
    };
    const KEYS: &[&str] = &[
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
        "DisabledSlots",
        "FallFlying",
        "Health",
        "HurtByTimestamp",
        "HurtTime",
        "Invisible",
        "NoBasePlate",
        "Pose",
        "ShowArms",
        "Small",
        "attributes",
        "equipment",
    ];
    ensure!(
        exact_fields(data, KEYS),
        "unsupported armor stand template fields"
    );
    ensure!(
        matches!(data.get("id"), Some(Tag::String(id)) if id.matches("minecraft:armor_stand")),
        "invalid armor stand ID"
    );
    ensure!(
        matches!(data.get("UUID"), Some(Tag::IntArray(v)) if v.len() == 4),
        "invalid armor stand UUID"
    );
    ensure!(
        matches!(data.get("Pos"), Some(Tag::List { element_type: TagType::Double, values }) if values.len() == 3 && values.iter().all(|v| matches!(v, Tag::Double(x) if x.is_finite()))),
        "invalid armor stand source position"
    );
    let yaw = match data.get("Rotation") {
        Some(Tag::List {
            element_type: TagType::Float,
            values,
        }) => match values.as_slice() {
            [Tag::Float(yaw), Tag::Float(pitch)] if yaw.is_finite() && pitch.to_bits() == 0 => *yaw,
            _ => anyhow::bail!("unsupported armor stand rotation"),
        },
        _ => anyhow::bail!("invalid armor stand rotation"),
    };
    let expected_motion = -0.0784000015258789_f64;
    ensure!(
        matches!(data.get("Motion"), Some(Tag::List { element_type: TagType::Double, values }) if matches!(values.as_slice(), [Tag::Double(x), Tag::Double(y), Tag::Double(z)] if x.to_bits() == 0 && y.to_bits() == expected_motion.to_bits() && z.to_bits() == 0)),
        "unsupported armor stand motion"
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
        ("DisabledSlots", Tag::Int(0)),
        ("FallFlying", Tag::Byte(0)),
        ("Health", Tag::Float(20.0)),
        ("HurtByTimestamp", Tag::Int(0)),
        ("HurtTime", Tag::Short(0)),
        ("Invisible", Tag::Byte(0)),
        ("NoBasePlate", Tag::Byte(0)),
        ("ShowArms", Tag::Byte(0)),
        ("Small", Tag::Byte(0)),
    ] {
        ensure!(
            data.get(key) == Some(&value),
            "unsupported armor stand state: {key}"
        );
    }
    let pose = data.get("Pose").context("armor stand pose missing")?;
    ensure!(
        exact_fields(pose, &["Body", "Head"]),
        "unsupported armor stand pose"
    );
    for key in ["Body", "Head"] {
        ensure!(
            matches!(pose.get(key), Some(Tag::List { element_type: TagType::Float, values }) if values.len() == 3 && values.iter().all(|v| matches!(v, Tag::Float(x) if x.is_finite() && x.abs() < 360.0)) && values.iter().any(|v| matches!(v, Tag::Float(x) if *x != 0.0))),
            "unsupported armor stand pose: {key}"
        );
    }
    let equipment = data
        .get("equipment")
        .context("armor stand equipment missing")?;
    let (slot, item, index) = if exact_fields(equipment, &["head"]) {
        ("head", "minecraft:iron_helmet", 3)
    } else if exact_fields(equipment, &["chest"]) {
        ("chest", "minecraft:iron_chestplate", 2)
    } else {
        anyhow::bail!("unsupported armor stand equipment")
    };
    let stack = equipment.get(slot).context("armor stand item missing")?;
    ensure!(
        exact_fields(stack, &["id", "count"])
            && matches!(stack.get("id"), Some(Tag::String(id)) if id.matches(item))
            && stack.get("count") == Some(&Tag::Int(1)),
        "unsupported armor stand item"
    );
    let empty = Tag::Compound(vec![]);
    ensure!(
        matches!(data.get("HandItems"), Some(Tag::List { element_type: TagType::Compound, values }) if values == &vec![empty.clone(), empty.clone()]),
        "unsupported armor stand legacy hand items"
    );
    ensure!(
        matches!(data.get("ArmorItems"), Some(Tag::List { element_type: TagType::Compound, values }) if values.len() == 4 && values.iter().enumerate().all(|(i, item)| if i == index { item == stack } else { item == &empty })),
        "unsupported armor stand legacy armor"
    );
    let expected_attributes = [
        ("minecraft:max_health", 20.0),
        ("minecraft:knockback_resistance", 0.0),
        ("minecraft:movement_speed", 0.699999988079071),
        ("minecraft:armor", 0.0),
        ("minecraft:armor_toughness", 0.0),
    ];
    let Some(Tag::List {
        element_type: TagType::Compound,
        values: attributes,
    }) = data.get("attributes")
    else {
        anyhow::bail!("invalid armor stand attributes")
    };
    ensure!(
        attributes.len() == expected_attributes.len()
            && expected_attributes.iter().all(|(name, base)| attributes
                .iter()
                .filter(|attribute| exact_fields(attribute, &["id", "base"])
                    && matches!(attribute.get("id"), Some(Tag::String(id)) if id.matches(name))
                    && attribute.get("base") == Some(&Tag::Double(*base)))
                .count()
                == 1),
        "unsupported armor stand attributes"
    );

    let uuid = uuid()?;
    let yaw = template_entity_yaw(yaw, rotation);
    let mut saved: Vec<_> = fields
        .iter()
        .filter(|(name, _)| {
            !["ArmorItems", "HandItems", "Dimension", "HurtByTimestamp"]
                .iter()
                .any(|key| name.matches(key))
        })
        .cloned()
        .collect();
    for (name, value) in &mut saved {
        if name.matches("UUID") {
            *value = Tag::IntArray(uuid.to_vec());
        } else if name.matches("Pos") {
            *value = Tag::List {
                element_type: TagType::Double,
                values: position.into_iter().map(Tag::Double).collect(),
            };
        } else if name.matches("Rotation") {
            *value = Tag::List {
                element_type: TagType::Float,
                values: vec![Tag::Float(yaw), Tag::Float(0.0)],
            };
        }
    }
    saved.push((
        "Brain".into(),
        compound(vec![("memories", Tag::Compound(vec![]))]),
    ));
    saved.push((
        "current_impulse_context_reset_grace_time".into(),
        Tag::Int(0),
    ));
    Ok(Some((anchor, Tag::Compound(saved))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(tag: Tag) -> Tag {
        match tag {
            Tag::Compound(fields) => {
                let mut fields: Vec<_> = fields
                    .into_iter()
                    .map(|(key, value)| {
                        let mut value = canonical(value);
                        if key.matches("attributes") {
                            if let Tag::List { values, .. } = &mut value {
                                values.sort_by_key(|value| match value.get("id") {
                                    Some(Tag::String(id)) => id.0.clone(),
                                    _ => Vec::new(),
                                });
                            }
                        }
                        (key, value)
                    })
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

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn vanilla_template_armor_stands_match_java_save() -> Result<()> {
        use native_persistence::nbt;
        use serde_json::Value;
        use std::{path::PathBuf, process::Command};
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
        let source = dir.path().join("GeneratedArmorStandTemplateOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GeneratedArmorStandTemplateOracle.java"),
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
            .arg("GeneratedArmorStandTemplateOracle")
            .arg(version.join("26.3.jar"))
            .arg(dir.path())
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == "STANDS 2"),
            "unexpected armor stand count"
        );
        let rotations = [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::Counterclockwise90,
        ];
        let clip = BoundingBox::new([-1024; 3], [1024; 3]).map_err(anyhow::Error::msg)?;
        for index in 0..2 {
            let source = nbt::read(&std::fs::read(
                dir.path().join(format!("source-{index}.nbt")),
            )?)?
            .value;
            for (rotation_index, rotation) in rotations.into_iter().enumerate() {
                for (pivot_index, pivot) in [[0; 3], [4, 0, 15]].into_iter().enumerate() {
                    let (_, actual) = draft_template_armor_stand(
                        &source,
                        [-32, 80, 48],
                        rotation,
                        pivot,
                        clip,
                        || Ok([0, 1, 0, 2]),
                    )?
                    .context("armor stand clipped")?;
                    let expected = nbt::read(&std::fs::read(
                        dir.path()
                            .join(format!("saved-{index}-{rotation_index}-{pivot_index}.nbt")),
                    )?)?
                    .value;
                    assert_eq!(
                        canonical(actual),
                        canonical(expected),
                        "armor stand {index}, {rotation:?}, pivot {pivot:?}"
                    );
                }
            }
            let small = BoundingBox::new([0; 3], [15; 3]).map_err(anyhow::Error::msg)?;
            let never = || -> Result<[i32; 4]> { anyhow::bail!("UUID requested") };
            assert!(
                draft_template_armor_stand(
                    &source,
                    [100, 0, 0],
                    Rotation::None,
                    [0; 3],
                    small,
                    never
                )?
                .is_none()
            );
            let mut unknown = source.clone();
            let Tag::Compound(outer) = &mut unknown else {
                unreachable!()
            };
            let (_, Tag::Compound(data)) = outer
                .iter_mut()
                .find(|(name, _)| name.matches("nbt"))
                .unwrap()
            else {
                unreachable!()
            };
            data.push(("unknown".into(), Tag::Int(1)));
            assert!(
                draft_template_armor_stand(&unknown, [0; 3], Rotation::None, [0; 3], small, never)
                    .is_err()
            );
        }
        Ok(())
    }
}
