//! Apply supported decoration features to intermediate generated chunks.
//!
//! The surrounding decoration scheduler must still supply Java's feature order,
//! random seed, writable region and complete CARVERS-stage neighborhood.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use native_resources::{
    block_column_feature::{BlockColumnFeature, ColumnWorld},
    disk_feature::{BlockPredicate, DiskFeature, DiskWorld},
    feature_order::{FeatureHolderIndex, FeatureKey, FeatureSchedule, FeatureTags},
    heightmap_tags::HeightmapTags,
    lake_feature::{LakeFeature, LakeWorld},
    ore_feature::{ConfiguredOre, OreBlockTags, OreVein, OreVeinWorld, OreWorld},
    packs::{ResourceId, ResourceProvider},
    placed_feature::{PlacedFeature, PlacementEnvironment},
    simple_block_feature::{SimpleBlockFeature, SimpleBlockWorld},
    spring_feature::{SpringFeature, SpringWorld},
    underwater_magma::{MagmaWorld, UnderwaterMagma, vanilla_full_occlusion},
};
use native_world::loaded::{
    BlockState, BlockStateId, ChunkPos, SavedBlockTick, SavedFluidTick, World,
};
use serde_json::Value;

use super::{CarvedTerrainWorld, DecorationPlan};

/// An unpublished generation attempt. Dropping it, or any failed placement,
/// leaves the original terrain and its snapshots untouched.
pub struct DecorationRegion<'a> {
    target: &'a mut CarvedTerrainWorld,
    world: World,
    tags: &'a OreBlockTags,
    motion: &'a HeightmapTags,
    writable: BTreeSet<ChunkPos>,
    heights: BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    surface_heights: BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    final_heights: BTreeMap<ChunkPos, Arc<FinalHeightmaps>>,
    feature_marks: BTreeMap<ChunkPos, Arc<native_core::postprocessing::PostProcessing>>,
    block_tick_keys: BTreeMap<ChunkPos, BTreeSet<([i32; 3], String)>>,
    fluid_tick_keys: BTreeMap<ChunkPos, BTreeSet<([i32; 3], String)>>,
    sugar_cane_fluids: Option<&'a BTreeSet<String>>,
    cactus_lava_fluids: Option<&'a BTreeSet<String>>,
    lake_water_fluids: Option<&'a BTreeSet<String>>,
    motion_cache: BTreeMap<BlockStateId, bool>,
    failed: bool,
}

/// Final 26.3 terrain heightmaps in surface, ocean, motion and no-leaves order.
#[derive(Clone)]
pub(super) struct FinalHeightmaps {
    values: Box<[[i32; 256]; 4]>,
}

/// A placed feature bound to the exact configured feature it references.
pub struct PlacedDecorationFeature {
    placement: PlacedFeature,
    feature: ConfiguredDecoration,
}

enum ConfiguredDecoration {
    Ore(ConfiguredOre<BlockState>),
    Disk(DiskFeature<BlockState>),
    Simple(SimpleBlockFeature<BlockState>),
    Spring(SpringFeature<BlockState>),
    Magma(UnderwaterMagma<BlockState>),
    Column(BlockColumnFeature<BlockState>),
    Lake(LakeFeature<BlockState>),
    Selector(Vec<PlacedDecorationFeature>),
    WeightedSelector(
        Vec<(f32, PlacedDecorationFeature)>,
        Box<PlacedDecorationFeature>,
    ),
}
#[derive(Default)]
struct FeatureLoadState {
    active: BTreeSet<String>,
    nodes: usize,
    tags: FeatureTags,
    registry: Option<BTreeSet<ResourceId>>,
}
impl ConfiguredDecoration {
    fn required_blocks(&self) -> BTreeSet<ResourceId> {
        match self {
            Self::Disk(v) => v.required_blocks(),
            Self::Simple(v) => v.required_blocks(),
            Self::Spring(v) => v.required_blocks(),
            Self::Column(v) => v.required_blocks(),
            Self::Lake(v) => v.required_blocks(),
            Self::Selector(values) => values
                .iter()
                .flat_map(PlacedDecorationFeature::required_blocks)
                .collect(),
            Self::WeightedSelector(values, default) => {
                let mut blocks = default.required_blocks();
                for (_, feature) in values {
                    blocks.extend(feature.required_blocks());
                }
                blocks
            }
            _ => BTreeSet::new(),
        }
    }
    fn needs_survival(&self, name: &str) -> bool {
        match self {
            Self::Disk(v) => v.needs_survival(name),
            Self::Simple(v) => v.needs_survival(name),
            Self::Column(v) => v.needs_survival(name),
            Self::Lake(v) => v.needs_survival(name),
            Self::Selector(values) => values.iter().any(|v| v.needs_survival(name)),
            Self::WeightedSelector(values, default) => {
                default.needs_survival(name) || values.iter().any(|(_, v)| v.needs_survival(name))
            }
            _ => false,
        }
    }
    fn needs_lake(&self) -> bool {
        match self {
            Self::Lake(_) => true,
            Self::Selector(values) => values.iter().any(PlacedDecorationFeature::needs_lake),
            Self::WeightedSelector(values, default) => {
                default.needs_lake() || values.iter().any(|(_, v)| v.needs_lake())
            }
            _ => false,
        }
    }
    fn decode(
        provider: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        state: &mut FeatureLoadState,
        depth: usize,
    ) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing feature type")?,
        )?;
        match (id.namespace.as_str(), id.path.as_str()) {
            ("minecraft", "ore" | "scattered_ore") => {
                Ok(Self::Ore(ConfiguredOre::decode(value, resolve)?))
            }
            ("minecraft", "disk") => Ok(Self::Disk(DiskFeature::decode(value, resolve)?)),
            ("minecraft", "simple_block") => {
                Ok(Self::Simple(SimpleBlockFeature::decode(value, resolve)?))
            }
            ("minecraft", "spring_feature") => {
                Ok(Self::Spring(SpringFeature::decode(value, resolve)?))
            }
            ("minecraft", "underwater_magma") => {
                Ok(Self::Magma(UnderwaterMagma::decode(value, resolve)?))
            }
            ("minecraft", "block_column") => {
                Ok(Self::Column(BlockColumnFeature::decode(value, resolve)?))
            }
            ("minecraft", "lake") => Ok(Self::Lake(LakeFeature::decode(value, resolve)?)),
            ("minecraft", "simple_random_selector") => {
                let features = value.get("features").context("missing selector features")?;
                let entries = PlacedDecorationFeature::load_holder_set(
                    provider,
                    features,
                    resolve,
                    state,
                    depth + 1,
                )?;
                ensure!(!entries.is_empty(), "empty selector feature set");
                Ok(Self::Selector(entries))
            }
            ("minecraft", "random_selector") => {
                let entries = value
                    .get("features")
                    .and_then(Value::as_array)
                    .context("missing weighted selector features")?;
                ensure!(entries.len() <= 4096, "too many weighted selector entries");
                let mut features = Vec::with_capacity(entries.len());
                for entry in entries {
                    let chance = entry
                        .get("chance")
                        .and_then(Value::as_f64)
                        .context("invalid selector chance")?
                        as f32;
                    ensure!(
                        chance.is_finite() && (0.0..=1.0).contains(&chance),
                        "invalid selector chance"
                    );
                    let selected = PlacedDecorationFeature::load_nested(
                        provider,
                        entry
                            .get("feature")
                            .context("missing weighted selector feature")?,
                        resolve,
                        state,
                        depth + 1,
                    )?;
                    features.push((chance, selected));
                }
                let default = PlacedDecorationFeature::load_nested(
                    provider,
                    value.get("default").context("missing selector default")?,
                    resolve,
                    state,
                    depth + 1,
                )?;
                Ok(Self::WeightedSelector(features, Box::new(default)))
            }
            _ => bail!(
                "unsupported decoration feature {}:{}",
                id.namespace,
                id.path
            ),
        }
    }
    fn required_tags(&self) -> BTreeSet<ResourceId> {
        match self {
            Self::Ore(v) => v.required_tags(),
            Self::Disk(v) => v.required_tags(),
            Self::Spring(v) => v.required_tags(),
            Self::Magma(_) => BTreeSet::new(),
            Self::Column(v) => v.required_tags(),
            Self::Lake(v) => v.required_tags(),
            Self::Selector(values) => values
                .iter()
                .flat_map(PlacedDecorationFeature::required_tags)
                .collect(),
            Self::WeightedSelector(values, default) => {
                let mut tags = default.required_tags();
                for (_, feature) in values {
                    tags.extend(feature.required_tags());
                }
                tags
            }
            Self::Simple(v) => {
                let mut tags = v.required_tags();
                for state in v.possible_states() {
                    let tag = match simple_plant_kind(state) {
                        Ok(SimplePlantKind::Vegetation | SimplePlantKind::Double) => {
                            Some("minecraft:supports_vegetation")
                        }
                        Ok(SimplePlantKind::DryVegetation) => {
                            Some("minecraft:supports_dry_vegetation")
                        }
                        _ => None,
                    };
                    if let Some(tag) = tag {
                        tags.insert(ResourceId::parse(tag).expect("constant tag ID"));
                    }
                }
                tags
            }
        }
    }
    fn place(
        &self,
        world: &mut DecorationRegion<'_>,
        random: &mut DecorationRandom,
        pos: [i32; 3],
        generator_min_y: i32,
        generator_height: i32,
    ) -> Result<bool> {
        match self {
            Self::Ore(v) => v.place(world, random, pos),
            Self::Disk(v) => v.place(world, random, pos),
            Self::Simple(v) => v.place(world, random, pos),
            Self::Spring(v) => v.place(world, pos),
            Self::Magma(v) => v.place(world, random, pos),
            Self::Column(v) => v.place(world, random, pos),
            Self::Lake(v) => v.place(world, random, pos),
            Self::Selector(values) => {
                let index = random
                    .next_int_bound(i32::try_from(values.len())?)
                    .context("empty selector feature set")? as usize;
                world.place_nested(
                    &values[index],
                    random,
                    pos,
                    generator_min_y,
                    generator_height,
                )
            }
            Self::WeightedSelector(values, default) => {
                for (chance, feature) in values {
                    if random.next_float() < *chance {
                        return world.place_nested(
                            feature,
                            random,
                            pos,
                            generator_min_y,
                            generator_height,
                        );
                    }
                }
                world.place_nested(default, random, pos, generator_min_y, generator_height)
            }
        }
    }
}

/// A placed feature resolved from one exact entry of the decoration schedule.
pub struct ScheduledDecorationFeature<'a> {
    key: FeatureKey,
    placed: PlacedDecorationFeature,
    schedule: &'a FeatureSchedule,
    independent_modifier: bool,
    holder_index: Option<&'a FeatureHolderIndex>,
}

/// A fully resolved chunk decoration attempt. Unsupported selected
/// features are rejected before any generated terrain is changed.
pub struct PreparedDecoration<'p, 's> {
    plan: &'p DecorationPlan<'s>,
    features: BTreeMap<FeatureKey, ScheduledDecorationFeature<'s>>,
    tags: OreBlockTags,
    motion: HeightmapTags,
    sugar_cane_fluids: Option<BTreeSet<String>>,
    cactus_lava_fluids: Option<BTreeSet<String>>,
    lake_water_fluids: Option<BTreeSet<String>>,
}

impl<'p, 's> PreparedDecoration<'p, 's> {
    /// The provider, complete block registry and resolver must describe the
    /// same immutable data-pack stack as the plan and generated terrain.
    pub fn load(
        provider: &mut impl ResourceProvider,
        plan: &'p DecorationPlan<'s>,
        block_registry: &BTreeSet<String>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Self> {
        let mut features = BTreeMap::new();
        let mut required = BTreeSet::new();
        let mut needs_sugar_cane = false;
        let mut needs_cactus = false;
        let mut needs_lake = false;
        for selected in &plan.features {
            let step = usize::try_from(selected.step).context("invalid feature step")?;
            let index = usize::try_from(selected.index).context("invalid feature index")?;
            let canonical = plan
                .schedule
                .steps()
                .get(step)
                .and_then(|v| v.get(index))
                .context("feature absent from decoration schedule")?;
            ensure!(
                std::ptr::eq(canonical, selected.feature),
                "plan refers to a different feature schedule"
            );
            if !features.contains_key(selected.feature) {
                let feature = ScheduledDecorationFeature::load(
                    provider,
                    plan.schedule,
                    selected.feature,
                    resolve,
                )
                .with_context(|| format!("unsupported planned feature {:?}", selected.feature))?;
                let blocks = feature.placed.required_blocks();
                needs_sugar_cane |= feature.placed.needs_survival("sugar_cane");
                needs_cactus |= feature.placed.needs_survival("cactus");
                needs_lake |= feature.placed.needs_lake();
                for id in blocks {
                    ensure!(
                        block_registry.contains(&format!("{}:{}", id.namespace, id.path)),
                        "feature predicate references an unregistered block {}:{}",
                        id.namespace,
                        id.path
                    );
                }
                required.extend(feature.required_tags());
                features.insert(selected.feature.clone(), feature);
            }
        }
        let tags = OreBlockTags::load(provider, block_registry, &required)?;
        let motion = HeightmapTags::load(provider, block_registry)?;
        let sugar_cane_fluids = needs_sugar_cane
            .then(|| native_resources::fluid_tags::load_sugar_cane_adjacent(provider))
            .transpose()?;
        let cactus_lava_fluids = needs_cactus
            .then(|| native_resources::fluid_tags::load_lava(provider))
            .transpose()?;
        let lake_water_fluids = needs_lake
            .then(|| native_resources::fluid_tags::load_water(provider))
            .transpose()?;
        Ok(Self {
            plan,
            features,
            tags,
            motion,
            sugar_cane_fluids,
            cactus_lava_fluids,
            lake_water_fluids,
        })
    }

    /// `structures_enabled` must be read from this world's generation settings.
    /// Until structure placement exists it is an error to enable it. Results
    /// match the selected feature order, and a failure publishes no changes.
    pub fn execute(
        &self,
        terrain: &mut CarvedTerrainWorld,
        structures_enabled: bool,
    ) -> Result<Vec<bool>> {
        ensure!(
            !structures_enabled,
            "structure decoration is not implemented"
        );
        ensure!(
            terrain.seed == self.plan.biomes.seed
                && terrain.min_y == self.plan.biomes.min_y
                && terrain.height == self.plan.biomes.height,
            "plan and terrain differ"
        );
        let center = ChunkPos {
            x: self.plan.origin[0].div_euclid(16),
            z: self.plan.origin[2].div_euclid(16),
        };
        ensure!(
            !terrain.completed_decoration.contains(&center),
            "chunk decoration already completed"
        );
        let mut writable = BTreeSet::new();
        // The 26.3 FEATURES generation step has write radius one.
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                writable.insert(ChunkPos {
                    x: center
                        .x
                        .checked_add(dx)
                        .context("decoration chunk X overflow")?,
                    z: center
                        .z
                        .checked_add(dz)
                        .context("decoration chunk Z overflow")?,
                });
            }
        }
        let mut region = terrain.decoration_region(&self.tags, &self.motion, writable)?;
        region.sugar_cane_fluids = self.sugar_cane_fluids.as_ref();
        region.cactus_lava_fluids = self.cactus_lava_fluids.as_ref();
        region.lake_water_fluids = self.lake_water_fluids.as_ref();
        let mut results = Vec::with_capacity(self.plan.features.len());
        for (index, selected) in self.plan.features.iter().enumerate() {
            let ore = self
                .features
                .get(selected.feature)
                .context("prepared ore is missing")?;
            results.push(region.place_planned_auto(self.plan, index, ore)?);
        }
        region.finish()?;
        terrain.completed_decoration.insert(center);
        Ok(results)
    }
}

impl<'a> ScheduledDecorationFeature<'a> {
    /// Read both named and inline placed features from the same active packs
    /// and block registry that were used to prepare the schedule.
    pub fn load(
        provider: &mut impl ResourceProvider,
        schedule: &'a FeatureSchedule,
        key: &FeatureKey,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Self> {
        ensure!(
            schedule.steps().iter().flatten().any(|item| item == key),
            "feature is absent from the schedule"
        );
        let placed = match key {
            FeatureKey::Registered(id) => PlacedDecorationFeature::load(provider, id, resolve)?,
            FeatureKey::Inline { biome, .. } => {
                let definition = schedule
                    .biome(biome)
                    .context("inline feature biome is missing")?
                    .inline
                    .get(key)
                    .context("inline feature definition is missing")?;
                PlacedDecorationFeature::from_placement(
                    provider,
                    PlacedFeature::decode(definition)?,
                    resolve,
                )?
            }
        };
        let independent_modifier = placed.placement.has_independent_modifier();
        let holder_index = (!independent_modifier).then(|| schedule.feature_holder_index(provider));
        Ok(Self {
            key: key.clone(),
            placed,
            schedule,
            independent_modifier,
            holder_index,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        self.placed.required_tags()
    }

    /// Java `BiomeGenerationSettings.hasFeature` tests value equality. Exact
    /// schedule identity is always equal; independently decoded height or
    /// block predicate objects can prove other placed features unequal.
    pub fn biome_allows(&self, biome: &ResourceId) -> Result<bool> {
        let settings = self
            .schedule
            .biome(biome)
            .context("biome is absent from the feature schedule")?;
        if self.schedule.biome_contains_identity(biome, &self.key)? {
            return Ok(true);
        }
        if self.independent_modifier || settings.steps.iter().all(Vec::is_empty) {
            return Ok(false);
        }
        if let Some(index) = self.holder_index {
            if index.excludes(biome, &self.placed.placement.feature)? {
                return Ok(false);
            }
        }
        bail!("cannot determine Java feature equality for this biome")
    }
}

impl PlacedDecorationFeature {
    pub fn load(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Self> {
        let placement = PlacedFeature::load(provider, id)?;
        Self::from_placement(provider, placement, resolve)
    }

    pub fn from_placement(
        provider: &mut impl ResourceProvider,
        placement: PlacedFeature,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Self> {
        Self::from_placement_inner(
            provider,
            placement,
            resolve,
            &mut FeatureLoadState::default(),
            0,
        )
    }
    fn from_placement_inner(
        provider: &mut impl ResourceProvider,
        placement: PlacedFeature,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        state: &mut FeatureLoadState,
        depth: usize,
    ) -> Result<Self> {
        ensure!(
            depth < 64 && state.nodes < 4096,
            "feature nesting or size limit exceeded"
        );
        state.nodes += 1;
        let feature = if let Some(id) = placement.feature.as_str() {
            let id = ResourceId::parse(id)?;
            let path = format!("data/{}/worldgen/feature/{}.json", id.namespace, id.path);
            ensure!(
                state.active.insert(path.clone()),
                "cyclic configured feature reference"
            );
            let result = (|| {
                let data = provider
                    .read_data(&path)?
                    .with_context(|| format!("missing feature {path}"))?;
                let value = serde_json::from_slice(&data)
                    .with_context(|| format!("invalid feature {path}"))?;
                ConfiguredDecoration::decode(provider, &value, resolve, state, depth + 1)
            })();
            state.active.remove(&path);
            result?
        } else {
            ConfiguredDecoration::decode(provider, &placement.feature, resolve, state, depth + 1)?
        };
        Ok(Self { placement, feature })
    }

    fn load_nested(
        provider: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        state: &mut FeatureLoadState,
        depth: usize,
    ) -> Result<Self> {
        if let Some(name) = value.as_str() {
            ensure!(!name.starts_with('#'), "tag is not a single placed feature");
            let id = ResourceId::parse(name)?;
            let path = format!(
                "data/{}/worldgen/placed_feature/{}.json",
                id.namespace, id.path
            );
            ensure!(
                state.active.insert(path.clone()),
                "cyclic placed feature reference"
            );
            let result = (|| {
                let placement = PlacedFeature::load(provider, &id)?;
                Self::from_placement_inner(provider, placement, resolve, state, depth + 1)
            })();
            state.active.remove(&path);
            result
        } else {
            Self::from_placement_inner(
                provider,
                PlacedFeature::decode(value)?,
                resolve,
                state,
                depth + 1,
            )
        }
    }
    fn load_holder_set(
        provider: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        state: &mut FeatureLoadState,
        depth: usize,
    ) -> Result<Vec<Self>> {
        if let Some(tag) = value.as_str().and_then(|s| s.strip_prefix('#')) {
            let id = ResourceId::parse(tag)?;
            if state.registry.is_none() {
                state.registry = Some(
                    provider
                        .list_data("worldgen/placed_feature")?
                        .into_iter()
                        .collect(),
                );
            }
            let members = state
                .tags
                .resolve_tag(provider, state.registry.as_ref().unwrap(), &id)?
                .context("selector placed-feature tag missing")?;
            return members
                .iter()
                .map(|id| {
                    Self::load_nested(
                        provider,
                        &Value::String(format!("{}:{}", id.namespace, id.path)),
                        resolve,
                        state,
                        depth + 1,
                    )
                })
                .collect();
        }
        if let Some(values) = value.as_array() {
            ensure!(values.len() <= 4096, "too many selector features");
            values
                .iter()
                .map(|entry| Self::load_nested(provider, entry, resolve, state, depth + 1))
                .collect()
        } else {
            Ok(vec![Self::load_nested(
                provider,
                value,
                resolve,
                state,
                depth + 1,
            )?])
        }
    }

    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = self.feature.required_tags();
        tags.extend(self.placement.required_tags());
        tags
    }
    fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = self.placement.required_blocks();
        blocks.extend(self.feature.required_blocks());
        blocks
    }
    fn needs_survival(&self, name: &str) -> bool {
        self.placement.needs_survival(name) || self.feature.needs_survival(name)
    }
    fn needs_lake(&self) -> bool {
        self.feature.needs_lake()
    }
}

impl CarvedTerrainWorld {
    /// Writable chunks must match the Java WorldGenRegion for this step.
    /// All reads, including neighbors and heightmaps, require loaded chunks.
    /// The tags must belong to the same registry and active data packs for the
    /// lifetime of the terrain. Other generation writes must apply the Java
    /// heightmap rules before this interface can be combined with them.
    pub fn decoration_region<'a>(
        &'a mut self,
        tags: &'a OreBlockTags,
        motion: &'a HeightmapTags,
        writable: BTreeSet<ChunkPos>,
    ) -> Result<DecorationRegion<'a>> {
        ensure!(
            writable.iter().all(|p| self.world.chunks.contains_key(p)),
            "writable generation chunk is missing"
        );
        let world = (*self.world).clone();
        let heights = self.ore_ocean_floor_heights.clone();
        let surface_heights = self.feature_world_surface_heights.clone();
        let final_heights = self.final_heightmaps.clone();
        let feature_marks = self.feature_postprocessing.clone();
        Ok(DecorationRegion {
            target: self,
            world,
            tags,
            motion,
            writable,
            heights,
            surface_heights,
            final_heights,
            feature_marks,
            block_tick_keys: BTreeMap::new(),
            fluid_tick_keys: BTreeMap::new(),
            sugar_cane_fluids: None,
            cactus_lava_fluids: None,
            lake_water_fluids: None,
            motion_cache: BTreeMap::new(),
            failed: false,
        })
    }
}

impl DecorationRegion<'_> {
    fn place_nested(
        &mut self,
        placed: &PlacedDecorationFeature,
        random: &mut DecorationRandom,
        origin: [i32; 3],
        generator_min_y: i32,
        generator_height: i32,
    ) -> Result<bool> {
        let mut positions =
            placed
                .placement
                .positions(origin, generator_min_y, generator_height)?;
        let mut result = false;
        loop {
            let next = {
                let mut env = NestedDecorationEnvironment { region: self };
                positions.next(random, &mut env)?
            };
            let Some(pos) = next else { break };
            result |= placed
                .feature
                .place(self, random, pos, generator_min_y, generator_height)?;
        }
        Ok(result)
    }
    /// A failed attempt must be discarded; on failure the caller's RNG is unchanged.
    /// If the region contains earlier successful placements, the caller must
    /// also restore its RNG to the beginning of the discarded region.
    pub fn place(
        &mut self,
        feature: &OreVein<BlockState>,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next = random.clone();
        self.failed = true;
        let result = feature.place(self, &mut next, origin)?;
        *random = next;
        self.failed = false;
        Ok(result)
    }

    /// Run placement modifiers and their feature in Java's depth-first
    /// order on one RNG. The environment must use the active biome settings,
    /// top-level placed feature, current heightmaps and level minimum Y.
    /// `generator_min_y` and `generator_height` are the placement context's
    /// generator bounds, which can differ from the level bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn place_placed(
        &mut self,
        placed: &PlacedDecorationFeature,
        random: &mut DecorationRandom,
        origin: [i32; 3],
        generator_min_y: i32,
        generator_height: i32,
        environment: &mut impl PlacementEnvironment,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next = random.clone();
        self.failed = true;
        let mut positions =
            placed
                .placement
                .positions(origin, generator_min_y, generator_height)?;
        let mut result = false;
        while let Some(position) = positions.next(&mut next, environment)? {
            result |= placed.feature.place(
                self,
                &mut next,
                position,
                generator_min_y,
                generator_height,
            )?;
        }
        *random = next;
        self.failed = false;
        Ok(result)
    }

    /// Place one planned feature using its own Java feature seed and the plan's
    /// biome neighborhood. Callers must execute every preceding feature and
    /// structure in order, and provide Java `hasFeature` semantics from the
    /// active biome settings; unknown equivalence must be an error.
    pub fn place_planned(
        &mut self,
        plan: &DecorationPlan<'_>,
        index: usize,
        ore: &ScheduledDecorationFeature<'_>,
        biome_allows: &mut impl FnMut(&FeatureKey, &ResourceId) -> Result<bool>,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        let selected = plan
            .features
            .get(index)
            .context("planned feature index is out of range")?;
        ensure!(
            *selected.feature == ore.key,
            "planned feature does not match loaded ore"
        );
        ensure!(
            self.target.seed == plan.biomes.seed
                && self.target.min_y == plan.biomes.min_y
                && self.target.height == plan.biomes.height,
            "planned biome neighborhood does not match generated terrain"
        );
        let mut random = plan.feature_random(selected);
        let mut positions = ore.placed.placement.positions(
            plan.origin,
            plan.generator_min_y,
            plan.generator_height,
        )?;
        self.failed = true;
        let mut result = false;
        loop {
            let next = {
                let mut env = PlannedDecorationEnvironment {
                    region: self,
                    plan,
                    key: &ore.key,
                    biome_allows,
                };
                positions.next(&mut random, &mut env)?
            };
            let Some(position) = next else { break };
            result |= ore.placed.feature.place(
                self,
                &mut random,
                position,
                plan.generator_min_y,
                plan.generator_height,
            )?;
        }
        self.failed = false;
        Ok(result)
    }

    /// Resolve biome membership from the active schedule for supported features.
    /// Certain independently decoded height providers and block predicates
    /// make distinct placed features unequal in Java.
    /// Ambiguous data-pack feature equality and unknown biomes are errors.
    pub fn place_planned_auto(
        &mut self,
        plan: &DecorationPlan<'_>,
        index: usize,
        ore: &ScheduledDecorationFeature<'_>,
    ) -> Result<bool> {
        let selected = plan
            .features
            .get(index)
            .context("planned feature index is out of range")?;
        let step = usize::try_from(selected.step).context("invalid feature step")?;
        let feature_index = usize::try_from(selected.index).context("invalid feature index")?;
        let canonical = ore
            .schedule
            .steps()
            .get(step)
            .and_then(|features| features.get(feature_index))
            .context("feature is absent from the loaded schedule")?;
        ensure!(
            std::ptr::eq(selected.feature, canonical),
            "plan and ore use different feature schedules"
        );
        self.place_planned(plan, index, ore, &mut |_, biome| ore.biome_allows(biome))
    }

    /// Publish this intermediate state; it is not a finished or saveable chunk.
    pub fn finish(self) -> Result<()> {
        ensure!(!self.failed, "cannot publish a failed generation attempt");
        self.target.world = Arc::new(self.world);
        self.target.ore_ocean_floor_heights = self.heights;
        self.target.feature_world_surface_heights = self.surface_heights;
        self.target.final_heightmaps = self.final_heights;
        self.target.feature_postprocessing = self.feature_marks;
        Ok(())
    }

    fn read(&self, p: [i32; 3]) -> Result<BlockState> {
        self.world
            .block(p[0], p[1], p[2])
            .cloned()
            .context("generation block or section is missing")
    }

    fn is_outside(&self, y: i32) -> bool {
        y < self.target.min_y || y >= self.target.min_y + self.target.height
    }

    // Java primes the entire heightmap on first access. Direct ore section
    // writes do not update it, including for data-pack replacement states.
    fn prime_heightmaps(&mut self, pos: ChunkPos) -> Result<()> {
        let need_ocean = !self.heights.contains_key(&pos);
        let need_surface = !self.surface_heights.contains_key(&pos);
        let need_final = !self.final_heights.contains_key(&pos);
        if !need_ocean && !need_surface && !need_final {
            return Ok(());
        }
        let base_x = pos.x.checked_mul(16).context("chunk X overflow")?;
        let base_z = pos.z.checked_mul(16).context("chunk Z overflow")?;
        ensure!(
            base_x.checked_add(15).is_some() && base_z.checked_add(15).is_some(),
            "chunk coordinate overflow"
        );
        ensure!(
            self.world.chunks.contains_key(&pos),
            "generation heightmap chunk is missing"
        );
        let mut ocean = Box::new([self.target.min_y; 256]);
        let mut surface = Box::new([self.target.min_y; 256]);
        let mut final_maps = FinalHeightmaps {
            values: Box::new([[self.target.min_y; 256]; 4]),
        };
        let mut final_cache = BTreeMap::<BlockStateId, [bool; 4]>::new();
        for z in 0..16 {
            for x in 0..16 {
                let mut missing_ocean = need_ocean;
                let mut missing_surface = need_surface;
                let mut missing_final = [need_final; 4];
                for y in (self.target.min_y..self.target.min_y + self.target.height).rev() {
                    let id = self
                        .world
                        .block_id(base_x + x, y, base_z + z)
                        .context("generation heightmap block is missing")?;
                    let state = self
                        .world
                        .state(id)
                        .context("generation block state is missing")?;
                    if is_air(state) {
                        continue;
                    }
                    let index = (x + z * 16) as usize;
                    if missing_surface {
                        surface[index] = y + 1;
                        missing_surface = false;
                    }
                    if missing_ocean {
                        let matches = if let Some(value) = self.motion_cache.get(&id) {
                            *value
                        } else {
                            let value = self.motion.for_block(&state.name)?.blocks_motion;
                            self.motion_cache.insert(id, value);
                            value
                        };
                        if matches {
                            ocean[index] = y + 1;
                            missing_ocean = false;
                        }
                    }
                    if missing_final.iter().any(|v| *v) {
                        let flags = if let Some(flags) = final_cache.get(&id) {
                            *flags
                        } else {
                            let flags = self.final_predicates(state)?;
                            final_cache.insert(id, flags);
                            flags
                        };
                        for kind in 0..4 {
                            if missing_final[kind] && flags[kind] {
                                final_maps.values[kind][index] = y + 1;
                                missing_final[kind] = false;
                            }
                        }
                    }
                    if !missing_ocean && !missing_surface && !missing_final.iter().any(|v| *v) {
                        break;
                    }
                }
            }
        }
        if need_ocean {
            self.heights.insert(pos, Arc::from(ocean));
        }
        if need_surface {
            self.surface_heights.insert(pos, Arc::from(surface));
        }
        if need_final {
            self.final_heights.insert(pos, Arc::new(final_maps));
        }
        Ok(())
    }

    fn final_predicates(&self, state: &BlockState) -> Result<[bool; 4]> {
        let motion = self.motion.for_block(&state.name)?;
        let fluid = native_world::fluid::FluidState::from_block(state)
            .map_err(anyhow::Error::msg)?
            .is_some();
        Ok([
            !is_air(state),
            motion.blocks_motion,
            motion.blocks_motion || fluid,
            motion.blocks_motion_no_leaves || fluid,
        ])
    }

    fn final_height(&mut self, kind: usize, x: i32, z: i32) -> Result<i32> {
        let pos = chunk([x, 0, z]);
        self.prime_heightmaps(pos)?;
        Ok(self.final_heights[&pos].values[kind]
            [(x.rem_euclid(16) + z.rem_euclid(16) * 16) as usize])
    }

    // Only WorldGenLevel.setBlock updates the final maps. Direct section
    // writes can leave them stale, and Java then updates that stale value.
    fn update_final_heightmaps(&mut self, p: [i32; 3], state: &BlockState) -> Result<()> {
        let pos = chunk(p);
        let index = (p[0].rem_euclid(16) + p[2].rem_euclid(16) * 16) as usize;
        let old = self
            .final_heights
            .get(&pos)
            .context("final heightmaps missing")?;
        let flags = self.final_predicates(state)?;
        let mut values = [0; 4];
        for kind in 0..4 {
            let previous = old.values[kind][index];
            values[kind] = previous;
            if p[1] <= previous - 2 {
                continue;
            }
            if flags[kind] && p[1] >= previous {
                values[kind] = p[1] + 1;
            } else if !flags[kind] && p[1] == previous - 1 {
                values[kind] = self.target.min_y;
                for y in (self.target.min_y..p[1]).rev() {
                    let below = self
                        .world
                        .block(p[0], y, p[2])
                        .context("heightmap scan block missing")?;
                    if self.final_predicates(below)?[kind] {
                        values[kind] = y + 1;
                        break;
                    }
                }
            }
        }
        let maps = Arc::make_mut(
            self.final_heights
                .get_mut(&pos)
                .context("final heightmaps missing")?,
        );
        for kind in 0..4 {
            maps.values[kind][index] = values[kind];
        }
        Ok(())
    }

    fn world_surface_height(&mut self, x: i32, z: i32) -> Result<i32> {
        let pos = chunk([x, 0, z]);
        self.prime_heightmaps(pos)?;
        Ok(self.surface_heights[&pos][(x.rem_euclid(16) + z.rem_euclid(16) * 16) as usize])
    }

    fn set_scattered_block(&mut self, p: [i32; 3], state: &BlockState, flags: i32) -> Result<bool> {
        ensure!(flags == 2, "unsupported worldgen block flags");
        if !self.writable.contains(&chunk(p)) {
            return Ok(false);
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(p)),
            "generation block chunk is missing"
        );
        // These states have no POI or block entity, and ancient debris does not
        // request postprocessing. Unknown data-pack replacements are rejected.
        ensure!(
            state.name == "minecraft:ancient_debris" && state.properties.is_empty(),
            "unsupported scattered ore replacement effects"
        );
        if self.is_outside(p[1]) {
            return Ok(true);
        }
        let old = self.read(p)?;
        ensure!(
            matches!(
                old.name.as_str(),
                "minecraft:air"
                    | "minecraft:cave_air"
                    | "minecraft:void_air"
                    | "minecraft:netherrack"
                    | "minecraft:basalt"
                    | "minecraft:blackstone"
                    | "minecraft:ancient_debris"
                    | "minecraft:stone"
                    | "minecraft:deepslate"
                    | "minecraft:water"
                    | "minecraft:lava"
            ),
            "unsupported scattered ore source effects"
        );
        // At TERRAIN, ProtoChunk updates final heightmaps, not the cached
        // world-generation maps. Saving remains outside this partial pipeline.
        self.set_section_block(p, state)?;
        self.update_final_heightmaps(p, state)?;
        Ok(true)
    }
}

struct PlannedDecorationEnvironment<'a, 'b, F> {
    region: &'a mut DecorationRegion<'b>,
    plan: &'a DecorationPlan<'a>,
    key: &'a FeatureKey,
    biome_allows: &'a mut F,
}

struct NestedDecorationEnvironment<'a, 'b> {
    region: &'a mut DecorationRegion<'b>,
}
impl PlacementEnvironment for NestedDecorationEnvironment<'_, '_> {
    fn min_y(&self) -> i32 {
        self.region.target.min_y
    }
    fn inside_build_height(&self, y: i32) -> Result<bool> {
        Ok(!self.region.is_outside(y))
    }
    fn heightmap(&mut self, kind: &str, x: i32, z: i32) -> Result<i32> {
        match kind {
            "OCEAN_FLOOR_WG" => self.region.ocean_floor_height(x, z),
            "WORLD_SURFACE_WG" => self.region.world_surface_height(x, z),
            "WORLD_SURFACE" => self.region.final_height(0, x, z),
            "OCEAN_FLOOR" => self.region.final_height(1, x, z),
            "MOTION_BLOCKING" => self.region.final_height(2, x, z),
            "MOTION_BLOCKING_NO_LEAVES" => self.region.final_height(3, x, z),
            _ => bail!("unsupported generation heightmap {kind}"),
        }
    }
    fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
        bail!("nested placed feature has no top-level biome feature")
    }
    fn block_predicate(&mut self, predicate: &BlockPredicate, pos: [i32; 3]) -> Result<bool> {
        predicate.test(self.region, pos)
    }
}
impl<F: FnMut(&FeatureKey, &ResourceId) -> Result<bool>> PlacementEnvironment
    for PlannedDecorationEnvironment<'_, '_, F>
{
    fn inside_build_height(&self, y: i32) -> Result<bool> {
        Ok(!self.region.is_outside(y))
    }
    fn min_y(&self) -> i32 {
        self.region.target.min_y
    }
    fn heightmap(&mut self, kind: &str, x: i32, z: i32) -> Result<i32> {
        match kind {
            "OCEAN_FLOOR_WG" => self.region.ocean_floor_height(x, z),
            "WORLD_SURFACE_WG" => self.region.world_surface_height(x, z),
            "WORLD_SURFACE" => self.region.final_height(0, x, z),
            "OCEAN_FLOOR" => self.region.final_height(1, x, z),
            "MOTION_BLOCKING" => self.region.final_height(2, x, z),
            "MOTION_BLOCKING_NO_LEAVES" => self.region.final_height(3, x, z),
            _ => bail!("unsupported generation heightmap {kind}"),
        }
    }
    fn biome_allows(&mut self, pos: [i32; 3]) -> Result<bool> {
        let biome = self.plan.biomes.biome_at_block(pos[0], pos[1], pos[2])?;
        (self.biome_allows)(self.key, biome)
    }
    fn block_predicate(&mut self, predicate: &BlockPredicate, pos: [i32; 3]) -> Result<bool> {
        predicate.test(self.region, pos)
    }
}

fn is_air(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}
fn chunk(p: [i32; 3]) -> ChunkPos {
    ChunkPos {
        x: p[0].div_euclid(16),
        z: p[2].div_euclid(16),
    }
}

impl OreWorld for DecorationRegion<'_> {
    type State = BlockState;
    fn block(&mut self, p: [i32; 3]) -> Result<BlockState> {
        if self.is_outside(p[1]) {
            ensure!(
                self.world.chunks.contains_key(&chunk(p)),
                "generation block chunk is missing"
            );
            return Ok(BlockState {
                name: "minecraft:void_air".into(),
                properties: BTreeMap::new(),
            });
        }
        self.read(p)
    }
    fn block_is(&mut self, state: &BlockState, id: &ResourceId) -> Result<bool> {
        Ok(state.name == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, state: &BlockState, tag: &ResourceId) -> Result<bool> {
        self.tags.contains(tag, &state.name)
    }
    fn is_air(&mut self, state: &BlockState) -> Result<bool> {
        Ok(is_air(state))
    }
    fn set_block(&mut self, p: [i32; 3], state: &BlockState, flags: i32) -> Result<bool> {
        let result = self.set_scattered_block(p, state, flags);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

// These complete vanilla block-state families were checked for solidness,
// block entities, POIs and postprocessing effects against Java 26.3.
fn disk_safe_solid(name: &str) -> Option<bool> {
    match name {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:water"
        | "minecraft:lava" => Some(false),
        "minecraft:stone"
        | "minecraft:deepslate"
        | "minecraft:netherrack"
        | "minecraft:basalt"
        | "minecraft:blackstone"
        | "minecraft:ancient_debris"
        | "minecraft:dirt"
        | "minecraft:grass_block"
        | "minecraft:mud"
        | "minecraft:clay"
        | "minecraft:sand"
        | "minecraft:sandstone"
        | "minecraft:gravel"
        | "minecraft:packed_ice"
        | "minecraft:snow_block"
        | "minecraft:ice"
        | "minecraft:podzol"
        | "minecraft:coarse_dirt"
        | "minecraft:mycelium"
        | "minecraft:tuff"
        | "minecraft:calcite"
        | "minecraft:granite"
        | "minecraft:diorite"
        | "minecraft:andesite"
        | "minecraft:bedrock"
        | "minecraft:red_sand"
        | "minecraft:moss_block" => Some(true),
        "minecraft:magma_block" | "minecraft:cactus" => Some(true),
        "minecraft:sugar_cane" | "minecraft:cactus_flower" => Some(false),
        _ => None,
    }
}

impl DiskWorld for DecorationRegion<'_> {
    fn inside_build_height(&mut self, y: i32) -> Result<bool> {
        Ok(!self.is_outside(y))
    }
    fn fluid_in_tag(&mut self, state: &BlockState, tag: &ResourceId) -> Result<bool> {
        let fluids = match (tag.namespace.as_str(), tag.path.as_str()) {
            ("minecraft", "supports_sugar_cane_adjacently") => self
                .sugar_cane_fluids
                .context("sugar cane fluid tags were not prepared")?,
            ("minecraft", "lava") => self
                .cactus_lava_fluids
                .context("lava fluid tags were not prepared")?,
            ("minecraft", "water") => self
                .lake_water_fluids
                .context("lake water fluid tags were not prepared")?,
            _ => bail!("unsupported survival fluid tag"),
        };
        let fluid =
            match native_world::fluid::FluidState::from_block(state).map_err(anyhow::Error::msg)? {
                None => "minecraft:empty",
                Some(value) => match (value.kind, value.source) {
                    (native_world::fluid::FluidKind::Water, true) => "minecraft:water",
                    (native_world::fluid::FluidKind::Water, false) => "minecraft:flowing_water",
                    (native_world::fluid::FluidKind::Lava, true) => "minecraft:lava",
                    (native_world::fluid::FluidKind::Lava, false) => "minecraft:flowing_lava",
                },
            };
        Ok(fluids.contains(fluid))
    }
    fn is_solid(&mut self, state: &BlockState) -> Result<bool> {
        disk_safe_solid(&state.name).context("unsupported disk solid predicate block")
    }
    fn is_liquid(&mut self, state: &BlockState) -> Result<bool> {
        if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            return Ok(true);
        }
        ensure!(
            disk_safe_solid(&state.name).is_some() || simple_plant_kind(state).is_ok(),
            "unsupported block liquid property"
        );
        Ok(false)
    }

    fn fluid_is(&mut self, state: &BlockState, id: &ResourceId) -> Result<bool> {
        use native_world::fluid::{FluidKind, FluidState};
        ensure!(
            id.namespace == "minecraft",
            "unsupported disk fluid namespace"
        );
        let (kind, source) = match id.path.as_str() {
            "water" => (FluidKind::Water, true),
            "flowing_water" => (FluidKind::Water, false),
            "lava" => (FluidKind::Lava, true),
            "flowing_lava" => (FluidKind::Lava, false),
            _ => bail!("unsupported disk fluid {}:{}", id.namespace, id.path),
        };
        Ok(FluidState::from_block(state)
            .map_err(anyhow::Error::msg)?
            .is_some_and(|fluid| fluid.kind == kind && fluid.source == source))
    }

    fn set_disk_block(&mut self, p: [i32; 3], state: &BlockState) -> Result<bool> {
        let result = (|| {
            if !self.writable.contains(&chunk(p)) {
                return Ok(false);
            }
            ensure!(
                self.world.chunks.contains_key(&chunk(p)),
                "generation block chunk is missing"
            );
            let allowed = match state.name.as_str() {
                "minecraft:dirt"
                | "minecraft:clay"
                | "minecraft:sand"
                | "minecraft:sandstone"
                | "minecraft:gravel"
                | "minecraft:packed_ice" => state.properties.is_empty(),
                "minecraft:grass_block" => {
                    state.properties.len() == 1
                        && matches!(
                            state.properties.get("snowy").map(String::as_str),
                            Some("true" | "false")
                        )
                }
                _ => false,
            };
            ensure!(allowed, "unsupported disk replacement effects");
            if self.is_outside(p[1]) {
                return Ok(true);
            }
            ensure!(
                disk_safe_solid(&self.read(p)?.name).is_some(),
                "unsupported disk source effects"
            );
            self.set_section_block(p, state)?;
            self.update_final_heightmaps(p, state)?;
            Ok(true)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn mark_postprocessing(&mut self, p: [i32; 3]) -> Result<()> {
        let pos = chunk(p);
        let result = (|| {
            ensure!(
                self.world.chunks.contains_key(&pos),
                "postprocessing chunk is missing"
            );
            if !self.feature_marks.contains_key(&pos) {
                let marks = native_core::postprocessing::PostProcessing::new(
                    self.target.min_y,
                    self.target.height,
                )
                .context("invalid generation postprocessing bounds")?;
                self.feature_marks.insert(pos, Arc::new(marks));
            }
            Arc::make_mut(
                self.feature_marks
                    .get_mut(&pos)
                    .expect("marks were inserted"),
            )
            .mark(p[0], p[1], p[2]);
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

#[derive(Clone, Copy)]
enum SimplePlantKind {
    Vegetation,
    DryVegetation,
    Double,
    Unconditional,
}

// All states of these vanilla blocks have been checked for their Java
// survival implementation, POI/block-entity effects and postprocessing.
fn simple_plant_kind(state: &BlockState) -> Result<SimplePlantKind> {
    let name = state.name.as_str();
    let kind = match name {
        "minecraft:short_grass"
        | "minecraft:fern"
        | "minecraft:bush"
        | "minecraft:dandelion"
        | "minecraft:poppy"
        | "minecraft:blue_orchid"
        | "minecraft:allium"
        | "minecraft:azure_bluet"
        | "minecraft:red_tulip"
        | "minecraft:orange_tulip"
        | "minecraft:white_tulip"
        | "minecraft:pink_tulip"
        | "minecraft:oxeye_daisy"
        | "minecraft:cornflower"
        | "minecraft:lily_of_the_valley"
        | "minecraft:firefly_bush"
        | "minecraft:closed_eyeblossom" => {
            ensure!(state.properties.is_empty(), "invalid vegetation state");
            SimplePlantKind::Vegetation
        }
        "minecraft:sweet_berry_bush" => {
            ensure!(
                state.properties.len() == 1
                    && matches!(
                        state.properties.get("age").map(String::as_str),
                        Some("0" | "1" | "2" | "3")
                    ),
                "invalid berry bush state"
            );
            SimplePlantKind::Vegetation
        }
        "minecraft:short_dry_grass" | "minecraft:tall_dry_grass" | "minecraft:dead_bush" => {
            ensure!(state.properties.is_empty(), "invalid dry vegetation state");
            SimplePlantKind::DryVegetation
        }
        "minecraft:sunflower"
        | "minecraft:tall_grass"
        | "minecraft:large_fern"
        | "minecraft:rose_bush"
        | "minecraft:peony"
        | "minecraft:lilac" => {
            ensure!(
                state.properties.len() == 1
                    && matches!(
                        state.properties.get("half").map(String::as_str),
                        Some("lower" | "upper")
                    ),
                "invalid double vegetation state"
            );
            SimplePlantKind::Double
        }
        "minecraft:melon" | "minecraft:pumpkin" => {
            ensure!(state.properties.is_empty(), "invalid simple block state");
            SimplePlantKind::Unconditional
        }
        _ => bail!("unsupported simple-block survival for {name}"),
    };
    Ok(kind)
}

impl DecorationRegion<'_> {
    fn write_simple_block(&mut self, p: [i32; 3], state: &BlockState) -> Result<bool> {
        if !self.writable.contains(&chunk(p)) {
            return Ok(false);
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(p)),
            "generation block chunk is missing"
        );
        if self.is_outside(p[1]) {
            return Ok(true);
        }
        let old = self.read(p)?;
        ensure!(
            disk_safe_solid(&old.name).is_some() || simple_plant_kind(&old).is_ok(),
            "unsupported simple-block source effects"
        );
        self.set_section_block(p, state)?;
        self.update_final_heightmaps(p, state)?;
        Ok(true)
    }
}

impl SimpleBlockWorld for DecorationRegion<'_> {
    fn place_simple_state(&mut self, pos: [i32; 3], state: &BlockState) -> Result<bool> {
        let result = (|| {
            let kind = simple_plant_kind(state)?;
            let below = [pos[0], pos[1].wrapping_sub(1), pos[2]];
            let survives = match kind {
                SimplePlantKind::Unconditional => true,
                SimplePlantKind::Double
                    if state.properties.get("half").map(String::as_str) == Some("upper") =>
                {
                    let below = self.block(below)?;
                    below.name == state.name
                        && below.properties.get("half").map(String::as_str) == Some("lower")
                }
                _ => {
                    let below = self.block(below)?;
                    let tag = match kind {
                        SimplePlantKind::DryVegetation => "minecraft:supports_dry_vegetation",
                        _ => "minecraft:supports_vegetation",
                    };
                    self.tags.contains(&ResourceId::parse(tag)?, &below.name)?
                }
            };
            if !survives {
                return Ok(false);
            }
            if matches!(kind, SimplePlantKind::Double) {
                let above = [pos[0], pos[1].wrapping_add(1), pos[2]];
                let upper = self.block(above)?;
                let free = if is_air(&upper) {
                    true
                } else if native_world::fluid::FluidState::from_block(&upper)
                    .map_err(anyhow::Error::msg)?
                    .is_some()
                {
                    false
                } else {
                    match upper.name.as_str() {
                        "minecraft:short_grass"
                        | "minecraft:fern"
                        | "minecraft:bush"
                        | "minecraft:short_dry_grass"
                        | "minecraft:tall_dry_grass"
                        | "minecraft:dead_bush"
                        | "minecraft:tall_grass"
                        | "minecraft:large_fern" => true,
                        _ if disk_safe_solid(&upper.name).is_some()
                            || simple_plant_kind(&upper).is_ok() =>
                        {
                            false
                        }
                        _ => bail!("unsupported double-plant replacement state"),
                    }
                };
                if !free {
                    return Ok(false);
                }
                let mut lower = state.clone();
                lower.properties.insert("half".into(), "lower".into());
                let mut upper = state.clone();
                upper.properties.insert("half".into(), "upper".into());
                self.write_simple_block(pos, &lower)?;
                self.write_simple_block(above, &upper)?;
            } else {
                self.write_simple_block(pos, state)?;
            }
            Ok(true)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn schedule_simple_tick(&mut self, pos: [i32; 3]) -> Result<()> {
        let result = (|| {
            // WorldGenTickAccess uses the chunk at the position even if a
            // preceding setBlock was rejected by the feature write radius.
            let block = self.block(pos)?.name;
            let chunk = chunk(pos);
            let keys = self.block_tick_keys.entry(chunk).or_insert_with(|| {
                self.world
                    .saved_block_ticks
                    .get(&chunk)
                    .into_iter()
                    .flatten()
                    .map(|tick| (tick.position, tick.block.clone()))
                    .collect()
            });
            if keys.insert((pos, block.clone())) {
                // ProtoChunkTicks discards the requested delay and stores 0.
                self.world
                    .saved_block_ticks
                    .entry(chunk)
                    .or_default()
                    .push(SavedBlockTick {
                        block,
                        position: pos,
                        delay: 0,
                        priority: 0,
                    });
            }
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl SpringWorld for DecorationRegion<'_> {
    fn set_spring_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<bool> {
        let result = (|| {
            if !self.writable.contains(&chunk(pos)) {
                return Ok(false);
            }
            ensure!(
                self.world.chunks.contains_key(&chunk(pos)),
                "generation block chunk is missing"
            );
            ensure!(
                matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava"),
                "unsupported spring block"
            );
            if self.is_outside(pos[1]) {
                return Ok(true);
            }
            ensure!(
                disk_safe_solid(&self.read(pos)?.name).is_some(),
                "unsupported spring source effects"
            );
            self.set_section_block(pos, state)?;
            self.update_final_heightmaps(pos, state)?;
            Ok(true)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn schedule_spring_tick(&mut self, pos: [i32; 3], fluid: &ResourceId) -> Result<()> {
        let result = (|| {
            let chunk = chunk(pos);
            ensure!(
                self.world.chunks.contains_key(&chunk),
                "generation tick chunk is missing"
            );
            let fluid = format!("{}:{}", fluid.namespace, fluid.path);
            let keys = self.fluid_tick_keys.entry(chunk).or_insert_with(|| {
                self.world
                    .saved_fluid_ticks
                    .get(&chunk)
                    .into_iter()
                    .flatten()
                    .map(|tick| (tick.position, tick.fluid.clone()))
                    .collect()
            });
            if keys.insert((pos, fluid.clone())) {
                self.world
                    .saved_fluid_ticks
                    .entry(chunk)
                    .or_default()
                    .push(SavedFluidTick {
                        fluid,
                        position: pos,
                        delay: 0,
                        priority: 0,
                    });
            }
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl MagmaWorld for DecorationRegion<'_> {
    fn face_is_full(&mut self, state: &BlockState, _direction: [i32; 3]) -> Result<bool> {
        vanilla_full_occlusion(&state.name)
            .with_context(|| format!("unsupported magma neighbor occlusion for {}", state.name))
    }
    fn set_magma(&mut self, pos: [i32; 3], state: &BlockState) -> Result<bool> {
        let result = (|| {
            if !self.writable.contains(&chunk(pos)) {
                return Ok(false);
            }
            ensure!(
                self.world.chunks.contains_key(&chunk(pos)),
                "generation block chunk is missing"
            );
            ensure!(
                state.name == "minecraft:magma_block" && state.properties.is_empty(),
                "invalid magma block state"
            );
            if !self.is_outside(pos[1]) {
                ensure!(
                    disk_safe_solid(&self.read(pos)?.name).is_some(),
                    "unsupported magma source effects"
                );
                self.set_section_block(pos, state)?;
                self.update_final_heightmaps(pos, state)?;
            }
            self.mark_postprocessing([pos[0], pos[1].wrapping_add(1), pos[2]])?;
            Ok(true)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl ColumnWorld for DecorationRegion<'_> {
    fn set_column_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<bool> {
        let result = (|| {
            if !self.writable.contains(&chunk(pos)) {
                return Ok(false);
            }
            ensure!(
                self.world.chunks.contains_key(&chunk(pos)),
                "generation block chunk is missing"
            );
            let age = state
                .properties
                .get("age")
                .and_then(|v| v.parse::<u8>().ok());
            let valid = match state.name.as_str() {
                "minecraft:sugar_cane" | "minecraft:cactus" => {
                    state.properties.len() == 1 && age.is_some_and(|v| v <= 15)
                }
                "minecraft:cactus_flower" => state.properties.is_empty(),
                _ => false,
            };
            ensure!(valid, "unsupported column replacement effects");
            if self.is_outside(pos[1]) {
                return Ok(true);
            }
            let old = self.read(pos)?;
            ensure!(
                disk_safe_solid(&old.name).is_some()
                    || old.name == "minecraft:sugar_cane"
                    || simple_plant_kind(&old).is_ok(),
                "unsupported column source effects"
            );
            self.set_section_block(pos, state)?;
            self.update_final_heightmaps(pos, state)?;
            Ok(true)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl LakeWorld for DecorationRegion<'_> {
    fn min_y(&self) -> i32 {
        self.target.min_y
    }
    fn set_lake_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<bool> {
        let result = (|| {
            if !self.writable.contains(&chunk(pos)) {
                return Ok(false);
            }
            ensure!(
                self.world.chunks.contains_key(&chunk(pos)),
                "generation block chunk is missing"
            );
            let valid = match state.name.as_str() {
                "minecraft:air" | "minecraft:cave_air" | "minecraft:stone" | "minecraft:ice" => {
                    state.properties.is_empty()
                }
                "minecraft:water" | "minecraft:lava" => {
                    native_world::fluid::FluidState::from_block(state)
                        .map_err(anyhow::Error::msg)?
                        .is_some()
                }
                _ => false,
            };
            ensure!(valid, "unsupported lake replacement effects");
            if self.is_outside(pos[1]) {
                return Ok(true);
            }
            let old = self.read(pos)?;
            ensure!(
                disk_safe_solid(&old.name).is_some() || simple_plant_kind(&old).is_ok(),
                "unsupported lake source effects"
            );
            self.set_section_block(pos, state)?;
            self.update_final_heightmaps(pos, state)?;
            Ok(true)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn schedule_lake_air_tick(&mut self, pos: [i32; 3]) -> Result<()> {
        let result = (|| {
            let chunk = chunk(pos);
            ensure!(
                self.world.chunks.contains_key(&chunk),
                "generation tick chunk is missing"
            );
            let block = "minecraft:cave_air".to_owned();
            let keys = self.block_tick_keys.entry(chunk).or_insert_with(|| {
                self.world
                    .saved_block_ticks
                    .get(&chunk)
                    .into_iter()
                    .flatten()
                    .map(|tick| (tick.position, tick.block.clone()))
                    .collect()
            });
            if keys.insert((pos, block.clone())) {
                self.world
                    .saved_block_ticks
                    .entry(chunk)
                    .or_default()
                    .push(SavedBlockTick {
                        block,
                        position: pos,
                        delay: 0,
                        priority: 0,
                    });
            }
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn should_freeze_lake(&mut self, _: [i32; 3]) -> Result<bool> {
        bail!("lake biome freezing is not yet available")
    }
}

impl OreVeinWorld for DecorationRegion<'_> {
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        let pos = chunk([x, 0, z]);
        self.prime_heightmaps(pos)?;
        Ok(self.heights[&pos][(x.rem_euclid(16) + z.rem_euclid(16) * 16) as usize])
    }
    fn outside_build_height(&self, y: i32) -> bool {
        self.is_outside(y)
    }
    fn ensure_can_write(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(self.writable.contains(&chunk(p)))
    }
    fn section_block(&mut self, p: [i32; 3]) -> Result<Option<BlockState>> {
        if self.is_outside(p[1]) {
            return Ok(None);
        }
        self.read(p).map(Some)
    }
    fn section_neighbor(&mut self, p: [i32; 3]) -> Result<BlockState> {
        if self.is_outside(p[1]) {
            return Ok(BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            });
        }
        self.read(p)
    }
    fn set_section_block(&mut self, p: [i32; 3], state: &BlockState) -> Result<()> {
        ensure!(!self.is_outside(p[1]), "ore write outside build height");
        // Preserve both original maps before the first direct write in this
        // chunk; untouched neighbors need no heightmap scan.
        self.prime_heightmaps(chunk(p))?;
        let id = self.world.intern_state(state.clone());
        let section_y = i8::try_from(p[1].div_euclid(16))?;
        let chunk = Arc::make_mut(
            self.world
                .chunks
                .get_mut(&chunk(p))
                .context("ore write chunk is missing")?,
        );
        let section = Arc::make_mut(
            chunk
                .sections
                .get_mut(&section_y)
                .context("ore write section is missing")?,
        );
        let blocks = section
            .blocks
            .as_mut()
            .context("ore write block data is missing")?;
        let index = p[0].rem_euclid(16) as usize
            + p[2].rem_euclid(16) as usize * 16
            + p[1].rem_euclid(16) as usize * 256;
        blocks[index] = id;
        Ok(())
    }
}

#[cfg(test)]
mod disk_tests;
#[cfg(test)]
mod scattered_tests;
#[cfg(test)]
mod simple_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::packs::ResourceProvider;
    use native_world::loaded::{BLOCK_COUNT, BlockStateId, Chunk, Section};
    use serde_json::{Value, json};

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                    Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore"]}"#.to_vec())
                }
                "data/minecraft/tags/block/base_stone_overworld.json" => {
                    Some(br#"{"values":["minecraft:stone"]}"#.to_vec())
                }
                "data/minecraft/tags/block/supports_vegetation.json" => {
                    Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore"]}"#.to_vec())
                }
                "data/minecraft/tags/block/air.json" => Some(
                    br#"{"values":["minecraft:air","minecraft:cave_air","minecraft:void_air"]}"#
                        .to_vec(),
                ),
                _ => None,
            })
        }
    }
    fn state(name: &str) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        }
    }
    fn fixture() -> (
        CarvedTerrainWorld,
        OreBlockTags,
        HeightmapTags,
        OreVein<BlockState>,
    ) {
        let registry = [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:diamond_ore",
            "minecraft:water",
            "minecraft:ancient_debris",
            "minecraft:clay",
            "minecraft:short_grass",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:netherrack",
            "minecraft:basalt",
            "minecraft:blackstone",
            "minecraft:deepslate",
            "minecraft:lava",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let mut data = Data;
        let feature = OreVein::decode(&json!({"type":"minecraft:ore", "size": 33, "discard_chance_on_air_exposure": 0.0, "targets": [{"target":{"predicate_type":"minecraft:tag_match", "tag":"minecraft:base_stone_overworld"}, "state":"minecraft:diamond_ore"}]}), &mut |v: &Value| Ok(state(v.as_str().unwrap().strip_prefix("minecraft:").unwrap()))).unwrap();
        let tags = OreBlockTags::load(&mut data, &registry, &feature.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut data, &registry).unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air"));
        let stone = world.intern_state(state("stone"));
        for z in -1..=1 {
            for x in -1..=1 {
                let position = ChunkPos { x, z };
                let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            blocks[x + z * 16 + y * 256] = if y < 8 { stone } else { air };
                        }
                    }
                }
                world.chunks.insert(
                    position,
                    Arc::new(Chunk {
                        position,
                        status: Some("minecraft:carvers".into()),
                        block_light_correct: false,
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let result = CarvedTerrainWorld {
            world: Arc::new(world),
            seed: 0,
            min_y: 0,
            height: 16,
            completed_decoration: BTreeSet::new(),
            ore_ocean_floor_heights: BTreeMap::new(),
            feature_world_surface_heights: BTreeMap::new(),
            final_heightmaps: BTreeMap::new(),
            postprocessing: BTreeMap::new(),
            noise_postprocessing: BTreeMap::new(),
            surface_postprocessing: BTreeMap::new(),
            carver_postprocessing: BTreeMap::new(),
            feature_postprocessing: BTreeMap::new(),
        };
        (result, tags, motion, feature)
    }

    #[test]
    fn spring_writes_and_fluid_ticks_commit_or_rollback_together() {
        let (mut terrain, tags, motion, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let before = terrain.snapshot();
        let feature = SpringFeature::decode(&json!({
            "type":"minecraft:spring_feature", "state":{"id":"minecraft:water","properties":{"falling":"true"}},
            "valid_blocks":["minecraft:stone"]
        }), &mut |v| {
            let mut result = resolve_test(v)?;
            result.properties.insert("level".into(), v["properties"]["level"].as_str().unwrap().into());
            Ok(result)
        }).unwrap();
        let pos = [8, 5, 8];
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        region.set_section_block([7, 5, 8], &state("air")).unwrap();
        assert!(feature.place(&mut region, pos).unwrap());
        assert!(!feature.place(&mut region, pos).unwrap());
        region.finish().unwrap();
        let after = terrain.snapshot();
        assert_eq!(after.block(8, 5, 8).unwrap().name, "minecraft:water");
        assert_eq!(after.block(8, 5, 8).unwrap().properties["level"], "0");
        assert_eq!(
            after.saved_fluid_ticks[&center],
            [SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: pos,
                delay: 0,
                priority: 0
            }]
        );
        assert!(before.saved_fluid_ticks.is_empty());
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        region
            .schedule_spring_tick(pos, &ResourceId::parse("minecraft:water").unwrap())
            .unwrap();
        assert_eq!(region.world.saved_fluid_ticks[&center].len(), 1);
        assert!(
            region
                .set_spring_block([8, 5, 8], &state("diamond_ore"))
                .is_err()
        );
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&after, &terrain.snapshot()));
    }

    #[test]
    fn writes_real_sections_and_preserves_snapshots_without_session_edits() {
        let (mut terrain, tags, motion, feature) = fixture();
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(123);
        assert!(region.place(&feature, &mut random, [15, 4, 7]).unwrap());
        region.finish().unwrap();
        let current = terrain.snapshot();
        let count = (0..32)
            .flat_map(|x| (0..16).flat_map(move |z| (0..8).map(move |y| (x, y, z))))
            .filter(|&(x, y, z)| current.block(x, y, z).unwrap().name == "minecraft:diamond_ore")
            .count();
        assert!(count > 0);
        assert_eq!(previous.block(15, 4, 7).unwrap().name, "minecraft:stone");
        assert_eq!(current.block_edits().count(), 0);
        let untouched = ChunkPos { x: -1, z: -1 };
        assert!(Arc::ptr_eq(
            &previous.chunks[&untouched],
            &current.chunks[&untouched]
        ));
    }

    #[test]
    fn primes_whole_heightmap_and_keeps_direct_section_writes_out_of_it() {
        let (mut terrain, tags, motion, _) = fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        assert_eq!(region.ocean_floor_height(0, 0).unwrap(), 8);
        region
            .set_section_block([1, 12, 1], &state("stone"))
            .unwrap();
        assert_eq!(region.ocean_floor_height(1, 1).unwrap(), 8);
        assert_eq!(region.section_neighbor([1, -1, 1]).unwrap(), state("air"));
        region.finish().unwrap();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut next = terrain.decoration_region(&tags, &motion, writable).unwrap();
        assert_eq!(next.ocean_floor_height(1, 1).unwrap(), 8);
    }

    #[test]
    fn neighboring_chunk_maps_are_primed_before_direct_feature_writes() {
        let (mut terrain, tags, motion, _) = fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        assert!(region.heights.is_empty() && region.surface_heights.is_empty());
        // Neither neighbor column was queried before these direct writes.
        region.set_section_block([16, 7, 5], &state("air")).unwrap();
        region
            .set_section_block([17, 12, 5], &state("stone"))
            .unwrap();
        for x in [16, 17] {
            assert_eq!(region.ocean_floor_height(x, 5).unwrap(), 8);
            assert_eq!(region.world_surface_height(x, 5).unwrap(), 8);
        }
        region.finish().unwrap();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut next = terrain.decoration_region(&tags, &motion, writable).unwrap();
        for x in [16, 17] {
            assert_eq!(next.ocean_floor_height(x, 5).unwrap(), 8);
            assert_eq!(next.world_surface_height(x, 5).unwrap(), 8);
        }
    }

    #[test]
    fn surface_and_ocean_floor_differ_above_water() {
        let (mut terrain, tags, motion, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let water = world.intern_state(state("water"));
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[12 * 256] = water;
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        assert_eq!(region.world_surface_height(0, 0).unwrap(), 13);
        assert_eq!(region.ocean_floor_height(0, 0).unwrap(), 8);
        assert_eq!(region.final_height(0, 0, 0).unwrap(), 13);
        assert_eq!(region.final_height(1, 0, 0).unwrap(), 8);
        assert_eq!(region.final_height(2, 0, 0).unwrap(), 13);
        assert_eq!(region.final_height(3, 0, 0).unwrap(), 13);
    }

    #[test]
    fn scattered_set_block_preserves_worldgen_maps_and_respects_write_bounds() {
        let (mut terrain, tags, motion, _) = fixture();
        let writable = BTreeSet::from([ChunkPos { x: 0, z: 0 }]);
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        assert_eq!(region.ocean_floor_height(0, 0).unwrap(), 8);
        assert_eq!(region.world_surface_height(0, 0).unwrap(), 8);
        for kind in 0..4 {
            assert_eq!(region.final_height(kind, 0, 0).unwrap(), 8);
        }
        // The fixture's motion tag intentionally excludes ancient debris.
        assert!(
            region
                .set_block([0, 7, 0], &state("ancient_debris"), 2)
                .unwrap()
        );
        assert_eq!(region.ocean_floor_height(0, 0).unwrap(), 8);
        assert_eq!(region.world_surface_height(0, 0).unwrap(), 8);
        assert_eq!(region.final_height(0, 0, 0).unwrap(), 8);
        for kind in 1..4 {
            assert_eq!(region.final_height(kind, 0, 0).unwrap(), 7);
        }
        assert!(
            region
                .set_block([0, 12, 0], &state("ancient_debris"), 2)
                .unwrap()
        );
        assert_eq!(region.ocean_floor_height(0, 0).unwrap(), 8);
        assert_eq!(region.world_surface_height(0, 0).unwrap(), 8);
        assert_eq!(region.final_height(0, 0, 0).unwrap(), 13);
        for kind in 1..4 {
            assert_eq!(region.final_height(kind, 0, 0).unwrap(), 7);
        }
        assert!(
            !region
                .set_block([16, 7, 0], &state("ancient_debris"), 2)
                .unwrap()
        );
        assert_eq!(region.block([0, -1, 0]).unwrap(), state("void_air"));
        assert!(
            region
                .set_block([0, -1, 0], &state("ancient_debris"), 2)
                .unwrap()
        );
        region.finish().unwrap();
        let mut next = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        assert_eq!(next.final_height(0, 0, 0).unwrap(), 13);
        assert_eq!(next.final_height(1, 0, 0).unwrap(), 7);
    }

    #[test]
    fn scattered_set_block_rejects_unknown_effects_without_publishing() {
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        assert!(
            region
                .set_block([0, 7, 0], &state("diamond_ore"), 2)
                .is_err()
        );
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    #[test]
    fn final_heightmaps_keep_stale_direct_section_writes() {
        let (mut terrain, tags, motion, _) = fixture();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        region
            .set_section_block([0, 12, 0], &state("stone"))
            .unwrap();
        for kind in 0..4 {
            assert_eq!(region.final_height(kind, 0, 0).unwrap(), 8);
        }
        region
            .set_block([0, 7, 0], &state("ancient_debris"), 2)
            .unwrap();
        assert_eq!(region.final_height(0, 0, 0).unwrap(), 8);
        for kind in 1..4 {
            assert_eq!(region.final_height(kind, 0, 0).unwrap(), 7);
        }
        region
            .set_block([0, 12, 0], &state("ancient_debris"), 2)
            .unwrap();
        assert_eq!(region.final_height(0, 0, 0).unwrap(), 13);
        for kind in 1..4 {
            assert_eq!(region.final_height(kind, 0, 0).unwrap(), 7);
        }
    }

    #[test]
    fn missing_neighbors_reject_and_discard_attempt_without_consuming_rng() {
        let (mut terrain, tags, motion, feature) = fixture();
        Arc::make_mut(&mut terrain.world)
            .chunks
            .remove(&ChunkPos { x: 0, z: -1 });
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(123);
        let mut expected = random.clone();
        assert!(region.place(&feature, &mut random, [15, 4, 7]).is_err());
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&previous, &terrain.snapshot()));
    }

    struct PlacementTestEnv {
        allow: bool,
        fail_after: Option<usize>,
        calls: usize,
    }
    impl PlacementEnvironment for PlacementTestEnv {
        fn min_y(&self) -> i32 {
            0
        }
        fn heightmap(&mut self, _: &str, _: i32, _: i32) -> Result<i32> {
            bail!("missing heightmap")
        }
        fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
            self.calls += 1;
            ensure!(self.fail_after != Some(self.calls), "missing biome data");
            Ok(self.allow)
        }
    }

    fn inline_placed_ore() -> PlacedDecorationFeature {
        let placement = PlacedFeature::decode(&json!({"feature": {
            "type":"minecraft:ore", "size":9, "discard_chance_on_air_exposure":0.0,
            "targets":[{"target":{"predicate_type":"minecraft:block_match", "block":"minecraft:stone"}, "state":"minecraft:diamond_ore"}]
        }, "placement":[{"type":"minecraft:count", "count":4},{"type":"minecraft:biome"}]})).unwrap();
        PlacedDecorationFeature::from_placement(&mut Data, placement, &mut |v| {
            Ok(state(
                v.as_str().unwrap().strip_prefix("minecraft:").unwrap(),
            ))
        })
        .unwrap()
    }

    #[test]
    fn placed_vein_filters_biomes_and_executes_all_successful_positions() {
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(7);
        let mut env = PlacementTestEnv {
            allow: false,
            fail_after: None,
            calls: 0,
        };
        assert!(
            !region
                .place_placed(
                    &inline_placed_ore(),
                    &mut random,
                    [7, 4, 7],
                    0,
                    16,
                    &mut env
                )
                .unwrap()
        );
        assert_eq!(env.calls, 4);
        env.allow = true;
        assert!(
            region
                .place_placed(
                    &inline_placed_ore(),
                    &mut random,
                    [7, 4, 7],
                    0,
                    16,
                    &mut env
                )
                .unwrap()
        );
        assert_eq!(
            env.calls, 8,
            "success must not stop later feature placements"
        );
        region.finish().unwrap();
        let after = terrain.snapshot();
        assert!((0..16).any(|x| (0..16).any(|z| {
            (0..8).any(|y| after.block(x, y, z).unwrap() != before.block(x, y, z).unwrap())
        })));
    }

    #[test]
    fn placement_failure_discards_earlier_writes_and_random_changes() {
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(7);
        let mut expected = random.clone();
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: Some(2),
            calls: 0,
        };
        assert!(
            region
                .place_placed(
                    &inline_placed_ore(),
                    &mut random,
                    [7, 4, 7],
                    0,
                    16,
                    &mut env
                )
                .is_err()
        );
        assert_eq!(env.calls, 2);
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    struct ScheduledData {
        heightmap: Option<&'static str>,
    }
    impl ScheduledData {
        fn definition(&self) -> Value {
            let mut placement = vec![
                json!({"type":"minecraft:count","count":4}),
                json!({"type":"minecraft:in_square"}),
            ];
            if self.heightmap == Some("no_height") {
                // Exercise the equality case where a distinct feature can match.
            } else if self.heightmap == Some("filter") {
                placement.push(json!({"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:matching_blocks","blocks":"minecraft:stone"}}));
            } else if self.heightmap == Some("true_filter") {
                placement.push(json!({"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:true"}}));
            } else if let Some(kind) = self.heightmap {
                placement.push(json!({"type":"minecraft:heightmap","heightmap":kind}));
            } else {
                placement.push(json!({"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":4},"max_inclusive":{"absolute":4}}}));
            }
            placement.push(json!({"type":"minecraft:biome"}));
            json!({"feature":{"type":"minecraft:ore","size":33,"discard_chance_on_air_exposure":0.0,"targets":[{"target":{"predicate_type":"minecraft:block_match","block":"minecraft:stone"},"state":"minecraft:diamond_ore"}]},"placement":placement})
        }
    }
    impl ResourceProvider for ScheduledData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            let value = match path {
                "data/minecraft/worldgen/biome/plains.json" => {
                    Some(json!({"features":[["test:ore",self.definition()]]}))
                }
                "data/minecraft/worldgen/biome/desert.json" => {
                    Some(json!({"features":[[self.definition()]]}))
                }
                "data/minecraft/worldgen/biome/forest.json" => Some(json!({"features":[[]]})),
                "data/test/worldgen/placed_feature/ore.json" => Some(self.definition()),
                _ => None,
            };
            match value {
                Some(value) => Ok(Some(serde_json::to_vec(&value)?)),
                None => Data.read_data(path),
            }
        }
    }
    fn scheduled_fixture(
        kind: Option<&'static str>,
    ) -> (
        ScheduledData,
        FeatureSchedule,
        super::super::BiomeNeighborhood,
    ) {
        let mut data = ScheduledData { heightmap: kind };
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let schedule =
            FeatureSchedule::load(&mut data, std::slice::from_ref(&plains), &registry).unwrap();
        let mut neighborhood = super::super::BiomeNeighborhood::new(0, 0, 16).unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                neighborhood
                    .insert(Arc::new(
                        super::super::QuartBiomeChunk::from_values(
                            [x, z],
                            0,
                            0,
                            16,
                            vec![plains.clone(); 64],
                        )
                        .unwrap(),
                    ))
                    .unwrap();
            }
        }
        (data, schedule, neighborhood)
    }
    fn fixture_plan<'a>(
        schedule: &'a FeatureSchedule,
        neighborhood: &'a super::super::BiomeNeighborhood,
    ) -> DecorationPlan<'a> {
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let mut random = DecorationRandom::new(0);
        DecorationPlan {
            origin: [0, 0, 0],
            decoration_seed: random.set_decoration_seed(0, 0, 0),
            features: schedule.select([&plains]),
            schedule,
            biomes: neighborhood,
            generator_min_y: 0,
            generator_height: 16,
        }
    }
    #[test]
    fn environment_scan_reads_generated_blocks_and_real_world_bounds() {
        let (_, schedule, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let feature=PlacedFeature::decode(&json!({"feature":"minecraft:lake_lava","placement":[{"type":"minecraft:environment_scan","direction_of_search":"down","max_steps":32,"target_condition":{"type":"minecraft:all_of","predicates":[{"type":"minecraft:not","predicate":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"}},{"type":"minecraft:inside_world_bounds","offset":[0,-5,0]}]}}]})).unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut allow = |_: &FeatureKey, _: &ResourceId| Ok(true);
        let mut env = PlannedDecorationEnvironment {
            region: &mut region,
            plan: &plan,
            key: plan.features[0].feature,
            biome_allows: &mut allow,
        };
        let mut random = DecorationRandom::new(42);
        let mut scan = feature.positions([8, 12, 8], -64, 384).unwrap();
        assert_eq!(scan.next(&mut random, &mut env).unwrap(), Some([8, 7, 8]));
        let mut outside = feature.positions([8, 3, 8], -64, 384).unwrap();
        assert_eq!(outside.next(&mut random, &mut env).unwrap(), None);
    }
    fn resolve_test(v: &Value) -> Result<BlockState> {
        Ok(state(
            v.as_str()
                .or_else(|| v.get("id").and_then(Value::as_str))
                .context("expected test block")?
                .strip_prefix("minecraft:")
                .context("expected minecraft namespace")?,
        ))
    }

    #[test]
    fn nested_selector_resolves_tags_and_rejects_cycles() {
        struct GraphData {
            cycle: bool,
        }
        impl ResourceProvider for GraphData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                ensure!(folder == "worldgen/placed_feature", "unexpected folder");
                Ok(["test:a", "test:b"]
                    .into_iter()
                    .map(|v| ResourceId::parse(v).unwrap())
                    .collect())
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/test/worldgen/placed_feature/root.json" => {
                        json!({"feature":"test:root","placement":[]})
                    }
                    "data/test/worldgen/feature/root.json" => {
                        json!({"type":"minecraft:simple_random_selector","features":if self.cycle {json!([{"feature":"test:root","placement":[]}])} else {json!("#test:flowers")}})
                    }
                    "data/test/tags/worldgen/placed_feature/flowers.json" => {
                        json!({"values":["test:a","test:b"]})
                    }
                    "data/test/worldgen/placed_feature/a.json" => {
                        json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:melon"}},"placement":[]})
                    }
                    "data/test/worldgen/placed_feature/b.json" => {
                        json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:pumpkin"}},"placement":[]})
                    }
                    _ => return Ok(None),
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
        }
        let key = ResourceId::parse("test:root").unwrap();
        let feature =
            PlacedDecorationFeature::load(&mut GraphData { cycle: false }, &key, &mut resolve_test)
                .unwrap();
        assert!(matches!(feature.feature,ConfiguredDecoration::Selector(ref v) if v.len()==2));
        let error =
            PlacedDecorationFeature::load(&mut GraphData { cycle: true }, &key, &mut resolve_test)
                .err()
                .unwrap()
                .to_string();
        assert!(error.contains("cyclic"), "{error}");

        let nested = json!({"feature":{"type":"minecraft:simple_random_selector","features":[{"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:melon"}},"placement":[{"type":"minecraft:biome"}]}]},"placement":[]});
        let placed = PlacedDecorationFeature::from_placement(
            &mut Data,
            PlacedFeature::decode(&nested).unwrap(),
            &mut resolve_test,
        )
        .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let mut random = DecorationRandom::new(0);
        let mut expected = random.clone();
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: None,
            calls: 0,
        };
        assert!(
            region
                .place_placed(&placed, &mut random, [8, 8, 8], 0, 16, &mut env)
                .is_err()
        );
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn nested_selectors_match_java_placement_and_randomness() {
        use std::{path::PathBuf, process::Command};
        fn run(command: &mut Command) -> String {
            let out = command.output().unwrap();
            assert!(
                out.status.success(),
                "selector oracle failed: {}\n{}",
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            String::from_utf8(out.stdout).unwrap()
        }
        fn leaf(name: &str, count: i32, filter: bool) -> Value {
            let mut placement = vec![
                json!({"type":"minecraft:count","count":count}),
                json!({"type":"minecraft:offset","x":{"type":"minecraft:uniform","min_inclusive":-2,"max_inclusive":2},"y":0,"z":{"type":"minecraft:uniform","min_inclusive":-2,"max_inclusive":2}}),
            ];
            if filter {
                placement.push(json!({"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"}}));
            }
            json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":format!("minecraft:{name}")}},"placement":placement})
        }
        fn definition(kind: i32) -> Value {
            let a = leaf("melon", 3, true);
            let b = leaf("pumpkin", 2, false);
            let inner = json!({"feature":{"type":"minecraft:random_selector","features":[{"chance":0.25,"feature":a},{"chance":0.6,"feature":b}],"default":a},"placement":[{"type":"minecraft:count","count":2}]});
            let feature = if kind == 0 {
                json!({"type":"minecraft:simple_random_selector","features":[a,b,inner]})
            } else {
                json!({"type":"minecraft:random_selector","features":[{"chance":0.4,"feature":inner},{"chance":0.3,"feature":b}],"default":a})
            };
            json!({"feature":feature,"placement":[{"type":"minecraft:count","count":2}]})
        }
        struct SelectorData;
        impl ResourceProvider for SelectorData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if matches!(
                    path,
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                        | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
                ) {
                    return Ok(Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore","minecraft:melon","minecraft:pumpkin"]}"#.to_vec()));
                }
                Data.read_data(path)
            }
        }
        #[derive(Default)]
        struct Case {
            kind: i32,
            profile: i32,
            limited: bool,
            seed: i64,
            result: bool,
            next: i64,
            blocks: BTreeMap<(i32, i32), String>,
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
                let p = game.join("libraries").join(p);
                if p.is_file() {
                    paths.push(p);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("SelectorOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/SelectorOracle.java"),
        )
        .unwrap();
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SelectorOracle"));
        assert!(output.lines().any(|line| line == "N rejected"));
        let mut cases = Vec::<Case>::new();
        for line in output.lines() {
            let p: Vec<_> = line.split_whitespace().collect();
            match p.as_slice() {
                ["C", kind, profile, limited, seed, result, next] => cases.push(Case {
                    kind: kind.parse().unwrap(),
                    profile: profile.parse().unwrap(),
                    limited: *limited == "1",
                    seed: seed.parse().unwrap(),
                    result: result.parse().unwrap(),
                    next: next.parse().unwrap(),
                    blocks: BTreeMap::new(),
                }),
                ["B", x, z, name] => {
                    cases
                        .last_mut()
                        .unwrap()
                        .blocks
                        .insert((x.parse().unwrap(), z.parse().unwrap()), (*name).into());
                }
                _ => {}
            }
        }
        assert_eq!(cases.len(), 2 * 2 * 2 * 16);
        let mut registry = batch_registry();
        registry.extend(["minecraft:melon".into(), "minecraft:pumpkin".into()]);
        let tags = OreBlockTags::load(&mut SelectorData, &registry, &BTreeSet::new()).unwrap();
        let motion = HeightmapTags::load(&mut SelectorData, &registry).unwrap();
        for (index, case) in cases.iter().enumerate() {
            let placed = PlacedDecorationFeature::from_placement(
                &mut SelectorData,
                PlacedFeature::decode(&definition(case.kind)).unwrap(),
                &mut resolve_test,
            )
            .unwrap();
            let (mut terrain, setup_tags, setup_motion, _) = fixture();
            let center = ChunkPos { x: 0, z: 0 };
            if case.profile == 1 {
                let mut setup = terrain
                    .decoration_region(&setup_tags, &setup_motion, BTreeSet::from([center]))
                    .unwrap();
                for x in 6i32..=10 {
                    for z in 6i32..=10 {
                        if (x * 7 + z * 3).rem_euclid(5) == 0 {
                            setup.set_section_block([x, 8, z], &state("stone")).unwrap();
                        }
                    }
                }
                setup.finish().unwrap();
            }
            let before = terrain.snapshot();
            let writable = if case.limited {
                BTreeSet::new()
            } else {
                BTreeSet::from([center])
            };
            let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
            let mut random = DecorationRandom::new(case.seed);
            let mut env = PlacementTestEnv {
                allow: true,
                fail_after: None,
                calls: 0,
            };
            assert_eq!(
                region
                    .place_placed(&placed, &mut random, [8, 8, 8], 0, 16, &mut env)
                    .unwrap(),
                case.result,
                "result {index}"
            );
            assert_eq!(random.next_long(), case.next, "random {index}");
            region.finish().unwrap();
            let after = terrain.snapshot();
            let mut actual = BTreeMap::new();
            for x in 6..=10 {
                for z in 6..=10 {
                    let a = after.block(x, 8, z).unwrap();
                    if a != before.block(x, 8, z).unwrap() {
                        actual.insert((x, z), a.name.clone());
                    }
                }
            }
            assert_eq!(actual, case.blocks, "blocks {index}");
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_forest_flower_selector_places_vegetation() {
        struct ForestData;
        impl ResourceProvider for ForestData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/supports_vegetation.json" {
                    return Ok(Some(br#"{"values":["minecraft:stone"]}"#.to_vec()));
                }
                Data.read_data(path)
            }
        }
        struct Env;
        impl PlacementEnvironment for Env {
            fn min_y(&self) -> i32 {
                0
            }
            fn heightmap(&mut self, _: &str, _: i32, _: i32) -> Result<i32> {
                Ok(8)
            }
            fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
                Ok(true)
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let states = resources.vanilla_generated_block_registry().unwrap();
        let placed = PlacedDecorationFeature::load(
            &mut resources,
            &ResourceId::parse("minecraft:forest_flowers").unwrap(),
            &mut |v| super::super::resolve_generated_terrain_state(v, &states),
        )
        .unwrap();
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:lilac".into(),
            "minecraft:rose_bush".into(),
            "minecraft:peony".into(),
            "minecraft:lily_of_the_valley".into(),
        ]);
        let tags = OreBlockTags::load(&mut ForestData, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut ForestData, &registry).unwrap();
        let mut success = false;
        for seed in 0..256 {
            let (mut terrain, _, _, _) = fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
            let mut random = DecorationRandom::new(seed);
            if !region
                .place_placed(&placed, &mut random, [0, 0, 0], 0, 16, &mut Env)
                .unwrap()
            {
                continue;
            }
            region.finish().unwrap();
            let snapshot = terrain.snapshot();
            success = (-7..23).any(|x| {
                (-7..23).any(|z| {
                    (0..16).any(|y| {
                        snapshot.block(x, y, z).is_some_and(|s| {
                            matches!(
                                s.name.as_str(),
                                "minecraft:lilac"
                                    | "minecraft:rose_bush"
                                    | "minecraft:peony"
                                    | "minecraft:lily_of_the_valley"
                            )
                        })
                    })
                })
            });
            if success {
                break;
            }
        }
        assert!(success, "installed forest selector never placed a flower");
    }

    #[test]
    fn scheduled_ores_use_plan_biomes_and_reject_mismatched_entries() {
        let (mut data, schedule, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        assert_eq!(plan.features.len(), 2);
        let named = ScheduledDecorationFeature::load(
            &mut data,
            &schedule,
            plan.features[0].feature,
            &mut resolve_test,
        )
        .unwrap();
        let inline = ScheduledDecorationFeature::load(
            &mut data,
            &schedule,
            plan.features[1].feature,
            &mut resolve_test,
        )
        .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut calls = 0;
        let mut allows = |_: &FeatureKey, biome: &ResourceId| {
            calls += 1;
            Ok(*biome == ResourceId::parse("minecraft:plains")?)
        };
        assert!(
            region
                .place_planned(&plan, 0, &inline, &mut allows)
                .is_err()
        );
        assert!(region.place_planned(&plan, 0, &named, &mut allows).unwrap());
        region
            .place_planned(&plan, 1, &inline, &mut allows)
            .unwrap();
        assert_eq!(calls, 8);
        region.finish().unwrap();
        let current = terrain.snapshot();
        assert!((0..16).any(|x| {
            (0..16).any(|z| (0..8).any(|y| current.block(x, y, z) != previous.block(x, y, z)))
        }));
    }

    #[test]
    fn scheduled_biome_failure_discards_prior_writes() {
        let (mut data, schedule, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let ore = ScheduledDecorationFeature::load(
            &mut data,
            &schedule,
            plan.features[0].feature,
            &mut resolve_test,
        )
        .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        let mut calls = 0;
        assert!(
            region
                .place_planned(&plan, 0, &ore, &mut |_, _| {
                    calls += 1;
                    ensure!(calls < 2, "unresolved biome feature equivalence");
                    Ok(true)
                })
                .is_err()
        );
        assert_eq!(calls, 2);
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    #[test]
    fn scheduled_ore_uses_generation_heightmaps() {
        for kind in [
            "OCEAN_FLOOR_WG",
            "WORLD_SURFACE_WG",
            "WORLD_SURFACE",
            "OCEAN_FLOOR",
            "MOTION_BLOCKING",
            "MOTION_BLOCKING_NO_LEAVES",
        ] {
            let (mut data, schedule, neighborhood) = scheduled_fixture(Some(kind));
            let plan = fixture_plan(&schedule, &neighborhood);
            let ore = ScheduledDecorationFeature::load(
                &mut data,
                &schedule,
                plan.features[0].feature,
                &mut resolve_test,
            )
            .unwrap();
            let (mut terrain, tags, motion, _) = fixture();
            let before = terrain.snapshot();
            let mut region = terrain
                .decoration_region(&tags, &motion, before.chunks.keys().copied().collect())
                .unwrap();
            assert!(
                region
                    .place_planned(&plan, 0, &ore, &mut |_, _| Ok(true))
                    .unwrap()
            );
            region.finish().unwrap();
        }
    }

    #[test]
    fn scheduled_ore_resolves_biome_membership_and_rejects_ambiguity() {
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let desert = ResourceId::parse("minecraft:desert").unwrap();
        let forest = ResourceId::parse("minecraft:forest").unwrap();
        let missing = ResourceId::parse("example:missing").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let mut data = ScheduledData { heightmap: None };
        let schedule = FeatureSchedule::load(
            &mut data,
            &[plains.clone(), desert.clone(), forest.clone()],
            &registry,
        )
        .unwrap();
        let key = FeatureKey::Registered(ResourceId::parse("test:ore").unwrap());
        let ore = ScheduledDecorationFeature::load(&mut data, &schedule, &key, &mut resolve_test)
            .unwrap();
        assert!(ore.biome_allows(&plains).unwrap());
        assert!(
            !ore.biome_allows(&desert).unwrap(),
            "distinct uniform providers are not equal"
        );
        assert!(!ore.biome_allows(&forest).unwrap());
        assert!(ore.biome_allows(&missing).is_err());

        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let index = plan
            .features
            .iter()
            .position(|item| *item.feature == key)
            .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        assert!(region.place_planned_auto(&plan, index, &ore).unwrap());
        region.finish().unwrap();

        let mut other_data = ScheduledData { heightmap: None };
        let other_schedule = FeatureSchedule::load(
            &mut other_data,
            &[plains.clone(), desert.clone(), forest.clone()],
            &registry,
        )
        .unwrap();
        let other_ore = ScheduledDecorationFeature::load(
            &mut other_data,
            &other_schedule,
            &key,
            &mut resolve_test,
        )
        .unwrap();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        assert!(region.place_planned_auto(&plan, index, &other_ore).is_err());
        region.finish().unwrap();
        assert!(Arc::ptr_eq(
            &before.chunks[&native_world::loaded::ChunkPos { x: 0, z: 0 }],
            &terrain.snapshot().chunks[&native_world::loaded::ChunkPos { x: 0, z: 0 }]
        ));

        let mut ambiguous = ScheduledData {
            heightmap: Some("no_height"),
        };
        let ambiguous_schedule = FeatureSchedule::load(
            &mut ambiguous,
            &[plains.clone(), desert.clone(), forest.clone()],
            &registry,
        )
        .unwrap();
        let ambiguous_ore = ScheduledDecorationFeature::load(
            &mut ambiguous,
            &ambiguous_schedule,
            &key,
            &mut resolve_test,
        )
        .unwrap();
        assert!(ambiguous_ore.biome_allows(&plains).unwrap());
        assert!(ambiguous_ore.biome_allows(&desert).is_err());
        assert!(!ambiguous_ore.biome_allows(&forest).unwrap());

        let mut filtered = ScheduledData {
            heightmap: Some("filter"),
        };
        let filtered_schedule = FeatureSchedule::load(
            &mut filtered,
            &[plains.clone(), desert.clone(), forest.clone()],
            &registry,
        )
        .unwrap();
        let feature = ScheduledDecorationFeature::load(
            &mut filtered,
            &filtered_schedule,
            &key,
            &mut resolve_test,
        )
        .unwrap();
        assert!(feature.biome_allows(&plains).unwrap());
        assert!(!feature.biome_allows(&desert).unwrap());
        assert!(!feature.biome_allows(&forest).unwrap());

        let mut singleton = ScheduledData {
            heightmap: Some("true_filter"),
        };
        let singleton_schedule = FeatureSchedule::load(
            &mut singleton,
            &[plains.clone(), desert.clone(), forest.clone()],
            &registry,
        )
        .unwrap();
        let feature = ScheduledDecorationFeature::load(
            &mut singleton,
            &singleton_schedule,
            &key,
            &mut resolve_test,
        )
        .unwrap();
        assert!(feature.biome_allows(&plains).unwrap());
        assert!(feature.biome_allows(&desert).is_err());
        assert!(!feature.biome_allows(&forest).unwrap());
    }

    #[test]
    fn planned_feature_crosses_biome_boundary_without_ambiguous_failure() {
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let desert = ResourceId::parse("minecraft:desert").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let mut data = ScheduledData {
            heightmap: Some("filter"),
        };
        let schedule =
            FeatureSchedule::load(&mut data, &[plains.clone(), desert.clone()], &registry).unwrap();
        let key = FeatureKey::Registered(ResourceId::parse("test:ore").unwrap());
        let feature =
            ScheduledDecorationFeature::load(&mut data, &schedule, &key, &mut resolve_test)
                .unwrap();
        let mut neighborhood = super::super::BiomeNeighborhood::new(0, 0, 16).unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                let biome = if x == 1 && z == 1 { &plains } else { &desert };
                neighborhood
                    .insert(Arc::new(
                        super::super::QuartBiomeChunk::from_values(
                            [x, z],
                            0,
                            0,
                            16,
                            vec![biome.clone(); 64],
                        )
                        .unwrap(),
                    ))
                    .unwrap();
            }
        }
        let plan = fixture_plan(&schedule, &neighborhood);
        let index = plan
            .features
            .iter()
            .position(|item| *item.feature == key)
            .unwrap();
        let (mut automatic, tags, motion, _) = fixture();
        let (mut expected, _, _, _) = fixture();
        let writable = automatic
            .snapshot()
            .chunks
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut auto_region = automatic
            .decoration_region(&tags, &motion, writable.clone())
            .unwrap();
        let actual = auto_region
            .place_planned_auto(&plan, index, &feature)
            .unwrap();
        auto_region.finish().unwrap();
        let mut expected_region = expected
            .decoration_region(&tags, &motion, writable)
            .unwrap();
        let mut saw_desert = false;
        let reference = expected_region
            .place_planned(&plan, index, &feature, &mut |_, biome| {
                saw_desert |= *biome == desert;
                Ok(*biome == plains)
            })
            .unwrap();
        expected_region.finish().unwrap();
        assert!(saw_desert);
        assert_eq!(actual, reference);
        let actual = automatic.snapshot();
        let expected = expected.snapshot();
        for z in -16..32 {
            for x in -16..32 {
                for y in 0..16 {
                    assert_eq!(
                        actual.block(x, y, z),
                        expected.block(x, y, z),
                        "block at {x},{y},{z}"
                    );
                }
            }
        }
    }

    struct BatchData {
        unsupported: bool,
    }
    impl ResourceProvider for BatchData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            if path == "data/minecraft/worldgen/biome/plains.json" {
                let second = if self.unsupported {
                    json!({"feature":{"type":"minecraft:tree"},"placement":[]})
                } else {
                    json!({"feature":{"type":"minecraft:scattered_ore","size":64,"discard_chance_on_air_exposure":0.0,"targets":[{"target":{"predicate_type":"minecraft:block_match","block":"minecraft:stone"},"state":"minecraft:diamond_ore"}]},"placement":[{"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":4},"max_inclusive":{"absolute":4}}},{"type":"minecraft:biome"}]})
                };
                return Ok(Some(serde_json::to_vec(
                    &json!({"features":[["test:ore",second]]}),
                )?));
            }
            ScheduledData { heightmap: None }.read_data(path)
        }
    }
    fn batch_registry() -> BTreeSet<String> {
        [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:diamond_ore",
            "minecraft:ancient_debris",
            "minecraft:clay",
            "minecraft:short_grass",
            "minecraft:cave_air",
            "minecraft:void_air",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn prepared_batch_places_spring_and_records_fluid_tick() {
        struct SpringData;
        impl ResourceProvider for SpringData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[{"feature":{
                        "type":"minecraft:spring_feature","state":{"id":"minecraft:water"},"valid_blocks":["minecraft:stone"]
                    },"placement":[{"type":"minecraft:offset","x":8,"y":5,"z":8}]}]]}))?));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut SpringData,
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.insert("minecraft:water".into());
        let prepared = PreparedDecoration::load(&mut SpringData, &plan, &registry, &mut |v| {
            let mut result = resolve_test(v)?;
            if let Some(level) = v
                .get("properties")
                .and_then(|p| p.get("level"))
                .and_then(Value::as_str)
            {
                result.properties.insert("level".into(), level.into());
            }
            Ok(result)
        })
        .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let mut setup = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        setup.set_section_block([7, 5, 8], &state("air")).unwrap();
        setup.finish().unwrap();
        assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
        let world = terrain.snapshot();
        assert_eq!(world.block(8, 5, 8).unwrap().name, "minecraft:water");
        assert_eq!(world.saved_fluid_ticks[&center][0].position, [8, 5, 8]);
    }

    #[test]
    fn prepared_batch_places_underwater_magma_and_rolls_back_unknown_faces() {
        struct MagmaData;
        impl ResourceProvider for MagmaData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[{"feature":{
                        "type":"minecraft:underwater_magma","floor_search_range":3,"placement_radius_around_floor":0,"placement_probability_per_valid_position":1.0
                    },"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8}]}]]}))?));
                }
                if matches!(
                    path,
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                        | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
                ) {
                    return Ok(Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore","minecraft:magma_block"]}"#.to_vec()));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut MagmaData,
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.extend(["minecraft:water".into(), "minecraft:magma_block".into()]);
        let prepared =
            PreparedDecoration::load(&mut MagmaData, &plan, &registry, &mut resolve_test).unwrap();
        for unknown in [false, true] {
            let (mut terrain, tags, motion, _) = fixture();
            let center = ChunkPos { x: 0, z: 0 };
            let mut setup = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([center]))
                .unwrap();
            let mut water = state("water");
            water.properties.insert("level".into(), "0".into());
            setup.set_section_block([8, 8, 8], &water).unwrap();
            if unknown {
                setup
                    .set_section_block([9, 7, 8], &state("diamond_ore"))
                    .unwrap();
            }
            setup.finish().unwrap();
            let before = terrain.snapshot();
            if unknown {
                assert!(prepared.execute(&mut terrain, false).is_err());
                assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
                assert!(terrain.feature_postprocessing(center).is_none());
            } else {
                assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
                assert_eq!(
                    terrain.snapshot().block(8, 7, 8).unwrap().name,
                    "minecraft:magma_block"
                );
                assert_eq!(
                    terrain.feature_postprocessing(center).unwrap().sections()[0],
                    [0x888]
                );
            }
        }
    }

    #[test]
    fn prepared_sugar_cane_column_uses_active_block_and_fluid_tags() {
        struct CaneData;
        impl ResourceProvider for CaneData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[{"feature":{
                        "type":"minecraft:block_column","direction":"up","prioritize_tip":false,"allowed_placement":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"},
                        "layers":[{"height":3,"provider":{"id":"minecraft:sugar_cane","properties":{"age":"0"}}}]
                    },"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8},{"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:would_survive","state":"minecraft:sugar_cane"}}]}]]}))?));
                }
                let data = match path {
                    "data/minecraft/tags/block/supports_sugar_cane.json" => {
                        Some(br#"{"values":["minecraft:stone"]}"#.as_slice())
                    }
                    "data/minecraft/tags/block/supports_sugar_cane_adjacently.json" => {
                        Some(br#"{"values":["minecraft:frosted_ice"]}"#.as_slice())
                    }
                    "data/minecraft/tags/fluid/supports_sugar_cane_adjacently.json" => Some(
                        br#"{"values":["minecraft:water","minecraft:flowing_water"]}"#.as_slice(),
                    ),
                    _ => None,
                };
                match data {
                    Some(value) => Ok(Some(value.to_vec())),
                    None => Data.read_data(path),
                }
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut CaneData,
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:water".into(),
            "minecraft:frosted_ice".into(),
            "minecraft:sugar_cane".into(),
        ]);
        let prepared = PreparedDecoration::load(&mut CaneData, &plan, &registry, &mut |v| {
            let mut result = resolve_test(v)?;
            if result.name == "minecraft:sugar_cane" {
                result.properties.insert("age".into(), "0".into());
            }
            Ok(result)
        })
        .unwrap();
        for (neighbor, expected) in [("water", true), ("frosted_ice", true), ("stone", false)] {
            let (mut terrain, tags, motion, _) = fixture();
            let center = ChunkPos { x: 0, z: 0 };
            let mut setup = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([center]))
                .unwrap();
            let mut block = state(neighbor);
            if neighbor == "water" {
                block.properties.insert("level".into(), "0".into());
            }
            setup.set_section_block([9, 7, 8], &block).unwrap();
            setup.finish().unwrap();
            assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [expected]);
            for y in 8..11 {
                assert_eq!(
                    terrain.snapshot().block(8, y, 8).unwrap().name,
                    if expected {
                        "minecraft:sugar_cane"
                    } else {
                        "minecraft:air"
                    }
                );
            }
        }
    }

    #[test]
    fn prepared_cactus_column_respects_block_and_fluid_tags() {
        struct CactusData {
            water_is_lava: bool,
        }
        impl ResourceProvider for CactusData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[{"feature":{
                    "type":"minecraft:block_column","direction":"up","prioritize_tip":false,"allowed_placement":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"},
                    "layers":[{"height":2,"provider":{"id":"minecraft:cactus","properties":{"age":"0"}}},{"height":1,"provider":{"id":"minecraft:cactus_flower"}}]
                },"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8},{"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:would_survive","state":"minecraft:cactus"}}]}]]}))?));
                }
                if path == "data/minecraft/tags/block/supports_cactus.json" {
                    return Ok(Some(br#"{"values":["minecraft:stone"]}"#.to_vec()));
                }
                if path == "data/minecraft/tags/fluid/lava.json" {
                    return Ok(Some(if self.water_is_lava {
                        br#"{"values":["minecraft:water"]}"#.to_vec()
                    } else {
                        br#"{"values":["minecraft:lava","minecraft:flowing_lava"]}"#.to_vec()
                    }));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut CactusData {
                water_is_lava: false,
            },
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:water".into(),
            "minecraft:lava".into(),
            "minecraft:cactus".into(),
            "minecraft:cactus_flower".into(),
        ]);
        for (block, location, water_is_lava, expected) in [
            ("air", [9, 8, 8], false, true),
            ("stone", [9, 8, 8], false, false),
            ("lava", [9, 8, 8], false, false),
            ("water", [8, 9, 8], false, false),
            ("water", [9, 8, 8], true, false),
            ("water", [9, 8, 8], false, true),
        ] {
            let prepared = PreparedDecoration::load(
                &mut CactusData { water_is_lava },
                &plan,
                &registry,
                &mut |v| {
                    let mut s = resolve_test(v)?;
                    if s.name == "minecraft:cactus" {
                        s.properties.insert("age".into(), "0".into());
                    }
                    Ok(s)
                },
            )
            .unwrap();
            let (mut terrain, tags, motion, _) = fixture();
            let center = ChunkPos { x: 0, z: 0 };
            let mut setup = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([center]))
                .unwrap();
            let mut state = state(block);
            if matches!(block, "water" | "lava") {
                state.properties.insert("level".into(), "0".into());
            }
            setup.set_section_block(location, &state).unwrap();
            setup.finish().unwrap();
            assert_eq!(
                prepared.execute(&mut terrain, false).unwrap(),
                [expected],
                "{block} {location:?} {water_is_lava}"
            );
            if expected {
                let world = terrain.snapshot();
                assert_eq!(world.block(8, 8, 8).unwrap().name, "minecraft:cactus");
                assert_eq!(world.block(8, 9, 8).unwrap().name, "minecraft:cactus");
                assert_eq!(
                    world.block(8, 10, 8).unwrap().name,
                    "minecraft:cactus_flower"
                );
            }
        }
    }

    #[test]
    fn prepared_lava_lake_commits_blocks_ticks_and_postprocessing() {
        struct LakeData;
        impl ResourceProvider for LakeData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[{"feature":{
                    "type":"minecraft:lake","fluid":{"id":"minecraft:lava"},"barrier":{"id":"minecraft:stone"},
                    "can_place_feature":{"type":"minecraft:true"},"can_replace_with_air_or_fluid":{"type":"minecraft:true"},"can_replace_with_barrier":{"type":"minecraft:true"}
                },"placement":[{"type":"minecraft:offset","x":0,"y":12,"z":0}]}]]}))?));
                }
                if path == "data/minecraft/tags/fluid/water.json" {
                    return Ok(Some(
                        br#"{"values":["minecraft:water","minecraft:flowing_water"]}"#.to_vec(),
                    ));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut LakeData,
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:water".into(),
            "minecraft:lava".into(),
            "minecraft:ice".into(),
        ]);
        let prepared = PreparedDecoration::load(&mut LakeData, &plan, &registry, &mut |v| {
            let mut s = resolve_test(v)?;
            if matches!(s.name.as_str(), "minecraft:water" | "minecraft:lava") {
                s.properties.insert("level".into(), "0".into());
            }
            Ok(s)
        })
        .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut setup = terrain.decoration_region(&tags, &motion, writable).unwrap();
        for x in -8..8 {
            for z in -8..8 {
                for y in 8..12 {
                    setup.set_section_block([x, y, z], &state("stone")).unwrap();
                }
            }
        }
        setup.finish().unwrap();
        let before = terrain.snapshot();
        assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
        let world = terrain.snapshot();
        assert!((-8..8).any(|x| {
            (-8..8).any(|z| (8..12).any(|y| world.block(x, y, z).unwrap().name == "minecraft:lava"))
        }));
        assert!(
            world
                .saved_block_ticks
                .values()
                .flatten()
                .any(|tick| tick.block == "minecraft:cave_air"
                    && tick.delay == 0
                    && tick.priority == 0)
        );
        assert!(!Arc::ptr_eq(&before, &world));
        assert!(
            [
                ChunkPos { x: -1, z: -1 },
                ChunkPos { x: -1, z: 0 },
                ChunkPos { x: 0, z: -1 },
                ChunkPos { x: 0, z: 0 }
            ]
            .iter()
            .any(|pos| terrain.feature_postprocessing(*pos).is_some())
        );
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut failed = terrain.decoration_region(&tags, &motion, writable).unwrap();
        failed.schedule_lake_air_tick([0, 8, 0]).unwrap();
        assert!(
            failed
                .set_lake_block([0, 8, 0], &state("diamond_ore"))
                .is_err()
        );
        assert!(failed.finish().is_err());
        assert!(Arc::ptr_eq(&previous, &terrain.snapshot()));
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_spring_and_magma_definitions_load_with_the_block_registry() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        for name in [
            "spring_water",
            "spring_lava",
            "spring_lava_frozen",
            "spring_delta",
            "underwater_magma",
            "patch_sugar_cane",
            "patch_cactus_desert",
            "lake_lava_surface",
            "lake_lava_underground",
            "forest_flowers",
        ] {
            let id = ResourceId::parse(&format!("minecraft:{name}")).unwrap();
            PlacedDecorationFeature::load(&mut resources, &id, &mut |value| {
                super::super::resolve_generated_terrain_state(value, &registry)
            })
            .unwrap_or_else(|err| panic!("{name}: {err:#}"));
        }
    }

    #[test]
    fn prepared_ore_batch_runs_in_order_and_rejects_structures() {
        let (mut data, schedule, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let prepared =
            PreparedDecoration::load(&mut data, &plan, &batch_registry(), &mut resolve_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        assert!(prepared.execute(&mut terrain, true).is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        let results = prepared.execute(&mut terrain, false).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|&placed| placed));
        let after = terrain.snapshot();
        assert!((0..16).any(|x| {
            (0..16).any(|z| (0..8).any(|y| after.block(x, y, z) != before.block(x, y, z)))
        }));
        assert!(prepared.execute(&mut terrain, false).is_err());
        assert!(Arc::ptr_eq(&after, &terrain.snapshot()));
    }

    #[test]
    fn prepared_ore_batch_rejects_unknown_features_and_rolls_back_failures() {
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let mut data = BatchData { unsupported: true };
        let schedule =
            FeatureSchedule::load(&mut data, std::slice::from_ref(&plains), &registry).unwrap();
        let plan = fixture_plan(&schedule, &neighborhood);
        assert!(
            PreparedDecoration::load(&mut data, &plan, &batch_registry(), &mut resolve_test)
                .is_err()
        );

        let mut data = BatchData { unsupported: false };
        let schedule =
            FeatureSchedule::load(&mut data, std::slice::from_ref(&plains), &registry).unwrap();
        let plan = fixture_plan(&schedule, &neighborhood);
        let prepared =
            PreparedDecoration::load(&mut data, &plan, &batch_registry(), &mut resolve_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        assert!(prepared.execute(&mut terrain, false).is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    #[test]
    fn prepared_batch_runs_vein_then_scattered_ore() {
        struct MixedData;
        impl ResourceProvider for MixedData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let scattered = json!({"feature":{"type":"minecraft:scattered_ore","size":64,"discard_chance_on_air_exposure":0.0,"targets":[{"target":{"predicate_type":"minecraft:block_match","block":"minecraft:stone"},"state":"minecraft:ancient_debris"}]},"placement":[{"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":4},"max_inclusive":{"absolute":4}}},{"type":"minecraft:biome"}]});
                    return Ok(Some(serde_json::to_vec(
                        &json!({"features":[["test:ore",scattered]]}),
                    )?));
                }
                ScheduledData { heightmap: None }.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let schedule =
            FeatureSchedule::load(&mut MixedData, std::slice::from_ref(&plains), &registry)
                .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let batch =
            PreparedDecoration::load(&mut MixedData, &plan, &batch_registry(), &mut resolve_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        let results = batch.execute(&mut terrain, false).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[1]);
        let world = terrain.snapshot();
        assert!((-8..8).any(|x| (-8..8).any(|z| {
            (0..8).any(|y| world.block(x, y, z).unwrap().name == "minecraft:ancient_debris")
        })));
    }

    #[test]
    fn prepared_batch_runs_vein_then_disk_and_publishes_marks() {
        struct MixedData;
        impl ResourceProvider for MixedData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let disk = json!({"feature":{"type":"minecraft:disk","target":{"type":"minecraft:matching_blocks","blocks":"minecraft:stone"},"state_provider":{"id":"minecraft:clay"},"radius":1,"half_height":1},"placement":[{"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":4},"max_inclusive":{"absolute":4}}},{"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:matching_blocks","blocks":["minecraft:stone","minecraft:diamond_ore"]}},{"type":"minecraft:biome"}]});
                    return Ok(Some(serde_json::to_vec(
                        &json!({"features":[["test:ore",disk]]}),
                    )?));
                }
                ScheduledData { heightmap: None }.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let schedule =
            FeatureSchedule::load(&mut MixedData, std::slice::from_ref(&plains), &registry)
                .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut incomplete = batch_registry();
        incomplete.remove("minecraft:stone");
        assert!(
            PreparedDecoration::load(&mut MixedData, &plan, &incomplete, &mut resolve_test)
                .is_err()
        );
        let batch =
            PreparedDecoration::load(&mut MixedData, &plan, &batch_registry(), &mut resolve_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        let results = batch.execute(&mut terrain, false).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[1]);
        assert!(terrain.snapshot().chunks.keys().any(|p| {
            terrain
                .feature_postprocessing(*p)
                .is_some_and(|v| v.sections().iter().any(|s| !s.is_empty()))
        }));
        assert!((-2..=2).any(|x| (-2..=2).any(|z| {
            (0..8).any(|y| terrain.snapshot().block(x, y, z).unwrap().name == "minecraft:clay")
        })));
    }

    #[test]
    fn prepared_batch_places_grass_after_ore_using_noise_and_pack_tags() {
        struct MixedData;
        impl ResourceProvider for MixedData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let grass = json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:short_grass"},"schedule_tick":true},"placement":[
                        {"type":"minecraft:noise_threshold_count","noise_level":0.0,"below_noise":4,"above_noise":4},
                        {"type":"minecraft:in_square"},
                        {"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":8},"max_inclusive":{"absolute":8}}},
                        {"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"}},
                        {"type":"minecraft:biome"}
                    ]});
                    return Ok(Some(serde_json::to_vec(
                        &json!({"features":[["test:ore",grass]]}),
                    )?));
                }
                ScheduledData { heightmap: None }.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let schedule =
            FeatureSchedule::load(&mut MixedData, std::slice::from_ref(&plains), &registry)
                .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let prepared =
            PreparedDecoration::load(&mut MixedData, &plan, &batch_registry(), &mut resolve_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        let results = prepared.execute(&mut terrain, false).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[1]);
        let world = terrain.snapshot();
        assert!((0..16).any(|x| {
            (0..16).any(|z| world.block(x, 8, z).unwrap().name == "minecraft:short_grass")
        }));
        let ticks = world
            .saved_block_ticks
            .get(&ChunkPos { x: 0, z: 0 })
            .unwrap();
        assert!(!ticks.is_empty());
        assert!(
            ticks
                .iter()
                .all(|tick| tick.block == "minecraft:short_grass"
                    && tick.delay == 0
                    && tick.priority == 0)
        );
    }

    #[test]
    fn simple_ticks_are_deduplicated_and_use_the_actual_block() {
        let (mut terrain, _, motion, _) = fixture();
        let tags = OreBlockTags::load(
            &mut Data,
            &batch_registry(),
            &BTreeSet::from([ResourceId::parse("minecraft:supports_vegetation").unwrap()]),
        )
        .unwrap();
        let before = terrain.snapshot();
        let center = ChunkPos { x: 0, z: 0 };
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        let feature=SimpleBlockFeature::decode(&json!({"type":"minecraft:simple_block","to_place":{"id":"minecraft:short_grass"},"schedule_tick":true}),&mut resolve_test).unwrap();
        let mut random = DecorationRandom::new(0);
        assert!(feature.place(&mut region, &mut random, [0, 8, 0]).unwrap());
        assert!(feature.place(&mut region, &mut random, [0, 8, 0]).unwrap());
        // A feature outside the write radius still schedules its current block.
        assert!(feature.place(&mut region, &mut random, [16, 8, 0]).unwrap());
        region.schedule_simple_tick([0, -1, 0]).unwrap();
        region.finish().unwrap();
        let after = terrain.snapshot();
        assert!(before.saved_block_ticks.is_empty());
        assert_eq!(
            after.saved_block_ticks[&center],
            [
                SavedBlockTick {
                    block: "minecraft:short_grass".into(),
                    position: [0, 8, 0],
                    delay: 0,
                    priority: 0
                },
                SavedBlockTick {
                    block: "minecraft:void_air".into(),
                    position: [0, -1, 0],
                    delay: 0,
                    priority: 0
                },
            ]
        );
        assert_eq!(
            after.saved_block_ticks[&ChunkPos { x: 1, z: 0 }],
            [SavedBlockTick {
                block: "minecraft:air".into(),
                position: [16, 8, 0],
                delay: 0,
                priority: 0
            },]
        );
        let mut next = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        next.schedule_simple_tick([0, 8, 0]).unwrap();
        next.set_section_block([0, 8, 0], &state("stone")).unwrap();
        next.schedule_simple_tick([0, 8, 0]).unwrap();
        next.schedule_simple_tick([0, 8, 0]).unwrap();
        next.finish().unwrap();
        let current = terrain.snapshot();
        let ticks = &current.saved_block_ticks[&center];
        assert_eq!(ticks.len(), 3);
        assert_eq!(
            ticks[2],
            SavedBlockTick {
                block: "minecraft:stone".into(),
                position: [0, 8, 0],
                delay: 0,
                priority: 0
            }
        );
        assert_eq!(after.saved_block_ticks[&center].len(), 2);
    }

    #[test]
    fn failed_simple_attempt_discards_scheduled_ticks() {
        let (mut terrain, _, motion, _) = fixture();
        let tags = OreBlockTags::load(
            &mut Data,
            &batch_registry(),
            &BTreeSet::from([ResourceId::parse("minecraft:supports_vegetation").unwrap()]),
        )
        .unwrap();
        let before = terrain.snapshot();
        let center = ChunkPos { x: 0, z: 0 };
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        let feature=SimpleBlockFeature::decode(&json!({"type":"minecraft:simple_block","to_place":{"id":"minecraft:short_grass"},"schedule_tick":true}),&mut resolve_test).unwrap();
        let invalid = SimpleBlockFeature::decode(
            &json!({"type":"minecraft:simple_block","to_place":{"id":"minecraft:diamond_ore"}}),
            &mut resolve_test,
        )
        .unwrap();
        let mut random = DecorationRandom::new(0);
        assert!(feature.place(&mut region, &mut random, [0, 8, 0]).unwrap());
        assert!(invalid.place(&mut region, &mut random, [1, 8, 0]).is_err());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        assert!(terrain.snapshot().saved_block_ticks.is_empty());
    }

    #[test]
    fn failed_disk_write_discards_blocks_and_marks() {
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let center = ChunkPos { x: 0, z: 0 };
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        assert!(region.set_disk_block([0, 4, 0], &state("clay")).unwrap());
        region.mark_postprocessing([0, 5, 0]).unwrap();
        assert!(
            region
                .set_disk_block([0, 5, 0], &state("diamond_ore"))
                .is_err()
        );
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        assert!(terrain.feature_postprocessing(center).is_none());
    }

    #[test]
    fn feature_marks_follow_carvers_without_losing_duplicates() {
        let (mut terrain, tags, motion, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        assert!(terrain.postprocessing_marks_to_date(center).is_none());
        let mut earlier = native_core::postprocessing::PostProcessing::new(0, 16).unwrap();
        earlier.mark(0, 5, 0);
        terrain
            .carver_postprocessing
            .insert(center, Arc::new(earlier));
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        region.mark_postprocessing([0, 5, 0]).unwrap();
        region.mark_postprocessing([1, 5, 0]).unwrap();
        region.finish().unwrap();
        assert_eq!(
            terrain.carver_postprocessing(center).unwrap().sections()[0],
            [0x50]
        );
        assert_eq!(
            terrain
                .postprocessing_marks_to_date(center)
                .unwrap()
                .sections()[0],
            [0x50, 0x50, 0x51]
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn generated_chunk_veins_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut installed = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = installed.vanilla_generated_block_registry().unwrap();
        for name in [
            "ore_diamond",
            "ore_coal_upper",
            "ore_iron_middle",
            "ore_dirt",
            "ore_ancient_debris_large",
            "ore_debris_small",
        ] {
            PlacedDecorationFeature::load(
                &mut installed,
                &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
                &mut |v| super::super::resolve_generated_terrain_state(v, &registry),
            )
            .unwrap();
        }
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("OreVeinOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../resources/tests/java/OreVeinOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("OreVeinOracle")
            .arg("--batch"));
        let mut expected =
            BTreeMap::<(i32, String, i64), (BTreeMap<[i32; 3], String>, bool, i64)>::new();
        let mut integrated = BTreeMap::<i64, (BTreeMap<[i32; 3], String>, i64)>::new();
        let mut batches = BTreeMap::<i64, BTreeMap<[i32; 3], String>>::new();
        let mut key = None;
        let mut integrated_key = None;
        let mut batch_key = None;
        let mut write_radius = None;
        let mut preserved_heightmaps = None;
        let mut writes = BTreeMap::new();
        for line in output.lines() {
            let words: Vec<_> = line.split_whitespace().collect();
            match words.as_slice() {
                ["C", size, chance, seed] => {
                    key = Some((
                        size.parse().unwrap(),
                        (*chance).to_owned(),
                        seed.parse().unwrap(),
                    ));
                    writes.clear();
                }
                ["I", seed] => {
                    integrated_key = Some(seed.parse().unwrap());
                    writes.clear();
                }
                ["B", seed] => {
                    batch_key = Some(seed.parse().unwrap());
                    writes.clear();
                }
                ["Z", radius] => {
                    write_radius = Some(radius.parse::<i32>().unwrap());
                }
                ["H", a, b, c, d, e, f] => {
                    preserved_heightmaps = Some([
                        a.parse::<i32>().unwrap(),
                        b.parse().unwrap(),
                        c.parse().unwrap(),
                        d.parse().unwrap(),
                        e.parse().unwrap(),
                        f.parse().unwrap(),
                    ]);
                }
                ["W", x, y, z, name]
                    if key.is_some() || integrated_key.is_some() || batch_key.is_some() =>
                {
                    writes.insert(
                        [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()],
                        (*name).to_owned(),
                    );
                }
                ["R", result, next] if key.is_some() => {
                    expected.insert(
                        key.take().unwrap(),
                        (
                            std::mem::take(&mut writes),
                            result.parse().unwrap(),
                            next.parse().unwrap(),
                        ),
                    );
                }
                ["T", next] if integrated_key.is_some() => {
                    integrated.insert(
                        integrated_key.take().unwrap(),
                        (std::mem::take(&mut writes), next.parse().unwrap()),
                    );
                }
                ["U", _] if batch_key.is_some() => {
                    batches.insert(batch_key.take().unwrap(), std::mem::take(&mut writes));
                }
                _ => {}
            }
        }
        assert_eq!(write_radius, Some(1));
        assert_eq!(preserved_heightmaps, Some([8, 8, 8, 8, 8, 13]));
        struct OracleData;
        impl ResourceProvider for OracleData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/tags/block/base_stone_overworld.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        Some(br#"{"values":["minecraft:stone","minecraft:deepslate"]}"#.to_vec())
                    }
                    _ => None,
                })
            }
        }
        let registry = ["air", "stone", "deepslate", "diamond_ore", "gold_ore"]
            .into_iter()
            .map(|x| format!("minecraft:{x}"))
            .collect();
        let mut data = OracleData;
        let motion = HeightmapTags::load(&mut data, &registry).unwrap();
        fn oracle_terrain() -> CarvedTerrainWorld {
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            world.chunks.clear();
            let ids = [
                world.intern_state(state("air")),
                world.intern_state(state("stone")),
                world.intern_state(state("deepslate")),
            ];
            for cz in 1..=4i32 {
                for cx in -3..=0i32 {
                    let pos = ChunkPos { x: cx, z: cz };
                    let mut sections = BTreeMap::new();
                    for sy in -1..=0i32 {
                        let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
                        for y in 0..16i32 {
                            for z in 0..16i32 {
                                for x in 0..16i32 {
                                    let (wx, wy, wz) = (cx * 16 + x, sy * 16 + y, cz * 16 + z);
                                    let v = wx * 31 + wy * 17 + wz * 13;
                                    blocks[(x + z * 16 + y * 256) as usize] = ids[if v & 7 == 0 {
                                        0
                                    } else if wy < 0 {
                                        2
                                    } else {
                                        1
                                    }];
                                }
                            }
                        }
                        sections.insert(
                            sy as i8,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        );
                    }
                    world.chunks.insert(
                        pos,
                        Arc::new(Chunk {
                            position: pos,
                            status: Some("minecraft:carvers".into()),
                            block_light_correct: false,
                            sections,
                        }),
                    );
                }
            }
            terrain.min_y = -16;
            terrain.height = 32;
            terrain
        }
        let mut checked = 0;
        for size in [3, 17, 64] {
            for chance in [0.0f32, 0.5, 1.0] {
                let feature = OreVein::decode(&json!({"type":"minecraft:ore","size":size,"discard_chance_on_air_exposure":chance,"targets":[
                    {"state":"minecraft:diamond_ore","target":{"predicate_type":"minecraft:all_of","rules":[{"predicate_type":"minecraft:tag_match","tag":"minecraft:base_stone_overworld"},{"predicate_type":"minecraft:height_match","min_inclusive":-2,"max_inclusive":2},{"predicate_type":"minecraft:not","rule":{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.25}}]}},
                    {"state":"minecraft:gold_ore","target":{"predicate_type":"minecraft:any_of","rules":[{"predicate_type":"minecraft:block_match","block":"minecraft:deepslate"},{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.5}]}}
                ]}), &mut |v: &Value| Ok(state(v.as_str().unwrap().strip_prefix("minecraft:").unwrap()))).unwrap();
                let tags =
                    OreBlockTags::load(&mut data, &registry, &feature.required_tags()).unwrap();
                for seed in [1i64, 2, 5, 7] {
                    let mut terrain = oracle_terrain();
                    let previous = terrain.snapshot();
                    let writable = previous.chunks.keys().copied().collect();
                    let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
                    let mut random = DecorationRandom::new(0);
                    let deco = random.set_decoration_seed(seed, -32, 48);
                    random.set_feature_seed(deco, 7, 3);
                    let result = region
                        .place(
                            &feature,
                            &mut random,
                            [-16, if seed % 2 == 0 { -1 } else { 3 }, 32],
                        )
                        .unwrap();
                    region.finish().unwrap();
                    let world = terrain.snapshot();
                    let mut actual = BTreeMap::new();
                    for z in 16..80 {
                        for x in -48..16 {
                            for y in -16..16 {
                                let after = world.block(x, y, z).unwrap();
                                if previous.block(x, y, z).unwrap() != after {
                                    actual.insert([x, y, z], after.name.clone());
                                }
                            }
                        }
                    }
                    let key = (size, format!("{chance:.1}"), seed);
                    let (java_writes, java_result, java_next) = &expected[&key];
                    assert_eq!(&actual, java_writes, "block writes {key:?}");
                    assert_eq!(result, *java_result, "feature result {key:?}");
                    assert_eq!(random.next_long(), *java_next, "random stream {key:?}");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 36);

        let ore = json!({"type":"minecraft:ore","size":9,"discard_chance_on_air_exposure":0.5,"targets":[
            {"state":"minecraft:diamond_ore","target":{"predicate_type":"minecraft:all_of","rules":[{"predicate_type":"minecraft:tag_match","tag":"minecraft:base_stone_overworld"},{"predicate_type":"minecraft:height_match","min_inclusive":-2,"max_inclusive":2},{"predicate_type":"minecraft:not","rule":{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.25}}]}},
            {"state":"minecraft:gold_ore","target":{"predicate_type":"minecraft:any_of","rules":[{"predicate_type":"minecraft:block_match","block":"minecraft:deepslate"},{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.5}]}}
        ]});
        let batch_ore = ore.clone();
        let placement = PlacedFeature::decode(&json!({"feature":ore,"placement":[
            {"type":"minecraft:count","count":4},{"type":"minecraft:in_square"},
            {"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":-5},"max_inclusive":{"absolute":5}}}
        ]})).unwrap();
        let placed = PlacedDecorationFeature::from_placement(&mut data, placement, &mut |v| {
            Ok(state(
                v.as_str().unwrap().strip_prefix("minecraft:").unwrap(),
            ))
        })
        .unwrap();
        let tags = OreBlockTags::load(&mut data, &registry, &placed.required_tags()).unwrap();
        for seed in 0..8i64 {
            let mut terrain = oracle_terrain();
            let previous = terrain.snapshot();
            let writable = previous.chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
            let mut random = DecorationRandom::new(0);
            let deco = random.set_decoration_seed(seed, -32, 48);
            random.set_feature_seed(deco, 7, 3);
            let mut env = PlacementTestEnv {
                allow: true,
                fail_after: None,
                calls: 0,
            };
            region
                .place_placed(&placed, &mut random, [-32, 0, 48], -16, 32, &mut env)
                .unwrap();
            region.finish().unwrap();
            let world = terrain.snapshot();
            let mut actual = BTreeMap::new();
            for z in 16..80 {
                for x in -48..16 {
                    for y in -16..16 {
                        let after = world.block(x, y, z).unwrap();
                        if previous.block(x, y, z).unwrap() != after {
                            actual.insert([x, y, z], after.name.clone());
                        }
                    }
                }
            }
            let (writes, next) = &integrated[&seed];
            assert_eq!(&actual, writes, "integrated blocks {seed}");
            assert_eq!(random.next_long(), *next, "integrated random stream {seed}");
        }
        let key = FeatureKey::Registered(ResourceId::parse("example:ore").unwrap());
        let (_, java_schedule, _) = scheduled_fixture(None);
        let scheduled = ScheduledDecorationFeature {
            key: key.clone(),
            independent_modifier: placed.placement.has_independent_modifier(),
            holder_index: None,
            placed,
            schedule: &java_schedule,
        };
        for seed in 0..8i64 {
            let mut terrain = oracle_terrain();
            terrain.seed = seed;
            let previous = terrain.snapshot();
            let mut neighborhood = super::super::BiomeNeighborhood::new(seed, -16, 32).unwrap();
            for z in 2..=4 {
                for x in -3..=-1 {
                    let biomes = vec![ResourceId::parse("minecraft:plains").unwrap(); 128];
                    neighborhood
                        .insert(Arc::new(
                            super::super::QuartBiomeChunk::from_values(
                                [x, z],
                                seed,
                                -16,
                                32,
                                biomes,
                            )
                            .unwrap(),
                        ))
                        .unwrap();
                }
            }
            let mut random = DecorationRandom::new(0);
            let decoration_seed = random.set_decoration_seed(seed, -32, 48);
            let plan = DecorationPlan {
                origin: [-32, -16, 48],
                decoration_seed,
                features: vec![native_resources::feature_order::SelectedFeature {
                    step: 3,
                    index: 7,
                    feature: &key,
                }],
                schedule: &java_schedule,
                biomes: &neighborhood,
                generator_min_y: -16,
                generator_height: 32,
            };
            let mut region = terrain
                .decoration_region(&tags, &motion, previous.chunks.keys().copied().collect())
                .unwrap();
            region
                .place_planned(&plan, 0, &scheduled, &mut |_, _| {
                    bail!("fixture contains no biome filter")
                })
                .unwrap();
            region.finish().unwrap();
            let world = terrain.snapshot();
            let mut actual = BTreeMap::new();
            for z in 16..80 {
                for x in -48..16 {
                    for y in -16..16 {
                        let after = world.block(x, y, z).unwrap();
                        if previous.block(x, y, z).unwrap() != after {
                            actual.insert([x, y, z], after.name.clone());
                        }
                    }
                }
            }
            assert_eq!(
                &actual, &integrated[&seed].0,
                "planned Java ore blocks {seed}"
            );
        }
        struct OracleBatchData(Value);
        impl ResourceProvider for OracleBatchData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/minecraft/worldgen/biome/plains.json" => {
                        Some(json!({"features":[["test:ore0","test:ore1"]]}))
                    }
                    "data/test/worldgen/placed_feature/ore0.json"
                    | "data/test/worldgen/placed_feature/ore1.json" => Some(
                        json!({"feature":self.0,"placement":[{"type":"minecraft:count","count":4},{"type":"minecraft:in_square"},{"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":-5},"max_inclusive":{"absolute":5}}}]}),
                    ),
                    _ => None,
                };
                match value {
                    Some(value) => Ok(Some(serde_json::to_vec(&value)?)),
                    None => OracleData.read_data(path),
                }
            }
        }
        let mut batch_data = OracleBatchData(batch_ore);
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let placed_registry = [
            ResourceId::parse("test:ore0").unwrap(),
            ResourceId::parse("test:ore1").unwrap(),
        ]
        .into_iter()
        .collect();
        let schedule = FeatureSchedule::load(
            &mut batch_data,
            std::slice::from_ref(&plains),
            &placed_registry,
        )
        .unwrap();
        let block_registry = ["air", "stone", "deepslate", "diamond_ore", "gold_ore"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        for seed in 0..8i64 {
            let mut terrain = oracle_terrain();
            terrain.seed = seed;
            let before = terrain.snapshot();
            let mut neighborhood = super::super::BiomeNeighborhood::new(seed, -16, 32).unwrap();
            for z in 2..=4 {
                for x in -3..=-1 {
                    neighborhood
                        .insert(Arc::new(
                            super::super::QuartBiomeChunk::from_values(
                                [x, z],
                                seed,
                                -16,
                                32,
                                vec![plains.clone(); 128],
                            )
                            .unwrap(),
                        ))
                        .unwrap();
                }
            }
            let mut random = DecorationRandom::new(0);
            let plan = DecorationPlan {
                origin: [-32, -16, 48],
                decoration_seed: random.set_decoration_seed(seed, -32, 48),
                features: schedule.select([&plains]),
                schedule: &schedule,
                biomes: &neighborhood,
                generator_min_y: -16,
                generator_height: 32,
            };
            assert_eq!(plan.features.len(), 2);
            let batch = PreparedDecoration::load(
                &mut batch_data,
                &plan,
                &block_registry,
                &mut resolve_test,
            )
            .unwrap();
            assert_eq!(batch.execute(&mut terrain, false).unwrap().len(), 2);
            let world = terrain.snapshot();
            let mut actual = BTreeMap::new();
            for z in 16..80 {
                for x in -48..16 {
                    for y in -16..16 {
                        let after = world.block(x, y, z).unwrap();
                        if before.block(x, y, z).unwrap() != after {
                            actual.insert([x, y, z], after.name.clone());
                        }
                    }
                }
            }
            assert_eq!(&actual, &batches[&seed], "ordered Java ore batch {seed}");
        }
    }
}
