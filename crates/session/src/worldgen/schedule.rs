//! Java 26.3 chunk-generation dependencies and a deterministic work plan.
//! Completing a plan entry only records an already successful generation step.
use anyhow::{Context, Result, bail, ensure};
use native_world::loaded::ChunkPos;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum GenerationStage {
    Empty,
    StructureStarts,
    StructureReferences,
    Biomes,
    Terrain,
    Features,
    InitializeLight,
    Light,
    Spawn,
    Full,
}
use GenerationStage::*;

pub(super) enum StructureFeatureLighting<'a> {
    Visible(&'a native_world::generation_light::VisibleLightSnapshot),
    Fresh(
        &'a mut super::FreshGenerationLightTracker,
        &'a native_resources::block_registry::GeneratedBlockRegistry,
    ),
    Staged(
        &'a mut super::StagedGenerationLightEngine,
        &'a native_resources::block_registry::GeneratedBlockRegistry,
    ),
}

pub(super) trait OceanFeatureRunner {
    fn supports_huts(&self) -> bool {
        false
    }

    /// Whether successful mineshaft placement records every generated cart.
    fn records_mineshaft_entities(&self) -> bool {
        false
    }

    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        huts: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>>;
}

/// Execute mineshafts and simple non-hut structures with a caller-provided
/// independent minecart constructor. Ocean ruins and huts remain explicit.
pub(super) struct MineshaftFeatureCallbacks<'a, U> {
    pub uuid_factory: &'a mut U,
}

#[derive(Clone)]
struct NoOceanPlacement {
    region_random: native_resources::worldgen_region_random::RegionRandom,
}
impl super::MixedOceanRuinPlacement for NoOceanPlacement {
    fn worldgen_region_random(
        &mut self,
    ) -> Option<&mut native_resources::worldgen_region_random::RegionRandom> {
        Some(&mut self.region_random)
    }
    fn place(
        &mut self,
        _: &mut super::DecorationRegion<'_>,
        _: &mut super::ocean_ruin::PreparedStart,
        _: native_core::beardifier::BoundingBox,
        _: &mut native_core::random::DecorationRandom,
    ) -> Result<()> {
        bail!("ocean ruin decoration context is missing")
    }
}

/// Execute prepared templates without manufacturing ocean, hut, or mineshaft effects.
pub(super) struct JigsawOnlyFeatureRunner;
impl OceanFeatureRunner for JigsawOnlyFeatureRunner {
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        _: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        _: &super::structure_batch::PreparedStructureStages,
        _: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        ensure!(jigsaws.is_some(), "template resources are missing");
        execute_mixed_lighting(
            terrain,
            prepared,
            structures,
            jigsaws,
            &mut NoOceanPlacement {
                region_random: region_random.clone(),
            },
            lighting,
        )
    }
}

impl<U> OceanFeatureRunner for MineshaftFeatureCallbacks<'_, U>
where
    U: Clone + FnMut([f64; 3]) -> Result<Option<[i32; 4]>>,
{
    fn records_mineshaft_entities(&self) -> bool {
        true
    }

    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        _: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        _: &super::structure_batch::PreparedStructureStages,
        _: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        let mut context = super::MixedMineshaftPlacementContext {
            other: NoOceanPlacement {
                region_random: region_random.clone(),
            },
            blocking_biomes: prepared
                .mineshaft_blocking_biomes()
                .context("mineshaft biome tags are not prepared")?,
            uuid_factory: self.uuid_factory.clone(),
        };
        let updated = execute_mixed_lighting(
            terrain,
            prepared,
            structures,
            jigsaws,
            &mut context,
            lighting,
        )?;
        *self.uuid_factory = context.uuid_factory;
        Ok(updated)
    }
}

pub(super) struct OceanFeatureCallbacks<'a, 'b, C, W> {
    pub ocean: &'a mut super::OceanRuinPlacementContext<'b, C, W>,
}

impl<C, W> OceanFeatureRunner for OceanFeatureCallbacks<'_, '_, C, W>
where
    C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
    W: super::ocean_ruin::DrownedMarkerContext,
{
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        _: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        _: &super::structure_batch::PreparedStructureStages,
        _: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        let mut staged = self.ocean.clone();
        staged.region_random = region_random.clone();
        let updated = execute_mixed_lighting(
            terrain,
            prepared,
            structures,
            jigsaws,
            &mut staged,
            lighting,
        )?;
        *self.ocean = staged;
        Ok(updated)
    }
}

pub(super) struct MixedFeatureCallbacks<'a, 'b, C, W, H, P> {
    pub context: &'a mut super::MixedHutOceanPlacementContext<'b, C, W, H, P>,
}

pub(super) struct MixedMineshaftFeatureCallbacks<'a, 'b, C, W, H, P, U> {
    pub context: &'a mut super::MixedHutOceanPlacementContext<'b, C, W, H, P>,
    pub uuid_factory: &'a mut U,
}

fn execute_mixed_lighting<R: super::MixedOceanRuinPlacement + Clone>(
    terrain: &mut super::CarvedTerrainWorld,
    prepared: &super::PreparedDecoration<'_, '_>,
    structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
    jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
    context: &mut R,
    lighting: StructureFeatureLighting<'_>,
) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
    if let Some(jigsaws) = jigsaws {
        let mut staged = jigsaws.wrap(context.clone());
        let updated =
            execute_mixed_lighting_inner(terrain, prepared, structures, &mut staged, lighting)?;
        *context = staged.other.other;
        return Ok(updated);
    }
    execute_mixed_lighting_inner(terrain, prepared, structures, context, lighting)
}

fn execute_mixed_lighting_inner<R: super::MixedOceanRuinPlacement + Clone>(
    terrain: &mut super::CarvedTerrainWorld,
    prepared: &super::PreparedDecoration<'_, '_>,
    structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
    context: &mut R,
    lighting: StructureFeatureLighting<'_>,
) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
    let (_, updated) = match lighting {
        StructureFeatureLighting::Visible(light) => {
            prepared.execute_mixed_with_visible_light(terrain, structures, context, light)?
        }
        StructureFeatureLighting::Fresh(tracker, registry) => prepared
            .execute_mixed_with_tracked_light(terrain, structures, context, tracker, registry)?,
        StructureFeatureLighting::Staged(engine, registry) => prepared
            .execute_mixed_with_staged_light(terrain, structures, context, engine, registry)?,
    };
    Ok(updated)
}

impl<C, W, H, P> OceanFeatureRunner for MixedFeatureCallbacks<'_, '_, C, W, H, P>
where
    C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
    W: super::ocean_ruin::DrownedMarkerContext,
    H: Clone
        + FnMut(
            native_resources::structure_piece::swamp_hut::HutMob,
        ) -> Result<super::FreshCreatureRandom>,
    P: Clone
        + FnMut(
            &super::structure_batch::TerrainStartKey,
            [i32; 3],
            &native_world::loaded::World,
            &serde_json::Value,
            native_core::beardifier::BoundingBox,
            &[super::structure_batch::StructurePieceRevision],
        ) -> Result<bool>,
{
    fn supports_huts(&self) -> bool {
        true
    }

    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        huts: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        _: &super::structure_batch::PreparedStructureStages,
        _: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        let mut staged = self.context.clone();
        staged.ocean.region_random = region_random.clone();
        staged.huts.clone_from(huts);
        let updated = execute_mixed_lighting(
            terrain,
            prepared,
            structures,
            jigsaws,
            &mut staged,
            lighting,
        )?;
        huts.clone_from(&staged.huts);
        *self.context = staged;
        Ok(updated)
    }
}

impl<C, W, H, P, U> OceanFeatureRunner for MixedMineshaftFeatureCallbacks<'_, '_, C, W, H, P, U>
where
    C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
    W: super::ocean_ruin::DrownedMarkerContext,
    H: Clone
        + FnMut(
            native_resources::structure_piece::swamp_hut::HutMob,
        ) -> Result<super::FreshCreatureRandom>,
    P: Clone
        + FnMut(
            &super::structure_batch::TerrainStartKey,
            [i32; 3],
            &native_world::loaded::World,
            &serde_json::Value,
            native_core::beardifier::BoundingBox,
            &[super::structure_batch::StructurePieceRevision],
        ) -> Result<bool>,
    U: Clone + FnMut([f64; 3]) -> Result<Option<[i32; 4]>>,
{
    fn supports_huts(&self) -> bool {
        true
    }

    fn records_mineshaft_entities(&self) -> bool {
        true
    }

    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        huts: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        _: &super::structure_batch::PreparedStructureStages,
        _: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        let mut other = self.context.clone();
        other.ocean.region_random = region_random.clone();
        other.huts.clone_from(huts);
        let updated = if let Some(blocking_biomes) = prepared.mineshaft_blocking_biomes() {
            let mut staged = super::MixedMineshaftPlacementContext {
                other,
                blocking_biomes,
                uuid_factory: self.uuid_factory.clone(),
            };
            let updated = execute_mixed_lighting(
                terrain,
                prepared,
                structures,
                jigsaws,
                &mut staged,
                lighting,
            )?;
            *self.uuid_factory = staged.uuid_factory;
            other = staged.other;
            updated
        } else {
            execute_mixed_lighting(terrain, prepared, structures, jigsaws, &mut other, lighting)?
        };
        huts.clone_from(&other.huts);
        *self.context = other;
        Ok(updated)
    }
}

pub(super) struct NativeMixedFeatureRunner<'a, 'b, C, W, U> {
    pub ocean: &'a mut super::OceanRuinPlacementContext<'b, C, W>,
    pub variants: &'a native_resources::cat_variants::CatSpawnVariants,
    pub conditions: &'a native_resources::cat_variants::CatWorldConditions,
    pub moon_phase_at: &'a dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
    pub uuid_factory: Option<&'a mut U>,
}

impl<C, W, U> OceanFeatureRunner for NativeMixedFeatureRunner<'_, '_, C, W, U>
where
    C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
    W: super::ocean_ruin::DrownedMarkerContext,
    U: Clone + FnMut([f64; 3]) -> Result<Option<[i32; 4]>>,
{
    fn supports_huts(&self) -> bool {
        true
    }

    fn records_mineshaft_entities(&self) -> bool {
        self.uuid_factory.is_some()
    }

    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        huts: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        use native_resources::structure_piece::swamp_hut::HutMob;
        let conditions = self.conditions;
        let moon_phase_at = self.moon_phase_at;
        let mut context = super::MixedHutOceanPlacementContext {
            ocean: self.ocean.clone(),
            huts: huts.clone(),
            variants: self.variants,
            construct_hut: |mob| {
                let id = match mob {
                    HutMob::Witch => "minecraft:witch",
                    HutMob::Cat => "minecraft:cat",
                };
                super::FreshCreatureRandom::from_entropy(
                    &native_resources::packs::ResourceId::parse(id)?,
                )
            },
            condition:
                |key: &super::structure_batch::TerrainStartKey,
                 pos: [i32; 3],
                 _: &native_world::loaded::World,
                 value: &serde_json::Value,
                 bounds: native_core::beardifier::BoundingBox,
                 revisions: &[super::structure_batch::StructurePieceRevision]| {
                    native_cat_condition(
                        conditions,
                        moon_phase_at,
                        all_structures,
                        biomes,
                        key,
                        pos,
                        value,
                        bounds,
                        revisions,
                    )
                },
        };
        let updated = if let Some(uuid_factory) = self.uuid_factory.as_deref_mut() {
            MixedMineshaftFeatureCallbacks {
                context: &mut context,
                uuid_factory,
            }
            .execute(
                terrain,
                prepared,
                structures,
                jigsaws,
                huts,
                region_random,
                lighting,
                all_structures,
                biomes,
            )?
        } else {
            MixedFeatureCallbacks {
                context: &mut context,
            }
            .execute(
                terrain,
                prepared,
                structures,
                jigsaws,
                huts,
                region_random,
                lighting,
                all_structures,
                biomes,
            )?
        };
        self.ocean.region_random = context.ocean.region_random;
        self.ocean.construct = context.ocean.construct;
        self.ocean.context = context.ocean.context;
        Ok(updated)
    }
}

#[derive(Clone)]
struct ContextualDrowned<'a, E, D, H> {
    base: super::ocean_ruin::DrownedGenerationContext<'a, E, D, H>,
    structures: &'a super::structure_batch::PreparedStructureStages,
    moon: &'a dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
    key: Option<super::structure_batch::TerrainStartKey>,
    revisions: Vec<super::structure_batch::StructurePieceRevision>,
    piece_index: Option<usize>,
    pieces: Vec<native_core::beardifier::BoundingBox>,
}

struct RuinSpawnEnvironment<'a> {
    biomes: &'a super::BiomeNeighborhood,
    position: [i32; 3],
    moon: &'a dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
    structures: &'a super::structure_batch::PreparedStructureStages,
    key: &'a super::structure_batch::TerrainStartKey,
    revisions: &'a [super::structure_batch::StructurePieceRevision],
    pieces: &'a [native_core::beardifier::BoundingBox],
    matching: Option<BTreeSet<native_resources::packs::ResourceId>>,
}

impl native_resources::animal_variants::SpawnVariantEnvironment for RuinSpawnEnvironment<'_> {
    fn biome(&self) -> Result<&native_resources::packs::ResourceId> {
        self.biomes
            .biome_at_block(self.position[0], self.position[1], self.position[2])
    }
    fn moon_phase(&mut self) -> Result<u8> {
        (self.moon)(self.position, self.biomes)
    }
    fn structures_with_piece(&mut self) -> Result<&BTreeSet<native_resources::packs::ResourceId>> {
        if self.matching.is_none() {
            self.matching = Some(ruin_structures_with_piece(
                self.structures,
                self.position,
                self.key,
                self.revisions,
                self.pieces,
            )?);
        }
        Ok(self
            .matching
            .as_ref()
            .expect("structure set was initialized"))
    }
}

fn ruin_structures_with_piece(
    structures: &super::structure_batch::PreparedStructureStages,
    position: [i32; 3],
    key: &super::structure_batch::TerrainStartKey,
    revisions: &[super::structure_batch::StructurePieceRevision],
    current_pieces: &[native_core::beardifier::BoundingBox],
) -> Result<BTreeSet<native_resources::packs::ResourceId>> {
    let chunk = ChunkPos {
        x: position[0].div_euclid(16),
        z: position[2].div_euclid(16),
    };
    let references = structures
        .references
        .get(&chunk)
        .context("ruin spawn structure references unavailable")?;
    let mut matching = BTreeSet::new();
    let contains = |bounds: &native_core::beardifier::BoundingBox| {
        (0..3).all(|axis| (bounds.min[axis]..=bounds.max[axis]).contains(&position[axis]))
    };
    for (id, sources) in references.membership() {
        for &packed in sources {
            let bits = packed as u64;
            let source = ChunkPos {
                x: bits as u32 as i32,
                z: (bits >> 32) as u32 as i32,
            };
            let candidate = (id.clone(), source);
            let simple = structures
                .starts
                .get(&source)
                .and_then(|starts| starts.get(id));
            let ocean = structures
                .ocean_starts
                .get(&source)
                .and_then(|starts| starts.get(id));
            let mine = structures
                .mineshafts
                .get(&source)
                .and_then(|starts| starts.get(id));
            let jigsaw = structures
                .jigsaw_starts
                .get(&source)
                .and_then(|starts| starts.get(id));
            let portal = structures
                .ruined_portal_starts
                .get(&source)
                .and_then(|starts| starts.get(id));
            ensure!(
                usize::from(simple.is_some())
                    + usize::from(ocean.is_some())
                    + usize::from(mine.is_some())
                    + usize::from(jigsaw.is_some())
                    + usize::from(portal.is_some())
                    == 1,
                "ruin spawn referenced structure is missing or ambiguous"
            );
            let actual_chunk = simple
                .map(|start| start.chunk)
                .or_else(|| ocean.map(|start| start.chunk))
                .or_else(|| mine.map(|start| start.chunk))
                .or_else(|| jigsaw.map(|start| start.chunk))
                .or_else(|| portal.map(|start| start.chunk))
                .context("ruin structure missing")?;
            ensure!(
                actual_chunk == [source.x, source.z],
                "ruin structure source differs"
            );
            let mut revised = revisions
                .iter()
                .filter(|(revision, _)| revision == &candidate)
                .peekable();
            let inside = if &candidate == key {
                ensure!(
                    !current_pieces.is_empty(),
                    "current ruin pieces unavailable"
                );
                current_pieces.iter().any(contains)
            } else if revised.peek().is_some() {
                revised.any(|(_, bounds)| contains(bounds))
            } else if let Some(start) = simple {
                contains(&start.bounds)
            } else if let Some(start) = ocean {
                start.pieces.iter().any(|piece| contains(&piece.bounds))
            } else if let Some(start) = mine {
                start
                    .pieces
                    .iter()
                    .any(|piece| contains(&piece.piece.bounds))
            } else if let Some(start) = portal {
                contains(&start.piece.bounds)
            } else {
                jigsaw
                    .expect("one start was validated")
                    .pieces
                    .iter()
                    .any(|piece| contains(&piece.bounds))
            };
            if inside {
                matching.insert(id.clone());
                break;
            }
        }
    }
    Ok(matching)
}

impl<E, D, H> super::ocean_ruin::DrownedMarkerContext for ContextualDrowned<'_, E, D, H>
where
    E: super::ocean_ruin::DrownedSpawnEffects,
    D: Clone
        + FnMut(
            [i32; 3],
            &native_world::loaded::World,
            &super::BiomeNeighborhood,
        ) -> Result<super::ocean_ruin::GenerationDifficultyInputs>,
    H: Clone + FnMut() -> Result<bool>,
{
    fn needs_ruin_revisions(&self) -> bool {
        self.base.effects.needs_structure_variants()
    }
    fn begin_ruin_start(
        &mut self,
        key: &super::structure_batch::TerrainStartKey,
        revisions: &[super::structure_batch::StructurePieceRevision],
    ) -> Result<()> {
        self.key = Some(key.clone());
        self.revisions = revisions.to_vec();
        self.piece_index = None;
        self.pieces.clear();
        Ok(())
    }
    fn begin_ruin_piece(
        &mut self,
        index: usize,
        pieces: &[native_core::beardifier::BoundingBox],
    ) -> Result<()> {
        ensure!(index < pieces.len(), "invalid ruin piece index");
        self.piece_index = Some(index);
        self.pieces = pieces.to_vec();
        Ok(())
    }
    fn set_aligned_ruin_piece(
        &mut self,
        bounds: native_core::beardifier::BoundingBox,
    ) -> Result<()> {
        let index = self.piece_index.context("ruin piece index unavailable")?;
        *self
            .pieces
            .get_mut(index)
            .context("ruin piece unavailable")? = bounds;
        Ok(())
    }
    fn special_multiplier(
        &mut self,
        pos: [i32; 3],
        world: &native_world::loaded::World,
    ) -> Result<f32> {
        self.base.special_multiplier(pos, world)
    }
    fn halloween(&mut self) -> Result<bool> {
        self.base.halloween()
    }
    fn try_existing_chicken(
        &mut self,
        pos: [i32; 3],
        world: &native_world::loaded::World,
    ) -> Result<bool> {
        self.base.try_existing_chicken(pos, world)
    }
    fn spawn_chicken_jockey(
        &mut self,
        pos: [i32; 3],
        world: &native_world::loaded::World,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
    ) -> Result<bool> {
        let mut env = RuinSpawnEnvironment {
            biomes: self.base.biomes,
            position: pos,
            moon: self.moon,
            structures: self.structures,
            key: self.key.as_ref().context("ruin start unavailable")?,
            revisions: &self.revisions,
            pieces: &self.pieces,
            matching: None,
        };
        self.base.effects.spawn_chicken_jockey_in_environment(
            pos,
            world,
            random,
            self.base.biomes,
            &mut env,
        )
    }
    fn take_created_chicken(&mut self) -> Result<Option<native_persistence::nbt::Tag>> {
        self.base.take_created_chicken()
    }
    fn take_created_nautilus(&mut self) -> Result<Option<native_persistence::nbt::Tag>> {
        self.base.take_created_nautilus()
    }
    fn enchant_weapon(
        &mut self,
        pos: [i32; 3],
        world: &native_world::loaded::World,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        weapon: native_resources::drowned_spawn::MainHandRoll,
        special: f32,
    ) -> Result<native_persistence::nbt::Tag> {
        self.base
            .enchant_weapon(pos, world, random, weapon, special)
    }
    fn more_frequent_drowned_biome(
        &mut self,
        pos: [i32; 3],
        world: &native_world::loaded::World,
    ) -> Result<bool> {
        self.base.more_frequent_drowned_biome(pos, world)
    }
    fn spawn_zombie_nautilus(
        &mut self,
        pos: [i32; 3],
        world: &native_world::loaded::World,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        persistent: bool,
    ) -> Result<bool> {
        let mut env = RuinSpawnEnvironment {
            biomes: self.base.biomes,
            position: pos,
            moon: self.moon,
            structures: self.structures,
            key: self.key.as_ref().context("ruin start unavailable")?,
            revisions: &self.revisions,
            pieces: &self.pieces,
            matching: None,
        };
        self.base.effects.spawn_zombie_nautilus_in_environment(
            pos,
            world,
            random,
            persistent,
            self.base.biomes,
            &mut env,
        )
    }
}

pub(super) struct NativeDrownedFeatureRunner<'a, 'b, C, E, D, H, U> {
    pub inputs: &'a mut super::ocean_ruin::DrownedPlacementInputs<'b, C, E, D, H>,
    pub variants: &'a native_resources::cat_variants::CatSpawnVariants,
    pub conditions: &'a native_resources::cat_variants::CatWorldConditions,
    pub moon_phase_at: &'a dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
    pub uuid_factory: Option<&'a mut U>,
}

impl<C, E, D, H, U> OceanFeatureRunner for NativeDrownedFeatureRunner<'_, '_, C, E, D, H, U>
where
    C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
    E: super::ocean_ruin::DrownedSpawnEffects,
    D: Clone
        + FnMut(
            [i32; 3],
            &native_world::loaded::World,
            &super::BiomeNeighborhood,
        ) -> Result<super::ocean_ruin::GenerationDifficultyInputs>,
    H: Clone + FnMut() -> Result<bool>,
    U: Clone + FnMut([f64; 3]) -> Result<Option<[i32; 4]>>,
{
    fn supports_huts(&self) -> bool {
        true
    }

    fn records_mineshaft_entities(&self) -> bool {
        self.uuid_factory.is_some()
    }

    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        huts: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        let mut ocean = super::OceanRuinPlacementContext {
            ice_tags: self.inputs.ice_tags,
            water_fluids: self.inputs.water_fluids,
            bubble_fluids: self.inputs.bubble_fluids,
            registry: self.inputs.registry,
            sea_level: self.inputs.sea_level,
            region_random: region_random.clone(),
            construct: self.inputs.construct.clone(),
            context: ContextualDrowned {
                base: super::ocean_ruin::DrownedGenerationContext {
                    biomes,
                    biome_tags: self.inputs.biome_tags,
                    difficulty_at: self.inputs.difficulty_at.clone(),
                    halloween_at: self.inputs.halloween_at.clone(),
                    effects: self.inputs.effects.clone(),
                },
                structures: all_structures,
                moon: self.moon_phase_at,
                key: None,
                revisions: Vec::new(),
                piece_index: None,
                pieces: Vec::new(),
            },
        };
        let updated = NativeMixedFeatureRunner {
            ocean: &mut ocean,
            variants: self.variants,
            conditions: self.conditions,
            moon_phase_at: self.moon_phase_at,
            uuid_factory: self.uuid_factory.as_deref_mut(),
        }
        .execute(
            terrain,
            prepared,
            structures,
            jigsaws,
            huts,
            region_random,
            lighting,
            all_structures,
            biomes,
        )?;
        self.inputs.construct = ocean.construct;
        self.inputs.effects = ocean.context.base.effects;
        self.inputs.difficulty_at = ocean.context.base.difficulty_at;
        self.inputs.halloween_at = ocean.context.base.halloween_at;
        self.inputs.last_region_random = Some(ocean.region_random);
        Ok(updated)
    }
}

pub(super) trait HutFeatureRunner {
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        states: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>>;
}

pub(super) struct HutFeatureCallbacks<'a, C, P> {
    pub variants: &'a native_resources::cat_variants::CatSpawnVariants,
    pub construct: &'a mut C,
    pub condition: &'a mut P,
}

impl<C, P> HutFeatureRunner for HutFeatureCallbacks<'_, C, P>
where
    C: Clone
        + FnMut(
            native_resources::structure_piece::swamp_hut::HutMob,
        ) -> Result<super::FreshCreatureRandom>,
    P: Clone
        + FnMut(
            &crate::worldgen::structure_batch::TerrainStartKey,
            [i32; 3],
            &native_world::loaded::World,
            &serde_json::Value,
            native_core::beardifier::BoundingBox,
            &[crate::worldgen::structure_batch::StructurePieceRevision],
        ) -> Result<bool>,
{
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        states: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        _all_structures: &super::structure_batch::PreparedStructureStages,
        _biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>> {
        let (_, updated) = match lighting {
            StructureFeatureLighting::Visible(light) => prepared.execute_huts_with_visible_light(
                terrain,
                structures,
                states,
                random,
                self.variants,
                self.construct,
                self.condition,
                light,
            )?,
            StructureFeatureLighting::Fresh(tracker, registry) => prepared
                .execute_huts_with_tracked_light(
                    terrain,
                    structures,
                    states,
                    random,
                    self.variants,
                    self.construct,
                    self.condition,
                    tracker,
                    registry,
                )?,
            StructureFeatureLighting::Staged(engine, registry) => prepared
                .execute_huts_with_staged_light(
                    terrain,
                    structures,
                    states,
                    random,
                    self.variants,
                    self.construct,
                    self.condition,
                    engine,
                    registry,
                )?,
        };
        Ok(updated)
    }
}

/// Evaluate cat conditions against the currently prepared generation state.
pub(super) struct NativeHutFeatureRunner<'a> {
    pub variants: &'a native_resources::cat_variants::CatSpawnVariants,
    pub conditions: &'a native_resources::cat_variants::CatWorldConditions,
    pub moon_phase_at: &'a dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
}

impl HutFeatureRunner for NativeHutFeatureRunner<'_> {
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        states: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>> {
        use native_resources::structure_piece::swamp_hut::HutMob;
        let conditions = self.conditions;
        let moon_phase_at = self.moon_phase_at;
        let mut construct = |mob| {
            let id = match mob {
                HutMob::Witch => "minecraft:witch",
                HutMob::Cat => "minecraft:cat",
            };
            super::FreshCreatureRandom::from_entropy(&native_resources::packs::ResourceId::parse(
                id,
            )?)
        };
        let mut condition = |key: &super::structure_batch::TerrainStartKey,
                             pos: [i32; 3],
                             _: &native_world::loaded::World,
                             value: &serde_json::Value,
                             bounds: native_core::beardifier::BoundingBox,
                             revisions: &[super::structure_batch::StructurePieceRevision]|
         -> Result<bool> {
            native_cat_condition(
                conditions,
                moon_phase_at,
                all_structures,
                biomes,
                key,
                pos,
                value,
                bounds,
                revisions,
            )
        };
        HutFeatureCallbacks {
            variants: self.variants,
            construct: &mut construct,
            condition: &mut condition,
        }
        .execute(
            terrain,
            prepared,
            structures,
            states,
            random,
            lighting,
            all_structures,
            biomes,
        )
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn native_cat_condition(
    conditions: &native_resources::cat_variants::CatWorldConditions,
    moon_phase_at: &dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
    all_structures: &super::structure_batch::PreparedStructureStages,
    biomes: &super::BiomeNeighborhood,
    key: &super::structure_batch::TerrainStartKey,
    pos: [i32; 3],
    value: &serde_json::Value,
    bounds: native_core::beardifier::BoundingBox,
    revisions: &[super::structure_batch::StructurePieceRevision],
) -> Result<bool> {
    use native_resources::cat_variants::CatSpawnContext;
    let kind = native_resources::packs::ResourceId::parse(
        value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .context("cat condition type missing")?,
    )?;
    let mut matching = BTreeSet::new();
    let is_biome = kind.namespace == "minecraft" && kind.path == "biome";
    let is_structure = kind.namespace == "minecraft" && kind.path == "structure";
    let is_moon = kind.namespace == "minecraft" && kind.path == "moon_brightness";
    let biome = if is_biome {
        Some(biomes.biome_at_block(pos[0], pos[1], pos[2])?)
    } else {
        None
    };
    if is_structure {
        let chunk = ChunkPos {
            x: pos[0].div_euclid(16),
            z: pos[2].div_euclid(16),
        };
        let references = all_structures
            .references
            .get(&chunk)
            .context("cat spawn chunk structure references unavailable")?;
        for (id, sources) in references.membership() {
            for &packed in sources {
                let bits = packed as u64;
                let source = ChunkPos {
                    x: bits as u32 as i32,
                    z: (bits >> 32) as u32 as i32,
                };
                let candidate = (id.clone(), source);
                let simple = all_structures
                    .starts
                    .get(&source)
                    .and_then(|starts| starts.get(id));
                let ocean = all_structures
                    .ocean_starts
                    .get(&source)
                    .and_then(|starts| starts.get(id));
                let mine = all_structures
                    .mineshafts
                    .get(&source)
                    .and_then(|starts| starts.get(id));
                let jigsaw = all_structures
                    .jigsaw_starts
                    .get(&source)
                    .and_then(|starts| starts.get(id));
                let portal = all_structures
                    .ruined_portal_starts
                    .get(&source)
                    .and_then(|starts| starts.get(id));
                ensure!(
                    usize::from(simple.is_some())
                        + usize::from(ocean.is_some())
                        + usize::from(mine.is_some())
                        + usize::from(jigsaw.is_some())
                        + usize::from(portal.is_some())
                        == 1,
                    "cat spawn referenced structure is missing or ambiguous"
                );
                let start_chunk = simple
                    .map(|start| start.chunk)
                    .or_else(|| ocean.map(|start| start.chunk))
                    .or_else(|| mine.map(|start| start.chunk))
                    .or_else(|| jigsaw.map(|start| start.chunk))
                    .or_else(|| portal.map(|start| start.chunk))
                    .context("cat spawn referenced structure is missing")?;
                ensure!(
                    start_chunk == [source.x, source.z],
                    "cat spawn structure source differs"
                );
                let contains = |piece: &native_core::beardifier::BoundingBox| {
                    (0..3).all(|axis| (piece.min[axis]..=piece.max[axis]).contains(&pos[axis]))
                };
                let mut matching_revisions = revisions
                    .iter()
                    .filter(|(revision, _)| revision == &candidate)
                    .peekable();
                let inside = if &candidate == key {
                    contains(&bounds)
                } else if matching_revisions.peek().is_some() {
                    matching_revisions.any(|(_, piece)| contains(piece))
                } else if let Some(start) = simple {
                    contains(&start.bounds)
                } else if let Some(start) = ocean {
                    start.pieces.iter().any(|piece| contains(&piece.bounds))
                } else if let Some(start) = mine {
                    start
                        .pieces
                        .iter()
                        .any(|piece| contains(&piece.piece.bounds))
                } else if let Some(start) = portal {
                    contains(&start.piece.bounds)
                } else {
                    jigsaw
                        .expect("one start was validated")
                        .pieces
                        .iter()
                        .any(|piece| contains(&piece.bounds))
                };
                if inside {
                    matching.insert(id.clone());
                }
            }
        }
    }
    let moon_phase = if is_moon {
        let phase = moon_phase_at(pos, biomes)?;
        ensure!(phase < 8, "invalid moon phase at cat spawn");
        Some(phase)
    } else {
        None
    };
    conditions.matches(
        value,
        &CatSpawnContext {
            biome,
            moon_phase,
            structures_with_piece: is_structure.then_some(&matching),
        },
    )
}
impl GenerationStage {
    pub const ALL: [Self; 10] = [
        Empty,
        StructureStarts,
        StructureReferences,
        Biomes,
        Terrain,
        Features,
        InitializeLight,
        Light,
        Spawn,
        Full,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Empty => "minecraft:empty",
            StructureStarts => "minecraft:structure_starts",
            StructureReferences => "minecraft:structure_references",
            Biomes => "minecraft:biomes",
            Terrain => "minecraft:terrain",
            Features => "minecraft:features",
            InitializeLight => "minecraft:initialize_light",
            Light => "minecraft:light",
            Spawn => "minecraft:spawn",
            Full => "minecraft:full",
        }
    }
    /// Parse an exact 26.3 saved status; unknown statuses must not be guessed.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stage| stage.id() == id)
    }
    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self as usize + 1).copied()
    }
    pub fn previous(self) -> Option<Self> {
        (self as usize)
            .checked_sub(1)
            .and_then(|n| Self::ALL.get(n))
            .copied()
    }
    /// Dependency at each Chebyshev distance, beginning with the center.
    pub fn direct(self) -> &'static [Self] {
        match self {
            Empty => &[],
            StructureStarts => &[Empty],
            StructureReferences => &[StructureStarts; 9],
            Biomes => &[
                StructureReferences,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Terrain => &[
                Biomes,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Features => &[
                Terrain,
                Terrain,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            InitializeLight => &[Features],
            Light => &[InitializeLight, InitializeLight],
            Spawn => &[Light, Biomes],
            Full => &[Spawn],
        }
    }
    pub fn accumulated(self) -> &'static [Self] {
        match self {
            Empty => &[],
            StructureStarts => &[Empty],
            StructureReferences => &[StructureStarts; 9],
            Biomes => &[
                StructureReferences,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Terrain => &[
                Biomes,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Features => &[
                Terrain,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            InitializeLight => &[
                Features,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Light => &[
                InitializeLight,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Spawn => &[
                Light,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Full => &[
                Spawn,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
        }
    }
    pub fn block_write_radius(self) -> Option<i32> {
        match self {
            Terrain => Some(0),
            Features => Some(1),
            _ => None,
        }
    }
}

/// Single-threaded plan. A caller may record a step only after performing the
/// actual operation; this planner does not generate or save chunk contents.
#[derive(Default)]
pub struct GenerationSchedule {
    required: BTreeMap<ChunkPos, GenerationStage>,
    completed: BTreeMap<ChunkPos, GenerationStage>,
    ready: BTreeSet<ChunkPos>,
    revision: u64,
}

impl GenerationSchedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Expand Java's accumulated dependencies transactionally. Requests can overlap or
    /// increase existing targets without discarding completed work.
    pub fn request(&mut self, pos: ChunkPos, target: GenerationStage) -> Result<()> {
        self.request_many([(pos, target)])
    }

    /// Expand overlapping requests in one transaction. If any coordinate
    /// overflows, none of the requested stages are recorded.
    pub fn request_many(
        &mut self,
        requests: impl IntoIterator<Item = (ChunkPos, GenerationStage)>,
    ) -> Result<()> {
        let mut staged = BTreeMap::new();
        for (pos, target) in requests {
            staged
                .entry(pos)
                .and_modify(|stage: &mut GenerationStage| *stage = (*stage).max(target))
                .or_insert(target);
            for (radius, &needed) in target.accumulated().iter().enumerate().skip(1) {
                let r = i32::try_from(radius)?;
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dz.abs()) != r {
                            continue;
                        }
                        let neighbor = ChunkPos {
                            x: pos
                                .x
                                .checked_add(dx)
                                .context("generation chunk X overflow")?,
                            z: pos
                                .z
                                .checked_add(dz)
                                .context("generation chunk Z overflow")?,
                        };
                        staged
                            .entry(neighbor)
                            .and_modify(|stage| *stage = (*stage).max(needed))
                            .or_insert(needed);
                    }
                }
            }
        }
        if staged.is_empty() {
            return Ok(());
        }
        let mut changed = Vec::new();
        for (pos, needed) in staged {
            if self.required.get(&pos).is_none_or(|&old| needed > old) {
                changed.push(pos);
            }
            self.required
                .entry(pos)
                .and_modify(|current| *current = (*current).max(needed))
                .or_insert(needed);
        }
        // Neighbor requirements are always at least StructureStarts; adding
        // an Empty entry therefore cannot make a neighbor ready by itself.
        for pos in changed {
            self.refresh_ready(pos);
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }
    /// The complete, deduplicated set of currently requested stage inputs,
    /// including prerequisites added for neighboring chunks. Adding another
    /// request can extend this set, so consumers must capture it again after
    /// any change to the schedule.
    pub fn requirements(&self) -> impl Iterator<Item = (ChunkPos, GenerationStage)> + '_ {
        self.required.iter().map(|(&pos, &stage)| (pos, stage))
    }
    pub fn required(&self, pos: ChunkPos) -> Option<GenerationStage> {
        self.required.get(&pos).copied()
    }
    pub fn completed(&self, pos: ChunkPos) -> Option<GenerationStage> {
        self.required
            .contains_key(&pos)
            .then(|| self.completed.get(&pos).copied().unwrap_or(Empty))
    }
    pub fn requested_chunks(&self) -> usize {
        self.required.len()
    }
    pub fn is_complete(&self) -> bool {
        self.required
            .iter()
            .all(|(pos, required)| self.completed.get(pos).copied().unwrap_or(Empty) >= *required)
    }

    pub(super) fn has_pending_stage(&self, stage: GenerationStage) -> bool {
        self.required.iter().any(|(pos, &required)| {
            required >= stage && self.completed.get(pos).copied().unwrap_or(Empty) < stage
        })
    }

    pub fn ready(&self, pos: ChunkPos, target: GenerationStage) -> bool {
        let Some(current) = self.completed(pos) else {
            return false;
        };
        if current.next() != Some(target)
            || self.required(pos).is_none_or(|required| target > required)
        {
            return false;
        }
        for (radius, &needed) in target.direct().iter().enumerate() {
            let r = radius as i32;
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) else {
                        return false;
                    };
                    if self
                        .completed(ChunkPos { x, z })
                        .is_none_or(|actual| actual < needed)
                    {
                        return false;
                    }
                }
            }
        }
        true
    }
    pub fn next_ready(&self) -> Option<(ChunkPos, GenerationStage)> {
        let &pos = self.ready.first()?;
        Some((pos, self.completed(pos)?.next()?))
    }

    fn refresh_ready(&mut self, pos: ChunkPos) {
        let next = self.completed(pos).and_then(GenerationStage::next);
        if next.is_some_and(|stage| self.ready(pos, stage)) {
            self.ready.insert(pos);
        } else {
            self.ready.remove(&pos);
        }
    }

    /// Record stages already restored from saved chunks, as one transaction.
    /// The caller must first restore and validate every piece of data required
    /// by each stage; a saved status alone is not evidence of usable inputs.
    /// This only updates the plan and does not import terrain or sidecars.
    pub fn restore_imported(
        &mut self,
        imported: impl IntoIterator<Item = (ChunkPos, GenerationStage)>,
    ) -> Result<()> {
        let mut changes = BTreeMap::new();
        for (pos, stage) in imported {
            let current = self
                .completed(pos)
                .context("imported chunk is not requested")?;
            ensure!(stage >= current, "imported chunk would regress its stage");
            ensure!(
                changes.insert(pos, (current, stage)).is_none(),
                "duplicate imported chunk"
            );
        }
        changes.retain(|_, (old, new)| old != new);
        if changes.is_empty() {
            return Ok(());
        }

        for (&pos, &(_, stage)) in &changes {
            self.completed.insert(pos, stage);
        }
        let radius = GenerationStage::ALL
            .iter()
            .map(|stage| stage.direct().len().saturating_sub(1))
            .max()
            .unwrap_or(0) as i32;
        let mut affected = BTreeSet::new();
        for (&pos, &(old, new)) in &changes {
            affected.insert(pos);
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) else {
                        continue;
                    };
                    let neighbor = ChunkPos { x, z };
                    let Some(next) = self.completed(neighbor).and_then(GenerationStage::next)
                    else {
                        continue;
                    };
                    let distance = dx.abs().max(dz.abs()) as usize;
                    if next
                        .direct()
                        .get(distance)
                        .is_some_and(|&needed| old < needed && needed <= new)
                    {
                        affected.insert(neighbor);
                    }
                }
            }
        }
        for pos in affected {
            self.refresh_ready(pos);
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    /// Call only after the corresponding operation succeeded. A failed or
    /// cancelled operation must not be recorded as complete.
    pub fn complete(&mut self, pos: ChunkPos, target: GenerationStage) -> Result<()> {
        ensure!(self.ready(pos, target), "generation step is not ready");
        self.completed.insert(pos, target);
        self.revision = self.revision.wrapping_add(1);
        self.refresh_ready(pos);
        // Only a next step depending on the stage just completed can have
        // become ready. Preserve lexicographic order in the ready set.
        let radius = GenerationStage::ALL
            .iter()
            .filter_map(|stage| stage.direct().iter().rposition(|&needed| needed == target))
            .max()
            .unwrap_or(0) as i32;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) else {
                    continue;
                };
                let neighbor = ChunkPos { x, z };
                let Some(next) = self.completed(neighbor).and_then(GenerationStage::next) else {
                    continue;
                };
                let distance = dx.abs().max(dz.abs()) as usize;
                if next.direct().get(distance) == Some(&target) {
                    self.refresh_ready(neighbor);
                }
            }
        }
        Ok(())
    }

    /// Generate and store a biome volume before marking BIOMES complete. The
    /// generator and neighborhood must use the same active packs and seed.
    /// On a generation error the caller must discard the generator/evaluator
    /// before retrying, because their search history may have advanced.
    pub fn run_biomes(
        &mut self,
        generator: &mut super::TerrainGenerator,
        evaluator: &mut native_resources::density::DensityEvaluator,
        neighborhood: &mut super::BiomeNeighborhood,
        pos: ChunkPos,
    ) -> Result<()> {
        ensure!(self.ready(pos, Biomes), "biome dependencies are incomplete");
        ensure!(
            neighborhood.seed == generator.seed
                && neighborhood.min_y == generator.min_y
                && neighborhood.height == generator.height,
            "biome neighborhood differs from generator"
        );
        ensure!(
            neighborhood.chunk([pos.x, pos.z]).is_none(),
            "biome chunk already exists"
        );
        let chunk = generator.generate_biomes([pos.x, pos.z], evaluator)?;
        neighborhood.insert(std::sync::Arc::new(chunk))?;
        self.complete(pos, Biomes)
    }

    /// Generate and assemble an intermediate terrain chunk with structures
    /// explicitly disabled. It is neither a full playable chunk nor safe to
    /// save. All assembled chunks must use the same generator and pack stack;
    /// matching seed, dimension and height cannot prove that. Discard mutable
    /// generator caches after an error before retrying to preserve their history.
    #[allow(clippy::too_many_arguments)]
    pub fn run_terrain<P: native_resources::packs::ResourceProvider>(
        &mut self,
        generator: &mut super::TerrainGenerator,
        provider: &mut P,
        evaluator: &mut native_resources::density::DensityEvaluator,
        cache: &mut native_resources::carver::schedule::CarverCache,
        neighborhood: &super::BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        assembled: &mut Option<super::CarvedTerrainWorld>,
        pos: ChunkPos,
        dimension: &str,
        structures_enabled: bool,
        classify: impl FnMut(&serde_json::Value) -> Result<super::CarverBlockState>,
        resolve: impl FnMut(&serde_json::Value) -> Result<native_world::loaded::BlockState>,
    ) -> Result<()> {
        ensure!(
            self.ready(pos, Terrain),
            "terrain dependencies are incomplete"
        );
        ensure!(
            !structures_enabled && !generator.structure_context,
            "structure terrain generation is not implemented by this path"
        );
        self.validate_terrain_inputs(generator, neighborhood, assembled.as_ref(), pos, dimension)?;
        let biome = neighborhood
            .chunk([pos.x, pos.z])
            .context("terrain biome chunk missing")?;
        let chunk = generator.generate_carved_surface(
            provider,
            evaluator,
            cache,
            [pos.x, pos.z],
            neighborhood,
            tags,
            classify,
        )?;
        let terrain = super::materialize_carved_terrain(chunk, biome, dimension, resolve)?;
        match assembled {
            Some(world) => world.insert(&terrain)?,
            None => *assembled = Some(super::CarvedTerrainWorld::new(terrain)),
        }
        self.complete(pos, Terrain)
    }

    /// Generate an intermediate terrain chunk using complete, precomputed
    /// structure references. The caller supplies Java's structure iteration
    /// order and matching generator, biome, block registry and pack inputs.
    /// This does not place structures or authorize a world save.
    #[allow(clippy::too_many_arguments)]
    pub fn run_terrain_with_structures<P: native_resources::packs::ResourceProvider>(
        &mut self,
        structures: &mut super::structure_batch::StructureGenerationBatch,
        provider: &mut P,
        cache: &mut native_resources::carver::schedule::CarverCache,
        neighborhood: &super::BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        assembled: &mut Option<super::CarvedTerrainWorld>,
        pos: ChunkPos,
        dimension: &str,
        order: &[super::structure_batch::TerrainStartKey],
        classify: impl FnMut(&serde_json::Value) -> Result<super::CarverBlockState>,
        resolve: impl FnMut(&serde_json::Value) -> Result<native_world::loaded::BlockState>,
    ) -> Result<()> {
        ensure!(
            self.ready(pos, Terrain),
            "terrain dependencies are incomplete"
        );
        self.validate_terrain_inputs(
            structures.generator(),
            neighborhood,
            assembled.as_ref(),
            pos,
            dimension,
        )?;
        let biome = neighborhood
            .chunk([pos.x, pos.z])
            .context("terrain biome chunk missing")?;
        let result = (|| {
            let chunk = structures.generate_carved_surface(
                provider,
                pos,
                neighborhood,
                tags,
                cache,
                order,
                classify,
            )?;
            let terrain = super::materialize_carved_terrain(chunk, biome, dimension, resolve)?;
            match assembled {
                Some(world) => world.insert(&terrain)?,
                None => *assembled = Some(super::CarvedTerrainWorld::new(terrain)),
            }
            self.complete(pos, Terrain)
        })();
        if result.is_err() {
            structures.invalidate();
        }
        result
    }

    /// Run the terrain stage with a context frozen from verified starts and
    /// references. This remains an intermediate, unsaveable terrain chunk.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_terrain_with_prepared_structure<
        P: native_resources::packs::ResourceProvider,
    >(
        &mut self,
        generator: &mut super::TerrainGenerator,
        provider: &mut P,
        evaluator: &mut native_resources::density::DensityEvaluator,
        cache: &mut native_resources::carver::schedule::CarverCache,
        neighborhood: &super::BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        assembled: &mut Option<super::CarvedTerrainWorld>,
        pos: ChunkPos,
        dimension: &str,
        structure: std::sync::Arc<native_core::beardifier::Beardifier>,
        classify: impl FnMut(&serde_json::Value) -> Result<super::CarverBlockState>,
        resolve: impl FnMut(&serde_json::Value) -> Result<native_world::loaded::BlockState>,
    ) -> Result<()> {
        ensure!(
            self.ready(pos, Terrain),
            "terrain dependencies are incomplete"
        );
        ensure!(
            generator.structure_context,
            "generator lacks structure context"
        );
        self.validate_terrain_inputs(generator, neighborhood, assembled.as_ref(), pos, dimension)?;
        let biome = neighborhood
            .chunk([pos.x, pos.z])
            .context("terrain biome chunk missing")?;
        let chunk = generator.generate_carved_surface_with_structure_context(
            provider,
            evaluator,
            cache,
            [pos.x, pos.z],
            neighborhood,
            tags,
            structure,
            classify,
        )?;
        let terrain = super::materialize_carved_terrain(chunk, biome, dimension, resolve)?;
        match assembled {
            Some(world) => world.insert(&terrain)?,
            None => *assembled = Some(super::CarvedTerrainWorld::new(terrain)),
        }
        self.complete(pos, Terrain)
    }

    fn validate_terrain_inputs(
        &self,
        generator: &super::TerrainGenerator,
        neighborhood: &super::BiomeNeighborhood,
        assembled: Option<&super::CarvedTerrainWorld>,
        pos: ChunkPos,
        dimension: &str,
    ) -> Result<()> {
        let dimension_id = native_resources::packs::ResourceId::parse(dimension)?;
        if let Some(world) = assembled {
            ensure!(
                world.world.dimension
                    == format!("{}:{}", dimension_id.namespace, dimension_id.path),
                "terrain dimension differs"
            );
            ensure!(
                world.seed == generator.seed
                    && world.min_y == generator.min_y
                    && world.height == generator.height,
                "terrain generator differs from assembled world"
            );
            ensure!(
                !world.postprocessing.contains_key(&pos) && !world.world.chunks.contains_key(&pos),
                "terrain chunk already exists"
            );
        }
        neighborhood.validate(
            [pos.x, pos.z],
            generator.seed,
            generator.min_y,
            generator.height,
        )
    }

    /// Apply supported decoration and record FEATURES only after it succeeds.
    pub fn run_features(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures_enabled: bool,
        fresh_light: Option<(
            &mut super::FreshGenerationLightTracker,
            &native_resources::block_registry::GeneratedBlockRegistry,
        )>,
    ) -> Result<Vec<bool>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let result = match fresh_light {
            Some((tracker, registry)) => prepared.execute_with_tracked_light(
                terrain,
                structures_enabled,
                tracker,
                registry,
            )?,
            None => prepared.execute(terrain, structures_enabled)?,
        };
        self.complete(center, Features)?;
        Ok(result)
    }

    /// Execute FEATURES using an already published staged-light revision.
    pub fn run_features_with_staged_light(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures_enabled: bool,
        engine: &mut super::StagedGenerationLightEngine,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<Vec<bool>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let result =
            prepared.execute_with_staged_light(terrain, structures_enabled, engine, registry)?;
        self.complete(center, Features)?;
        Ok(result)
    }

    /// Publish FEATURES only after the complete structure and feature pass succeeds.
    pub(super) fn run_features_with_structures(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        lighting: StructureFeatureLighting<'_>,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let (_, updated) = match lighting {
            StructureFeatureLighting::Visible(light) => {
                prepared.execute_structures_with_visible_light(terrain, structures, light)?
            }
            StructureFeatureLighting::Fresh(tracker, registry) => prepared
                .execute_structures_with_tracked_light(terrain, structures, tracker, registry)?,
            StructureFeatureLighting::Staged(engine, registry) => prepared
                .execute_structures_with_staged_light(terrain, structures, engine, registry)?,
        };
        self.complete(center, Features)?;
        Ok(updated)
    }

    /// Publish FEATURES only after the mixed structure pass succeeds.
    pub(super) fn run_features_with_ocean(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        jigsaws: Option<&super::ore::PreparedJigsawResources<'_>>,
        huts: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        runner: &mut dyn OceanFeatureRunner,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let updated = runner.execute(
            terrain,
            prepared,
            structures,
            jigsaws,
            huts,
            region_random,
            lighting,
            all_structures,
            biomes,
        )?;
        self.complete(center, Features)?;
        Ok(updated)
    }

    /// Complete FEATURES only after the hut-aware pass succeeds.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_features_with_huts(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        states: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        runner: &mut dyn HutFeatureRunner,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let updated = runner.execute(
            terrain,
            prepared,
            structures,
            states,
            random,
            lighting,
            all_structures,
            biomes,
        )?;
        self.complete(center, Features)?;
        Ok(updated)
    }

    /// Apply features against a complete, immutable visible light revision.
    pub fn run_features_with_visible_light(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures_enabled: bool,
        light: &native_world::generation_light::VisibleLightSnapshot,
    ) -> Result<Vec<bool>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let result = prepared.execute_with_visible_light(terrain, structures_enabled, light)?;
        self.complete(center, Features)?;
        Ok(result)
    }

    /// Calculate direct-sky sources and register sections before advancing the
    /// fresh INITIALIZE_LIGHT stage. No light propagation is performed here.
    pub fn run_light_initialization(
        &mut self,
        terrain: &super::CarvedTerrainWorld,
        center: ChunkPos,
        tracker: &mut super::FreshGenerationLightTracker,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<()> {
        ensure!(
            self.ready(center, InitializeLight),
            "light initialization dependencies are incomplete"
        );
        tracker.initialize_chunk(terrain, center, registry)?;
        self.complete(center, InitializeLight)
    }

    /// Advance LIGHT within a frozen, fully initialized generation batch.
    /// The returned snapshot does not write a chunk or make it saveable.
    pub fn run_light(
        &mut self,
        terrain: &super::CarvedTerrainWorld,
        center: ChunkPos,
        engine: &mut super::StagedGenerationLightEngine,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        ensure!(
            self.ready(center, Light),
            "light dependencies are incomplete"
        );
        let snapshot = engine.activate(terrain, center, [center], registry)?;
        self.complete(center, Light)?;
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruin_variant_lookup_checks_jigsaw_piece_bounds() -> Result<()> {
        use super::super::structure_references::collect_ordered_structure_references;
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_resources::{
            jigsaw_placement::Piece,
            packs::ResourceId,
            structure_pool::{HolderValue, LiquidSettings, PoolElement, Projection},
            structure_transform::Rotation,
        };
        let source = ChunkPos { x: 0, z: 0 };
        let id = ResourceId::parse("example:jigsaw")?;
        let piece = |min, max| Piece {
            element: PoolElement::Feature {
                feature: HolderValue::Reference(ResourceId::parse("example:tree").unwrap()),
                projection: Projection::Rigid,
            },
            origin: min,
            rotation: Rotation::None,
            bounds: BoundingBox::new(min, max).unwrap(),
            ground_level_delta: 1,
            junctions: vec![],
        };
        let start = super::super::jigsaw::PreparedStart {
            chunk: [0, 0],
            generation_point: [0, 0, 0],
            terrain_adjustment: TerrainAdjustment::None,
            liquid_settings: LiquidSettings::ApplyWaterlogging,
            pieces: vec![piece([2, 1, 2], [4, 5, 4]), piece([12, 1, 2], [14, 5, 4])],
        };
        let bounds = start.reference_bounds(id.clone())?;
        let mut stages = super::super::structure_batch::PreparedStructureStages::default();
        stages.starts.insert(source, BTreeMap::new());
        stages
            .jigsaw_starts
            .insert(source, BTreeMap::from([(id.clone(), start)]));
        stages.references.insert(
            source,
            collect_ordered_structure_references(source, |pos| {
                Ok(if pos == source {
                    vec![bounds.clone()]
                } else {
                    vec![]
                })
            })?,
        );
        let current = (ResourceId::parse("example:other")?, source);
        assert_eq!(
            ruin_structures_with_piece(&stages, [13, 3, 3], &current, &[], &[])?,
            BTreeSet::from([id])
        );
        assert!(ruin_structures_with_piece(&stages, [8, 3, 3], &current, &[], &[])?.is_empty());
        Ok(())
    }

    #[test]
    fn ruin_variant_structure_lookup_uses_current_and_revised_pieces() -> Result<()> {
        use super::super::structure_references::{
            StructureStartBounds, collect_ordered_structure_references,
        };
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_resources::{
            packs::ResourceId,
            structure_start::{SimpleStructureKind, SimpleStructureStart},
        };
        let source = ChunkPos { x: -1, z: -1 };
        let id = ResourceId::parse("example:ruin")?;
        let other = ResourceId::parse("example:hut")?;
        let make_start = |x| SimpleStructureStart {
            kind: SimpleStructureKind::SwampHut,
            chunk: [source.x, source.z],
            generation_point: [x, 64, -8],
            orientation: None,
            bounds: BoundingBox::new([x, 64, -9], [x + 2, 66, -7]).unwrap(),
            terrain_adjustment: TerrainAdjustment::BeardThin,
        };
        let first = make_start(-12);
        let second = make_start(-6);
        let references = [
            StructureStartBounds::from_simple(id.clone(), &first)?,
            StructureStartBounds::from_simple(other.clone(), &second)?,
        ];
        let mut stages = super::super::structure_batch::PreparedStructureStages::default();
        stages.starts.insert(
            source,
            [(id.clone(), first), (other.clone(), second)].into(),
        );
        stages.references.insert(
            source,
            collect_ordered_structure_references(source, |pos| {
                Ok(if pos == source {
                    references.to_vec()
                } else {
                    vec![]
                })
            })?,
        );
        let key = (id.clone(), source);
        let revised = vec![(
            (other.clone(), source),
            BoundingBox::new([-9, 70, -9], [-7, 72, -7]).unwrap(),
        )];
        let current = [
            BoundingBox::new([-11, 70, -9], [-9, 72, -7]).unwrap(),
            BoundingBox::new([-8, 70, -9], [-6, 72, -7]).unwrap(),
        ];
        assert_eq!(
            ruin_structures_with_piece(&stages, [-8, 71, -8], &key, &revised, &current)?,
            BTreeSet::from([id.clone(), other])
        );
        assert!(
            ruin_structures_with_piece(&stages, [-11, 64, -8], &key, &revised, &current)?
                .is_empty()
        );
        assert_eq!(
            ruin_structures_with_piece(&stages, [-10, 70, -8], &key, &revised, &current)?,
            BTreeSet::from([id])
        );
        assert!(ruin_structures_with_piece(&stages, [-10, 70, -8], &key, &revised, &[]).is_err());
        {
            use super::super::ocean_ruin::{
                DrownedGenerationContext, DrownedMarkerContext, DrownedSpawnEffects,
                GenerationDifficultyInputs,
            };
            use native_persistence::nbt::Tag;
            use native_resources::{
                animal_variants::SpawnVariantEnvironment,
                drowned_spawn::{DrownedBiomeTags, MainHandRoll},
                packs::ResourceProvider,
                worldgen_region_random::RegionRandom,
            };
            use native_world::loaded::World;
            use std::sync::Arc;
            struct Data;
            impl ResourceProvider for Data {
                fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                    Ok(None)
                }
                fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                    Ok((path
                        == "data/minecraft/tags/worldgen/biome/more_frequent_drowned_spawns.json")
                        .then(|| br#"{"values":[]}"#.to_vec()))
                }
                fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                    Ok(if folder == "worldgen/biome" {
                        vec![ResourceId::parse("minecraft:ocean")?]
                    } else {
                        vec![]
                    })
                }
            }
            #[derive(Clone, Default)]
            struct Effects {
                seen: usize,
            }
            impl DrownedSpawnEffects for Effects {
                fn try_existing_chicken(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
                    Ok(false)
                }
                fn spawn_chicken_jockey(
                    &mut self,
                    _: [i32; 3],
                    _: &World,
                    _: &mut RegionRandom,
                    _: &super::super::BiomeNeighborhood,
                ) -> Result<bool> {
                    anyhow::bail!("missing contextual chicken path")
                }
                fn spawn_chicken_jockey_in_environment<V: SpawnVariantEnvironment>(
                    &mut self,
                    _: [i32; 3],
                    _: &World,
                    _: &mut RegionRandom,
                    _: &super::super::BiomeNeighborhood,
                    env: &mut V,
                ) -> Result<bool> {
                    assert_eq!(env.biome()?, &ResourceId::parse("minecraft:ocean")?);
                    assert_eq!(env.moon_phase()?, 3);
                    assert_eq!(
                        env.structures_with_piece()?,
                        &BTreeSet::from([
                            ResourceId::parse("example:ruin")?,
                            ResourceId::parse("example:hut")?
                        ])
                    );
                    self.seen += 1;
                    Ok(true)
                }
                fn spawn_zombie_nautilus(
                    &mut self,
                    _: [i32; 3],
                    _: &World,
                    _: &mut RegionRandom,
                    _: bool,
                ) -> Result<bool> {
                    anyhow::bail!("missing contextual nautilus path")
                }
                fn spawn_zombie_nautilus_in_environment<V: SpawnVariantEnvironment>(
                    &mut self,
                    pos: [i32; 3],
                    world: &World,
                    random: &mut RegionRandom,
                    _: bool,
                    biomes: &super::super::BiomeNeighborhood,
                    env: &mut V,
                ) -> Result<bool> {
                    self.spawn_chicken_jockey_in_environment(pos, world, random, biomes, env)
                }
                fn enchant_weapon(
                    &mut self,
                    _: [i32; 3],
                    _: &World,
                    _: &mut RegionRandom,
                    _: MainHandRoll,
                    _: f32,
                ) -> Result<Tag> {
                    anyhow::bail!("unused")
                }
            }
            let tags = DrownedBiomeTags::load(&mut Data)?;
            let mut biomes = super::super::BiomeNeighborhood::new(42, 64, 16)?;
            for z in -2..=0 {
                for x in -2..=0 {
                    biomes.insert(Arc::new(super::super::QuartBiomeChunk::from_values(
                        [x, z],
                        42,
                        64,
                        16,
                        vec![ResourceId::parse("minecraft:ocean")?; 64],
                    )?))?;
                }
            }
            let mut context = ContextualDrowned {
                base: DrownedGenerationContext {
                    biomes: &biomes,
                    biome_tags: &tags,
                    difficulty_at: |_, _: &World, _: &super::super::BiomeNeighborhood| {
                        Ok(GenerationDifficultyInputs {
                            difficulty: native_persistence::world::Difficulty::Normal,
                            overworld_clock_time: 0,
                            moon_brightness: 1.0,
                        })
                    },
                    halloween_at: || Ok(false),
                    effects: Effects::default(),
                },
                structures: &stages,
                moon: &|_, _| Ok(3),
                key: None,
                revisions: vec![],
                piece_index: None,
                pieces: vec![],
            };
            context.begin_ruin_start(&key, &revised)?;
            let mut before_alignment = current;
            before_alignment[1] = BoundingBox::new([-8, 64, -9], [-6, 66, -7]).unwrap();
            context.begin_ruin_piece(1, &before_alignment)?;
            context.set_aligned_ruin_piece(current[1])?;
            let world = World::new("minecraft:overworld");
            let mut random = RegionRandom::Legacy(native_core::random::LegacyRandom::new(42));
            assert!(context.spawn_chicken_jockey([-8, 71, -8], &world, &mut random)?);
            assert!(context.spawn_zombie_nautilus([-8, 71, -8], &world, &mut random, true)?);
            assert_eq!(context.base.effects.seen, 2);
        }
        stages.starts.clear();
        assert!(
            ruin_structures_with_piece(&stages, [-10, 70, -8], &key, &revised, &current).is_err()
        );
        Ok(())
    }

    fn scan_next(plan: &GenerationSchedule) -> Option<(ChunkPos, GenerationStage)> {
        plan.required.iter().find_map(|(&pos, &required)| {
            let next = plan.completed(pos)?.next()?;
            (next <= required && plan.ready(pos, next)).then_some((pos, next))
        })
    }

    #[test]
    fn imported_stages_refresh_ready_index_and_reject_partial_updates() {
        let mut plan = GenerationSchedule::new();
        plan.request(ChunkPos { x: 0, z: 0 }, Full).unwrap();
        let positions: Vec<_> = plan.required.keys().copied().collect();
        for batch in positions.chunks(11) {
            let imported = batch
                .iter()
                .copied()
                .enumerate()
                .map(|(i, pos)| (pos, if i % 3 == 0 { Full } else { StructureStarts }));
            plan.restore_imported(imported).unwrap();
            let expected: BTreeSet<_> = plan
                .required
                .keys()
                .copied()
                .filter(|&pos| {
                    plan.completed(pos)
                        .and_then(GenerationStage::next)
                        .is_some_and(|stage| plan.ready(pos, stage))
                })
                .collect();
            assert_eq!(plan.ready, expected);
            assert_eq!(plan.next_ready(), scan_next(&plan));
        }
        let before_completed = plan.completed.clone();
        let before_ready = plan.ready.clone();
        let before_revision = plan.revision;
        let pos = positions[0];
        assert!(plan.restore_imported([(pos, Full), (pos, Full)]).is_err());
        assert!(
            plan.restore_imported([(pos, Full), (ChunkPos { x: 1000, z: 1000 }, Full)])
                .is_err()
        );
        assert!(plan.restore_imported([(pos, Empty)]).is_err());
        assert_eq!(plan.completed, before_completed);
        assert_eq!(plan.ready, before_ready);
        assert_eq!(plan.revision, before_revision);
        while let Some((pos, stage)) = scan_next(&plan) {
            assert_eq!(plan.next_ready(), Some((pos, stage)));
            plan.complete(pos, stage).unwrap();
        }
        assert!(plan.is_complete());
        for stage in GenerationStage::ALL {
            assert_eq!(GenerationStage::from_id(stage.id()), Some(stage));
        }
        assert_eq!(GenerationStage::from_id("other:full"), None);
        assert_eq!(GenerationStage::from_id("minecraft:unknown"), None);

        let edge = ChunkPos {
            x: i32::MAX,
            z: i32::MIN,
        };
        let mut edge_plan = GenerationSchedule::new();
        edge_plan.request(edge, StructureStarts).unwrap();
        edge_plan.restore_imported([(edge, Full)]).unwrap();
        assert!(edge_plan.is_complete());
        assert_eq!(edge_plan.next_ready(), None);
    }

    #[test]
    fn ready_index_matches_full_scan_with_interleaved_requests() {
        // The index relies on new, uncompleted neighbors not satisfying a
        // direct dependency. Guard that property when stages are changed.
        for stage in GenerationStage::ALL {
            assert!(stage.direct().iter().skip(1).all(|&needed| needed > Empty));
        }
        let mut plan = GenerationSchedule::new();
        let requests = [
            (ChunkPos { x: 0, z: 0 }, Features),
            (ChunkPos { x: -2, z: 1 }, Terrain),
            (ChunkPos { x: 0, z: 0 }, InitializeLight),
            (ChunkPos { x: 3, z: -2 }, Light),
            (ChunkPos { x: -2, z: 1 }, Full),
            (
                ChunkPos {
                    x: i32::MAX,
                    z: i32::MIN,
                },
                StructureStarts,
            ),
        ];
        for step in 0..1800 {
            if let Some(&(pos, target)) = requests.get(step / 80).filter(|_| step % 80 == 0) {
                plan.request(pos, target).unwrap();
            }
            let expected = scan_next(&plan);
            assert_eq!(plan.next_ready(), expected, "step {step}");
            if step % 31 == 0 {
                let all: BTreeSet<_> = plan
                    .required
                    .keys()
                    .copied()
                    .filter(|&pos| {
                        plan.completed(pos)
                            .and_then(GenerationStage::next)
                            .is_some_and(|stage| plan.ready(pos, stage))
                    })
                    .collect();
                assert_eq!(plan.ready, all, "ready set at step {step}");
            }
            let Some((mut pos, mut stage)) = expected else {
                break;
            };
            // Callers may complete any ready step, not just next_ready().
            if step % 7 == 0 {
                pos = *plan.ready.last().unwrap();
                stage = plan.completed(pos).unwrap().next().unwrap();
            }
            plan.complete(pos, stage).unwrap();
        }
        assert_eq!(
            plan.required(ChunkPos {
                x: i32::MAX,
                z: i32::MIN,
            }),
            Some(StructureStarts)
        );
        while let Some((pos, stage)) = scan_next(&plan) {
            assert_eq!(plan.next_ready(), Some((pos, stage)));
            plan.complete(pos, stage).unwrap();
        }
        assert_eq!(plan.next_ready(), None);
        assert!(plan.is_complete());
    }
    #[test]
    fn requirements_match_accumulated_radii_for_every_stage() {
        let center = ChunkPos { x: -3, z: 4 };
        for target in GenerationStage::ALL {
            let mut schedule = GenerationSchedule::new();
            schedule.request(center, target).unwrap();
            for dz in -14i32..=14 {
                for dx in -14i32..=14 {
                    let d = dx.abs().max(dz.abs()) as usize;
                    let expected = if d == 0 {
                        Some(target)
                    } else {
                        target.accumulated().get(d).copied()
                    };
                    assert_eq!(
                        schedule.required(ChunkPos {
                            x: center.x + dx,
                            z: center.z + dz
                        }),
                        expected,
                        "{:?} {dx},{dz}",
                        target
                    );
                }
            }
        }
    }
    #[test]
    fn overlapping_requests_are_batched_without_partial_updates() {
        let requests = [
            (ChunkPos { x: -2, z: 1 }, Features),
            (ChunkPos { x: 0, z: 0 }, InitializeLight),
            (ChunkPos { x: -2, z: 1 }, Terrain),
            (ChunkPos { x: 3, z: -1 }, StructureReferences),
        ];
        let mut sequential = GenerationSchedule::new();
        for (pos, stage) in requests {
            sequential.request(pos, stage).unwrap();
        }
        let mut combined = GenerationSchedule::new();
        combined.request_many(requests).unwrap();
        assert_eq!(combined.required, sequential.required);
        assert_eq!(combined.ready, sequential.ready);
        let before = combined.required.clone();
        let ready = combined.ready.clone();
        let revision = combined.revision;
        assert!(
            combined
                .request_many([
                    (ChunkPos { x: 100, z: 100 }, Features),
                    (ChunkPos { x: i32::MAX, z: 0 }, Features),
                ])
                .is_err()
        );
        assert_eq!(combined.required, before);
        assert_eq!(combined.ready, ready);
        assert_eq!(combined.revision, revision);
        combined.request_many([]).unwrap();
        assert_eq!(combined.revision, revision);
    }
    #[test]
    fn steps_require_neighbor_completion_and_requests_are_transactional() {
        let center = ChunkPos { x: 0, z: 0 };
        let mut plan = GenerationSchedule::new();
        plan.request(center, Features).unwrap();
        assert!(!plan.ready(center, Features));
        assert!(plan.complete(center, Features).is_err());
        let total = plan.requested_chunks();
        assert!(
            plan.request(ChunkPos { x: i32::MAX, z: 0 }, Features)
                .is_err()
        );
        assert_eq!(plan.requested_chunks(), total);
        let mut count = 0usize;
        while !plan.ready(center, Features) {
            let (pos, stage) = plan.next_ready().expect("dependency graph must progress");
            assert_ne!((pos, stage), (center, Features));
            plan.complete(pos, stage).unwrap();
            count += 1;
            assert!(count < 10_000);
        }
        plan.complete(center, Features).unwrap();
        assert_eq!(plan.completed(center), Some(Features));
        assert!(plan.complete(center, Features).is_err());
        plan.request(center, InitializeLight).unwrap();
        assert!(plan.ready(center, InitializeLight));
        plan.complete(center, InitializeLight).unwrap();
    }
}
