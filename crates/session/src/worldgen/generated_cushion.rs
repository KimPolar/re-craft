//! Vanilla cushion entities loaded from 26.3 jigsaw templates.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::{Tag, TagType};
use native_resources::structure_transform::{Mirror, Rotation, transform_entity};

use super::template_entities::template_entity_anchor;

/// Prepare a known vanilla cushion without consuming the decoration RNG.
/// Unknown entity data is rejected until its Java load/save behavior is known.
pub(super) fn draft_template_cushion(
    entity: &Tag,
    origin: [i32; 3],
    rotation: Rotation,
    pivot: [i32; 3],
    clip: BoundingBox,
    uuid: impl FnOnce() -> Result<[i32; 4]>,
) -> Result<Option<([i32; 3], Tag)>> {
    ensure!(
        (0..3).all(|axis| clip.min[axis] <= clip.max[axis]),
        "invalid template entity clipping box"
    );
    let anchor = template_entity_anchor(entity, origin, Mirror::None, rotation, pivot)?;
    if !(0..3).all(|axis| (clip.min[axis]..=clip.max[axis]).contains(&anchor[axis])) {
        return Ok(None);
    }
    let Tag::Compound(outer) = entity else {
        anyhow::bail!("invalid template entity");
    };
    for key in ["pos", "nbt"] {
        ensure!(
            outer.iter().filter(|(name, _)| name.matches(key)).count() == 1,
            "missing or duplicate template entity {key}"
        );
    }
    let local = match entity.get("pos") {
        Some(Tag::List {
            element_type: TagType::Double,
            values,
        }) => match values.as_slice() {
            [Tag::Double(x), Tag::Double(y), Tag::Double(z)] => [*x, *y, *z],
            _ => anyhow::bail!("invalid template entity pos"),
        },
        _ => anyhow::bail!("invalid template entity pos"),
    };
    ensure!(
        local.iter().all(|value| value.is_finite()),
        "nonfinite template entity position"
    );
    let transformed = transform_entity(local, Mirror::None, rotation, pivot);
    let position =
        std::array::from_fn::<_, 3, _>(|axis| transformed[axis] + f64::from(origin[axis]));
    ensure!(
        position.iter().all(|v| v.is_finite())
            && position[0].abs() <= 30_000_512.0
            && position[2].abs() <= 30_000_512.0
            && position[1].abs() <= 20_000_000.0,
        "template entity position is outside supported bounds"
    );
    ensure!(
        (0..3).all(|axis| position[axis].floor() == f64::from(anchor[axis])),
        "template cushion position differs from its anchor"
    );
    let Some(Tag::Compound(fields)) = entity.get("nbt") else {
        anyhow::bail!("template entity NBT is missing");
    };
    let data = entity.get("nbt").context("template entity NBT missing")?;
    ensure!(
        matches!(data.get("id"), Some(Tag::String(id)) if id.matches("minecraft:cushion")),
        "structure template entity placement is not yet supported"
    );
    const NAMES: [&str; 13] = [
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
        "block_pos",
        "color",
    ];
    ensure!(
        fields.len() == NAMES.len()
            && NAMES
                .iter()
                .all(|key| fields.iter().filter(|(name, _)| name.matches(key)).count() == 1),
        "unsupported cushion template fields"
    );
    ensure!(
        matches!(data.get("UUID"), Some(Tag::IntArray(v)) if v.len() == 4),
        "invalid cushion UUID"
    );
    ensure!(
        matches!(data.get("block_pos"), Some(Tag::IntArray(v)) if v.len() == 3),
        "invalid cushion anchor"
    );
    ensure!(
        matches!(data.get("Pos"), Some(Tag::List { element_type: TagType::Double, values }) if values.len() == 3 && values.iter().all(|v| matches!(v, Tag::Double(x) if x.is_finite()))),
        "invalid cushion source position"
    );
    ensure!(
        matches!(data.get("Motion"), Some(Tag::List { element_type: TagType::Double, values }) if values.len() == 3 && values.iter().all(|v| matches!(v, Tag::Double(x) if x.to_bits() == 0))),
        "unsupported cushion motion"
    );
    ensure!(
        matches!(data.get("fall_distance"), Some(Tag::Double(v)) if v.to_bits() == 0)
            && data.get("Fire") == Some(&Tag::Short(0))
            && data.get("Air") == Some(&Tag::Short(300))
            && data.get("OnGround") == Some(&Tag::Byte(0))
            && data.get("Invulnerable") == Some(&Tag::Byte(0))
            && data.get("PortalCooldown") == Some(&Tag::Int(0)),
        "unsupported cushion state"
    );
    let yaw = match data.get("Rotation") {
        Some(Tag::List {
            element_type: TagType::Float,
            values,
        }) => match values.as_slice() {
            [Tag::Float(yaw), Tag::Float(pitch)] if yaw.is_finite() && pitch.to_bits() == 0 => *yaw,
            _ => anyhow::bail!("unsupported cushion rotation"),
        },
        _ => anyhow::bail!("unsupported cushion rotation"),
    };
    const COLORS: [&str; 16] = [
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
    ensure!(
        matches!(data.get("color"), Some(Tag::String(color)) if COLORS.iter().any(|value| color.matches(value))),
        "unsupported cushion color"
    );

    // Entity.load applies Java's remainder before rotate/mirror are called.
    let loaded_yaw = yaw % 360.0;
    let mut wrapped = loaded_yaw % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    let rotated = wrapped
        + match rotation {
            Rotation::None => 0.0,
            Rotation::Clockwise90 => 90.0,
            Rotation::Clockwise180 => 180.0,
            Rotation::Counterclockwise90 => 270.0,
        };
    let final_yaw = rotated + (wrapped - loaded_yaw);
    let uuid = uuid()?;
    let mut output = fields.clone();
    for (name, value) in &mut output {
        if name.matches("UUID") {
            *value = Tag::IntArray(uuid.to_vec());
        } else if name.matches("Pos") {
            *value = Tag::List {
                element_type: TagType::Double,
                values: position.into_iter().map(Tag::Double).collect(),
            };
        } else if name.matches("block_pos") {
            *value = Tag::IntArray(anchor.to_vec());
        } else if name.matches("Rotation") {
            *value = Tag::List {
                element_type: TagType::Float,
                values: vec![Tag::Float(final_yaw), Tag::Float(0.0)],
            };
        }
    }
    Ok(Some((anchor, Tag::Compound(output))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(yaw: f32) -> Tag {
        let doubles = |v: Vec<f64>| Tag::List {
            element_type: TagType::Double,
            values: v.into_iter().map(Tag::Double).collect(),
        };
        Tag::Compound(vec![
            ("blockPos".into(), Tag::IntArray(vec![4, 0, 6])),
            ("pos".into(), doubles(vec![4.5, 0.9375, 6.5])),
            (
                "nbt".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:cushion".into())),
                    ("UUID".into(), Tag::IntArray(vec![9, 8, 7, 6])),
                    ("Pos".into(), doubles(vec![-89.5, -60.0625, -131.5])),
                    ("Motion".into(), doubles(vec![0.0, 0.0, 0.0])),
                    (
                        "Rotation".into(),
                        Tag::List {
                            element_type: TagType::Float,
                            values: vec![Tag::Float(yaw), Tag::Float(0.0)],
                        },
                    ),
                    ("fall_distance".into(), Tag::Double(0.0)),
                    ("Fire".into(), Tag::Short(0)),
                    ("Air".into(), Tag::Short(300)),
                    ("OnGround".into(), Tag::Byte(0)),
                    ("Invulnerable".into(), Tag::Byte(0)),
                    ("PortalCooldown".into(), Tag::Int(0)),
                    ("block_pos".into(), Tag::IntArray(vec![-90, -61, -132])),
                    ("color".into(), Tag::String("orange".into())),
                ]),
            ),
        ])
    }

    #[test]
    fn cushion_uses_rotated_anchor_and_java_yaw_without_reusing_uuid() -> Result<()> {
        let clip = BoundingBox::new([-100, 0, 0], [0, 100, 100]).map_err(anyhow::Error::msg)?;
        let (anchor, value) = draft_template_cushion(
            &sample(180.0),
            [-32, 80, 48],
            Rotation::Clockwise90,
            [0; 3],
            clip,
            || Ok([0, 1, 0, 2]),
        )?
        .context("cushion was clipped")?;
        assert_eq!(anchor, [-38, 80, 52]);
        assert_eq!(value.get("UUID"), Some(&Tag::IntArray(vec![0, 1, 0, 2])));
        assert_eq!(
            value.get("block_pos"),
            Some(&Tag::IntArray(anchor.to_vec()))
        );
        assert_eq!(
            value.get("Pos"),
            Some(&Tag::List {
                element_type: TagType::Double,
                values: vec![Tag::Double(-37.5), Tag::Double(80.9375), Tag::Double(52.5)]
            })
        );
        assert_eq!(
            value.get("Rotation"),
            Some(&Tag::List {
                element_type: TagType::Float,
                values: vec![Tag::Float(-450.0), Tag::Float(0.0)]
            })
        );
        Ok(())
    }

    #[test]
    fn clipped_and_unsupported_records_never_request_uuid() -> Result<()> {
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let never = || -> Result<[i32; 4]> { anyhow::bail!("UUID unexpectedly requested") };
        assert!(
            draft_template_cushion(
                &sample(0.0),
                [32, 0, 0],
                Rotation::None,
                [0; 3],
                clip,
                never
            )?
            .is_none()
        );
        let mut bad = sample(0.0);
        let Tag::Compound(outer) = &mut bad else {
            unreachable!()
        };
        let (_, Tag::Compound(fields)) = outer
            .iter_mut()
            .find(|(name, _)| name.matches("nbt"))
            .unwrap()
        else {
            unreachable!()
        };
        fields.push(("custom".into(), Tag::Int(1)));
        let error = draft_template_cushion(&bad, [0, 0, 0], Rotation::None, [0; 3], clip, never)
            .err()
            .context("unknown data was accepted")?;
        assert!(format!("{error:#}").contains("unsupported cushion template fields"));
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25"]
    fn vanilla_cushion_template_matches_java_save_for_colors_and_rotations() -> Result<()> {
        use native_persistence::{nbt, structure_template::StructureTemplate};
        use native_resources::packs::{ResourceId, ResourceStack};
        use serde_json::Value;
        use std::{path::PathBuf, process::Command};

        fn canonical(tag: Tag) -> Tag {
            match tag {
                Tag::Compound(fields) => {
                    let mut fields: Vec<_> = fields
                        .into_iter()
                        .map(|(key, value)| (key, canonical(value)))
                        .collect();
                    fields.sort_by(|a, b| a.0.0.cmp(&b.0.0));
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
                ) => {
                    at == bt && a.len() == b.len() && a.iter().zip(b).all(|(a, b)| identical(a, b))
                }
                _ => a == b,
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let template = StructureTemplate::load(
            &mut resources,
            &ResourceId::parse(
                "minecraft:abandoned_camp/camp/bamboo_jungle/campsite_bamboo_jungle_1",
            )?,
        )?
        .context("vanilla cushion template missing")?;
        let source = template
            .entities
            .first()
            .context("template cushion missing")?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source_path = temp.path().join("GeneratedCushionTemplateOracle.java");
        std::fs::write(
            &source_path,
            include_str!("../../tests/java/GeneratedCushionTemplateOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source_path)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("GeneratedCushionTemplateOracle")
            .arg(version.join("26.3.jar"))
            .arg(temp.path())
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == "CASES 320"),
            "Java case count missing"
        );
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
        let yaws = [-450.0, 0.0, 90.0, 180.0, 270.0];
        let rotations = [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::Counterclockwise90,
        ];
        let clip = BoundingBox::new([-100, 0, 0], [0, 100, 100]).map_err(anyhow::Error::msg)?;
        let mut index = 0;
        for color in colors {
            for yaw in yaws {
                for rotation in rotations {
                    let mut entity = source.clone();
                    let Tag::Compound(outer) = &mut entity else {
                        anyhow::bail!("invalid source entity")
                    };
                    let (_, Tag::Compound(fields)) = outer
                        .iter_mut()
                        .find(|(name, _)| name.matches("nbt"))
                        .context("source NBT missing")?
                    else {
                        anyhow::bail!("invalid source NBT")
                    };
                    for (name, value) in fields {
                        if name.matches("color") {
                            *value = Tag::String(color.into());
                        } else if name.matches("Rotation") {
                            *value = Tag::List {
                                element_type: TagType::Float,
                                values: vec![Tag::Float(yaw), Tag::Float(0.0)],
                            };
                        }
                    }
                    let (_, actual) = draft_template_cushion(
                        &entity,
                        [-32, 80, 48],
                        rotation,
                        [0; 3],
                        clip,
                        || Ok([0, 1, 0, 2]),
                    )?
                    .context("cushion clipped")?;
                    let expected = nbt::read(&std::fs::read(
                        temp.path().join(format!("cushion-{index}.nbt")),
                    )?)?
                    .value;
                    let actual = canonical(actual);
                    let expected = canonical(expected);
                    assert!(
                        identical(&actual, &expected),
                        "case {index}: {color} {yaw} {rotation:?}: {actual:?} != {expected:?}"
                    );
                    index += 1;
                }
            }
        }
        assert_eq!(index, 320);
        ensure!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == "VANILLA 224"),
            "vanilla cushion count differs"
        );
        let clip = BoundingBox::new([-1024; 3], [1024; 3]).map_err(anyhow::Error::msg)?;
        for entity_index in 0..224 {
            let source = nbt::read(&std::fs::read(
                temp.path()
                    .join(format!("vanilla-source-{entity_index}.nbt")),
            )?)?
            .value;
            for (rotation_index, rotation) in rotations.into_iter().enumerate() {
                for (prefix, pivot) in [("vanilla", [0; 3]), ("shipwreck-pivot", [4, 0, 15])] {
                    let (_, actual) = draft_template_cushion(
                        &source,
                        [-32, 80, 48],
                        rotation,
                        pivot,
                        clip,
                        || Ok([0, 1, 0, 2]),
                    )?
                    .with_context(|| format!("vanilla cushion {entity_index} was clipped"))?;
                    let expected = nbt::read(&std::fs::read(
                        temp.path()
                            .join(format!("{prefix}-{entity_index}-{rotation_index}.nbt")),
                    )?)?
                    .value;
                    let actual = canonical(actual);
                    let expected = canonical(expected);
                    assert!(
                        identical(&actual, &expected),
                        "{prefix} cushion {entity_index}, {rotation:?}: {actual:?} != {expected:?}"
                    );
                }
            }
        }
        Ok(())
    }
}
