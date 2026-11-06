//! Place supported rigid jigsaw templates in an unpublished decoration region.
use super::*;
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
use native_resources::{block_registry::GeneratedBlockRegistry, structure_pool::LiquidSettings};
use native_world::fluid::{FluidKind, FluidState};

use super::super::jigsaw::{
    JigsawPlacementWorld, PreparedJigsawPlacement, PreparedStart, PreparedTemplatePiece,
};

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
    /// Place every intersecting piece of a preflighted start as one staged
    /// operation. A failure prevents publication and preserves the RNG.
    pub fn place_jigsaw_start(
        &mut self,
        prepared: &PreparedJigsawPlacement,
        start: &PreparedStart,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<usize> {
        ensure!(!self.failed, "generation attempt previously failed");
        let result = prepared.place_with(start, clip, random, |piece, liquids, next| {
            self.place_rigid_jigsaw_piece(piece, registry, clip, liquids, next)
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }

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

    struct TemplateData {
        bytes: Vec<u8>,
        reads: usize,
    }
    impl ResourceProvider for TemplateData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            self.reads += 1;
            Ok(Some(self.bytes.clone()))
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
        Ok(TemplateData {
            bytes: encoder.finish()?,
            reads: 0,
        })
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

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn prepared_start_places_intersecting_pieces_and_rejects_stale_start() -> Result<()> {
        use super::super::super::jigsaw::{PreparedJigsawPlacement, PreparedStart};
        use native_core::beardifier::TerrainAdjustment;
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let single = PoolElement::Single {
            legacy: false,
            location: ResourceId::parse("example:test")?,
            processors: holder,
            projection: Projection::Rigid,
            override_liquid_settings: None,
        };
        let make_piece = |x, element| -> Result<Piece> {
            Ok(Piece {
                element,
                origin: [x, 8, 1],
                rotation: Rotation::None,
                bounds: BoundingBox::new([x, 8, 1], [x + 1, 8, 1]).map_err(anyhow::Error::msg)?,
                ground_level_delta: 0,
                junctions: vec![],
            })
        };
        let start = PreparedStart {
            chunk: [0, 0],
            generation_point: [8, 8, 8],
            terrain_adjustment: TerrainAdjustment::None,
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
            pieces: vec![
                make_piece(1, single.clone())?,
                make_piece(
                    4,
                    PoolElement::List {
                        elements: vec![single.clone(), PoolElement::Empty],
                        projection: Projection::Rigid,
                    },
                )?,
                make_piece(20, single)?,
            ],
        };
        let mut templates = template(false)?;
        let prepared = PreparedJigsawPlacement::load(&start, &mut templates, &blocks, &registry)?;
        assert_eq!(
            templates.reads, 1,
            "duplicate templates should be loaded once"
        );
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        assert_eq!(
            region.place_jigsaw_start(
                &prepared,
                &start,
                &registry,
                clip,
                &mut DecorationRandom::new(42)
            )?,
            2
        );
        region.finish()?;
        for x in [1, 2, 4, 5] {
            assert_eq!(
                terrain.snapshot().block(x, 8, 1).unwrap().name,
                "minecraft:stone"
            );
        }
        assert_eq!(
            terrain.snapshot().block(20, 8, 1).unwrap().name,
            "minecraft:air"
        );

        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut stale = start.clone();
        stale.pieces[1].origin[0] += 1;
        let mut random = DecorationRandom::new(42);
        let mut original = random.clone();
        assert!(
            region
                .place_jigsaw_start(&prepared, &stale, &registry, clip, &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), original.next_long());
        assert!(region.finish().is_err());
        assert_eq!(terrain.snapshot().block(1, 8, 1), before.block(1, 8, 1));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn mixed_scheduler_places_jigsaw_and_rejects_missing_context() -> Result<()> {
        use super::super::super::{
            jigsaw::PreparedStart,
            structure_batch::{PlannedMixedStructureDecoration, PlannedStructureStart},
        };
        use native_core::beardifier::TerrainAdjustment;
        use native_resources::structure_decoration::StructureDecorationEntry;
        #[derive(Clone)]
        struct NoOcean;
        impl MixedOceanRuinPlacement for NoOcean {
            fn place(
                &mut self,
                _: &mut DecorationRegion<'_>,
                _: &mut super::super::super::ocean_ruin::PreparedStart,
                _: BoundingBox,
                _: &mut DecorationRandom,
            ) -> Result<()> {
                bail!("unexpected ocean ruin")
            }
        }
        #[derive(Clone)]
        struct Corrupt;
        impl MixedOceanRuinPlacement for Corrupt {
            fn place(
                &mut self,
                _: &mut DecorationRegion<'_>,
                _: &mut super::super::super::ocean_ruin::PreparedStart,
                _: BoundingBox,
                _: &mut DecorationRandom,
            ) -> Result<()> {
                bail!("unexpected ocean ruin")
            }
            fn place_jigsaw(
                &mut self,
                _: &mut DecorationRegion<'_>,
                _: &super::super::super::structure_batch::TerrainStartKey,
                start: &mut PreparedStart,
                _: BoundingBox,
                _: &mut DecorationRandom,
            ) -> Result<()> {
                start.pieces[0].origin[0] += 1;
                Ok(())
            }
        }
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let bounds = BoundingBox::new([1, 8, 1], [2, 8, 1]).map_err(anyhow::Error::msg)?;
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
            bounds,
            ground_level_delta: 0,
            junctions: vec![],
        };
        let start = PreparedStart {
            chunk: [0, 0],
            generation_point: [8, 8, 8],
            terrain_adjustment: TerrainAdjustment::None,
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
            pieces: vec![piece],
        };
        let entry = StructureDecorationEntry {
            id: ResourceId::parse("example:jigsaw")?,
            step: 0,
            index: 0,
        };
        let structures = [PlannedMixedStructureDecoration {
            entry: &entry,
            starts: vec![PlannedStructureStart::Jigsaw(&start)],
        }];
        let (mut data, schedule, biomes) = super::super::tests::scheduled_fixture(None);
        let plan = super::super::tests::fixture_plan(&schedule, &biomes);
        let prepared = PreparedDecoration::load(
            &mut data,
            &plan,
            &super::super::tests::batch_registry(),
            &mut super::super::tests::resolve_test,
        )?;
        let (mut failed, _, _, _) = super::super::tests::fixture();
        let before = failed.snapshot();
        assert!(
            prepared
                .execute_with_mixed_structure_plan(&mut failed, &structures, &mut NoOcean)
                .is_err()
        );
        assert!(Arc::ptr_eq(&before, &failed.snapshot()));
        let error = prepared
            .execute_with_mixed_structure_plan(&mut failed, &structures, &mut Corrupt)
            .err()
            .context("corrupted jigsaw accepted")?;
        assert!(format!("{error:#}").contains("jigsaw start changed"));
        assert!(Arc::ptr_eq(&before, &failed.snapshot()));
        let mut context = MixedJigsawPlacementContext::load(
            NoOcean,
            &structures,
            &mut template(false)?,
            &blocks,
            &registry,
        )?;
        let (mut terrain, _, _, _) = super::super::tests::fixture();
        let (_, updated) =
            prepared.execute_with_mixed_structure_plan(&mut terrain, &structures, &mut context)?;
        assert!(matches!(
            &updated[0][0],
            super::super::super::PreparedStructureStart::Jigsaw(_)
        ));
        assert_eq!(
            terrain.snapshot().block(1, 8, 1).unwrap().name,
            "minecraft:stone"
        );
        assert_eq!(
            terrain.snapshot().block(2, 8, 1).unwrap().name,
            "minecraft:stone"
        );
        Ok(())
    }
}
