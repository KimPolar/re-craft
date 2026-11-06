//! Lossless supported subset of Java 26.3 jigsaw piece NBT.
//!
//! This prepares and reads tags; it does not authorize writing a world. Inline
//! feature and processor holders are rejected until their codecs can be saved
//! without losing types or fields.
use anyhow::{Context, Result, bail, ensure};
use native_core::beardifier::BoundingBox;
use native_resources::{
    jigsaw_placement::{Junction, Piece},
    packs::ResourceId,
    structure_pool::{HolderValue, LiquidSettings, PoolElement, Projection},
    structure_transform::Rotation,
};

use crate::nbt::{Tag, TagType};

#[derive(Clone, Debug, PartialEq)]
pub struct SavedJigsawPiece {
    pub piece: Piece,
    pub liquid_settings: LiquidSettings,
}

/// A complete start composed exclusively of supported pool pieces.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedJigsawStart {
    pub structure: ResourceId,
    pub chunk: [i32; 2],
    pub references: i32,
    pub pieces: Vec<SavedJigsawPiece>,
}

impl SavedJigsawStart {
    pub fn to_tag(&self) -> Result<Tag> {
        ensure!(!self.pieces.is_empty(), "jigsaw start has no pieces");
        let children = self
            .pieces
            .iter()
            .map(SavedJigsawPiece::to_tag)
            .collect::<Result<Vec<_>>>()?;
        Ok(Tag::Compound(vec![
            ("id".into(), string(&id_text(&self.structure)?)),
            ("ChunkX".into(), Tag::Int(self.chunk[0])),
            ("ChunkZ".into(), Tag::Int(self.chunk[1])),
            ("references".into(), Tag::Int(self.references)),
            (
                "Children".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: children,
                },
            ),
        ]))
    }

    /// Unknown start or piece data is rejected rather than discarded.
    pub fn from_tag(tag: &Tag) -> Result<Self> {
        strict_fields(
            tag,
            &["id", "ChunkX", "ChunkZ", "references", "Children"],
            &[],
        )?;
        let structure = parse_id(&text(tag, "id")?)?;
        let Some(Tag::List {
            element_type: TagType::Compound,
            values,
        }) = tag.get("Children")
        else {
            bail!("invalid jigsaw start children")
        };
        ensure!(!values.is_empty(), "jigsaw start has no pieces");
        Ok(Self {
            structure,
            chunk: [integer(tag, "ChunkX")?, integer(tag, "ChunkZ")?],
            references: integer(tag, "references")?,
            pieces: values
                .iter()
                .map(SavedJigsawPiece::from_tag)
                .collect::<Result<_>>()?,
        })
    }
}

fn string(value: &str) -> Tag {
    Tag::String(value.into())
}
fn id_text(id: &ResourceId) -> Result<String> {
    let value = format!("{}:{}", id.namespace, id.path);
    ensure!(
        ResourceId::parse(&value)? == *id,
        "invalid resource identifier"
    );
    Ok(value)
}
fn projection_name(value: Projection) -> &'static str {
    match value {
        Projection::Rigid => "rigid",
        Projection::TerrainMatching => "terrain_matching",
    }
}
fn liquid_name(value: LiquidSettings) -> &'static str {
    match value {
        LiquidSettings::ApplyWaterlogging => "apply_waterlogging",
        LiquidSettings::IgnoreWaterlogging => "ignore_waterlogging",
    }
}
fn holder_tag(value: &HolderValue) -> Result<Tag> {
    match value {
        HolderValue::Reference(id) => Ok(string(&id_text(id)?)),
        HolderValue::Inline(_) => bail!("inline jigsaw holder cannot be preserved"),
    }
}

fn element_tag(element: &PoolElement, depth: usize) -> Result<Tag> {
    ensure!(depth <= 64, "jigsaw element nesting is too deep");
    let mut fields = Vec::new();
    match element {
        PoolElement::Empty => fields.push((
            "element_type".into(),
            string("minecraft:empty_pool_element"),
        )),
        PoolElement::Single {
            legacy,
            location,
            processors,
            projection,
            override_liquid_settings,
        } => {
            fields.push((
                "element_type".into(),
                string(if *legacy {
                    "minecraft:legacy_single_pool_element"
                } else {
                    "minecraft:single_pool_element"
                }),
            ));
            fields.push(("location".into(), string(&id_text(location)?)));
            fields.push(("processors".into(), holder_tag(processors)?));
            fields.push(("projection".into(), string(projection_name(*projection))));
            if let Some(liquid) = override_liquid_settings {
                fields.push((
                    "override_liquid_settings".into(),
                    string(liquid_name(*liquid)),
                ));
            }
        }
        PoolElement::Feature {
            feature,
            projection,
        } => {
            fields.push((
                "element_type".into(),
                string("minecraft:feature_pool_element"),
            ));
            fields.push(("feature".into(), holder_tag(feature)?));
            fields.push(("projection".into(), string(projection_name(*projection))));
        }
        PoolElement::List {
            elements,
            projection,
        } => {
            ensure!(!elements.is_empty(), "empty jigsaw element list");
            let values = elements
                .iter()
                .map(|element| {
                    if let Some(child_projection) = element_projection(element) {
                        ensure!(
                            child_projection == *projection,
                            "jigsaw list projection mismatch"
                        );
                    }
                    element_tag(element, depth + 1)
                })
                .collect::<Result<Vec<_>>>()?;
            fields.push(("element_type".into(), string("minecraft:list_pool_element")));
            fields.push((
                "elements".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values,
                },
            ));
            fields.push(("projection".into(), string(projection_name(*projection))));
        }
    }
    Ok(Tag::Compound(fields))
}

fn element_projection(element: &PoolElement) -> Option<Projection> {
    match element {
        PoolElement::Empty => None,
        PoolElement::Single { projection, .. }
        | PoolElement::Feature { projection, .. }
        | PoolElement::List { projection, .. } => Some(*projection),
    }
}

fn strict_fields(tag: &Tag, required: &[&str], optional: &[&str]) -> Result<()> {
    let Tag::Compound(fields) = tag else {
        bail!("jigsaw data must be a compound")
    };
    for required in required {
        ensure!(
            fields
                .iter()
                .filter(|(key, _)| key.matches(required))
                .count()
                == 1,
            "missing or duplicate jigsaw field {required}"
        );
    }
    for (key, _) in fields {
        ensure!(
            required
                .iter()
                .chain(optional)
                .any(|name| key.matches(name)),
            "unknown jigsaw field {}",
            key.to_string_lossy()
        );
        ensure!(
            fields.iter().filter(|(other, _)| other == key).count() == 1,
            "duplicate jigsaw field"
        );
    }
    Ok(())
}
fn text(tag: &Tag, name: &str) -> Result<String> {
    match tag.get(name) {
        Some(Tag::String(s)) => {
            String::from_utf16(&s.0).with_context(|| format!("invalid jigsaw field {name}"))
        }
        _ => bail!("invalid jigsaw field {name}"),
    }
}
fn integer(tag: &Tag, name: &str) -> Result<i32> {
    match tag.get(name) {
        Some(Tag::Int(value)) => Ok(*value),
        _ => bail!("invalid jigsaw field {name}"),
    }
}
fn parse_id(value: &str) -> Result<ResourceId> {
    let id = ResourceId::parse(value)?;
    ensure!(id_text(&id)? == value, "noncanonical resource identifier");
    Ok(id)
}
fn parse_projection(value: &str) -> Result<Projection> {
    match value {
        "rigid" => Ok(Projection::Rigid),
        "terrain_matching" => Ok(Projection::TerrainMatching),
        _ => bail!("invalid jigsaw projection"),
    }
}
fn parse_liquid(value: &str) -> Result<LiquidSettings> {
    match value {
        "apply_waterlogging" => Ok(LiquidSettings::ApplyWaterlogging),
        "ignore_waterlogging" => Ok(LiquidSettings::IgnoreWaterlogging),
        _ => bail!("invalid jigsaw liquid settings"),
    }
}
fn parse_element(tag: &Tag, depth: usize) -> Result<PoolElement> {
    ensure!(depth <= 64, "jigsaw element nesting is too deep");
    let kind = text(tag, "element_type")?;
    match kind.as_str() {
        "minecraft:empty_pool_element" => {
            strict_fields(tag, &["element_type"], &[])?;
            Ok(PoolElement::Empty)
        }
        "minecraft:single_pool_element" | "minecraft:legacy_single_pool_element" => {
            strict_fields(
                tag,
                &["element_type", "location", "processors", "projection"],
                &["override_liquid_settings"],
            )?;
            Ok(PoolElement::Single {
                legacy: kind == "minecraft:legacy_single_pool_element",
                location: parse_id(&text(tag, "location")?)?,
                processors: HolderValue::Reference(parse_id(&text(tag, "processors")?)?),
                projection: parse_projection(&text(tag, "projection")?)?,
                override_liquid_settings: tag
                    .get("override_liquid_settings")
                    .map(|_| parse_liquid(&text(tag, "override_liquid_settings")?))
                    .transpose()?,
            })
        }
        "minecraft:feature_pool_element" => {
            strict_fields(tag, &["element_type", "feature", "projection"], &[])?;
            Ok(PoolElement::Feature {
                feature: HolderValue::Reference(parse_id(&text(tag, "feature")?)?),
                projection: parse_projection(&text(tag, "projection")?)?,
            })
        }
        "minecraft:list_pool_element" => {
            strict_fields(tag, &["element_type", "elements", "projection"], &[])?;
            let projection = parse_projection(&text(tag, "projection")?)?;
            let Some(Tag::List {
                element_type: TagType::Compound,
                values,
            }) = tag.get("elements")
            else {
                bail!("invalid jigsaw element list")
            };
            ensure!(!values.is_empty(), "empty jigsaw element list");
            let elements = values
                .iter()
                .map(|v| parse_element(v, depth + 1))
                .collect::<Result<Vec<_>>>()?;
            ensure!(
                elements
                    .iter()
                    .all(|e| element_projection(e).is_none_or(|p| p == projection)),
                "jigsaw list projection mismatch"
            );
            Ok(PoolElement::List {
                elements,
                projection,
            })
        }
        _ => bail!("unsupported jigsaw element type {kind}"),
    }
}

impl SavedJigsawPiece {
    pub fn to_tag(&self) -> Result<Tag> {
        let piece = &self.piece;
        ensure!(
            !matches!(piece.element, PoolElement::Empty),
            "empty element cannot be a jigsaw piece"
        );
        ensure!(
            (0..3).all(|i| piece.bounds.min[i] <= piece.bounds.max[i]),
            "invalid jigsaw piece bounds"
        );
        let rotation = match piece.rotation {
            Rotation::None => "NONE",
            Rotation::Clockwise90 => "CLOCKWISE_90",
            Rotation::Clockwise180 => "CLOCKWISE_180",
            Rotation::Counterclockwise90 => "COUNTERCLOCKWISE_90",
        };
        let junctions = piece
            .junctions
            .iter()
            .map(|j| {
                Tag::Compound(vec![
                    ("source_x".into(), Tag::Int(j.source_x)),
                    ("source_ground_y".into(), Tag::Int(j.source_ground_y)),
                    ("source_z".into(), Tag::Int(j.source_z)),
                    ("delta_y".into(), Tag::Int(j.delta_y)),
                    (
                        "dest_proj".into(),
                        string(projection_name(j.destination_projection)),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        let mut fields = vec![
            ("id".into(), string("minecraft:jigsaw")),
            (
                "BB".into(),
                Tag::IntArray(
                    piece
                        .bounds
                        .min
                        .into_iter()
                        .chain(piece.bounds.max)
                        .collect(),
                ),
            ),
            ("O".into(), Tag::Int(-1)),
            ("GD".into(), Tag::Int(0)),
            ("PosX".into(), Tag::Int(piece.origin[0])),
            ("PosY".into(), Tag::Int(piece.origin[1])),
            ("PosZ".into(), Tag::Int(piece.origin[2])),
            (
                "ground_level_delta".into(),
                Tag::Int(piece.ground_level_delta),
            ),
            ("pool_element".into(), element_tag(&piece.element, 0)?),
            ("rotation".into(), string(rotation)),
            (
                "junctions".into(),
                Tag::List {
                    element_type: if junctions.is_empty() {
                        TagType::End
                    } else {
                        TagType::Compound
                    },
                    values: junctions,
                },
            ),
        ];
        if self.liquid_settings != LiquidSettings::ApplyWaterlogging {
            fields.push((
                "liquid_settings".into(),
                string(liquid_name(self.liquid_settings)),
            ));
        }
        Ok(Tag::Compound(fields))
    }

    /// Reject fields and holder encodings that cannot be written back intact.
    pub fn from_tag(tag: &Tag) -> Result<Self> {
        strict_fields(
            tag,
            &[
                "id",
                "BB",
                "O",
                "GD",
                "PosX",
                "PosY",
                "PosZ",
                "ground_level_delta",
                "pool_element",
                "rotation",
                "junctions",
            ],
            &["liquid_settings"],
        )?;
        ensure!(
            text(tag, "id")? == "minecraft:jigsaw"
                && integer(tag, "O")? == -1
                && integer(tag, "GD")? == 0,
            "unsupported jigsaw identity or orientation"
        );
        let Some(Tag::IntArray(values)) = tag.get("BB") else {
            bail!("invalid jigsaw bounds")
        };
        ensure!(values.len() == 6, "invalid jigsaw bounds length");
        let bounds = BoundingBox::new(
            [values[0], values[1], values[2]],
            [values[3], values[4], values[5]],
        )
        .map_err(anyhow::Error::msg)?;
        let rotation = match text(tag, "rotation")?.as_str() {
            "NONE" => Rotation::None,
            "CLOCKWISE_90" => Rotation::Clockwise90,
            "CLOCKWISE_180" => Rotation::Clockwise180,
            "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
            _ => bail!("invalid jigsaw rotation"),
        };
        let element = parse_element(tag.get("pool_element").context("missing pool element")?, 0)?;
        ensure!(
            !matches!(element, PoolElement::Empty),
            "empty element cannot be a jigsaw piece"
        );
        let Some(Tag::List {
            element_type,
            values,
        }) = tag.get("junctions")
        else {
            bail!("invalid jigsaw junctions")
        };
        ensure!(
            (*element_type == TagType::End && values.is_empty())
                || (*element_type == TagType::Compound && !values.is_empty()),
            "invalid jigsaw junction list type"
        );
        let junctions = values
            .iter()
            .map(|j| {
                strict_fields(
                    j,
                    &[
                        "source_x",
                        "source_ground_y",
                        "source_z",
                        "delta_y",
                        "dest_proj",
                    ],
                    &[],
                )?;
                Ok(Junction {
                    source_x: integer(j, "source_x")?,
                    source_ground_y: integer(j, "source_ground_y")?,
                    source_z: integer(j, "source_z")?,
                    delta_y: integer(j, "delta_y")?,
                    destination_projection: parse_projection(&text(j, "dest_proj")?)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let liquid_settings = tag
            .get("liquid_settings")
            .map(|_| parse_liquid(&text(tag, "liquid_settings")?))
            .transpose()?
            .unwrap_or(LiquidSettings::ApplyWaterlogging);
        ensure!(
            tag.get("liquid_settings").is_none()
                || liquid_settings != LiquidSettings::ApplyWaterlogging,
            "explicit default jigsaw liquid settings cannot be preserved"
        );
        Ok(Self {
            piece: Piece {
                element,
                origin: [
                    integer(tag, "PosX")?,
                    integer(tag, "PosY")?,
                    integer(tag, "PosZ")?,
                ],
                rotation,
                bounds,
                ground_level_delta: integer(tag, "ground_level_delta")?,
                junctions,
            },
            liquid_settings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::structure_pool::TemplatePool;
    use serde_json::json;

    fn sample() -> Result<SavedJigsawPiece> {
        let pool = TemplatePool::decode(
            &json!({"fallback":"minecraft:empty","elements":[{"weight":1,"element":{"element_type":"minecraft:single_pool_element","location":"example:house","processors":"minecraft:empty","projection":"rigid"}}]}),
        )?;
        Ok(SavedJigsawPiece {
            piece: Piece {
                element: pool.entries()[0].0.clone(),
                origin: [1, 2, -3],
                rotation: Rotation::Clockwise90,
                bounds: BoundingBox::new([0, 2, -3], [1, 4, -1]).unwrap(),
                ground_level_delta: 1,
                junctions: vec![Junction {
                    source_x: 1,
                    source_ground_y: 2,
                    source_z: 3,
                    delta_y: -1,
                    destination_projection: Projection::TerrainMatching,
                }],
            },
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
        })
    }

    #[test]
    fn saved_piece_roundtrips_and_rejects_unpreserved_data() -> Result<()> {
        let value = sample()?;
        let tag = value.to_tag()?;
        assert_eq!(SavedJigsawPiece::from_tag(&tag)?, value);
        let mut extra = tag.clone();
        let Tag::Compound(ref mut fields) = extra else {
            unreachable!()
        };
        fields.push(("unknown".into(), Tag::Int(1)));
        assert!(SavedJigsawPiece::from_tag(&extra).is_err());
        let mut inline = value.clone();
        let PoolElement::Single { processors, .. } = &mut inline.piece.element else {
            unreachable!()
        };
        *processors = HolderValue::Inline(json!({"processors":[]}));
        assert!(inline.to_tag().is_err());
        let mut default = value;
        default.piece.junctions.clear();
        default.liquid_settings = LiquidSettings::ApplyWaterlogging;
        let tag = default.to_tag()?;
        assert!(tag.get("liquid_settings").is_none());
        assert_eq!(SavedJigsawPiece::from_tag(&tag)?, default);
        let mut explicit_default = tag.clone();
        let Tag::Compound(ref mut fields) = explicit_default else {
            unreachable!()
        };
        fields.push(("liquid_settings".into(), string("apply_waterlogging")));
        assert!(SavedJigsawPiece::from_tag(&explicit_default).is_err());
        let mut typed_empty = tag;
        let Tag::Compound(ref mut fields) = typed_empty else {
            unreachable!()
        };
        let (_, junctions) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("junctions"))
            .unwrap();
        *junctions = Tag::List {
            element_type: TagType::Compound,
            values: vec![],
        };
        assert!(SavedJigsawPiece::from_tag(&typed_empty).is_err());
        Ok(())
    }

    fn equivalent(a: &Tag, b: &Tag) -> bool {
        match (a, b) {
            (Tag::Compound(a), Tag::Compound(b)) => {
                a.len() == b.len()
                    && a.iter().all(|(k, v)| {
                        b.iter()
                            .find(|(other, _)| other == k)
                            .is_some_and(|(_, other)| equivalent(v, other))
                    })
            }
            (
                Tag::List {
                    element_type: a_type,
                    values: a,
                },
                Tag::List {
                    element_type: b_type,
                    values: b,
                },
            ) => {
                a_type == b_type
                    && a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
            }
            _ => a == b,
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn pool_element_variants_and_liquid_settings_match_java() -> Result<()> {
        use std::{collections::BTreeMap, path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _resources = native_resources::packs::ResourceStack::open(&game)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home missing")?
            .join("bin");
        let source = temp.path().join("JigsawPieceOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawPieceOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawPieceOracle")
            .arg(temp.path())
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let mut cases = BTreeMap::new();
        let mut count = 0;
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["D", id, element] => {
                    let element: serde_json::Value = serde_json::from_str(element)?;
                    let pool = TemplatePool::decode(
                        &json!({"fallback":"minecraft:empty","elements":[{"weight":1,"element":element}]}),
                    )?;
                    cases.insert(id.parse::<usize>()?, pool.entries()[0].0.clone());
                }
                ["T", id, rotation, liquid, filename] => {
                    let element = cases.get(&id.parse()?).context("missing element")?.clone();
                    let rotation = match *rotation {
                        "0" => Rotation::None,
                        "1" => Rotation::Clockwise90,
                        "2" => Rotation::Clockwise180,
                        "3" => Rotation::Counterclockwise90,
                        _ => bail!("invalid rotation"),
                    };
                    let saved = SavedJigsawPiece {
                        piece: Piece {
                            element,
                            origin: [1, 2, -3],
                            rotation,
                            bounds: BoundingBox::new([-2, 2, -3], [5, 8, 9]).unwrap(),
                            ground_level_delta: 4,
                            junctions: vec![
                                Junction {
                                    source_x: 12,
                                    source_ground_y: -5,
                                    source_z: 7,
                                    delta_y: -2,
                                    destination_projection: Projection::Rigid,
                                },
                                Junction {
                                    source_x: -4,
                                    source_ground_y: 60,
                                    source_z: 2,
                                    delta_y: 3,
                                    destination_projection: Projection::TerrainMatching,
                                },
                            ],
                        },
                        liquid_settings: parse_liquid(liquid)?,
                    };
                    let java_tag =
                        crate::nbt::read(&std::fs::read(temp.path().join(filename))?)?.value;
                    let rust_tag = saved.to_tag()?;
                    assert!(
                        equivalent(&rust_tag, &java_tag),
                        "variant {id}: Rust {rust_tag:?}; Java {java_tag:?}"
                    );
                    assert_eq!(SavedJigsawPiece::from_tag(&java_tag)?, saved);
                    count += 1;
                }
                _ => {}
            }
        }
        assert_eq!(count, 32);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn ancient_city_piece_tags_match_java_and_roundtrip() -> Result<()> {
        use crate::jigsaw_assembly::TemplateAssemblyContext;
        use native_resources::{
            height_provider::HeightContext,
            jigsaw_placement::{StartEnvironment, StartHeightmap},
            jigsaw_structure::JigsawStructure,
            packs::ResourceProvider,
        };
        use std::{collections::BTreeMap, path::PathBuf, process::Command};
        struct Terrain;
        impl StartEnvironment for Terrain {
            fn valid_column(&mut self, _: i32, _: i32, _: i32, _: i32) -> Result<bool> {
                bail!("unexpected column check")
            }
            fn first_free_height(&mut self, _: i32, _: i32, _: StartHeightmap) -> Result<i32> {
                bail!("unexpected height check")
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home missing")?
            .join("bin");
        let shared = temp.path().join("JigsawPlacementOracle.java");
        let source = temp.path().join("JigsawConfiguredCityOracle.java");
        std::fs::write(
            &shared,
            include_str!("../../resources/tests/java/JigsawPlacementOracle.java"),
        )?;
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawConfiguredCityOracle.java"),
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
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawConfiguredCityOracle")
            .arg(version.join("26.3.jar"))
            .arg(temp.path())
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let data = resources
            .read_data("data/minecraft/worldgen/structure/ancient_city.json")?
            .context("missing city")?;
        let mut definition: serde_json::Value = serde_json::from_slice(&data)?;
        definition["size"] = json!(2);
        let settings = JigsawStructure::decode(&definition)?;
        let mut cache = BTreeMap::new();
        let mut checked = 0;
        let mut checked_starts = 0;
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            let record = match fields.as_slice() {
                ["T", seed, x, z, index, filename] => {
                    Some((*seed, *x, *z, Some(*index), *filename))
                }
                ["S", seed, x, z, filename] => Some((*seed, *x, *z, None, *filename)),
                _ => None,
            };
            if let Some((seed, x, z, index, filename)) = record {
                let key = (seed.parse::<i64>()?, x.parse::<i32>()?, z.parse::<i32>()?);
                if !cache.contains_key(&key) {
                    let mut seeded = settings.begin(
                        key.0,
                        [key.1, key.2],
                        HeightContext {
                            min_y: 0,
                            height: 0,
                            sea_level: 0,
                        },
                    )?;
                    let mut context = TemplateAssemblyContext::new(
                        &mut resources,
                        |_| Ok(-27),
                        seeded.aliases.clone(),
                    );
                    let start = seeded
                        .prepare(&settings, &mut context, &mut Terrain, -64, 319)?
                        .context("missing start")?;
                    let pieces =
                        start.assemble(&mut context, &mut |n| seeded.random.next_int_bound(n))?;
                    cache.insert(key, pieces);
                }
                let java_tag = crate::nbt::read(&std::fs::read(temp.path().join(filename))?)?.value;
                if let Some(index) = index {
                    let piece = cache[&key]
                        .get(index.parse::<usize>()?)
                        .context("missing piece")?
                        .clone();
                    let saved = SavedJigsawPiece {
                        piece,
                        liquid_settings: settings.liquid_settings,
                    };
                    let rust_tag = saved.to_tag()?;
                    assert!(
                        equivalent(&rust_tag, &java_tag),
                        "piece {key:?} {index}: Rust {rust_tag:?}; Java {java_tag:?}"
                    );
                    assert_eq!(SavedJigsawPiece::from_tag(&java_tag)?, saved);
                    checked += 1;
                } else {
                    let saved = SavedJigsawStart {
                        structure: ResourceId::parse("minecraft:ancient_city")?,
                        chunk: [key.1, key.2],
                        references: 0,
                        pieces: cache[&key]
                            .iter()
                            .cloned()
                            .map(|piece| SavedJigsawPiece {
                                piece,
                                liquid_settings: settings.liquid_settings,
                            })
                            .collect(),
                    };
                    let rust_tag = saved.to_tag()?;
                    assert!(
                        equivalent(&rust_tag, &java_tag),
                        "start {key:?}: Rust {rust_tag:?}; Java {java_tag:?}"
                    );
                    assert_eq!(SavedJigsawStart::from_tag(&java_tag)?, saved);
                    checked_starts += 1;
                }
            }
        }
        assert!(checked > 6, "expected multiple city pieces");
        assert_eq!(checked_starts, 6);
        Ok(())
    }
}
