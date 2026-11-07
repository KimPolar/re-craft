//! Prepare shipwreck starts using templates from the active data-pack stack.
//! Actual placement, structure saving and integration remain separate stages.
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use native_core::beardifier::{BoundingBox, TerrainAdjustment};
use native_persistence::structure_template::StructureTemplate;
use native_resources::{
    packs::{ResourceId, ResourceProvider},
    shipwreck::{self, Piece},
    structure_start::{
        Heightmap, StartEnvironment, center_generation_point, decode_terrain_adjustment,
    },
};
use serde_json::Value;

#[derive(Clone)]
pub struct PreparedStart {
    pub chunk: [i32; 2],
    pub generation_point: [i32; 3],
    pub terrain_adjustment: TerrainAdjustment,
    pub piece: Piece,
    pub template: Arc<StructureTemplate>,
    /// Java retains the constructor box until the piece is postprocessed.
    pub bounds: BoundingBox,
    pub height_adjusted: bool,
}

impl PreparedStart {
    pub fn generate(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        environment: &mut impl StartEnvironment,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<Option<Self>> {
        let beached = shipwreck::decode_beached(definition)?;
        let terrain_adjustment = decode_terrain_adjustment(definition)?;
        let map = if beached {
            Heightmap::WorldSurface
        } else {
            Heightmap::OceanFloor
        };
        let Some(generation_point) = center_generation_point(environment, chunk, map)? else {
            return Ok(None);
        };
        let (mut piece, mut random) = shipwreck::select_with_chunk_random(seed, chunk, beached)?;
        let id = ResourceId::parse(&piece.template)?;
        let template = StructureTemplate::load(provider, &id)?
            .with_context(|| format!("missing shipwreck template {}", piece.template))?;
        ensure!(
            template.data_version == Some(5023),
            "shipwreck template {} needs data-version conversion",
            piece.template
        );
        let (bounds, height_adjusted) =
            shipwreck::align_during_start(&mut piece, template.size, environment, &mut random)?;
        Ok(Some(Self {
            chunk,
            generation_point,
            terrain_adjustment,
            piece,
            template: Arc::new(template),
            bounds,
            height_adjusted,
        }))
    }

    pub fn reference_bounds(
        &self,
        structure: ResourceId,
    ) -> Result<super::structure_references::StructureStartBounds> {
        super::structure_references::StructureStartBounds::from_pieces(
            structure,
            &[self.bounds],
            self.terrain_adjustment,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use native_persistence::nbt::{self, NamedTag, Tag, TagType};
    use native_resources::structure_start::BiomeQuery;
    use serde_json::json;
    use std::io::Write;

    struct Data {
        bytes: Option<Vec<u8>>,
        reads: usize,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            self.reads += 1;
            Ok(self.bytes.clone())
        }
    }
    fn template(version: i32, size: [i32; 3]) -> Vec<u8> {
        let list = |element_type, values| Tag::List {
            element_type,
            values,
        };
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(version)),
                (
                    "size".into(),
                    list(TagType::Int, size.into_iter().map(Tag::Int).collect()),
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
        encoder.write_all(&nbt::write(&document).unwrap()).unwrap();
        encoder.finish().unwrap()
    }
    struct Environment {
        allowed: bool,
        heights: usize,
    }
    impl StartEnvironment for Environment {
        fn vertical_bounds(&self) -> (i32, i32) {
            (-64, 319)
        }
        fn sea_level(&self) -> i32 {
            63
        }
        fn first_occupied_height(&mut self, _: i32, _: i32, _: Heightmap) -> Result<i32> {
            self.heights += 1;
            Ok(40)
        }
        fn biome_allowed(&mut self, _: [i32; 3], _: BiomeQuery) -> Result<bool> {
            Ok(self.allowed)
        }
    }

    #[test]
    fn prepares_only_valid_active_templates_after_biome_check() -> Result<()> {
        let definition = json!({"type":"minecraft:shipwreck", "is_beached":true});
        let mut data = Data {
            bytes: Some(template(5023, [10, 8, 20])),
            reads: 0,
        };
        let mut env = Environment {
            allowed: false,
            heights: 0,
        };
        assert!(PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2])?.is_none());
        assert_eq!(data.reads, 0);
        env.allowed = true;
        let small =
            PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2])?.unwrap();
        assert_eq!(small.generation_point, [-40, 40, 40]);
        assert!(!small.height_adjusted);
        assert_eq!(small.piece.position[1], 90);
        assert_eq!(
            small
                .reference_bounds(ResourceId::parse("minecraft:shipwreck")?)?
                .bounds,
            small.bounds
        );
        data.bytes = Some(template(5023, [33, 8, 20]));
        let large =
            PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2])?.unwrap();
        assert!(large.height_adjusted);
        assert!((34..=36).contains(&large.piece.position[1]));
        assert_eq!(large.bounds.min[1], 90);
        data.bytes = Some(template(4000, [1, 1, 1]));
        assert!(PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2]).is_err());
        data.bytes = None;
        assert!(PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2]).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft 26.3"]
    fn prepares_all_installed_shipwreck_templates() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let mut seen = std::collections::BTreeSet::new();
        for beached in [false, true] {
            let definition = json!({"type":"minecraft:shipwreck", "is_beached":beached});
            for seed in 0..256 {
                let mut env = Environment {
                    allowed: true,
                    heights: 0,
                };
                let start =
                    PreparedStart::generate(&mut data, &definition, &mut env, seed, [-3, 2])?
                        .unwrap();
                assert!(start.template.size.iter().all(|&n| n > 0));
                seen.insert(start.piece.template);
            }
        }
        assert_eq!(seen.len(), 20);
        Ok(())
    }
}
