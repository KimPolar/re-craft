//! Access the current decoration region for ocean-ruin floor adjustment.
use super::*;
use native_resources::ocean_ruin::FloorWorld;

struct RuinFloorWorld<'r, 'a, 't> {
    region: &'r mut DecorationRegion<'a>,
    ice_tags: &'t OreBlockTags,
    water_fluids: &'t BTreeSet<String>,
}

impl DecorationRegion<'_> {
    /// Adjust a piece against this unpublished region using active data-pack
    /// ice and water tags. A failed read does not alter the piece origin.
    pub fn align_ocean_ruin(
        &mut self,
        piece: &mut super::super::ocean_ruin::PreparedPiece,
        ice_tags: &OreBlockTags,
        water_fluids: &BTreeSet<String>,
    ) -> Result<()> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut world = RuinFloorWorld {
            region: self,
            ice_tags,
            water_fluids,
        };
        piece.align_to_floor(&mut world)
    }
}

impl FloorWorld for RuinFloorWorld<'_, '_, '_> {
    fn min_y(&self) -> i32 {
        self.region.target.min_y
    }

    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        OreVeinWorld::ocean_floor_height(self.region, x, z)
    }

    fn floor_scan_passable(&mut self, position: [i32; 3]) -> Result<bool> {
        let state = OreWorld::block(self.region, position)?;
        let fluid = match native_world::fluid::FluidState::from_block(&state)
            .map_err(anyhow::Error::msg)?
        {
            None => "minecraft:empty",
            Some(value) => match (value.kind, value.source) {
                (native_world::fluid::FluidKind::Water, true) => "minecraft:water",
                (native_world::fluid::FluidKind::Water, false) => "minecraft:flowing_water",
                (native_world::fluid::FluidKind::Lava, true) => "minecraft:lava",
                (native_world::fluid::FluidKind::Lava, false) => "minecraft:flowing_lava",
            },
        };
        if is_air(&state) || self.water_fluids.contains(fluid) {
            return Ok(true);
        }
        static ICE_TAG: OnceLock<ResourceId> = OnceLock::new();
        self.ice_tags.contains(
            ICE_TAG.get_or_init(|| ResourceId::parse("minecraft:ice").unwrap()),
            &state.name,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::beardifier::BoundingBox;
    use native_persistence::{
        nbt::{NamedTag, Tag},
        structure_template::StructureTemplate,
    };
    use native_resources::{
        ocean_ruin::{Piece, Rotation},
        packs::ResourceProvider,
    };

    struct IceTag(bool);
    impl ResourceProvider for IceTag {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            if path != "data/minecraft/tags/block/ice.json" {
                return Ok(None);
            }
            Ok(Some(if self.0 {
                br#"{"values":["minecraft:stone"]}"#.to_vec()
            } else {
                br#"{"values":[]}"#.to_vec()
            }))
        }
    }

    #[test]
    fn region_uses_active_ice_tag_and_keeps_piece_on_failure() {
        let registry = BTreeSet::from(["minecraft:air".to_string(), "minecraft:stone".to_string()]);
        let id = ResourceId::parse("minecraft:ice").unwrap();
        let tags = |include| {
            OreBlockTags::load(
                &mut IceTag(include),
                &registry,
                &BTreeSet::from([id.clone()]),
            )
            .unwrap()
        };
        let empty_ice = tags(false);
        let custom_ice = tags(true);
        let missing = OreBlockTags::load(&mut IceTag(false), &registry, &BTreeSet::new()).unwrap();
        let (mut terrain, region_tags, motion, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain
            .decoration_region(&region_tags, &motion, writable)
            .unwrap();
        let mut piece = super::super::super::ocean_ruin::PreparedPiece {
            selection: Piece {
                template: "minecraft:test".into(),
                position: [8, 90, 8],
                rotation: Rotation::None,
                integrity: 1.0,
                large: false,
            },
            template: Arc::new(StructureTemplate {
                document: NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                },
                data_version: Some(5023),
                size: [1, 1, 1],
                palettes: vec![],
                blocks: vec![],
                entities: vec![],
            }),
            bounds: BoundingBox::new([8, 90, 8], [8, 90, 8]).unwrap(),
        };
        let water = BTreeSet::from([
            "minecraft:water".to_string(),
            "minecraft:flowing_water".to_string(),
        ]);
        assert!(
            region
                .align_ocean_ruin(&mut piece, &missing, &water)
                .is_err()
        );
        assert_eq!(piece.selection.position, [8, 90, 8]);
        region
            .align_ocean_ruin(&mut piece, &empty_ice, &water)
            .unwrap();
        assert_eq!(piece.selection.position, [8, 8, 8]);
        region
            .align_ocean_ruin(&mut piece, &custom_ice, &water)
            .unwrap();
        assert_eq!(piece.selection.position, [8, 2, 8]);
        assert_eq!(piece.bounds.min, [8, 2, 8]);
        assert_eq!(piece.bounds.max, [8, 2, 8]);
    }
}
