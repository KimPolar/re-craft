//! Preparation and isolated placement for ordinary template features.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use native_core::{beardifier::BoundingBox, random::DecorationRandom};
use native_persistence::{
    jigsaw_template::{
        StructureTemplateSettings, process_structure_template_with_palette, resolve_template_state,
    },
    structure_processors::PreparedProcessors,
    structure_template::StructureTemplate,
};
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    packs::{ResourceId, ResourceProvider},
    structure_transform::Mirror,
    template_feature::TemplateFeature,
};
use native_world::loaded::BlockState;
use serde_json::Value;

use super::jigsaw::{JigsawPlacementWorld, StructurePlacementWorld, place_structure_records};

/// Resolved against one unchanged data-pack stack. A failed placement must be
/// discarded together with the caller's unpublished world draft.
pub struct PreparedTemplateFeature {
    feature: TemplateFeature,
    templates: BTreeMap<ResourceId, StructureTemplate>,
    processors: PreparedProcessors,
}

impl PreparedTemplateFeature {
    /// Every selectable template is resolved from the active packs and
    /// contains no entity records. Preparation also rejects processors.
    pub(super) fn is_entity_free(&self) -> bool {
        self.templates
            .values()
            .all(|template| template.entities.is_empty())
    }

    pub fn load(
        value: &Value,
        provider: &mut impl ResourceProvider,
        _blocks: &BTreeSet<String>,
        _registry: &GeneratedBlockRegistry,
    ) -> Result<Self> {
        Self::load_unprocessed(value, provider)
    }

    /// Load templates that have no processor list; processor RNG semantics
    /// require a separate implementation before other configurations can run.
    pub fn load_unprocessed(value: &Value, provider: &mut impl ResourceProvider) -> Result<Self> {
        let feature = TemplateFeature::decode(value)?;
        // TemplateFeature supplies one shared RNG to its processors. The
        // existing structure-piece processors use positional RNG instead.
        ensure!(
            feature.processors().is_none(),
            "template feature processors are not yet supported"
        );
        let processors = PreparedProcessors::empty();
        let mut templates = BTreeMap::new();
        for entry in feature.entries() {
            if !templates.contains_key(&entry.id) {
                let template = StructureTemplate::load(provider, &entry.id)?
                    .context("template feature resource missing")?;
                ensure!(
                    template.data_version == Some(5023),
                    "template feature requires data-version conversion"
                );
                templates.insert(entry.id.clone(), template);
            }
        }
        Ok(Self {
            feature,
            templates,
            processors,
        })
    }

    /// Place into a caller-owned draft. Unsupported entities or block effects
    /// return an error and leave the caller's RNG unchanged.
    pub fn place(
        &self,
        world: &mut impl StructurePlacementWorld,
        registry: &GeneratedBlockRegistry,
        random: &mut DecorationRandom,
        position: [i32; 3],
        is_static_full: impl FnMut(&BlockState) -> Result<bool>,
    ) -> Result<bool> {
        let mut next = random.clone();
        let (entry, rotation) = self.feature.select(&mut next)?;
        let template = self
            .templates
            .get(&entry.id)
            .context("prepared template missing")?;
        if template.palettes.is_empty()
            || template.size.iter().any(|&v| v < 1)
            || (template.blocks.is_empty() && template.entities.is_empty())
        {
            *random = next;
            return Ok(false);
        }
        ensure!(
            template.entities.is_empty(),
            "template feature entities are not yet supported"
        );
        let origin = TemplateFeature::origin(position, template.size, rotation);
        let count = i32::try_from(template.palettes.len()).context("too many template palettes")?;
        let palette = next
            .next_int_bound(count)
            .context("template has no palettes")? as usize;
        let clip = BoundingBox::new([i32::MIN; 3], [i32::MAX; 3]).map_err(anyhow::Error::msg)?;
        let shared = std::cell::RefCell::new(world);
        let records = process_structure_template_with_palette(
            template,
            &self.processors,
            StructureTemplateSettings {
                origin,
                mirror: Mirror::None,
                rotation,
                pivot: [0; 3],
                clip,
            },
            palette,
            |tag| resolve_template_state(tag, registry),
            is_static_full,
            &mut |position| JigsawPlacementWorld::block(&mut **shared.borrow_mut(), position),
        )?;
        ensure!(
            records.iter().all(|record| !matches!(
                record.block.state.name.as_str(),
                "minecraft:jigsaw" | "minecraft:structure_block"
            )),
            "template feature structure block entities are not yet supported"
        );
        let world = shared.into_inner();
        place_structure_records(
            &records,
            world,
            registry,
            clip,
            Mirror::None,
            rotation,
            &mut next,
        )?;
        *random = next;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::{self, NamedTag, Tag, TagType};
    use native_resources::{structure_transform::Rotation, tree_leaf_update::ShapeEdgeWorld};
    use native_world::fluid::FluidState;
    use std::io::Write;

    struct Data(Vec<u8>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok((path == "data/test/structure/fixture.nbt").then(|| self.0.clone()))
        }
    }
    #[derive(Default)]
    struct Draft {
        blocks: BTreeMap<[i32; 3], BlockState>,
        fail: bool,
    }
    fn air() -> BlockState {
        BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        }
    }
    impl JigsawPlacementWorld for Draft {
        fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
            Ok(self.blocks.get(&pos).cloned().unwrap_or_else(air))
        }
        fn fluid_at(&mut self, _: [i32; 3]) -> Result<Option<FluidState>> {
            Ok(None)
        }
        fn fluid_of_state(&mut self, _: &BlockState) -> Result<Option<FluidState>> {
            Ok(None)
        }
        fn place_template_block(
            &mut self,
            _: [i32; 3],
            _: &BlockState,
            _: Option<&Tag>,
            _: &mut DecorationRandom,
        ) -> Result<bool> {
            anyhow::bail!("unexpected jigsaw write")
        }
        fn is_liquid_container(&mut self, _: &BlockState) -> Result<bool> {
            Ok(false)
        }
        fn place_liquid(
            &mut self,
            _: [i32; 3],
            _: &BlockState,
            _: Option<FluidState>,
        ) -> Result<()> {
            Ok(())
        }
        fn mark_block_entity_changed(&mut self, _: [i32; 3]) -> Result<()> {
            Ok(())
        }
        fn place_template_entities(
            &mut self,
            _: &[Tag],
            _: [i32; 3],
            _: Rotation,
            _: BoundingBox,
        ) -> Result<()> {
            anyhow::bail!("unexpected entity")
        }
    }
    impl ShapeEdgeWorld for Draft {
        type State = BlockState;
        fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
            JigsawPlacementWorld::block(self, pos)
        }
        fn update_shape(
            &mut self,
            _: [i32; 3],
            state: &BlockState,
            _: [i32; 3],
            _: [i32; 3],
            _: &BlockState,
        ) -> Result<BlockState> {
            Ok(state.clone())
        }
        fn set_block(&mut self, pos: [i32; 3], state: &BlockState, _: i32) -> Result<()> {
            self.blocks.insert(pos, state.clone());
            Ok(())
        }
    }
    impl StructurePlacementWorld for Draft {
        fn place_structure_block(
            &mut self,
            pos: [i32; 3],
            state: &BlockState,
            _: Option<&Tag>,
            _: &mut DecorationRandom,
        ) -> Result<bool> {
            ensure!(!self.fail, "injected write failure");
            self.blocks.insert(pos, state.clone());
            Ok(true)
        }
        fn update_from_neighbor_shapes(
            &mut self,
            _: [i32; 3],
            state: &BlockState,
        ) -> Result<BlockState> {
            Ok(state.clone())
        }
        fn update_neighbors_at(&mut self, _: [i32; 3], _: &BlockState) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn template_feature_uses_shared_palette_random_and_rolls_back_on_error() -> Result<()> {
        let list = |element_type, values| Tag::List {
            element_type,
            values,
        };
        let palette = |name: &str| {
            list(
                TagType::Compound,
                vec![Tag::Compound(vec![("id".into(), Tag::String(name.into()))])],
            )
        };
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "size".into(),
                    list(TagType::Int, vec![Tag::Int(3), Tag::Int(3), Tag::Int(3)]),
                ),
                (
                    "palettes".into(),
                    list(
                        TagType::List,
                        vec![palette("minecraft:stone"), palette("minecraft:tuff")],
                    ),
                ),
                (
                    "blocks".into(),
                    list(
                        TagType::Compound,
                        vec![Tag::Compound(vec![
                            (
                                "pos".into(),
                                list(TagType::Int, vec![Tag::Int(0), Tag::Int(0), Tag::Int(0)]),
                            ),
                            ("state".into(), Tag::Int(0)),
                        ])],
                    ),
                ),
                ("entities".into(), list(TagType::Compound, vec![])),
            ]),
        };
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&nbt::write(&document)?)?;
        let mut data = Data(encoder.finish()?);
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let blocks = registry.ids().map(str::to_owned).collect();
        let config = serde_json::json!({"type":"minecraft:template","templates":[{"weight":1,"data":{"id":"test:fixture"}}]});
        let feature = PreparedTemplateFeature::load(&config, &mut data, &blocks, &registry)?;
        for seed in 0..64 {
            let mut random = DecorationRandom::new(seed);
            let mut expected = random.clone();
            let (_, rotation) = feature.feature.select(&mut expected)?;
            let palette = expected.next_int_bound(2).unwrap();
            let origin = TemplateFeature::origin([8, 8, 8], [3, 3, 3], rotation);
            let mut draft = Draft::default();
            assert!(feature.place(&mut draft, &registry, &mut random, [8, 8, 8], |_| Ok(true))?);
            assert_eq!(
                draft.blocks[&origin].name,
                if palette == 0 {
                    "minecraft:stone"
                } else {
                    "minecraft:tuff"
                }
            );
            assert_eq!(random.next_long(), expected.next_long());
        }
        let mut random = DecorationRandom::new(0);
        let mut expected = random.clone();
        assert!(
            feature
                .place(
                    &mut Draft {
                        fail: true,
                        ..Default::default()
                    },
                    &registry,
                    &mut random,
                    [8, 8, 8],
                    |_| Ok(true)
                )
                .is_err()
        );
        assert_eq!(random.next_long(), expected.next_long());
        Ok(())
    }
}
