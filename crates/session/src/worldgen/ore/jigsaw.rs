//! Place supported rigid jigsaw templates in an unpublished decoration region.
use super::*;
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
use native_resources::{block_registry::GeneratedBlockRegistry, structure_pool::LiquidSettings};
use native_world::fluid::{FluidKind, FluidState};

use super::super::jigsaw::{JigsawPlacementWorld, PreparedTemplatePiece};

struct JigsawRegion<'r, 'a, 'g> {
    region: &'r mut DecorationRegion<'a>,
    registry: &'g GeneratedBlockRegistry,
}

fn jigsaw_static_full(state: &BlockState) -> Result<bool> {
    if state.name == "minecraft:stone" && state.properties.is_empty() {
        return Ok(true);
    }
    super::super::ocean_ruin::ocean_template_static_full(state)
}

impl JigsawRegion<'_, '_, '_> {
    fn checked_fluid(&self, state: &BlockState) -> Result<Option<FluidState>> {
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties),
            "unverified jigsaw fluid state"
        );
        FluidState::from_block(state).map_err(anyhow::Error::msg)
    }
}

impl JigsawPlacementWorld for JigsawRegion<'_, '_, '_> {
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self.region, pos)
    }
    fn fluid_at(&mut self, pos: [i32; 3]) -> Result<Option<FluidState>> {
        let state = self.block(pos)?;
        self.checked_fluid(&state)
    }
    fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>> {
        self.checked_fluid(state)
    }
    fn place_template_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        _: &mut DecorationRandom,
    ) -> Result<bool> {
        if !self.region.writable.contains(&chunk(pos)) {
            return Ok(false);
        }
        ensure!(
            self.region.world.chunks.contains_key(&chunk(pos)),
            "jigsaw block chunk missing"
        );
        if self.region.is_outside(pos[1]) {
            return Ok(true);
        }
        ensure!(
            nbt.is_none(),
            "jigsaw block entity placement is not yet supported"
        );
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties)
                && self.registry.has_block_entity(&state.name) == Some(false)
                && jigsaw_static_full(state).is_ok(),
            "unsupported jigsaw block effects"
        );
        let old = self.region.read(pos)?;
        ensure!(
            self.registry.is_canonical_state(&old.name, &old.properties)
                && self.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some() || jigsaw_static_full(&old).is_ok())
                && !self.region.monster_entities.contains_key(&pos)
                && !self.region.bee_nests.contains_key(&pos),
            "unsupported jigsaw replacement source"
        );
        self.region.set_proto_block(pos, state)?;
        Ok(true)
    }
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name.starts_with("minecraft:")
            && state.name.ends_with("_stairs")
            && self
                .registry
                .is_canonical_state(&state.name, &state.properties))
    }
    fn place_liquid(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        fluid: Option<FluidState>,
    ) -> Result<()> {
        if state.properties.get("waterlogged").map(String::as_str) == Some("false")
            && fluid.is_some_and(|fluid| fluid.kind == FluidKind::Water && fluid.source)
        {
            let mut wet = state.clone();
            wet.properties.insert("waterlogged".into(), "true".into());
            if self.region.writable.contains(&chunk(pos)) && !self.region.is_outside(pos[1]) {
                self.region.set_proto_block(pos, &wet)?;
            }
            SpringWorld::schedule_spring_tick(
                self.region,
                pos,
                &ResourceId::parse("minecraft:water")?,
            )?;
        }
        Ok(())
    }
    fn mark_block_entity_changed(&mut self, _: [i32; 3]) -> Result<()> {
        bail!("jigsaw block entity marking is not yet supported")
    }
    fn place_template_entities(
        &mut self,
        _: &[Tag],
        _: [i32; 3],
        _: native_resources::structure_transform::Rotation,
        _: BoundingBox,
    ) -> Result<()> {
        bail!("jigsaw template entity placement is not yet supported")
    }
}

impl DecorationRegion<'_> {
    /// Place one supported rigid piece in this unpublished region. On failure,
    /// `finish` refuses to publish any of the region's intermediate writes.
    pub fn place_rigid_jigsaw_piece(
        &mut self,
        piece: &PreparedTemplatePiece,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
        liquids: LiquidSettings,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        let result = piece.place(
            &mut JigsawRegion {
                region: self,
                registry,
            },
            registry,
            clip,
            liquids,
            jigsaw_static_full,
            random,
        )?;
        self.failed = false;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use native_persistence::{
        nbt::{self, NamedTag, TagType},
        structure_processors::PreparedProcessors,
    };
    use native_resources::{
        jigsaw_placement::Piece,
        structure_pool::{HolderValue, PoolElement, Projection},
        structure_transform::Rotation,
    };
    use std::io::Write;

    struct TemplateData(Vec<u8>);
    impl ResourceProvider for TemplateData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(Some(self.0.clone()))
        }
    }
    fn ints(values: &[i32]) -> Tag {
        Tag::List {
            element_type: TagType::Int,
            values: values.iter().copied().map(Tag::Int).collect(),
        }
    }
    fn template(with_unsupported_nbt: bool) -> Result<TemplateData> {
        let mut second = vec![
            ("pos".into(), ints(&[1, 0, 0])),
            ("state".into(), Tag::Int(0)),
        ];
        if with_unsupported_nbt {
            second.push(("nbt".into(), Tag::Compound(vec![])));
        }
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("size".into(), ints(&[2, 1, 1])),
                (
                    "palette".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![(
                            "id".into(),
                            Tag::String("minecraft:stone".into()),
                        )])],
                    },
                ),
                (
                    "blocks".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![
                            Tag::Compound(vec![
                                ("pos".into(), ints(&[0, 0, 0])),
                                ("state".into(), Tag::Int(0)),
                            ]),
                            Tag::Compound(second),
                        ],
                    },
                ),
                (
                    "entities".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
            ]),
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&document)?)?;
        Ok(TemplateData(encoder.finish()?))
    }
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn rigid_piece_updates_draft_and_failed_piece_is_not_published() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = data.vanilla_block_ids()?;
        let registry = data.vanilla_generated_block_registry()?;
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let processors = super::super::super::jigsaw::PieceProcessors::Single(Arc::new(
            PreparedProcessors::load(&holder, &mut data, &blocks, &registry)?,
        ));
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let piece = Piece {
            element: PoolElement::Single {
                legacy: false,
                location: ResourceId::parse("example:test")?,
                processors: holder,
                projection: Projection::Rigid,
                override_liquid_settings: None,
            },
            origin: [1, 8, 1],
            rotation: Rotation::None,
            bounds: clip,
            ground_level_delta: 0,
            junctions: vec![],
        };
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let prepared = PreparedTemplatePiece::load(&piece, &processors, &mut template(false)?)?;
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        assert!(region.place_rigid_jigsaw_piece(
            &prepared,
            &registry,
            clip,
            LiquidSettings::IgnoreWaterlogging,
            &mut DecorationRandom::new(42)
        )?);
        region.finish()?;
        assert_eq!(
            terrain.snapshot().block(1, 8, 1).unwrap().name,
            "minecraft:stone"
        );
        assert_eq!(
            terrain.snapshot().block(2, 8, 1).unwrap().name,
            "minecraft:stone"
        );

        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let prepared = PreparedTemplatePiece::load(&piece, &processors, &mut template(true)?)?;
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut original = random.clone();
        assert!(
            region
                .place_rigid_jigsaw_piece(
                    &prepared,
                    &registry,
                    clip,
                    LiquidSettings::IgnoreWaterlogging,
                    &mut random
                )
                .is_err()
        );
        assert_eq!(random.next_long(), original.next_long());
        assert!(region.finish().is_err());
        assert_eq!(terrain.snapshot().block(1, 8, 1), before.block(1, 8, 1));
        assert_eq!(terrain.snapshot().block(2, 8, 1), before.block(2, 8, 1));
        Ok(())
    }
}
