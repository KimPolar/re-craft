use std::io::Write;

use super::{
    PreparedDecoration,
    tests::{fixture, fixture_plan, scheduled_fixture},
};
use anyhow::{Context, Result};
use native_persistence::nbt::{self, NamedTag, Tag, TagType};
use native_resources::{
    feature_order::FeatureSchedule,
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::json;

struct TemplateData {
    vanilla: ResourceStack,
    template: Vec<u8>,
}

impl ResourceProvider for TemplateData {
    fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        self.vanilla.read_asset(path)
    }

    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        match path {
            "data/test/structure/template.nbt" => Ok(Some(self.template.clone())),
            "data/minecraft/worldgen/biome/plains.json" => Ok(Some(serde_json::to_vec(&json!({
                "features": [[{
                    "feature": {
                        "type": "minecraft:sequence",
                        "features": [{
                            "feature": {
                                "type": "minecraft:template",
                                "templates": [{ "weight": 1, "data": { "id": "test:template" } }]
                            },
                            "placement": []
                        }]
                    },
                    "placement": [{ "type": "minecraft:offset", "x": 8, "y": 8, "z": 8 }]
                }]]
            }))?)),
            _ => self.vanilla.read_data(path),
        }
    }

    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        self.vanilla.list_data(folder)
    }
}

fn template(block: &str) -> Result<Vec<u8>> {
    let list = |element_type, values| Tag::List {
        element_type,
        values,
    };
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            (
                "size".into(),
                list(TagType::Int, vec![Tag::Int(1), Tag::Int(1), Tag::Int(1)]),
            ),
            (
                "palette".into(),
                list(
                    TagType::Compound,
                    vec![Tag::Compound(vec![(
                        "id".into(),
                        Tag::String(block.into()),
                    )])],
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
    Ok(encoder.finish()?)
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3"]
fn saved_decoration_uses_registry_for_nested_template_features() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| native_resources::default_minecraft_dir().ok())
        .context("Minecraft directory missing")?;
    let mut vanilla = ResourceStack::open(&game)?;
    let registry = vanilla.vanilla_generated_block_registry()?;
    let blocks = vanilla.vanilla_block_ids()?;
    let mut data = TemplateData {
        vanilla,
        template: template("minecraft:stone")?,
    };
    let plains = ResourceId::parse("minecraft:plains")?;
    let schedule = FeatureSchedule::load(&mut data, &[plains], &Default::default())?;
    let (_, _, neighborhood) = scheduled_fixture(None);
    let plan = fixture_plan(&schedule, &neighborhood);
    let mut resolve =
        |value: &serde_json::Value| super::super::resolve_generated_terrain_state(value, &registry);
    let missing =
        PreparedDecoration::load_with_mineshaft(&mut data, &plan, &blocks, &mut resolve, false);
    assert!(
        format!(
            "{:#}",
            missing.err().context("missing registry was accepted")?
        )
        .contains("template feature registry missing")
    );

    let prepared = PreparedDecoration::load_with_registry(
        &mut data,
        &plan,
        &blocks,
        &registry,
        &mut resolve,
        false,
    )?;
    let (mut terrain, _, _, _) = fixture();
    assert_eq!(
        terrain.snapshot().block(8, 8, 8).unwrap().name,
        "minecraft:air"
    );
    assert_eq!(prepared.execute(&mut terrain, false)?, [true]);
    assert_eq!(
        terrain.snapshot().block(8, 8, 8).unwrap().name,
        "minecraft:stone"
    );
    Ok(())
}
