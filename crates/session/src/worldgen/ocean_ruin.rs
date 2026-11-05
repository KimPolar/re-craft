//! Prepare ocean-ruin template selections without placing blocks or saving.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use native_persistence::structure_template::StructureTemplate;
use native_resources::{
    ocean_ruin::{self, Piece, Settings},
    packs::{ResourceId, ResourceProvider},
};
use serde_json::Value;

pub struct PreparedPiece {
    pub selection: Piece,
    pub template: Arc<StructureTemplate>,
    /// Initial Java template bounds, before ocean-floor postprocessing.
    pub bounds: native_core::beardifier::BoundingBox,
}

impl PreparedPiece {
    pub fn block_position(
        &self,
        block: &native_persistence::structure_template::TemplateBlock,
    ) -> [i32; 3] {
        native_resources::structure_transform::block_world_position(
            block.position,
            self.selection.position,
            native_resources::structure_transform::Mirror::None,
            self.selection.rotation,
            [0; 3],
        )
    }
}

/// Read the exact selected templates from the same active data-pack stack.
/// Data fixing, processors, entity handling and actual placement remain open.
pub fn prepare_pieces(
    provider: &mut impl ResourceProvider,
    definition: &Value,
    seed: i64,
    chunk: [i32; 2],
) -> Result<Vec<PreparedPiece>> {
    let settings = Settings::decode(definition)?;
    let selected = ocean_ruin::select_for_chunk(seed, chunk, settings)?;
    let mut templates = BTreeMap::<String, Arc<StructureTemplate>>::new();
    let mut result = Vec::with_capacity(selected.len());
    for selection in selected {
        let template = if let Some(template) = templates.get(&selection.template) {
            Arc::clone(template)
        } else {
            let id = ResourceId::parse(&selection.template)?;
            let template = StructureTemplate::load(provider, &id)?
                .with_context(|| format!("missing ocean ruin template {}", selection.template))?;
            ensure!(
                template.data_version == Some(5023),
                "ocean ruin template {} needs data-version conversion",
                selection.template
            );
            let template = Arc::new(template);
            templates.insert(selection.template.clone(), Arc::clone(&template));
            template
        };
        let bounds = native_resources::structure_transform::bounding_box(
            template.size,
            selection.position,
            native_resources::structure_transform::Mirror::None,
            selection.rotation,
            [0; 3],
        )?;
        result.push(PreparedPiece {
            selection,
            template,
            bounds,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use native_persistence::nbt::{self, NamedTag, Tag, TagType};
    use serde_json::json;
    use std::io::Write;

    struct Data {
        bytes: Option<Vec<u8>>,
        reads: BTreeMap<String, usize>,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            *self.reads.entry(path.to_owned()).or_default() += 1;
            Ok(self.bytes.clone())
        }
    }
    fn template(version: i32) -> Vec<u8> {
        let list = |kind, values| Tag::List {
            element_type: kind,
            values,
        };
        let doc = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(version)),
                (
                    "size".into(),
                    list(TagType::Int, vec![Tag::Int(1), Tag::Int(1), Tag::Int(1)]),
                ),
                (
                    "palette".into(),
                    list(TagType::Compound, vec![Tag::Compound(vec![])]),
                ),
                ("blocks".into(), list(TagType::Compound, vec![])),
                ("entities".into(), list(TagType::Compound, vec![])),
            ]),
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&doc).unwrap()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn prepares_active_templates_once_and_rejects_missing_or_old_data() {
        let definition = json!({"type":"minecraft:ocean_ruin","biome_temp":"cold","large_probability":1.0,"cluster_probability":1.0});
        let mut data = Data {
            bytes: Some(template(5023)),
            reads: BTreeMap::new(),
        };
        let prepared = prepare_pieces(&mut data, &definition, 42, [0, 0]).unwrap();
        assert!(!prepared.is_empty());
        assert!(data.reads.values().all(|&count| count == 1));
        for (index, piece) in prepared.iter().enumerate() {
            for other in &prepared[..index] {
                if other.selection.template == piece.selection.template {
                    assert!(Arc::ptr_eq(&other.template, &piece.template));
                }
            }
        }
        data.bytes = Some(template(4000));
        assert!(
            format!(
                "{:#}",
                prepare_pieces(&mut data, &definition, 42, [0, 0])
                    .err()
                    .unwrap()
            )
            .contains("data-version conversion")
        );
        data.bytes = None;
        assert!(
            format!(
                "{:#}",
                prepare_pieces(&mut data, &definition, 42, [0, 0])
                    .err()
                    .unwrap()
            )
            .contains("missing ocean ruin template")
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn prepares_selected_vanilla_ocean_ruin_templates() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        for temperature in ["warm", "cold"] {
            let definition = json!({"type":"minecraft:ocean_ruin","biome_temp":temperature,"large_probability":0.5,"cluster_probability":0.9});
            for seed in 0..16 {
                let pieces = prepare_pieces(&mut data, &definition, seed, [-3, 2])?;
                assert!(!pieces.is_empty());
                assert!(pieces.iter().all(|piece| !piece.template.blocks.is_empty()));
                for piece in &pieces {
                    for block in &piece.template.blocks {
                        let pos = piece.block_position(block);
                        assert!((0..3).all(|axis| {
                            (piece.bounds.min[axis]..=piece.bounds.max[axis]).contains(&pos[axis])
                        }));
                    }
                }
            }
        }
        Ok(())
    }
}
