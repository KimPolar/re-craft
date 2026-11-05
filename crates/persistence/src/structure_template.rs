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
}
