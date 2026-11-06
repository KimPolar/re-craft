//! Read structure templates from the active data-pack stack.
//!
//! Template data versions are retained, not upgraded. Callers must run the
//! appropriate data conversion and block-state validation before placement.
use anyhow::{Context, Result, bail, ensure};
use native_resources::packs::{ResourceId, ResourceProvider};

use crate::nbt::{self, NamedTag, Tag, TagType};

#[derive(Clone, Debug, PartialEq)]
pub struct TemplateBlock {
    pub position: [i32; 3],
    pub state: usize,
    /// Preserves all template-provided block-entity fields.
    pub nbt: Option<Tag>,
}

/// Raw palette and entity tags are preserved for later Java-compatible
/// data-version conversion and placement processing.
#[derive(Clone, Debug, PartialEq)]
pub struct StructureTemplate {
    pub document: NamedTag,
    pub data_version: Option<i32>,
    pub size: [i32; 3],
    pub palettes: Vec<Vec<Tag>>,
    pub blocks: Vec<TemplateBlock>,
    pub entities: Vec<Tag>,
}

fn list<'a>(tag: &'a Tag, element: TagType, label: &str) -> Result<&'a [Tag]> {
    match tag {
        Tag::List {
            element_type,
            values,
        } if *element_type == element || (values.is_empty() && *element_type == TagType::End) => {
            Ok(values)
        }
        _ => bail!("{label} has an invalid list type"),
    }
}

fn int_vec3(tag: &Tag, label: &str) -> Result<[i32; 3]> {
    let values = list(tag, TagType::Int, label)?;
    ensure!(values.len() == 3, "{label} must contain three integers");
    let mut result = [0; 3];
    for (out, value) in result.iter_mut().zip(values) {
        let Tag::Int(value) = value else {
            bail!("invalid {label} component")
        };
        *out = *value;
    }
    Ok(result)
}

impl StructureTemplate {
    /// Match StructureTemplate.getJigsaws: select the palette from the
    /// position-derived random source, then rotate and translate its records.
    /// Data-version conversion is still the caller's responsibility.
    pub fn jigsaws_at(
        &self,
        origin: [i32; 3],
        rotation: native_resources::structure_transform::Rotation,
    ) -> Result<Vec<native_resources::jigsaw::Connector>> {
        let count = i32::try_from(self.palettes.len()).context("too many template palettes")?;
        ensure!(count > 0, "template has no palettes");
        let seed = native_resources::blockstates::positional_seed(origin[0], origin[1], origin[2]);
        let mut random = native_core::random::LegacyRandom::new(seed);
        let index = random
            .next_int_bound(count)
            .context("invalid template palette count")? as usize;
        Ok(self
            .jigsaw_connectors(index)?
            .into_iter()
            .map(|connector| connector.transformed(origin, rotation))
            .collect())
    }

    /// Read the Java jigsaw records from one already-selected palette. The
    /// caller must first apply data-version conversion and select the palette
    /// using StructurePlaceSettings' position-dependent random source.
    pub fn jigsaw_connectors(
        &self,
        palette_index: usize,
    ) -> Result<Vec<native_resources::jigsaw::Connector>> {
        use native_resources::jigsaw::{Connector, Joint, Orientation};
        let palette = self
            .palettes
            .get(palette_index)
            .context("template palette index out of bounds")?;
        let mut blocks = Vec::new();
        for block in &self.blocks {
            let state = palette
                .get(block.state)
                .context("template state index outside palette")?;
            let id = match state {
                Tag::String(id) => Some(id),
                _ => match state.get("id") {
                    Some(Tag::String(id)) => Some(id),
                    _ => None,
                },
            };
            if id.is_some_and(|id| id.matches("minecraft:jigsaw")) {
                blocks.push((block, state));
            }
        }
        // Jigsaw blocks have block-entity NBT, so Java's template loader sorts
        // them by Y, X, Z before constructing the palette's connector list.
        blocks.sort_by_key(|(block, _)| (block.position[1], block.position[0], block.position[2]));
        let empty = ResourceId::parse("minecraft:empty")?;
        let mut result = Vec::with_capacity(blocks.len());
        for (block, state) in blocks {
            let nbt = block
                .nbt
                .as_ref()
                .context("jigsaw block entity data is missing")?;
            let orientation = match state.get("properties").and_then(|p| p.get("orientation")) {
                Some(Tag::String(value)) => Orientation::parse(&value.to_string_lossy())?,
                None => Orientation::parse("north_up")?,
                _ => bail!("invalid jigsaw orientation property"),
            };
            let id_or_empty = |name: &str| {
                nbt.get(name)
                    .and_then(|value| match value {
                        Tag::String(value) => String::from_utf16(&value.0)
                            .ok()
                            .and_then(|s| ResourceId::parse(&s).ok()),
                        _ => None,
                    })
                    .unwrap_or_else(|| empty.clone())
            };
            let joint = match nbt.get("joint") {
                Some(Tag::String(value)) if value.matches("aligned") => Joint::Aligned,
                Some(Tag::String(value)) if value.matches("rollable") => Joint::Rollable,
                _ => orientation.default_joint(),
            };
            result.push(Connector {
                position: block.position,
                orientation,
                joint,
                name: Some(id_or_empty("name")),
                target: id_or_empty("target"),
                pool: id_or_empty("pool"),
                placement_priority: nbt
                    .get("placement_priority")
                    .and_then(Tag::as_java_int)
                    .unwrap_or(0),
                selection_priority: nbt
                    .get("selection_priority")
                    .and_then(Tag::as_java_int)
                    .unwrap_or(0),
            });
        }
        Ok(result)
    }

    /// Java sorts full, non-full, then NBT-bearing blocks separately by
    /// local Y, X, Z. Classification applies to untransformed palette states.
    /// Returns source indices without modifying the original template.
    pub fn java_block_order(
        &self,
        palette_index: usize,
        mut is_static_full: impl FnMut(usize, &Tag) -> Result<bool>,
    ) -> Result<Vec<usize>> {
        let palette = self
            .palettes
            .get(palette_index)
            .context("template palette index out of bounds")?;
        let mut cached = vec![None; palette.len()];
        let mut keys = Vec::with_capacity(self.blocks.len());
        for (index, block) in self.blocks.iter().enumerate() {
            let state = palette
                .get(block.state)
                .context("template state index outside palette")?;
            let group = if block.nbt.is_some() {
                2_u8
            } else {
                let full = match cached[block.state] {
                    Some(value) => value,
                    None => {
                        let value = is_static_full(block.state, state)?;
                        cached[block.state] = Some(value);
                        value
                    }
                };
                u8::from(!full)
            };
            keys.push((
                group,
                block.position[1],
                block.position[0],
                block.position[2],
                index,
            ));
        }
        keys.sort_by_key(|&(group, y, x, z, _)| (group, y, x, z));
        Ok(keys.into_iter().map(|(_, _, _, _, index)| index).collect())
    }

    /// Resolve the highest-priority active data-pack template. A missing
    /// resource is distinct from malformed or unsupported template data.
    pub fn load(provider: &mut impl ResourceProvider, id: &ResourceId) -> Result<Option<Self>> {
        let checked = ResourceId::parse(&format!("{}:{}", id.namespace, id.path))?;
        ensure!(&checked == id, "invalid template identifier");
        let path = format!("data/{}/structure/{}.nbt", id.namespace, id.path);
        let Some(bytes) = provider.read_data(&path)? else {
            return Ok(None);
        };
        let document =
            nbt::read_gzip(bytes.as_slice()).with_context(|| format!("invalid template {path}"))?;
        Self::from_document(document)
            .with_context(|| format!("invalid template {path}"))
            .map(Some)
    }

    pub fn from_document(document: NamedTag) -> Result<Self> {
        let root = &document.value;
        ensure!(
            matches!(root, Tag::Compound(_)),
            "template root is not a compound"
        );
        let data_version = match root.get("DataVersion") {
            Some(Tag::Int(value)) => Some(*value),
            None => None,
            _ => bail!("template DataVersion is not an integer"),
        };
        let size = int_vec3(
            root.get("size").context("template size missing")?,
            "template size",
        )?;
        ensure!(size.iter().all(|&v| v >= 0), "negative template size");
        let palettes = match (root.get("palette"), root.get("palettes")) {
            (Some(palette), None) => vec![list(palette, TagType::Compound, "palette")?.to_vec()],
            (None, Some(palettes)) => list(palettes, TagType::List, "palettes")?
                .iter()
                .map(|p| Ok(list(p, TagType::Compound, "palette")?.to_vec()))
                .collect::<Result<Vec<_>>>()?,
            _ => bail!("template must contain exactly one palette form"),
        };
        ensure!(!palettes.is_empty(), "template has no palettes");
        let block_tags = list(
            root.get("blocks").context("template blocks missing")?,
            TagType::Compound,
            "blocks",
        )?;
        let mut blocks = Vec::with_capacity(block_tags.len());
        for block in block_tags {
            let position = int_vec3(
                block
                    .get("pos")
                    .context("template block position missing")?,
                "template block position",
            )?;
            let state = match block.get("state") {
                Some(Tag::Int(value)) => {
                    usize::try_from(*value).context("negative template state index")?
                }
                _ => bail!("template block state index missing"),
            };
            ensure!(
                palettes.iter().all(|palette| state < palette.len()),
                "template state index outside palette"
            );
            let nbt = match block.get("nbt") {
                Some(tag @ Tag::Compound(_)) => Some(tag.clone()),
                None => None,
                _ => bail!("template block entity is not a compound"),
            };
            blocks.push(TemplateBlock {
                position,
                state,
                nbt,
            });
        }
        let entities = list(
            root.get("entities").context("template entities missing")?,
            TagType::Compound,
            "entities",
        )?
        .to_vec();
        Ok(Self {
            document,
            data_version,
            size,
            palettes,
            blocks,
            entities,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::{collections::BTreeMap, io::Write};

    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    fn list_tag(kind: TagType, values: Vec<Tag>) -> Tag {
        Tag::List {
            element_type: kind,
            values,
        }
    }
    fn compound(fields: Vec<(&str, Tag)>) -> Tag {
        Tag::Compound(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }
    fn ints(values: [i32; 3]) -> Tag {
        list_tag(TagType::Int, values.into_iter().map(Tag::Int).collect())
    }
    fn document(state: i32) -> NamedTag {
        let palette = || {
            list_tag(
                TagType::Compound,
                vec![compound(vec![(
                    "Name",
                    Tag::String("minecraft:stone".into()),
                )])],
            )
        };
        NamedTag {
            name: "".into(),
            value: compound(vec![
                ("DataVersion", Tag::Int(5023)),
                ("size", ints([2, 3, 4])),
                (
                    "palettes",
                    list_tag(TagType::List, vec![palette(), palette()]),
                ),
                (
                    "blocks",
                    list_tag(
                        TagType::Compound,
                        vec![compound(vec![
                            ("pos", ints([1, 2, 3])),
                            ("state", Tag::Int(state)),
                            ("nbt", compound(vec![("custom", Tag::Int(42))])),
                        ])],
                    ),
                ),
                (
                    "entities",
                    list_tag(
                        TagType::Compound,
                        vec![compound(vec![("custom", Tag::String("preserved".into()))])],
                    ),
                ),
            ]),
        }
    }
    fn gzip(doc: &NamedTag) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(doc).unwrap()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn preserves_template_data_and_checks_all_palette_indices() {
        let original = document(0);
        let mut data = Data::default();
        data.0.insert(
            "data/example/structure/ruin/test.nbt".into(),
            gzip(&original),
        );
        let id = ResourceId::parse("example:ruin/test").unwrap();
        let template = StructureTemplate::load(&mut data, &id).unwrap().unwrap();
        assert_eq!(template.document, original);
        assert_eq!(template.data_version, Some(5023));
        assert_eq!(template.size, [2, 3, 4]);
        assert_eq!(template.palettes.len(), 2);
        assert_eq!(template.blocks[0].position, [1, 2, 3]);
        assert_eq!(
            template.blocks[0].nbt.as_ref().unwrap().get("custom"),
            Some(&Tag::Int(42))
        );
        assert_eq!(
            template.entities[0].get("custom"),
            Some(&Tag::String("preserved".into()))
        );
        assert!(StructureTemplate::from_document(document(1)).is_err());
        assert!(
            StructureTemplate::load(&mut data, &ResourceId::parse("example:absent").unwrap())
                .unwrap()
                .is_none()
        );
        data.0
            .insert("data/example/structure/ruin/test.nbt".into(), vec![0, 1, 2]);
        assert!(StructureTemplate::load(&mut data, &id).is_err());
    }

    #[test]
    fn block_order_groups_full_nonfull_and_nbt_with_stable_local_coordinates() {
        let mut template = StructureTemplate::from_document(document(0)).unwrap();
        template.palettes[0].push(Tag::Compound(vec![]));
        let block = |position, state, nbt| TemplateBlock {
            position,
            state,
            nbt,
        };
        template.blocks = vec![
            block([3, 0, 2], 1, None),
            block([1, 0, 2], 0, Some(Tag::Compound(vec![]))),
            block([2, 1, 0], 0, None),
            block([1, 0, 1], 0, None),
            block([3, 0, 2], 1, None),
        ];
        let mut calls = Vec::new();
        let order = template
            .java_block_order(0, |index, _| {
                calls.push(index);
                Ok(index == 0)
            })
            .unwrap();
        assert_eq!(order, [3, 2, 0, 4, 1]);
        assert_eq!(calls, [1, 0]);
        assert!(template.java_block_order(2, |_, _| Ok(true)).is_err());
        template.blocks[1].state = 99;
        assert!(template.java_block_order(0, |_, _| Ok(true)).is_err());
    }

    #[test]
    fn jigsaw_records_use_loader_order_defaults_and_priorities() -> Result<()> {
        use native_resources::jigsaw::{Joint, Orientation};
        let mut template = StructureTemplate::from_document(document(0))?;
        template.palettes = vec![vec![compound(vec![
            ("id", Tag::String("minecraft:jigsaw".into())),
            (
                "properties",
                compound(vec![("orientation", Tag::String("up_north".into()))]),
            ),
        ])]];
        template.blocks = vec![
            TemplateBlock {
                position: [3, 2, 1],
                state: 0,
                nbt: Some(compound(vec![
                    ("name", Tag::String("example:door".into())),
                    ("selection_priority", Tag::Int(5)),
                ])),
            },
            TemplateBlock {
                position: [1, 1, 2],
                state: 0,
                nbt: Some(compound(vec![
                    ("joint", Tag::String("invalid".into())),
                    ("target", Tag::String("Invalid".into())),
                    ("placement_priority", Tag::Byte(-3)),
                ])),
            },
        ];
        let found = template.jigsaw_connectors(0)?;
        assert_eq!(
            found.iter().map(|v| v.position).collect::<Vec<_>>(),
            [[1, 1, 2], [3, 2, 1]]
        );
        assert_eq!(found[0].orientation, Orientation::parse("up_north")?);
        assert_eq!(found[0].joint, Joint::Rollable);
        assert_eq!(found[0].target, ResourceId::parse("minecraft:empty")?);
        assert_eq!(found[0].placement_priority, -3);
        assert_eq!(found[1].name, Some(ResourceId::parse("example:door")?));
        assert_eq!(found[1].selection_priority, 5);
        template.blocks[0].nbt = None;
        assert!(template.jigsaw_connectors(0).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn ancient_city_jigsaws_match_java_templates() -> Result<()> {
        use native_resources::jigsaw::{Joint, Orientation};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
        let cp = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home is missing")?
            .join("bin");
        let source = temp.path().join("JigsawTemplateOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawTemplateOracle.java"),
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
        let names = [
            "ancient_city/city_center/city_center_1",
            "ancient_city/city_center/city_center_2",
            "ancient_city/city_center/city_center_3",
            "ancient_city/structures/chamber_1",
        ];
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawTemplateOracle")
            .args(names)
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        use native_resources::structure_transform::Rotation;
        let rotations = [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::Counterclockwise90,
        ];
        let origins = [[0, 0, 0], [-128, -27, 83]];
        let mut actual = Vec::new();
        for name in names {
            let template = StructureTemplate::load(
                &mut resources,
                &ResourceId::parse(&format!("minecraft:{name}"))?,
            )?
            .context("missing template")?;
            ensure!(
                template.palettes.len() == 1,
                "test template has multiple palettes"
            );
            let mut by_rotation = Vec::new();
            for rotation in rotations {
                let mut by_origin = Vec::new();
                for origin in origins {
                    by_origin.push(template.jigsaws_at(origin, rotation)?);
                }
                by_rotation.push(by_origin);
            }
            actual.push(by_rotation);
        }
        let mut seen = vec![vec![vec![0usize; origins.len()]; rotations.len()]; names.len()];
        let mut counts = 0;
        let mut groups = 0;
        let palette_origins = [
            [0, 0, 0],
            [-128, -27, 83],
            [1, 2, 3],
            [-1, -1, -1],
            [30_000_000, 319, -30_000_000],
            [i32::MIN, 0, i32::MAX],
        ];
        let mut multi = StructureTemplate::from_document(document(0))?;
        multi.palettes = ["north_up", "south_up"]
            .into_iter()
            .map(|orientation| {
                vec![compound(vec![
                    ("id", Tag::String("minecraft:jigsaw".into())),
                    (
                        "properties",
                        compound(vec![("orientation", Tag::String(orientation.into()))]),
                    ),
                ])]
            })
            .collect();
        multi.blocks = vec![TemplateBlock {
            position: [0, 0, 0],
            state: 0,
            nbt: Some(compound(vec![])),
        }];
        let mut palettes_seen = 0;
        for line in String::from_utf8(output.stdout)?.lines() {
            let values: Vec<_> = line.split('\t').collect();
            match values.as_slice() {
                ["P", origin, index] => {
                    let origin = palette_origins[origin.parse::<usize>()?];
                    let expected = if *index == "0" {
                        "north_up"
                    } else {
                        "south_up"
                    };
                    let connectors = multi.jigsaws_at(origin, Rotation::None)?;
                    assert_eq!(connectors[0].orientation, Orientation::parse(expected)?);
                    assert_eq!(connectors[0].position, origin);
                    palettes_seen += 1;
                }
                ["N", index, rotation, origin, count] => {
                    let (i, r, o) = (
                        index.parse::<usize>()?,
                        rotation.parse::<usize>()?,
                        origin.parse::<usize>()?,
                    );
                    assert_eq!(actual[i][r][o].len(), count.parse::<usize>()?);
                    groups += 1;
                }
                [
                    "J",
                    index,
                    rotation,
                    origin,
                    x,
                    y,
                    z,
                    orientation,
                    joint,
                    name,
                    target,
                    pool,
                    placement,
                    selection,
                ] => {
                    let index = index.parse::<usize>()?;
                    let rotation = rotation.parse::<usize>()?;
                    let origin = origin.parse::<usize>()?;
                    let block = &actual[index][rotation][origin][seen[index][rotation][origin]];
                    assert_eq!(
                        block.position,
                        [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?]
                    );
                    assert_eq!(block.orientation, Orientation::parse(orientation)?);
                    assert_eq!(
                        block.joint,
                        if *joint == "aligned" {
                            Joint::Aligned
                        } else {
                            Joint::Rollable
                        }
                    );
                    assert_eq!(block.name, Some(ResourceId::parse(name)?));
                    assert_eq!(block.target, ResourceId::parse(target)?);
                    assert_eq!(block.pool, ResourceId::parse(pool)?);
                    assert_eq!(block.placement_priority, placement.parse::<i32>()?);
                    assert_eq!(block.selection_priority, selection.parse::<i32>()?);
                    seen[index][rotation][origin] += 1;
                    counts += 1;
                }
                _ => {}
            }
        }
        assert!(counts > 0);
        assert_eq!(palettes_seen, palette_origins.len());
        assert_eq!(groups, names.len() * rotations.len() * origins.len());
        for i in 0..names.len() {
            for r in 0..rotations.len() {
                for o in 0..origins.len() {
                    assert_eq!(seen[i][r][o], actual[i][r][o].len());
                }
            }
        }
        Ok(())
    }
}
