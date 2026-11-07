//! Prepare the block records of a jigsaw template before world placement.
//!
//! The returned states are deliberately unrotated: Java applies processors to
//! the original state and only rotates it when the block is written.
use anyhow::{Context, Result, ensure};
use native_core::{beardifier::BoundingBox, random::LegacyRandom};
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    blockstates::positional_seed,
    structure_transform::{Mirror, Rotation, block_world_position},
};
use native_world::loaded::BlockState;
use std::collections::BTreeMap;

use crate::{
    jigsaw_replacement::PlacementBlock, nbt::Tag, structure_processors::PreparedProcessors,
    structure_template::StructureTemplate,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessedTemplateBlock {
    /// Index in the source template; marker and finalization passes need it.
    pub source_index: usize,
    pub block: PlacementBlock,
}

/// Placement settings used by a single jigsaw pool element. All currently
/// supported processors are per-block and permit Java's early clip check.
#[derive(Clone, Copy, Debug)]
pub struct TemplateSettings {
    pub origin: [i32; 3],
    pub rotation: Rotation,
    pub clip: BoundingBox,
    pub legacy: bool,
    pub keep_jigsaws: bool,
}

/// Settings for a non-jigsaw template with a pivot and optional mirror.
#[derive(Clone, Copy, Debug)]
pub struct StructureTemplateSettings {
    pub origin: [i32; 3],
    pub mirror: Mirror,
    pub rotation: Rotation,
    pub pivot: [i32; 3],
    pub clip: BoundingBox,
}

/// Process an ordinary template without the jigsaw-specific processors.
/// States are still untransformed, as in Java's processBlockInfos result.
pub fn process_structure_template(
    template: &StructureTemplate,
    processors: &PreparedProcessors,
    settings: StructureTemplateSettings,
    mut resolve_state: impl FnMut(&Tag) -> Result<BlockState>,
    mut is_static_full: impl FnMut(&BlockState) -> Result<bool>,
    world: &mut impl FnMut([i32; 3]) -> Result<BlockState>,
) -> Result<Vec<ProcessedTemplateBlock>> {
    ensure!(
        template.data_version == Some(5023),
        "template needs unsupported data-version conversion"
    );
    ensure!(
        (0..3).all(|i| settings.clip.min[i] <= settings.clip.max[i]),
        "invalid template clipping box"
    );
    let count = i32::try_from(template.palettes.len()).context("too many template palettes")?;
    let [x, y, z] = settings.origin;
    let index = LegacyRandom::new(positional_seed(x, y, z))
        .next_int_bound(count)
        .context("template has no palettes")? as usize;
    let palette = template.palettes[index]
        .iter()
        .map(&mut resolve_state)
        .collect::<Result<Vec<_>>>()?;
    let order = template.java_block_order(index, |index, _| is_static_full(&palette[index]))?;
    let mut result = Vec::with_capacity(order.len());
    for source_index in order {
        let source = &template.blocks[source_index];
        let position = block_world_position(
            source.position,
            settings.origin,
            settings.mirror,
            settings.rotation,
            settings.pivot,
        );
        if !(0..3).all(|axis| {
            (settings.clip.min[axis]..=settings.clip.max[axis]).contains(&position[axis])
        }) {
            continue;
        }
        let current = PlacementBlock {
            position,
            state: palette[source.state].clone(),
            nbt: source.nbt.clone(),
        };
        if let Some(block) = processors.process(current, world)? {
            result.push(ProcessedTemplateBlock {
                source_index,
                block,
            });
        }
    }
    Ok(result)
}

/// Decode a template palette entry with the available verified block schemas.
/// Structure and jigsaw blocks are kept for their processors and marker pass.
pub fn resolve_template_state(tag: &Tag, registry: &GeneratedBlockRegistry) -> Result<BlockState> {
    let name = match tag.get("id") {
        Some(Tag::String(name)) => {
            String::from_utf16(&name.0).context("invalid template block identifier")?
        }
        _ => anyhow::bail!("template block identifier missing"),
    };
    let mut properties = BTreeMap::new();
    if let Some(value) = tag.get("properties") {
        let Tag::Compound(entries) = value else {
            anyhow::bail!("template properties are not a compound")
        };
        for (key, value) in entries {
            let key = String::from_utf16(&key.0).context("invalid template property name")?;
            let Tag::String(value) = value else {
                anyhow::bail!("template property is not a string")
            };
            let value = String::from_utf16(&value.0).context("invalid template property value")?;
            ensure!(
                properties.insert(key, value).is_none(),
                "duplicate template property"
            );
        }
    }
    let id = native_resources::packs::ResourceId::parse(&name)?;
    let name = format!("{}:{}", id.namespace, id.path);
    if name == "minecraft:jigsaw" {
        let orientation = properties
            .get("orientation")
            .map_or("north_up", String::as_str);
        native_resources::jigsaw::Orientation::parse(orientation)?;
        ensure!(
            properties.keys().all(|key| key == "orientation"),
            "unsupported jigsaw property"
        );
        properties.insert("orientation".into(), orientation.to_owned());
        return Ok(BlockState { name, properties });
    }
    if name == "minecraft:structure_block" {
        let mode = properties.get("mode").map_or("load", String::as_str);
        ensure!(
            matches!(mode, "save" | "load" | "corner" | "data"),
            "invalid structure block mode"
        );
        ensure!(
            properties.keys().all(|key| key == "mode"),
            "unsupported structure block property"
        );
        properties.insert("mode".into(), mode.to_owned());
        return Ok(BlockState { name, properties });
    }
    let (name, properties) =
        registry.resolve(&serde_json::json!({"id": name, "properties": properties}))?;
    Ok(BlockState { name, properties })
}

/// Process a selected palette in Java's template load order, using an isolated
/// world view. Palette decoding and collision classification must use the
/// verified active block definitions. No block or entity is written here.
pub fn process_template(
    template: &StructureTemplate,
    registry: &GeneratedBlockRegistry,
    processors: &PreparedProcessors,
    settings: TemplateSettings,
    mut resolve_state: impl FnMut(&Tag) -> Result<BlockState>,
    mut is_static_full: impl FnMut(&BlockState) -> Result<bool>,
    world: &mut impl FnMut([i32; 3]) -> Result<BlockState>,
    mut projection: impl FnMut(&PlacementBlock, PlacementBlock) -> Result<Option<PlacementBlock>>,
) -> Result<Vec<ProcessedTemplateBlock>> {
    ensure!(
        template.data_version == Some(5023),
        "template needs unsupported data-version conversion"
    );
    ensure!(
        (0..3).all(|i| settings.clip.min[i] <= settings.clip.max[i]),
        "invalid template clipping box"
    );
    let count = i32::try_from(template.palettes.len()).context("too many template palettes")?;
    ensure!(count > 0, "template has no palettes");
    let [x, y, z] = settings.origin;
    let palette_index = LegacyRandom::new(positional_seed(x, y, z))
        .next_int_bound(count)
        .context("invalid template palette count")? as usize;
    let palette = template.palettes[palette_index]
        .iter()
        .map(&mut resolve_state)
        .collect::<Result<Vec<_>>>()?;
    let order =
        template.java_block_order(palette_index, |index, _| is_static_full(&palette[index]))?;
    let mut result = Vec::with_capacity(order.len());
    for source_index in order {
        let source = &template.blocks[source_index];
        let position = block_world_position(
            source.position,
            settings.origin,
            Mirror::None,
            settings.rotation,
            [0; 3],
        );
        if !(0..3).all(|axis| {
            (settings.clip.min[axis]..=settings.clip.max[axis]).contains(&position[axis])
        }) {
            continue;
        }
        let original = PlacementBlock {
            position: source.position,
            state: palette[source.state].clone(),
            nbt: source.nbt.clone(),
        };
        let current = PlacementBlock {
            position,
            ..original.clone()
        };
        let block = processors.process_pool_block(
            registry,
            &original,
            current,
            settings.legacy,
            settings.keep_jigsaws,
            world,
            &mut projection,
        )?;
        if let Some(block) = block {
            result.push(ProcessedTemplateBlock {
                source_index,
                block,
            });
        }
    }
    Ok(result)
}
