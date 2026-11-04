//! Apply supported decoration features to intermediate generated chunks.
//!
//! The surrounding decoration scheduler must still supply Java's feature order,
//! random seed, writable region and complete terrain-stage neighborhood.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
};

use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use native_resources::{
    block_column_feature::{BlockColumnFeature, ColumnWorld},
    disk_feature::{BlockPredicate, DiskFeature, DiskWorld, is_simple_carpet_id},
    feature_order::{FeatureHolderIndex, FeatureKey, FeatureSchedule, FeatureTags},
    freeze_feature::freeze_top_layer,
    geode_feature::{Direction as GeodeDirection, GeodeEnvironment, GeodeFeature},
    heightmap_tags::HeightmapTags,
    huge_mushroom_feature::{HugeMushroomFeature, MushroomWorld},
    lake_feature::{LakeFeature, LakeWorld},
    monster_room::{RoomBlock, RoomMob, place_monster_room},
    multiface_growth::{BlockSet as MultifaceBlockSet, Face as MultifaceFace, MultifaceGrowth},
    ore_feature::{ConfiguredOre, OreBlockTags, OreVein, OreVeinWorld, OreWorld},
    packs::{ResourceId, ResourceProvider},
    placed_feature::{PlacedFeature, PlacementEnvironment},
    root_system_feature::{RootSystemFeature, RootSystemWorld},
    simple_block_feature::{SimpleBlockFeature, SimpleBlockWorld},
    spring_feature::{SpringFeature, SpringWorld},
    tree_beehive::BeehiveWorld,
    tree_fallen_decorators::Direction as FallenDirection,
    tree_fallen_feature::{FallenFeatureWorld, FallenTreeFeature},
    tree_feature::{TreeFeature, TreeWorld, TreeWrite},
    tree_ground_decorator::GroundDecorationWorld,
    tree_leaf_update::{LeafUpdateWorld, finish_generated_tree},
    underwater_magma::{MagmaWorld, UnderwaterMagma, vanilla_full_occlusion},
    vegetation_patch::{VegetationPatch, VegetationPatchWorld},
    vines_feature::{VinesFeature, VinesWorld},
};
use native_world::loaded::{
    BlockState, BlockStateId, ChunkPos, SavedBlockTick, SavedFluidTick, World,
};
use serde_json::Value;

use super::{
    CarvedTerrainWorld, DecorationPlan, FreshGenerationLightTracker, StagedGenerationLightEngine,
};

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
    bubble_column_fluids: Option<&'a BTreeSet<String>>,
    motion_cache: BTreeMap<BlockStateId, bool>,
    tree_logs: Vec<[i32; 3]>,
    tree_leaves: Vec<[i32; 3]>,
    freeze_context: Option<FreezeContext<'a>>,
    raw_brightness: Option<&'a dyn Fn(&World, [i32; 3]) -> Result<u8>>,
    tree_decorations: Vec<[i32; 3]>,
    bee_nests: Arc<BTreeMap<[i32; 3], super::GeneratedBeeNest>>,
    monster_entities: Arc<BTreeMap<[i32; 3], Option<super::GeneratedMonsterRoomEntity>>>,
    pending_monster_spawners: usize,
    light_guard: Option<DecorationLightGuard<'a>>,
    tree_block_write: bool,
    column_block_write: bool,
    patch_block_write: bool,
    failed: bool,
}

struct DecorationLightGuard<'a> {
    registered: BTreeSet<ChunkPos>,
    registry: &'a native_resources::block_registry::GeneratedBlockRegistry,
    require_unchanged_light: bool,
    changed_blocks: BTreeSet<String>,
    changed_chunks: BTreeSet<ChunkPos>,
    light_replay_positions: BTreeSet<[i32; 3]>,
    light_check_order: Vec<[i32; 3]>,
    vine_light_positions: BTreeSet<[i32; 3]>,
    new_nonempty_sections: Vec<(ChunkPos, i32)>,
    non_air_counts: BTreeMap<(ChunkPos, i8), usize>,
}

enum DecorationLightTracking<'a> {
    Fresh(
        &'a mut FreshGenerationLightTracker,
        &'a native_resources::block_registry::GeneratedBlockRegistry,
    ),
    Staged(
        &'a mut StagedGenerationLightEngine,
        &'a native_resources::block_registry::GeneratedBlockRegistry,
    ),
}

struct FreezeContext<'a> {
    biomes: &'a super::BiomeNeighborhood,
    climates: &'a BTreeMap<ResourceId, native_resources::biome_climate::BiomeClimate>,
    support: &'a native_resources::respawn_tags::SnowSupportTags,
    sea_level: i32,
    block_light: Option<&'a dyn Fn([i32; 3]) -> Result<u8>>,
}

/// Final 26.3 terrain heightmaps in surface, ocean, motion and no-leaves order.
#[derive(Clone)]
pub(super) struct FinalHeightmaps {
    pub(super) values: Box<[[i32; 256]; 4]>,
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
    Tree(TreeFeature<BlockState>),
    FallenTree(FallenTreeFeature<BlockState>),
    Freeze,
    MonsterRoom,
    Geode(GeodeFeature<BlockState>),
    Multiface(MultifaceGrowth),
    Vines(VinesFeature<BlockState>),
    HugeMushroom(HugeMushroomFeature<BlockState>),
    VegetationPatch(VegetationPatch<BlockState>, Box<PlacedDecorationFeature>),
    RootSystem(RootSystemFeature<BlockState>, Box<PlacedDecorationFeature>),
    Selector(Vec<PlacedDecorationFeature>),
    BooleanSelector(Box<PlacedDecorationFeature>, Box<PlacedDecorationFeature>),
    WeightedSelector(
        Vec<(f32, PlacedDecorationFeature)>,
        Box<PlacedDecorationFeature>,
    ),
    WeightedListSelector(Vec<(i32, PlacedDecorationFeature)>, i32),
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
            Self::Tree(v) => v.required_blocks(),
            Self::FallenTree(v) => v.required_blocks(),
            Self::Geode(v) => v.required_blocks(),
            Self::Multiface(v) => v.required_blocks(),
            Self::Vines(_) => BTreeSet::from([ResourceId::parse("minecraft:vine").unwrap()]),
            Self::HugeMushroom(v) => v.required_blocks(),
            Self::VegetationPatch(v, nested) => {
                let mut blocks = v.required_blocks();
                blocks.extend(nested.required_blocks());
                blocks
            }
            Self::RootSystem(v, nested) => {
                let mut blocks = v.required_blocks();
                blocks.extend(nested.required_blocks());
                blocks
            }
            Self::Freeze => ["minecraft:ice", "minecraft:snow"]
                .into_iter()
                .map(|v| ResourceId::parse(v).unwrap())
                .collect(),
            Self::MonsterRoom => [
                "minecraft:cave_air",
                "minecraft:cobblestone",
                "minecraft:mossy_cobblestone",
                "minecraft:chest",
                "minecraft:spawner",
            ]
            .into_iter()
            .map(|v| ResourceId::parse(v).unwrap())
            .collect(),
            Self::Selector(values) => values
                .iter()
                .flat_map(PlacedDecorationFeature::required_blocks)
                .collect(),
            Self::BooleanSelector(a, b) => {
                let mut blocks = a.required_blocks();
                blocks.extend(b.required_blocks());
                blocks
            }
            Self::WeightedSelector(values, default) => {
                let mut blocks = default.required_blocks();
                for (_, feature) in values {
                    blocks.extend(feature.required_blocks());
                }
                blocks
            }
            Self::WeightedListSelector(values, _) => values
                .iter()
                .flat_map(|(_, v)| v.required_blocks())
                .collect(),
            _ => BTreeSet::new(),
        }
    }
    fn needs_survival(&self, name: &str) -> bool {
        match self {
            Self::Disk(v) => v.needs_survival(name),
            Self::Simple(v) => v.needs_survival(name),
            Self::Column(v) => v.needs_survival(name),
            Self::Lake(v) => v.needs_survival(name),
            Self::Geode(v) => v.needs_survival(name),
            Self::Tree(v) => v.needs_survival(name),
            Self::HugeMushroom(v) => v.needs_survival(name),
            Self::VegetationPatch(v, nested) => {
                v.needs_survival(name) || nested.needs_survival(name)
            }
            Self::RootSystem(v, nested) => v.needs_survival(name) || nested.needs_survival(name),
            Self::Selector(values) => values.iter().any(|v| v.needs_survival(name)),
            Self::BooleanSelector(a, b) => a.needs_survival(name) || b.needs_survival(name),
            Self::WeightedSelector(values, default) => {
                default.needs_survival(name) || values.iter().any(|(_, v)| v.needs_survival(name))
            }
            Self::WeightedListSelector(values, _) => {
                values.iter().any(|(_, v)| v.needs_survival(name))
            }
            _ => false,
        }
    }
    fn needs_water_fluids(&self) -> bool {
        match self {
            Self::Lake(_) => true,
            Self::Simple(v) => v.possible_states().iter().any(|s| {
                matches!(
                    s.name.as_str(),
                    "minecraft:tall_seagrass"
                        | "minecraft:small_dripleaf"
                        | "minecraft:spore_blossom"
                )
            }),
            Self::VegetationPatch(_, nested) => nested.needs_water_fluids(),
            Self::RootSystem(_, _) => true,
            Self::Selector(values) => values
                .iter()
                .any(PlacedDecorationFeature::needs_water_fluids),
            Self::BooleanSelector(a, b) => a.needs_water_fluids() || b.needs_water_fluids(),
            Self::WeightedSelector(values, default) => {
                default.needs_water_fluids() || values.iter().any(|(_, v)| v.needs_water_fluids())
            }
            Self::WeightedListSelector(values, _) => {
                values.iter().any(|(_, v)| v.needs_water_fluids())
            }
            _ => false,
        }
    }
    fn needs_freeze(&self) -> bool {
        match self {
            Self::Freeze => true,
            Self::VegetationPatch(_, nested) => nested.needs_freeze(),
            Self::RootSystem(_, nested) => nested.needs_freeze(),
            Self::Selector(values) => values.iter().any(PlacedDecorationFeature::needs_freeze),
            Self::BooleanSelector(a, b) => a.needs_freeze() || b.needs_freeze(),
            Self::WeightedSelector(values, default) => {
                default.needs_freeze() || values.iter().any(|(_, v)| v.needs_freeze())
            }
            Self::WeightedListSelector(values, _) => values.iter().any(|(_, v)| v.needs_freeze()),
            _ => false,
        }
    }
    fn needs_root_system(&self) -> bool {
        match self {
            Self::RootSystem(_, _) => true,
            Self::VegetationPatch(_, nested) => nested.feature.needs_root_system(),
            Self::Selector(values) => values.iter().any(|v| v.feature.needs_root_system()),
            Self::BooleanSelector(a, b) => {
                a.feature.needs_root_system() || b.feature.needs_root_system()
            }
            Self::WeightedSelector(values, default) => {
                default.feature.needs_root_system()
                    || values.iter().any(|(_, v)| v.feature.needs_root_system())
            }
            Self::WeightedListSelector(values, _) => {
                values.iter().any(|(_, v)| v.feature.needs_root_system())
            }
            _ => false,
        }
    }
    fn needs_tree_edges(&self) -> bool {
        match self {
            Self::Tree(_) => true,
            Self::VegetationPatch(_, nested) | Self::RootSystem(_, nested) => {
                nested.feature.needs_tree_edges()
            }
            Self::Selector(values) => values.iter().any(|v| v.feature.needs_tree_edges()),
            Self::BooleanSelector(a, b) => {
                a.feature.needs_tree_edges() || b.feature.needs_tree_edges()
            }
            Self::WeightedSelector(values, default) => {
                default.feature.needs_tree_edges()
                    || values.iter().any(|(_, v)| v.feature.needs_tree_edges())
            }
            Self::WeightedListSelector(values, _) => {
                values.iter().any(|(_, v)| v.feature.needs_tree_edges())
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
            ("minecraft", "simple_block") => Ok(Self::Simple(
                SimpleBlockFeature::decode_with_resources(provider, value, resolve)?,
            )),
            ("minecraft", "spring_feature") => {
                Ok(Self::Spring(SpringFeature::decode(value, resolve)?))
            }
            ("minecraft", "underwater_magma") => {
                Ok(Self::Magma(UnderwaterMagma::decode(value, resolve)?))
            }
            ("minecraft", "block_column") => Ok(Self::Column(
                BlockColumnFeature::decode_with_resources(provider, value, resolve)?,
            )),
            ("minecraft", "lake") => Ok(Self::Lake(LakeFeature::decode(value, resolve)?)),
            ("minecraft", "tree") => Ok(Self::Tree(TreeFeature::decode(provider, value, resolve)?)),
            ("minecraft", "fallen_tree") => Ok(Self::FallenTree(FallenTreeFeature::decode(
                provider, value, resolve,
            )?)),
            ("minecraft", "freeze_top_layer") => Ok(Self::Freeze),
            ("minecraft", "monster_room") => Ok(Self::MonsterRoom),
            ("minecraft", "geode") => Ok(Self::Geode(GeodeFeature::decode(value, resolve)?)),
            ("minecraft", "multiface_growth") => {
                Ok(Self::Multiface(MultifaceGrowth::decode(value)?))
            }
            ("minecraft", "vines") => Ok(Self::Vines(VinesFeature::decode(value, resolve)?)),
            ("minecraft", "huge_brown_mushroom" | "huge_red_mushroom") => Ok(Self::HugeMushroom(
                HugeMushroomFeature::decode(provider, value, resolve)?,
            )),
            ("minecraft", "vegetation_patch" | "waterlogged_vegetation_patch") => {
                let patch = VegetationPatch::decode(provider, value, resolve)?;
                let nested = PlacedDecorationFeature::load_nested(
                    provider,
                    patch.vegetation(),
                    resolve,
                    state,
                    depth + 1,
                )?;
                Ok(Self::VegetationPatch(patch, Box::new(nested)))
            }
            ("minecraft", "root_system") => {
                let root = RootSystemFeature::decode(provider, value, resolve)?;
                let nested = PlacedDecorationFeature::load_nested(
                    provider,
                    root.tree(),
                    resolve,
                    state,
                    depth + 1,
                )?;
                Ok(Self::RootSystem(root, Box::new(nested)))
            }
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
            ("minecraft", "random_boolean_selector") => {
                let a = PlacedDecorationFeature::load_nested(
                    provider,
                    value
                        .get("feature_true")
                        .context("missing true selector feature")?,
                    resolve,
                    state,
                    depth + 1,
                )?;
                let b = PlacedDecorationFeature::load_nested(
                    provider,
                    value
                        .get("feature_false")
                        .context("missing false selector feature")?,
                    resolve,
                    state,
                    depth + 1,
                )?;
                Ok(Self::BooleanSelector(Box::new(a), Box::new(b)))
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
            ("minecraft", "weighted_random_selector") => {
                let entries = value
                    .get("features")
                    .and_then(Value::as_array)
                    .context("missing weighted list features")?;
                ensure!(entries.len() <= 4096, "too many weighted list features");
                let mut features = Vec::with_capacity(entries.len());
                let mut total = 0i32;
                for entry in entries {
                    let weight = entry
                        .get("weight")
                        .and_then(Value::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .context("invalid selector weight")?;
                    ensure!(weight >= 0, "invalid selector weight");
                    total = total
                        .checked_add(weight)
                        .context("selector weight total overflow")?;
                    let nested = PlacedDecorationFeature::load_nested(
                        provider,
                        entry.get("data").context("missing weighted list feature")?,
                        resolve,
                        state,
                        depth + 1,
                    )?;
                    features.push((weight, nested));
                }
                Ok(Self::WeightedListSelector(features, total))
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
            Self::Tree(v) => {
                let mut tags = v.required_tags();
                tags.insert(ResourceId::parse("minecraft:prevents_nearby_leaf_decay").unwrap());
                tags.insert(ResourceId::parse("minecraft:snow").unwrap());
                tags.insert(ResourceId::parse("minecraft:supports_vegetation").unwrap());
                tags.insert(ResourceId::parse("minecraft:supports_azalea").unwrap());
                tags.insert(ResourceId::parse("minecraft:supports_dry_vegetation").unwrap());
                tags.insert(
                    ResourceId::parse("minecraft:enables_bubble_column_drag_down").unwrap(),
                );
                tags.insert(ResourceId::parse("minecraft:enables_bubble_column_push_up").unwrap());
                tags
            }
            Self::FallenTree(v) => v.required_tags(),
            Self::Geode(v) => v.required_tags(),
            Self::Multiface(v) => v.required_tags(),
            Self::HugeMushroom(v) => v.required_tags(),
            Self::VegetationPatch(v, nested) => {
                let mut tags = v.required_tags();
                tags.extend(nested.required_tags());
                tags
            }
            Self::RootSystem(v, nested) => {
                let mut tags = v.required_tags();
                tags.extend(nested.required_tags());
                tags
            }
            Self::Freeze | Self::Vines(_) => BTreeSet::new(),
            Self::MonsterRoom => {
                BTreeSet::from([ResourceId::parse("minecraft:features_cannot_replace").unwrap()])
            }
            Self::Selector(values) => values
                .iter()
                .flat_map(PlacedDecorationFeature::required_tags)
                .collect(),
            Self::BooleanSelector(a, b) => {
                let mut tags = a.required_tags();
                tags.extend(b.required_tags());
                tags
            }
            Self::WeightedSelector(values, default) => {
                let mut tags = default.required_tags();
                for (_, feature) in values {
                    tags.extend(feature.required_tags());
                }
                tags
            }
            Self::WeightedListSelector(values, _) => {
                values.iter().flat_map(|(_, v)| v.required_tags()).collect()
            }
            Self::Simple(v) => {
                let mut tags = v.required_tags();
                for state in v.possible_states() {
                    if matches!(simple_plant_kind(state), Ok(SimplePlantKind::SmallDripleaf)) {
                        tags.insert(
                            ResourceId::parse("minecraft:supports_small_dripleaf").unwrap(),
                        );
                        tags.insert(ResourceId::parse("minecraft:supports_vegetation").unwrap());
                    }
                    let tag = match simple_plant_kind(state) {
                        Ok(SimplePlantKind::Vegetation | SimplePlantKind::Double) => {
                            Some("minecraft:supports_vegetation")
                        }
                        Ok(SimplePlantKind::DryVegetation) => {
                            Some("minecraft:supports_dry_vegetation")
                        }
                        Ok(SimplePlantKind::Azalea) => Some("minecraft:supports_azalea"),
                        Ok(SimplePlantKind::Mushroom) => {
                            Some("minecraft:overrides_mushroom_light_requirement")
                        }
                        Ok(SimplePlantKind::Seagrass | SimplePlantKind::TallSeagrass) => {
                            Some("minecraft:cannot_support_seagrass")
                        }
                        Ok(SimplePlantKind::SporeBlossom) => {
                            Some("minecraft:unstable_bottom_center")
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
            Self::FallenTree(v) => v.place(world, random, pos),
            Self::Freeze => freeze_top_layer(world, pos),
            Self::MonsterRoom => place_monster_room(world, random, pos),
            Self::Geode(v) => v.place(world, random, world.target.seed, pos),
            Self::Multiface(v) => v.place(world, random, pos),
            Self::Vines(v) => v.place(world, pos),
            Self::HugeMushroom(v) => v.place(world, random, pos),
            Self::VegetationPatch(v, nested) => {
                v.place(world, random, pos, |world, random, origin| {
                    world.place_nested(nested, random, origin, generator_min_y, generator_height)
                })
            }
            Self::RootSystem(v, nested) => v.place(world, random, pos, |world, random, origin| {
                world.place_nested(nested, random, origin, generator_min_y, generator_height)
            }),
            Self::Tree(v) => {
                world.tree_logs.clear();
                world.tree_leaves.clear();
                world.tree_decorations.clear();
                let placed = v.place_components(world, random, pos)?;
                let mut logs = std::mem::take(&mut world.tree_logs);
                let mut leaves = std::mem::take(&mut world.tree_leaves);
                if placed && (!logs.is_empty() || !leaves.is_empty()) {
                    v.place_decorators(world, random, &logs, &leaves)?;
                }
                let mut decorations = std::mem::take(&mut world.tree_decorations);
                let result =
                    finish_generated_tree(world, placed, &logs, &leaves, &[], &decorations);
                logs.clear();
                leaves.clear();
                decorations.clear();
                world.tree_logs = logs;
                world.tree_leaves = leaves;
                world.tree_decorations = decorations;
                result
            }
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
            Self::BooleanSelector(a, b) => world.place_nested(
                if random.next_bool() { a } else { b },
                random,
                pos,
                generator_min_y,
                generator_height,
            ),
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
            Self::WeightedListSelector(values, total) => {
                if *total == 0 {
                    return Ok(false);
                }
                let mut choice = random
                    .next_int_bound(*total)
                    .context("invalid selector total")?;
                for (weight, feature) in values {
                    choice -= weight;
                    if choice < 0 {
                        return world.place_nested(
                            feature,
                            random,
                            pos,
                            generator_min_y,
                            generator_height,
                        );
                    }
                }
                bail!("weighted selector total does not match entries")
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
    bubble_column_fluids: Option<BTreeSet<String>>,
    freeze: Option<PreparedFreeze>,
}

struct PreparedFreeze {
    climates: BTreeMap<ResourceId, native_resources::biome_climate::BiomeClimate>,
    support: native_resources::respawn_tags::SnowSupportTags,
}

impl<'p, 's> PreparedDecoration<'p, 's> {
    pub(crate) fn center_chunk(&self) -> ChunkPos {
        ChunkPos {
            x: self.plan.origin[0].div_euclid(16),
            z: self.plan.origin[2].div_euclid(16),
        }
    }
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
        let mut needs_water_fluids = false;
        let mut needs_freeze = false;
        let mut needs_tree_edges = false;
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
                needs_cactus |= feature.placed.needs_survival("cactus")
                    || feature.placed.feature.needs_root_system();
                needs_water_fluids |= feature.placed.needs_water_fluids();
                needs_freeze |= feature.placed.needs_freeze();
                needs_tree_edges |= feature.placed.feature.needs_tree_edges();
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
        let lake_water_fluids = needs_water_fluids
            .then(|| native_resources::fluid_tags::load_water(provider))
            .transpose()?;
        let bubble_column_fluids = needs_tree_edges
            .then(|| native_resources::fluid_tags::load_bubble_column_occupiable(provider))
            .transpose()?;
        let freeze = if needs_freeze {
            let ids = plan
                .biomes
                .chunks
                .values()
                .flat_map(|chunk| chunk.values.iter())
                .collect::<BTreeSet<_>>();
            let mut climates = BTreeMap::new();
            for id in ids {
                let name = format!("{}:{}", id.namespace, id.path);
                let climate =
                    native_resources::biome_climate::BiomeClimate::load_world(provider, &name)?
                        .with_context(|| format!("missing biome climate {name}"))?;
                climates.insert(id.clone(), climate);
            }
            Some(PreparedFreeze {
                climates,
                support: native_resources::respawn_tags::SnowSupportTags::load(
                    provider,
                    block_registry,
                )?,
            })
        } else {
            None
        };
        Ok(Self {
            plan,
            features,
            tags,
            motion,
            sugar_cane_fluids,
            cactus_lava_fluids,
            lake_water_fluids,
            bubble_column_fluids,
            freeze,
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
        self.execute_inner(terrain, structures_enabled, None, None, None)
    }

    /// Supply the level light engine's block-light values for FEATURES-stage
    /// positions. The sampler must match Java's current generation state.
    pub fn execute_with_block_light(
        &self,
        terrain: &mut CarvedTerrainWorld,
        structures_enabled: bool,
        block_light: &dyn Fn([i32; 3]) -> Result<u8>,
    ) -> Result<Vec<bool>> {
        self.execute_inner(terrain, structures_enabled, Some(block_light), None, None)
    }

    /// Supply Java's raw brightness with zero sky subtraction for mushrooms.
    /// The sampler must reflect all earlier writes in this generation attempt.
    pub fn execute_with_raw_brightness(
        &self,
        terrain: &mut CarvedTerrainWorld,
        structures_enabled: bool,
        raw_brightness: &dyn Fn(&World, [i32; 3]) -> Result<u8>,
    ) -> Result<Vec<bool>> {
        self.execute_inner(
            terrain,
            structures_enabled,
            None,
            Some(raw_brightness),
            None,
        )
    }

    /// Supply both light queries when the selected features need them.
    pub fn execute_with_lighting(
        &self,
        terrain: &mut CarvedTerrainWorld,
        structures_enabled: bool,
        block_light: &dyn Fn([i32; 3]) -> Result<u8>,
        raw_brightness: &dyn Fn(&World, [i32; 3]) -> Result<u8>,
    ) -> Result<Vec<bool>> {
        self.execute_inner(
            terrain,
            structures_enabled,
            Some(block_light),
            Some(raw_brightness),
            None,
        )
    }

    /// Use a complete published light-engine snapshot when its visible revision
    /// is known to remain fixed throughout this decoration attempt.
    pub fn execute_with_visible_light(
        &self,
        terrain: &mut CarvedTerrainWorld,
        structures_enabled: bool,
        light: &native_world::generation_light::VisibleLightSnapshot,
    ) -> Result<Vec<bool>> {
        let block = |p| {
            light
                .block_light(p)
                .context("block-light column not captured")
        };
        let raw = |_: &World, p| {
            light
                .raw_brightness(p, 0)
                .context("raw-brightness column not captured")
        };
        self.execute_with_lighting(terrain, structures_enabled, &block, &raw)
    }

    /// Use a fresh, unpropagated light-engine revision. Checks for supported
    /// writes are drained while every source is disabled, before activation.
    /// Sections must retain their occupancy and direct sky behavior.
    pub fn execute_with_tracked_light(
        &self,
        terrain: &mut CarvedTerrainWorld,
        structures_enabled: bool,
        tracker: &mut FreshGenerationLightTracker,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<Vec<bool>> {
        let center = ChunkPos {
            x: self.plan.origin[0].div_euclid(16),
            z: self.plan.origin[2].div_euclid(16),
        };
        let mut covered = BTreeSet::new();
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                covered.insert(ChunkPos {
                    x: center.x.checked_add(dx).context("light X overflow")?,
                    z: center.z.checked_add(dz).context("light Z overflow")?,
                });
            }
        }
        let light = tracker.snapshot(terrain, covered, registry)?;
        let block = |p| {
            light
                .block_light(p)
                .context("block-light column not captured")
        };
        let raw = |_: &World, p| {
            light
                .raw_brightness(p, 0)
                .context("raw-brightness column not captured")
        };
        self.execute_inner(
            terrain,
            structures_enabled,
            Some(&block),
            Some(&raw),
            Some(DecorationLightTracking::Fresh(tracker, registry)),
        )
    }

    /// Read a staged engine revision before FEATURES begins. Registered
    /// edits must preserve light behavior and section occupancy.
    pub fn execute_with_staged_light(
        &self,
        terrain: &mut CarvedTerrainWorld,
        structures_enabled: bool,
        engine: &mut StagedGenerationLightEngine,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<Vec<bool>> {
        let center = self.center_chunk();
        let mut covered = BTreeSet::new();
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                covered.insert(ChunkPos {
                    x: center.x.checked_add(dx).context("light X overflow")?,
                    z: center.z.checked_add(dz).context("light Z overflow")?,
                });
            }
        }
        let light = engine.snapshot(terrain, covered, registry)?;
        let block = |p| {
            light
                .block_light(p)
                .context("block-light column not captured")
        };
        let raw = |_: &World, p| {
            light
                .raw_brightness(p, 0)
                .context("raw-brightness column not captured")
        };
        self.execute_inner(
            terrain,
            structures_enabled,
            Some(&block),
            Some(&raw),
            Some(DecorationLightTracking::Staged(engine, registry)),
        )
    }

    fn execute_inner(
        &self,
        terrain: &mut CarvedTerrainWorld,
        structures_enabled: bool,
        block_light: Option<&dyn Fn([i32; 3]) -> Result<u8>>,
        raw_brightness: Option<&dyn Fn(&World, [i32; 3]) -> Result<u8>>,
        mut light_tracking: Option<DecorationLightTracking<'_>>,
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
        let guard_inputs = match &light_tracking {
            Some(DecorationLightTracking::Fresh(tracker, registry)) => {
                Some((&**tracker, *registry, false))
            }
            Some(DecorationLightTracking::Staged(engine, registry)) => {
                Some((&engine.fresh, *registry, true))
            }
            None => None,
        };
        if let Some((tracker, registry, require_unchanged_light)) = guard_inputs {
            region.light_guard = Some(DecorationLightGuard {
                registered: region
                    .writable
                    .iter()
                    .filter(|pos| tracker.registered.contains_key(pos))
                    .copied()
                    .collect(),
                registry,
                require_unchanged_light,
                changed_blocks: BTreeSet::new(),
                changed_chunks: BTreeSet::new(),
                light_replay_positions: BTreeSet::new(),
                light_check_order: Vec::new(),
                vine_light_positions: BTreeSet::new(),
                new_nonempty_sections: Vec::new(),
                non_air_counts: BTreeMap::new(),
            });
        }
        region.sugar_cane_fluids = self.sugar_cane_fluids.as_ref();
        region.cactus_lava_fluids = self.cactus_lava_fluids.as_ref();
        region.lake_water_fluids = self.lake_water_fluids.as_ref();
        region.bubble_column_fluids = self.bubble_column_fluids.as_ref();
        region.raw_brightness = raw_brightness;
        region.freeze_context = self.freeze.as_ref().map(|v| FreezeContext {
            biomes: self.plan.biomes,
            climates: &v.climates,
            support: &v.support,
            sea_level: self.plan.sea_level,
            block_light,
        });
        let mut results = Vec::with_capacity(self.plan.features.len());
        for (index, selected) in self.plan.features.iter().enumerate() {
            let ore = self
                .features
                .get(selected.feature)
                .context("prepared ore is missing")?;
            results.push(region.place_planned_auto(self.plan, index, ore)?);
        }
        finish_decoration_with_light(region, &mut light_tracking)?;
        terrain.completed_decoration.insert(center);
        Ok(results)
    }
}

fn finish_decoration_with_light(
    mut region: DecorationRegion<'_>,
    light_tracking: &mut Option<DecorationLightTracking<'_>>,
) -> Result<()> {
    let mut staged_replay = None;
    let mut incremental_columns = None;
    let mut incremental_allocations = None;
    let mut fresh_sources = None;
    let mut fresh_allocations = None;
    if let Some(DecorationLightTracking::Fresh(tracker, _)) = &*light_tracking {
        let guard = region
            .light_guard
            .as_ref()
            .context("fresh light guard missing")?;
        if !guard.light_replay_positions.is_empty() {
            let max_y = tracker
                .min_y
                .checked_add(tracker.height)
                .context("light height overflow")?;
            let bounds = native_world::block_light::LightBounds::new(tracker.min_y, max_y)
                .context("unsupported light bounds")?;
            let mut sources = BTreeMap::new();
            for &p in &guard.light_replay_positions {
                let pos = chunk(p);
                if sources.contains_key(&pos) {
                    continue;
                }
                ensure!(
                    tracker.sky_sources.contains_key(&pos),
                    "feature update requires initialized sky sources"
                );
                let heights = native_world::sky_light::generated_chunk_sky_sources(
                    &region.world,
                    bounds,
                    pos,
                )
                .context("updated fresh sky sources unavailable")?;
                sources.insert(pos, Arc::from(heights));
            }
            fresh_sources = Some(sources);
            fresh_allocations = Some(guard.new_nonempty_sections.clone());
        }
    }
    if let Some(DecorationLightTracking::Staged(engine, registry)) = &*light_tracking {
        let tracker = &engine.fresh;
        let frozen = &engine.frozen_world;
        let active = &engine.active;
        let guard = region
            .light_guard
            .as_ref()
            .context("staged light guard missing")?;
        if !guard.light_replay_positions.is_empty() {
            ensure!(
                !tracker.has_skylight
                    || guard
                        .vine_light_positions
                        .iter()
                        .all(|p| !active.contains(&chunk(*p))),
                "incremental active cave vine lighting is not supported"
            );
            let baseline = frozen.as_ref().unwrap_or(&region.target.world);
            // Preserve the current registry lineage and replay only tracked
            // registered writes into the earlier light input snapshot.
            let mut world = region.world.clone();
            world.chunks = baseline.chunks.clone();
            let mut chunks = BTreeSet::new();
            for &p in &guard.light_replay_positions {
                let pos = chunk(p);
                ensure!(
                    tracker.sky_sources.contains_key(&pos),
                    "feature update requires initialized sky sources"
                );
                let state = region
                    .world
                    .block(p[0], p[1], p[2])
                    .context("updated feature block missing")?
                    .clone();
                let id = world.intern_state(state);
                world
                    .set_block_id(p[0], p[1], p[2], id)
                    .map_err(|error| anyhow::anyhow!("cannot replay feature update: {error:?}"))?;
                chunks.insert(pos);
            }
            let max_y = tracker
                .min_y
                .checked_add(tracker.height)
                .context("light height overflow")?;
            let bounds = native_world::block_light::LightBounds::new(tracker.min_y, max_y)
                .context("unsupported light bounds")?;
            let mut sources = BTreeMap::new();
            let mut needs_incremental = !engine.incremental_columns.is_empty()
                || !guard.new_nonempty_sections.is_empty()
                || !guard.vine_light_positions.is_empty();
            for pos in chunks {
                let heights = native_world::sky_light::generated_chunk_sky_sources(
                    &region.world,
                    bounds,
                    pos,
                )
                .context("updated sky sources unavailable")?;
                // A registered inactive chunk can contain light that was
                // propagated from an active neighbor. Apply its checks to
                // that stored revision instead of replaying the activation.
                if !active.contains(&pos)
                    && (-1i32..=1).any(|dz| {
                        (-1i32..=1).any(|dx| {
                            pos.x
                                .checked_add(dx)
                                .zip(pos.z.checked_add(dz))
                                .is_some_and(|(x, z)| active.contains(&ChunkPos { x, z }))
                        })
                    })
                {
                    needs_incremental = true;
                }
                sources.insert(pos, Arc::from(heights));
            }
            if needs_incremental {
                let mut covered = BTreeSet::new();
                for &p in &guard.light_replay_positions {
                    let pos = chunk(p);
                    for dz in -2i32..=2 {
                        for dx in -2i32..=2 {
                            covered.insert(ChunkPos {
                                x: pos.x.checked_add(dx).context("light X overflow")?,
                                z: pos.z.checked_add(dz).context("light Z overflow")?,
                            });
                        }
                    }
                }
                let before = engine.snapshot(region.target, covered.iter().copied(), registry)?;
                let updating = before
                    .allocate_nonempty_sections(&guard.new_nonempty_sections, active)
                    .context(
                        "incremental section allocation inputs are incomplete or unsupported",
                    )?;
                let after_block =
                    native_world::generation_block_light::check_generated_block_light(
                        &world,
                        bounds,
                        &updating,
                        active,
                        &guard.light_check_order,
                    )
                    .context(
                        "incremental feature block light inputs are incomplete or unsupported",
                    )?;
                let after = if tracker.has_skylight {
                    // Only an active check's own and adjacent columns can
                    // influence its source update. Do not clone the entire
                    // batch's source map for every decoration transaction.
                    let mut current_sources = sources.clone();
                    for &p in &guard.light_check_order {
                        if !active.contains(&chunk(p)) {
                            continue;
                        }
                        for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                            let x = p[0].checked_add(dx).context("sky source X overflow")?;
                            let z = p[2].checked_add(dz).context("sky source Z overflow")?;
                            let pos = chunk([x, p[1], z]);
                            if let Some(heights) = tracker.sky_sources.get(&pos) {
                                current_sources
                                    .entry(pos)
                                    .or_insert_with(|| Arc::clone(heights));
                            }
                        }
                    }
                    native_world::generation_sky_light::check_generated_sky_blocks(
                        &world,
                        bounds,
                        &after_block,
                        active,
                        &guard.light_check_order,
                        &current_sources,
                    )
                    .context("incremental feature sky light inputs are incomplete or unsupported")?
                } else {
                    after_block
                };
                let mut columns = BTreeMap::new();
                for pos in covered {
                    columns.insert(
                        pos,
                        after
                            .complete_column(pos)
                            .context("incremental light column missing")?,
                    );
                }
                incremental_columns = Some(columns);
                let mut allocated = Vec::new();
                for &(pos, sy) in &guard.new_nonempty_sections {
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            let neighbor = ChunkPos {
                                x: pos.x + dx,
                                z: pos.z + dz,
                            };
                            for y in sy - 1..=sy + 1 {
                                allocated.push((neighbor, y));
                            }
                        }
                    }
                }
                incremental_allocations = Some(allocated);
            }
            staged_replay = Some((Arc::new(world), sources));
        }
        let validation_world = staged_replay
            .as_ref()
            .map(|(world, _)| world)
            .or_else(|| frozen.as_ref());
        StagedGenerationLightEngine::validate_active_world_parts(
            validation_world,
            active,
            tracker.min_y,
            tracker.height,
            &region.world,
        )?;
        for (pos, before) in &tracker.registered {
            let after = region
                .world
                .chunks
                .get(pos)
                .context("registered light chunk is missing")?;
            if !Arc::ptr_eq(before, after) {
                ensure!(
                    guard.changed_chunks.contains(pos),
                    "untracked registered light chunk change"
                );
                let reference_world = validation_world.unwrap_or(&region.target.world);
                let reference_chunk = reference_world
                    .chunks
                    .get(pos)
                    .context("registered light reference missing")?;
                ensure!(
                    super::unchanged_chunk_light_behavior(
                        reference_world,
                        &region.world,
                        reference_chunk,
                        after,
                        tracker.min_y,
                        tracker.height
                    )?,
                    "registered light behavior changed"
                );
            }
        }
    }
    let checked = region
        .light_guard
        .as_mut()
        .map(|guard| std::mem::take(&mut guard.changed_blocks));
    let committed = region.finish_with_world()?;
    let tracker = match &mut *light_tracking {
        Some(DecorationLightTracking::Fresh(tracker, _)) => Some(&mut **tracker),
        Some(DecorationLightTracking::Staged(engine, _)) => Some(&mut engine.fresh),
        None => None,
    };
    if let Some(tracker) = tracker {
        for (pos, current) in &mut tracker.registered {
            *current = Arc::clone(
                committed
                    .chunks
                    .get(pos)
                    .expect("registered light chunk was validated"),
            );
        }
        tracker
            .checked_blocks
            .extend(checked.expect("tracked decoration has a light guard"));
        if let Some(sources) = fresh_sources {
            tracker.sky_sources.extend(sources);
        }
        if let Some(allocations) = fresh_allocations {
            for (pos, y) in allocations {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        let neighbor = ChunkPos {
                            x: pos.x + dx,
                            z: pos.z + dz,
                        };
                        tracker
                            .allocated
                            .entry(neighbor)
                            .or_default()
                            .extend(y - 1..=y + 1);
                    }
                }
            }
        }
    }
    if let Some((world, sources)) = staged_replay {
        let Some(DecorationLightTracking::Staged(engine, _)) = &mut *light_tracking else {
            unreachable!("staged replay requires staged tracking");
        };
        engine.fresh.sky_sources.extend(sources);
        if let Some(allocated) = incremental_allocations {
            for (pos, y) in allocated {
                engine.fresh.allocated.entry(pos).or_default().insert(y);
            }
        }
        if engine.frozen_world.is_some() {
            engine.frozen_world = Some(world);
        }
        if let Some(columns) = incremental_columns {
            engine.incremental_columns.extend(columns);
        }
    }
    Ok(())
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
    fn needs_water_fluids(&self) -> bool {
        self.feature.needs_water_fluids()
    }
    fn needs_freeze(&self) -> bool {
        self.feature.needs_freeze()
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
        let bee_nests = self.generated_bee_nests.clone();
        let monster_entities = self.generated_monster_entities.clone();
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
            bubble_column_fluids: None,
            motion_cache: BTreeMap::new(),
            tree_logs: Vec::new(),
            tree_leaves: Vec::new(),
            freeze_context: None,
            raw_brightness: None,
            tree_decorations: Vec::new(),
            bee_nests,
            monster_entities,
            pending_monster_spawners: 0,
            light_guard: None,
            tree_block_write: false,
            column_block_write: false,
            patch_block_write: false,
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
                positions
                    .next(&mut random, &mut env)
                    .with_context(|| format!("placing {:?}", selected.feature))?
            };
            let Some(position) = next else { break };
            result |= ore
                .placed
                .feature
                .place(
                    self,
                    &mut random,
                    position,
                    plan.generator_min_y,
                    plan.generator_height,
                )
                .with_context(|| format!("placing {:?} at {position:?}", selected.feature))?;
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
        self.finish_with_world().map(drop)
    }

    fn finish_with_world(self) -> Result<Arc<World>> {
        ensure!(!self.failed, "cannot publish a failed generation attempt");
        ensure!(
            self.pending_monster_spawners == 0,
            "unfinished generated spawner data"
        );
        let mut world = self.world;
        let mut affected = BTreeSet::new();
        for (pos, chunk) in &world.chunks {
            if self
                .target
                .world
                .chunks
                .get(pos)
                .is_some_and(|old| Arc::ptr_eq(old, chunk))
            {
                continue;
            }
            // Light from a changed block reaches at most one neighboring chunk.
            for dz in -1i32..=1 {
                for dx in -1i32..=1 {
                    if let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) {
                        affected.insert(ChunkPos { x, z });
                    }
                }
            }
        }
        for pos in affected {
            let Some(chunk) = world.chunks.get_mut(&pos) else {
                continue;
            };
            if !chunk.block_light_correct
                && chunk
                    .sections
                    .values()
                    .all(|s| s.block_light.is_none() && s.sky_light.is_none())
            {
                continue;
            }
            let chunk = Arc::make_mut(chunk);
            chunk.block_light_correct = false;
            for section in chunk.sections.values_mut() {
                if section.block_light.is_some() || section.sky_light.is_some() {
                    let section = Arc::make_mut(section);
                    section.block_light = None;
                    section.sky_light = None;
                }
            }
        }
        self.target.world = Arc::new(world);
        self.target.ore_ocean_floor_heights = self.heights;
        self.target.feature_world_surface_heights = self.surface_heights;
        self.target.final_heightmaps = self.final_heights;
        self.target.feature_postprocessing = self.feature_marks;
        self.target.generated_bee_nests = self.bee_nests;
        self.target.generated_monster_entities = self.monster_entities;
        Ok(Arc::clone(&self.target.world))
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

fn tree_oak_leaf(state: &BlockState) -> Result<bool> {
    if state.name != "minecraft:oak_leaves" {
        return Ok(false);
    }
    valid_tree_leaf_properties(state)
}

fn valid_tree_leaf_properties(state: &BlockState) -> Result<bool> {
    ensure!(state.properties.len() == 3, "unsupported oak leaf state");
    ensure!(
        state
            .properties
            .get("distance")
            .and_then(|v| v.parse::<u8>().ok())
            .is_some_and(|v| (1..=7).contains(&v)),
        "invalid oak leaf distance"
    );
    for property in ["persistent", "waterlogged"] {
        ensure!(
            matches!(
                state.properties.get(property).map(String::as_str),
                Some("true" | "false")
            ),
            "invalid oak leaf property"
        );
    }
    Ok(true)
}

fn tree_generated_leaf(state: &BlockState) -> Result<bool> {
    if matches!(
        state.name.as_str(),
        "minecraft:oak_leaves"
            | "minecraft:dark_oak_leaves"
            | "minecraft:birch_leaves"
            | "minecraft:azalea_leaves"
            | "minecraft:flowering_azalea_leaves"
    ) {
        valid_tree_leaf_properties(state)
    } else {
        Ok(false)
    }
}

fn tree_decay_tag() -> &'static ResourceId {
    static TAG: OnceLock<ResourceId> = OnceLock::new();
    TAG.get_or_init(|| ResourceId::parse("minecraft:prevents_nearby_leaf_decay").unwrap())
}

fn tree_vegetation_tag(dry: bool) -> &'static ResourceId {
    static NORMAL: OnceLock<ResourceId> = OnceLock::new();
    static DRY: OnceLock<ResourceId> = OnceLock::new();
    if dry {
        DRY.get_or_init(|| ResourceId::parse("minecraft:supports_dry_vegetation").unwrap())
    } else {
        NORMAL.get_or_init(|| ResourceId::parse("minecraft:supports_vegetation").unwrap())
    }
}

fn tree_azalea_tag() -> &'static ResourceId {
    static AZALEA: OnceLock<ResourceId> = OnceLock::new();
    AZALEA.get_or_init(|| ResourceId::parse("minecraft:supports_azalea").unwrap())
}

fn tree_oak_log(state: &BlockState) -> bool {
    state.name == "minecraft:oak_log"
        && state.properties.len() == 1
        && matches!(
            state.properties.get("axis").map(String::as_str),
            Some("x" | "y" | "z")
        )
}

fn tree_generated_log(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:oak_log" | "minecraft:dark_oak_log" | "minecraft:birch_log"
    ) && state.properties.len() == 1
        && matches!(
            state.properties.get("axis").map(String::as_str),
            Some("x" | "y" | "z")
        )
}

fn tree_leaf_litter(state: &BlockState) -> Result<bool> {
    if state.name != "minecraft:leaf_litter" {
        return Ok(false);
    }
    ensure!(
        state.properties.len() == 2
            && matches!(
                state.properties.get("facing").map(String::as_str),
                Some("north" | "south" | "east" | "west")
            )
            && matches!(
                state.properties.get("segment_amount").map(String::as_str),
                Some("1" | "2" | "3" | "4")
            ),
        "invalid leaf litter state"
    );
    Ok(true)
}

fn tree_cave_vines(state: &BlockState) -> Result<bool> {
    if !matches!(
        state.name.as_str(),
        "minecraft:cave_vines" | "minecraft:cave_vines_plant"
    ) {
        return Ok(false);
    }
    let valid_berries = matches!(
        state.properties.get("berries").map(String::as_str),
        Some("true" | "false")
    );
    let valid = if state.name == "minecraft:cave_vines" {
        state.properties.len() == 2
            && state.properties.get("age").is_some_and(|raw| {
                raw.parse::<u8>()
                    .ok()
                    .is_some_and(|age| age <= 25 && age.to_string() == *raw)
            })
    } else {
        state.properties.len() == 1
    };
    ensure!(valid && valid_berries, "invalid cave vines state");
    Ok(true)
}

fn tree_glow_lichen(state: &BlockState) -> Result<bool> {
    if state.name != "minecraft:glow_lichen" {
        return Ok(false);
    }
    ensure!(valid_lichen(state), "invalid glow lichen state");
    Ok(true)
}

// All vanilla dripleaf states have no block entity, POI, or placement
// postprocessing. Block-column features write their supplied states directly.
fn generated_dripleaf(state: &BlockState) -> Result<bool> {
    if !matches!(
        state.name.as_str(),
        "minecraft:big_dripleaf" | "minecraft:big_dripleaf_stem" | "minecraft:small_dripleaf"
    ) {
        return Ok(false);
    }
    let common = matches!(
        state.properties.get("facing").map(String::as_str),
        Some("north" | "south" | "east" | "west")
    ) && matches!(
        state.properties.get("waterlogged").map(String::as_str),
        Some("true" | "false")
    );
    let extra = match state.name.as_str() {
        "minecraft:big_dripleaf" => {
            state.properties.len() == 3
                && matches!(
                    state.properties.get("tilt").map(String::as_str),
                    Some("none" | "unstable" | "partial" | "full")
                )
        }
        "minecraft:small_dripleaf" => {
            state.properties.len() == 3
                && matches!(
                    state.properties.get("half").map(String::as_str),
                    Some("lower" | "upper")
                )
        }
        _ => state.properties.len() == 2,
    };
    ensure!(common && extra, "invalid generated dripleaf state");
    Ok(true)
}

fn geode_full_cube(state: &BlockState) -> Result<bool> {
    if matches!(
        state.name.as_str(),
        "minecraft:smooth_basalt" | "minecraft:amethyst_block" | "minecraft:budding_amethyst"
    ) {
        ensure!(state.properties.is_empty(), "invalid geode layer state");
        return Ok(true);
    }
    Ok(false)
}

fn geode_crystal(state: &BlockState) -> Result<bool> {
    if matches!(
        state.name.as_str(),
        "minecraft:small_amethyst_bud"
            | "minecraft:medium_amethyst_bud"
            | "minecraft:large_amethyst_bud"
            | "minecraft:amethyst_cluster"
    ) {
        geode_safe_state(state)?;
        return Ok(true);
    }
    Ok(false)
}

fn tree_solid_render(state: &BlockState) -> Result<bool> {
    if geode_full_cube(state)? {
        return Ok(true);
    }
    if geode_crystal(state)? {
        return Ok(false);
    }
    if tree_generated_log(state) {
        return Ok(true);
    }
    if tree_mushroom_block(state)? {
        return Ok(true);
    }
    if tree_generated_leaf(state)?
        || tree_leaf_litter(state)?
        || tree_cave_vines(state)?
        || tree_glow_lichen(state)?
        || generated_dripleaf(state)?
        || state.name == "minecraft:vine"
    {
        return Ok(false);
    }
    if tree_edge_plant(state)?.is_some()
        || matches!(simple_plant_kind(state), Ok(kind) if !matches!(kind, SimplePlantKind::Unconditional))
    {
        return Ok(false);
    }
    vanilla_full_occlusion(&state.name)
        .with_context(|| format!("unsupported tree ground shape {}", state.name))
}

fn tree_sturdy_up(state: &BlockState) -> Result<bool> {
    if geode_full_cube(state)? {
        return Ok(true);
    }
    if geode_crystal(state)? {
        return Ok(false);
    }
    if tree_generated_log(state) {
        return Ok(true);
    }
    if tree_mushroom_block(state)? {
        return Ok(true);
    }
    if matches!(
        state.name.as_str(),
        "minecraft:azalea" | "minecraft:flowering_azalea"
    ) {
        simple_plant_kind(state)?;
        return Ok(true);
    }
    if tree_generated_leaf(state)?
        || tree_leaf_litter(state)?
        || tree_cave_vines(state)?
        || tree_glow_lichen(state)?
        || generated_dripleaf(state)?
        || state.name == "minecraft:vine"
    {
        return Ok(false);
    }
    if tree_edge_plant(state)?.is_some()
        || matches!(simple_plant_kind(state), Ok(kind) if !matches!(kind, SimplePlantKind::Unconditional))
    {
        return Ok(false);
    }
    vanilla_full_occlusion(&state.name)
        .with_context(|| format!("unsupported tree support shape {}", state.name))
}

fn generation_face_sturdy(state: &BlockState, direction: [i32; 3]) -> Result<bool> {
    ensure!(
        matches!(
            direction,
            [0, -1, 0] | [0, 1, 0] | [0, 0, -1] | [0, 0, 1] | [-1, 0, 0] | [1, 0, 0]
        ),
        "invalid support direction"
    );
    if matches!(
        state.name.as_str(),
        "minecraft:azalea" | "minecraft:flowering_azalea"
    ) {
        simple_plant_kind(state)?;
        return Ok(direction == [0, 1, 0]);
    }
    if is_simple_carpet_id(&state.name) {
        simple_plant_kind(state)?;
        return Ok(direction == [0, -1, 0]);
    }
    tree_sturdy_up(state)
}

fn tree_grass_block(state: &BlockState) -> bool {
    state.name == "minecraft:grass_block"
        && state.properties.len() == 1
        && matches!(
            state.properties.get("snowy").map(String::as_str),
            Some("true" | "false")
        )
}

fn tree_mushroom_block(state: &BlockState) -> Result<bool> {
    if !matches!(
        state.name.as_str(),
        "minecraft:red_mushroom_block"
            | "minecraft:brown_mushroom_block"
            | "minecraft:mushroom_stem"
    ) {
        return Ok(false);
    }
    ensure!(
        state.properties.len() == 6
            && ["up", "down", "north", "south", "east", "west"]
                .iter()
                .all(|face| matches!(
                    state.properties.get(*face).map(String::as_str),
                    Some("true" | "false")
                )),
        "invalid giant mushroom block state"
    );
    Ok(true)
}

fn tree_edge_plant(state: &BlockState) -> Result<Option<SimplePlantKind>> {
    if is_simple_carpet_id(&state.name) {
        Ok(Some(simple_plant_kind(state)?))
    } else if matches!(
        state.name.as_str(),
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
            | "minecraft:closed_eyeblossom"
            | "minecraft:azalea"
            | "minecraft:flowering_azalea"
            | "minecraft:sweet_berry_bush"
            | "minecraft:short_dry_grass"
            | "minecraft:tall_dry_grass"
            | "minecraft:dead_bush"
            | "minecraft:tall_grass"
            | "minecraft:large_fern"
            | "minecraft:sunflower"
            | "minecraft:rose_bush"
            | "minecraft:peony"
            | "minecraft:lilac"
    ) {
        Ok(Some(simple_plant_kind(state)?))
    } else if matches!(
        state.name.as_str(),
        "minecraft:oak_sapling"
            | "minecraft:spruce_sapling"
            | "minecraft:birch_sapling"
            | "minecraft:jungle_sapling"
            | "minecraft:acacia_sapling"
            | "minecraft:dark_oak_sapling"
            | "minecraft:cherry_sapling"
            | "minecraft:pale_oak_sapling"
    ) {
        ensure!(
            state.properties.len() == 1
                && matches!(
                    state.properties.get("stage").map(String::as_str),
                    Some("0" | "1")
                ),
            "invalid sapling state"
        );
        Ok(Some(SimplePlantKind::Vegetation))
    } else {
        Ok(None)
    }
}

impl DecorationRegion<'_> {
    /// Known-shape writes used by supported tree states. Unknown
    /// block-entity, POI or postprocessing effects abort the transaction.
    fn set_tree_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        ensure!(flags == 19 || flags == 2, "unsupported tree write flags");
        if !self.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(pos)),
            "tree write chunk is missing"
        );
        let allowed = tree_generated_leaf(state)?
            || tree_generated_log(state)
            || tree_leaf_litter(state)?
            || (flags == 2 && tree_mushroom_block(state)?)
            || (flags == 2 && tree_grass_block(state))
            || (flags == 2 && state.name == "minecraft:air" && state.properties.is_empty())
            || (matches!(
                state.name.as_str(),
                "minecraft:dirt" | "minecraft:rooted_dirt"
            ) && state.properties.is_empty());
        ensure!(
            allowed,
            "unsupported tree replacement effects at {pos:?}, flags {flags}: {state:?}"
        );
        if self.is_outside(pos[1]) {
            return Ok(());
        }
        let old = self.read(pos)?;
        if tree_mushroom_block(state)? {
            ensure!(
                old.name == state.name && tree_mushroom_block(&old)?,
                "unsupported tree mushroom replacement"
            );
        }
        ensure!(
            disk_safe_solid(&old.name).is_some()
                || tree_generated_leaf(&old)?
                || tree_generated_log(&old)
                || tree_leaf_litter(&old)?
                || tree_mushroom_block(&old)?
                || simple_plant_kind(&old).is_ok()
                || tree_edge_plant(&old)?.is_some(),
            "unsupported tree source effects"
        );
        self.tree_block_write = true;
        let result = self.set_section_block(pos, state);
        self.tree_block_write = false;
        result?;
        self.update_final_heightmaps(pos, state)?;
        Ok(())
    }
}

impl TreeWorld for DecorationRegion<'_> {
    fn tree_min_y(&self) -> i32 {
        self.target.min_y
    }
    fn tree_max_y(&self) -> i32 {
        self.target.min_y + self.target.height - 1
    }
    fn trunk_axis(
        &mut self,
        mut state: BlockState,
        axis: native_resources::tree_fancy_trunk::LogAxis,
    ) -> Result<BlockState> {
        if let Some(value) = state.properties.get_mut("axis") {
            ensure!(
                matches!(value.as_str(), "x" | "y" | "z"),
                "invalid log axis"
            );
            *value = match axis {
                native_resources::tree_fancy_trunk::LogAxis::X => "x",
                native_resources::tree_fancy_trunk::LogAxis::Y => "y",
                native_resources::tree_fancy_trunk::LogAxis::Z => "z",
            }
            .into();
        }
        Ok(state)
    }
    fn persistent_foliage(&mut self, pos: [i32; 3]) -> Result<bool> {
        let state = OreWorld::block(self, pos)?;
        match state.properties.get("persistent").map(String::as_str) {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            _ => bail!("invalid persistent property"),
        }
    }
    fn foliage_state(&mut self, pos: [i32; 3], mut state: BlockState) -> Result<BlockState> {
        ensure!(
            tree_generated_leaf(&state)?,
            "unsupported tree foliage state"
        );
        let old = OreWorld::block(self, pos)?;
        let water = native_world::fluid::FluidState::from_block(&old)
            .map_err(anyhow::Error::msg)?
            .is_some_and(|f| f.kind == native_world::fluid::FluidKind::Water && f.source);
        state
            .properties
            .insert("waterlogged".into(), water.to_string());
        Ok(state)
    }
    fn write_tree(&mut self, pos: [i32; 3], state: &BlockState, kind: TreeWrite) -> Result<()> {
        match kind {
            TreeWrite::Trunk => self.tree_logs.push(pos),
            TreeWrite::Foliage => self.tree_leaves.push(pos),
        }
        self.set_tree_block(pos, state, 19)
    }
}

impl MushroomWorld for DecorationRegion<'_> {
    fn mushroom_min_y(&self) -> i32 {
        self.target.min_y
    }
    fn mushroom_max_y(&self) -> i32 {
        self.target.min_y + self.target.height - 1
    }
    fn mushroom_cap_faces(
        &mut self,
        mut state: BlockState,
        faces: &[(&str, bool)],
    ) -> Result<BlockState> {
        if faces
            .iter()
            .all(|(name, _)| state.properties.contains_key(*name))
        {
            for &(name, value) in faces {
                let current = state
                    .properties
                    .get_mut(name)
                    .expect("checked cap property");
                ensure!(
                    matches!(current.as_str(), "true" | "false"),
                    "invalid mushroom face"
                );
                *current = value.to_string();
            }
        }
        Ok(state)
    }
    fn write_mushroom_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        if !self.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(pos)),
            "mushroom write chunk is missing"
        );
        ensure!(
            matches!(
                state.name.as_str(),
                "minecraft:brown_mushroom_block"
                    | "minecraft:red_mushroom_block"
                    | "minecraft:mushroom_stem"
            ) && state.properties.len() == 6
                && ["up", "down", "north", "south", "east", "west"]
                    .iter()
                    .all(|name| matches!(
                        state.properties.get(*name).map(String::as_str),
                        Some("true" | "false")
                    )),
            "unsupported mushroom replacement effects"
        );
        if self.is_outside(pos[1]) {
            return Ok(());
        }
        let old = self.read(pos)?;
        ensure!(
            is_air(&old) || tree_generated_leaf(&old)?,
            "unsupported mushroom source effects"
        );
        self.set_section_block(pos, state)?;
        self.update_final_heightmaps(pos, state)?;
        Ok(())
    }
}

impl VegetationPatchWorld for DecorationRegion<'_> {
    fn same_block(&mut self, a: &BlockState, b: &BlockState) -> Result<bool> {
        Ok(a.name == b.name)
    }
    fn patch_face_sturdy(
        &mut self,
        state: &BlockState,
        _: [i32; 3],
        direction: [i32; 3],
    ) -> Result<bool> {
        generation_face_sturdy(state, direction)
    }
    fn set_patch_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        let result = (|| {
            if !self.writable.contains(&chunk(pos)) {
                return Ok(());
            }
            ensure!(
                self.world.chunks.contains_key(&chunk(pos)),
                "vegetation patch chunk is missing"
            );
            let valid = (matches!(
                state.name.as_str(),
                "minecraft:moss_block" | "minecraft:clay"
            ) && state.properties.is_empty())
                || (state.name == "minecraft:water"
                    && native_world::fluid::FluidState::from_block(state)
                        .map_err(anyhow::Error::msg)?
                        .is_some())
                || tree_generated_leaf(state)?
                || generated_dripleaf(state)?;
            ensure!(valid, "unsupported vegetation patch replacement effects");
            if self.is_outside(pos[1]) {
                return Ok(());
            }
            let old = self.read(pos)?;
            ensure!(
                disk_safe_solid(&old.name).is_some()
                    || tree_generated_leaf(&old)?
                    || generated_dripleaf(&old)?,
                "unsupported vegetation patch source effects"
            );
            self.patch_block_write = true;
            let write = self.set_section_block(pos, state);
            self.patch_block_write = false;
            write?;
            self.update_final_heightmaps(pos, state)?;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn patch_waterlogged(&mut self, mut state: BlockState) -> Result<Option<BlockState>> {
        match state.properties.get("waterlogged").map(String::as_str) {
            None | Some("true") => Ok(None),
            Some("false") => {
                ensure!(
                    tree_generated_leaf(&state)? || generated_dripleaf(&state)?,
                    "unsupported vegetation patch waterlogging for {:?}",
                    state
                );
                state.properties.insert("waterlogged".into(), "true".into());
                Ok(Some(state))
            }
            _ => bail!("invalid waterlogged property"),
        }
    }
}

impl RootSystemWorld for DecorationRegion<'_> {
    fn root_world_surface(&mut self, x: i32, z: i32) -> Result<i32> {
        self.final_height(0, x, z)
    }
    fn root_can_survive(&mut self, state: &BlockState, pos: [i32; 3]) -> Result<bool> {
        ensure!(
            valid_hanging_roots(state),
            "unsupported hanging root survival"
        );
        let above = OreWorld::block(self, [pos[0], pos[1].wrapping_add(1), pos[2]])?;
        DiskWorld::face_sturdy(
            self,
            &above,
            [pos[0], pos[1].wrapping_add(1), pos[2]],
            [0, -1, 0],
        )
    }
    fn set_root_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<bool> {
        let result = (|| {
            if !self.writable.contains(&chunk(pos)) {
                return Ok(false);
            }
            ensure!(
                self.world.chunks.contains_key(&chunk(pos)),
                "root system chunk is missing"
            );
            ensure!(
                (state.name == "minecraft:rooted_dirt" && state.properties.is_empty())
                    || valid_hanging_roots(state),
                "unsupported root replacement effects"
            );
            if self.is_outside(pos[1]) {
                return Ok(true);
            }
            let old = self.read(pos)?;
            ensure!(
                disk_safe_solid(&old.name).is_some() || valid_hanging_roots(&old),
                "unsupported root source effects"
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

fn valid_hanging_roots(state: &BlockState) -> bool {
    state.name == "minecraft:hanging_roots"
        && state.properties.len() == 1
        && matches!(
            state.properties.get("waterlogged").map(String::as_str),
            Some("true" | "false")
        )
}

impl GroundDecorationWorld for DecorationRegion<'_> {
    fn ground_solid_render(&mut self, state: &BlockState) -> Result<bool> {
        tree_solid_render(state)
    }
    fn ground_heightmap_y(&mut self, x: i32, z: i32) -> Result<i32> {
        self.final_height(3, x, z)
    }
    fn place_ground_decoration(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        self.tree_decorations.push(pos);
        if !self.writable.contains(&chunk(pos)) || self.is_outside(pos[1]) {
            return Ok(());
        }
        ensure!(
            tree_leaf_litter(state)?,
            "unsupported ground decoration state"
        );
        let old = self.read(pos)?;
        ensure!(
            is_air(&old) || old.name == "minecraft:vine",
            "unsupported ground decoration source"
        );
        self.set_section_block(pos, state)?;
        self.update_final_heightmaps(pos, state)?;
        Ok(())
    }
}

impl BeehiveWorld for DecorationRegion<'_> {
    fn is_air(&mut self, pos: [i32; 3]) -> Result<bool> {
        Ok(is_air(&OreWorld::block(self, pos)?))
    }
    fn place_bee_nest(&mut self, pos: [i32; 3]) -> Result<bool> {
        self.tree_decorations.push(pos);
        if !self.writable.contains(&chunk(pos)) || self.is_outside(pos[1]) {
            return Ok(false);
        }
        ensure!(
            !self.bee_nests.contains_key(&pos),
            "existing generated bee nest"
        );
        ensure!(is_air(&self.read(pos)?), "bee nest target is not air");
        let state = BlockState {
            name: "minecraft:bee_nest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "south".into()),
                ("honey_level".into(), "0".into()),
            ]),
        };
        self.set_section_block(pos, &state)?;
        self.update_final_heightmaps(pos, &state)?;
        Ok(true)
    }
    fn store_bees(&mut self, pos: [i32; 3], ticks: &[i32]) -> Result<()> {
        let nest = super::GeneratedBeeNest::new(pos, ticks.to_vec())?;
        ensure!(
            Arc::make_mut(&mut self.bee_nests)
                .insert(pos, nest)
                .is_none(),
            "duplicate generated bee nest"
        );
        Ok(())
    }
}

impl DecorationRegion<'_> {
    fn set_fallen_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        if !self.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(pos)),
            "fallen tree chunk is missing"
        );
        if self.is_outside(pos[1]) {
            return Ok(());
        }
        let old = self.read(pos)?;
        ensure!(
            disk_safe_solid(&old.name).is_some()
                || tree_oak_leaf(&old)?
                || tree_oak_log(&old)
                || simple_plant_kind(&old).is_ok()
                || tree_edge_plant(&old)?.is_some()
                || matches!(
                    old.name.as_str(),
                    "minecraft:vine" | "minecraft:red_mushroom" | "minecraft:brown_mushroom"
                ),
            "unsupported fallen tree source effects"
        );
        self.set_section_block(pos, state)?;
        self.update_final_heightmaps(pos, state)?;
        Ok(())
    }
}
impl FallenFeatureWorld for DecorationRegion<'_> {
    fn fallen_sturdy_below(&mut self, pos: [i32; 3]) -> Result<bool> {
        let below = OreWorld::block(self, [pos[0], pos[1].wrapping_sub(1), pos[2]])?;
        if tree_oak_log(&below) {
            return Ok(true);
        }
        ensure!(
            !matches!(
                below.name.as_str(),
                "minecraft:cactus" | "minecraft:cactus_flower" | "minecraft:sugar_cane"
            ),
            "unsupported fallen tree support shape"
        );
        disk_safe_solid(&below.name).context("unsupported fallen tree support shape")
    }
    fn fallen_write_log(
        &mut self,
        pos: [i32; 3],
        mut state: BlockState,
        axis: Option<native_resources::tree_fancy_trunk::LogAxis>,
    ) -> Result<()> {
        if let Some(axis) = axis {
            state = TreeWorld::trunk_axis(self, state, axis)?;
        }
        ensure!(tree_oak_log(&state), "unsupported fallen tree log effects");
        self.set_fallen_block(pos, &state)
    }
    fn fallen_postprocess_above(&mut self, pos: [i32; 3]) -> Result<()> {
        for dy in 1..=2 {
            let above = [pos[0], pos[1].wrapping_add(dy), pos[2]];
            if is_air(&OreWorld::block(self, above)?) {
                break;
            }
            DiskWorld::mark_postprocessing(self, above)?;
        }
        Ok(())
    }
    fn fallen_write_vine(&mut self, pos: [i32; 3], attachment: FallenDirection) -> Result<()> {
        let mut properties = BTreeMap::from([
            ("up".into(), "false".into()),
            ("north".into(), "false".into()),
            ("south".into(), "false".into()),
            ("east".into(), "false".into()),
            ("west".into(), "false".into()),
        ]);
        let side = match attachment {
            FallenDirection::North => "north",
            FallenDirection::South => "south",
            FallenDirection::West => "west",
            FallenDirection::East => "east",
            _ => bail!("invalid vine support"),
        };
        properties.insert(side.into(), "true".into());
        self.set_fallen_block(
            pos,
            &BlockState {
                name: "minecraft:vine".into(),
                properties,
            },
        )
    }
    fn fallen_write_decoration(&mut self, pos: [i32; 3], state: BlockState) -> Result<()> {
        ensure!(
            matches!(
                state.name.as_str(),
                "minecraft:red_mushroom" | "minecraft:brown_mushroom"
            ) && state.properties.is_empty(),
            "unsupported fallen tree decoration effects"
        );
        self.set_fallen_block(pos, &state)
    }
}

// Only state families independently checked against 26.3 are accepted here.
fn monster_block_flags(state: &BlockState) -> Result<(bool, bool)> {
    let name = state.name.as_str();
    if is_air(state) {
        return Ok((false, false));
    }
    if matches!(name, "minecraft:water" | "minecraft:lava") {
        native_world::fluid::FluidState::from_block(state)
            .map_err(anyhow::Error::msg)?
            .context("invalid monster-room fluid")?;
        return Ok((false, false));
    }
    if name == "minecraft:chest" {
        ensure!(
            state.properties.len() == 3
                && matches!(
                    state.properties.get("facing").map(String::as_str),
                    Some("north" | "south" | "east" | "west")
                )
                && matches!(
                    state.properties.get("type").map(String::as_str),
                    Some("single" | "left" | "right")
                )
                && matches!(
                    state.properties.get("waterlogged").map(String::as_str),
                    Some("true" | "false")
                ),
            "invalid monster-room chest state"
        );
        return Ok((true, false));
    }
    if name == "minecraft:spawner" {
        ensure!(state.properties.is_empty(), "invalid spawner state");
        return Ok((true, false));
    }
    if matches!(
        name,
        "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
    ) {
        ensure!(
            state.properties.len() == 1
                && matches!(
                    state.properties.get("snowy").map(String::as_str),
                    Some("true" | "false")
                ),
            "invalid snowy monster-room block state"
        );
        return Ok((true, true));
    }
    if matches!(
        name,
        "minecraft:stone"
            | "minecraft:deepslate"
            | "minecraft:tuff"
            | "minecraft:granite"
            | "minecraft:diorite"
            | "minecraft:andesite"
            | "minecraft:calcite"
            | "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:clay"
            | "minecraft:gravel"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:sandstone"
            | "minecraft:bedrock"
            | "minecraft:cobblestone"
            | "minecraft:mossy_cobblestone"
            | "minecraft:cobbled_deepslate"
            | "minecraft:coal_ore"
            | "minecraft:deepslate_coal_ore"
            | "minecraft:iron_ore"
            | "minecraft:deepslate_iron_ore"
            | "minecraft:copper_ore"
            | "minecraft:deepslate_copper_ore"
            | "minecraft:gold_ore"
            | "minecraft:deepslate_gold_ore"
            | "minecraft:redstone_ore"
            | "minecraft:deepslate_redstone_ore"
            | "minecraft:emerald_ore"
            | "minecraft:deepslate_emerald_ore"
            | "minecraft:lapis_ore"
            | "minecraft:deepslate_lapis_ore"
            | "minecraft:diamond_ore"
            | "minecraft:deepslate_diamond_ore"
            | "minecraft:netherrack"
            | "minecraft:basalt"
            | "minecraft:smooth_basalt"
            | "minecraft:amethyst_block"
            | "minecraft:budding_amethyst"
            | "minecraft:blackstone"
            | "minecraft:ancient_debris"
    ) {
        // These state families retain their flags across axis and lit values.
        ensure!(
            state.properties.is_empty()
                || matches!(
                    name,
                    "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore"
                ) && state.properties.len() == 1
                    && matches!(
                        state.properties.get("lit").map(String::as_str),
                        Some("true" | "false")
                    )
                || matches!(name, "minecraft:deepslate" | "minecraft:basalt")
                    && state.properties.len() == 1
                    && matches!(
                        state.properties.get("axis").map(String::as_str),
                        Some("x" | "y" | "z")
                    ),
            "unsupported monster-room block state"
        );
        return Ok((true, true));
    }
    bail!("unsupported monster-room block {}", name)
}
impl native_resources::monster_room::MonsterRoomWorld for DecorationRegion<'_> {
    type State = BlockState;
    fn min_y(&self) -> i32 {
        self.target.min_y
    }
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self, pos)
    }
    fn is_solid(&mut self, state: &BlockState) -> Result<bool> {
        Ok(monster_block_flags(state)?.0)
    }
    fn is_solid_render(&mut self, state: &BlockState) -> Result<bool> {
        Ok(monster_block_flags(state)?.1)
    }
    fn is_air(&mut self, state: &BlockState) -> Result<bool> {
        Ok(is_air(state))
    }
    fn is_chest(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name == "minecraft:chest")
    }
    fn is_spawner(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name == "minecraft:spawner")
    }
    fn may_replace(&mut self, state: &BlockState) -> Result<bool> {
        static CANNOT: OnceLock<ResourceId> = OnceLock::new();
        Ok(!self.tags.contains(
            CANNOT.get_or_init(|| ResourceId::parse("minecraft:features_cannot_replace").unwrap()),
            &state.name,
        )?)
    }
    fn set_block(&mut self, pos: [i32; 3], kind: RoomBlock) -> Result<()> {
        if !self.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(pos)),
            "monster-room chunk is missing"
        );
        if self.is_outside(pos[1]) {
            return Ok(());
        }
        let old = self.read(pos)?;
        monster_block_flags(&old)?;
        if matches!(old.name.as_str(), "minecraft:chest" | "minecraft:spawner") {
            ensure!(
                self.monster_entities.contains_key(&pos),
                "existing monster-room block entity is unavailable"
            );
            ensure!(
                !(old.name == "minecraft:spawner" && matches!(kind, RoomBlock::Spawner)),
                "rewriting an existing generated spawner is not supported"
            );
        }
        let state = match kind {
            RoomBlock::CaveAir => BlockState {
                name: "minecraft:cave_air".into(),
                properties: BTreeMap::new(),
            },
            RoomBlock::Cobblestone => BlockState {
                name: "minecraft:cobblestone".into(),
                properties: BTreeMap::new(),
            },
            RoomBlock::MossyCobblestone => BlockState {
                name: "minecraft:mossy_cobblestone".into(),
                properties: BTreeMap::new(),
            },
            RoomBlock::Spawner => BlockState {
                name: "minecraft:spawner".into(),
                properties: BTreeMap::new(),
            },
            RoomBlock::Chest(facing) => {
                let facing = match facing {
                    native_resources::structure_start::HorizontalDirection::North => "north",
                    native_resources::structure_start::HorizontalDirection::East => "east",
                    native_resources::structure_start::HorizontalDirection::South => "south",
                    native_resources::structure_start::HorizontalDirection::West => "west",
                };
                BlockState {
                    name: "minecraft:chest".into(),
                    properties: BTreeMap::from([
                        ("facing".into(), facing.into()),
                        ("type".into(), "single".into()),
                        ("waterlogged".into(), "false".into()),
                    ]),
                }
            }
        };
        if matches!(self.monster_entities.get(&pos), Some(None)) {
            self.pending_monster_spawners = self
                .pending_monster_spawners
                .checked_sub(1)
                .context("invalid pending spawner count")?;
        }
        if self.monster_entities.contains_key(&pos) {
            Arc::make_mut(&mut self.monster_entities).remove(&pos);
        }
        self.set_section_block(pos, &state)?;
        self.update_final_heightmaps(pos, &state)?;
        match kind {
            RoomBlock::Chest(facing) => {
                let chest = super::GeneratedLootChest::new(
                    pos,
                    facing,
                    "minecraft:chests/simple_dungeon",
                    0,
                )?;
                Arc::make_mut(&mut self.monster_entities)
                    .insert(pos, Some(super::GeneratedMonsterRoomEntity::Chest(chest)));
            }
            RoomBlock::Spawner => {
                Arc::make_mut(&mut self.monster_entities).insert(pos, None);
                self.pending_monster_spawners += 1;
            }
            _ => {}
        }
        Ok(())
    }
    fn chest_entity_exists(&mut self, pos: [i32; 3]) -> Result<bool> {
        if OreWorld::block(self, pos)?.name != "minecraft:chest" {
            return Ok(false);
        }
        match self.monster_entities.get(&pos) {
            Some(Some(super::GeneratedMonsterRoomEntity::Chest(_))) => Ok(true),
            _ => bail!("chest block entity is unavailable"),
        }
    }
    fn set_chest_loot(&mut self, pos: [i32; 3], seed: i64) -> Result<()> {
        let Some(Some(super::GeneratedMonsterRoomEntity::Chest(chest))) =
            self.monster_entities.get(&pos)
        else {
            bail!("generated chest is unavailable")
        };
        let updated = super::GeneratedLootChest::new(
            pos,
            chest.facing,
            "minecraft:chests/simple_dungeon",
            seed,
        )?;
        Arc::make_mut(&mut self.monster_entities)
            .insert(pos, Some(super::GeneratedMonsterRoomEntity::Chest(updated)));
        Ok(())
    }
    fn spawner_entity_exists(&mut self, pos: [i32; 3]) -> Result<bool> {
        if OreWorld::block(self, pos)?.name != "minecraft:spawner" {
            return Ok(false);
        }
        match self.monster_entities.get(&pos) {
            Some(None) | Some(Some(super::GeneratedMonsterRoomEntity::Spawner(_))) => Ok(true),
            _ => bail!("spawner block entity is unavailable"),
        }
    }
    fn set_spawner_mob(
        &mut self,
        pos: [i32; 3],
        mob: RoomMob,
        _: &mut DecorationRandom,
    ) -> Result<()> {
        ensure!(
            matches!(
                self.monster_entities.get(&pos),
                Some(None) | Some(Some(super::GeneratedMonsterRoomEntity::Spawner(_)))
            ),
            "generated spawner is unavailable"
        );
        if matches!(self.monster_entities.get(&pos), Some(None)) {
            self.pending_monster_spawners = self
                .pending_monster_spawners
                .checked_sub(1)
                .context("invalid pending spawner count")?;
        }
        Arc::make_mut(&mut self.monster_entities).insert(
            pos,
            Some(super::GeneratedMonsterRoomEntity::Spawner(
                super::GeneratedMonsterSpawner { position: pos, mob },
            )),
        );
        Ok(())
    }
}

impl DecorationRegion<'_> {
    fn set_freeze_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        if !self.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(pos)),
            "freeze write chunk is missing"
        );
        if self.is_outside(pos[1]) {
            return Ok(());
        }
        let old = self.read(pos)?;
        let safe = if state.name == "minecraft:ice" {
            old.name == "minecraft:water" && state.properties.is_empty()
        } else if state.name == "minecraft:snow" {
            (is_air(&old) || native_world::collision::snow_layers(&old).is_some())
                && state.properties.len() == 1
                && state.properties.get("layers").is_some_and(|v| v == "1")
        } else {
            matches!(
                state.name.as_str(),
                "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
            ) && old.name == state.name
                && old.properties.len() == 1
                && matches!(
                    old.properties.get("snowy").map(String::as_str),
                    Some("true" | "false")
                )
                && state.properties.len() == 1
                && state.properties.get("snowy").is_some_and(|v| v == "true")
        };
        ensure!(safe, "unsupported freeze block replacement effects");
        self.set_section_block(pos, state)?;
        self.update_final_heightmaps(pos, state)?;
        Ok(())
    }
}
impl native_resources::freeze_feature::FreezeWorld for DecorationRegion<'_> {
    type State = BlockState;
    fn motion_height(&mut self, x: i32, z: i32) -> Result<i32> {
        self.final_height(2, x, z)
    }
    fn biome(&mut self, pos: [i32; 3]) -> Result<native_resources::biome_climate::BiomeClimate> {
        let ctx = self
            .freeze_context
            .as_ref()
            .context("freeze biome context is unavailable")?;
        let id = ctx.biomes.biome_at_block(pos[0], pos[1], pos[2])?;
        ctx.climates
            .get(id)
            .copied()
            .context("freeze biome climate is unavailable")
    }
    fn sea_level(&self) -> Result<i32> {
        Ok(self
            .freeze_context
            .as_ref()
            .context("freeze context is unavailable")?
            .sea_level)
    }
    fn inside_build_height(&self, y: i32) -> bool {
        !self.is_outside(y)
    }
    fn block_light(&mut self, pos: [i32; 3]) -> Result<u8> {
        let sample = self
            .freeze_context
            .as_ref()
            .and_then(|v| v.block_light)
            .context("generation block light is unavailable")?;
        let light = sample(pos)?;
        ensure!(light <= 15, "invalid generation block light");
        Ok(light)
    }
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self, pos)
    }
    fn freezable_water(&mut self, _: [i32; 3], state: &BlockState) -> Result<bool> {
        if state.name != "minecraft:water" {
            return Ok(false);
        }
        Ok(native_world::fluid::FluidState::from_block(state)
            .map_err(anyhow::Error::msg)?
            .is_some_and(|v| v.kind == native_world::fluid::FluidKind::Water && v.source))
    }
    fn air_or_snow(&mut self, state: &BlockState) -> Result<bool> {
        if state.name == "minecraft:snow" {
            ensure!(
                native_world::collision::snow_layers(state).is_some(),
                "invalid snow layers"
            );
            return Ok(true);
        }
        Ok(is_air(state))
    }
    fn snow_survives(&mut self, pos: [i32; 3]) -> Result<bool> {
        let below = OreWorld::block(self, [pos[0], pos[1].wrapping_sub(1), pos[2]])?;
        if let Some(result) = self
            .freeze_context
            .as_ref()
            .context("snow support tags are unavailable")?
            .support
            .support_override(&below.name)
        {
            return Ok(result);
        }
        if below.name == "minecraft:snow" {
            return Ok(native_world::collision::snow_layers(&below)
                .context("invalid snow layers")?
                == 8);
        }
        if tree_oak_log(&below) || tree_oak_leaf(&below)? {
            return Ok(true);
        }
        if below.name == "minecraft:cactus" {
            return Ok(false);
        }
        disk_safe_solid(&below.name).context("unsupported snow support shape")
    }
    fn put_ice(&mut self, pos: [i32; 3]) -> Result<()> {
        self.set_freeze_block(
            pos,
            &BlockState {
                name: "minecraft:ice".into(),
                properties: BTreeMap::new(),
            },
        )
    }
    fn put_snow(&mut self, pos: [i32; 3]) -> Result<()> {
        self.set_freeze_block(
            pos,
            &BlockState {
                name: "minecraft:snow".into(),
                properties: BTreeMap::from([("layers".into(), "1".into())]),
            },
        )
    }
    fn mark_snowy(&mut self, pos: [i32; 3]) -> Result<()> {
        let mut state = OreWorld::block(self, pos)?;
        if let Some(value) = state.properties.get_mut("snowy") {
            ensure!(
                matches!(value.as_str(), "true" | "false"),
                "invalid snowy property"
            );
            *value = "true".into();
            self.set_freeze_block(pos, &state)?;
        }
        Ok(())
    }
}

impl LeafUpdateWorld for DecorationRegion<'_> {
    fn optional_leaf_distance(&mut self, pos: [i32; 3]) -> Result<Option<u8>> {
        let state = OreWorld::block(self, pos)?;
        if self.tags.contains(tree_decay_tag(), &state.name)? {
            return Ok(Some(0));
        }
        match state.properties.get("distance") {
            None => Ok(None),
            Some(value) => Ok(Some(
                value
                    .parse::<u8>()
                    .ok()
                    .filter(|v| (1..=7).contains(v))
                    .context("unsupported leaf distance")?,
            )),
        }
    }
    fn set_leaf_distance(&mut self, pos: [i32; 3], distance: u8) -> Result<()> {
        let mut state = OreWorld::block(self, pos)?;
        ensure!(
            tree_generated_leaf(&state)?,
            "unsupported distance-bearing block"
        );
        state
            .properties
            .insert("distance".into(), distance.to_string());
        self.set_tree_block(pos, &state, 19)
    }
}

impl native_resources::tree_leaf_update::ShapeEdgeWorld for DecorationRegion<'_> {
    type State = BlockState;
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self, pos)
    }
    fn update_shape(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        direction: [i32; 3],
        _: [i32; 3],
        neighbor: &BlockState,
    ) -> Result<BlockState> {
        if tree_generated_leaf(state)? {
            if state
                .properties
                .get("waterlogged")
                .is_some_and(|v| v == "true")
            {
                SpringWorld::schedule_spring_tick(
                    self,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            let neighbor_distance = if self.tags.contains(tree_decay_tag(), &neighbor.name)? {
                0
            } else {
                match neighbor.properties.get("distance") {
                    Some(value) => value
                        .parse::<u8>()
                        .ok()
                        .filter(|v| (1..=7).contains(v))
                        .context("unsupported neighbor leaf distance")?,
                    None => 7,
                }
            };
            let own = state.properties["distance"].parse::<u8>()?;
            if neighbor_distance + 1 != 1 || own != 1 {
                SimpleBlockWorld::schedule_simple_tick(self, pos)?;
            }
        } else if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            use native_world::fluid::{FluidKind, FluidState};
            ensure!(
                state.properties.len() == 1
                    && state.properties.get("level").is_some_and(|raw| raw
                        .parse::<u8>()
                        .ok()
                        .is_some_and(|v| v <= 15 && v.to_string() == *raw)),
                "invalid generation fluid state"
            );
            let fluid = FluidState::from_block(state)
                .map_err(anyhow::Error::msg)?
                .context("missing generation fluid")?;
            let neighbor_fluid = FluidState::from_block(neighbor).map_err(anyhow::Error::msg)?;
            let id = match (fluid.kind, fluid.source) {
                (FluidKind::Water, true) => "minecraft:water",
                (FluidKind::Water, false) => "minecraft:flowing_water",
                (FluidKind::Lava, true) => "minecraft:lava",
                (FluidKind::Lava, false) => "minecraft:flowing_lava",
            };
            if fluid.source || neighbor_fluid.is_some_and(|f| f.source) {
                SpringWorld::schedule_spring_tick(self, pos, &ResourceId::parse(id)?)?;
            }
            if direction == [0, -1, 0]
                && fluid.source
                && fluid.amount == 8
                && self
                    .bubble_column_fluids
                    .context("bubble-column fluid tags unavailable")?
                    .contains(id)
            {
                let down = ResourceId::parse("minecraft:enables_bubble_column_drag_down")?;
                let up = ResourceId::parse("minecraft:enables_bubble_column_push_up")?;
                if self.tags.contains(&down, &neighbor.name)?
                    || self.tags.contains(&up, &neighbor.name)?
                {
                    SimpleBlockWorld::schedule_simple_tick(self, pos)?;
                }
            }
        } else if state.properties.is_empty()
            && matches!(
                state.name.as_str(),
                "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
            )
        {
            SimpleBlockWorld::schedule_simple_tick(self, pos)?;
        } else if tree_mushroom_block(state)? {
            if neighbor.name == state.name {
                ensure!(
                    tree_mushroom_block(neighbor)?,
                    "invalid neighboring giant mushroom"
                );
                let face = match direction {
                    [0, -1, 0] => "down",
                    [0, 1, 0] => "up",
                    [0, 0, -1] => "north",
                    [0, 0, 1] => "south",
                    [-1, 0, 0] => "west",
                    [1, 0, 0] => "east",
                    _ => bail!("invalid mushroom edge direction"),
                };
                let mut updated = state.clone();
                updated.properties.insert(face.into(), "false".into());
                return Ok(updated);
            }
        } else if tree_grass_block(state) {
            if direction == [0, 1, 0] {
                let snowy = self
                    .tags
                    .contains(&ResourceId::parse("minecraft:snow")?, &neighbor.name)?;
                let mut updated = state.clone();
                updated.properties.insert("snowy".into(), snowy.to_string());
                return Ok(updated);
            }
        } else if tree_leaf_litter(state)? {
            let below = OreWorld::block(self, [pos[0], pos[1].wrapping_sub(1), pos[2]])?;
            if !tree_sturdy_up(&below)? {
                return Ok(BlockState {
                    name: "minecraft:air".into(),
                    properties: BTreeMap::new(),
                });
            }
        } else if let Some(kind) = tree_edge_plant(state)? {
            if matches!(kind, SimplePlantKind::Double) {
                let half = state.properties["half"].as_str();
                let pair_direction = (half == "lower" && direction == [0, 1, 0])
                    || (half == "upper" && direction == [0, -1, 0]);
                if pair_direction && neighbor.name == state.name {
                    simple_plant_kind(neighbor)?;
                }
                if pair_direction
                    && (neighbor.name != state.name
                        || neighbor.properties.get("half").map(String::as_str) == Some(half))
                {
                    return Ok(BlockState {
                        name: "minecraft:air".into(),
                        properties: BTreeMap::new(),
                    });
                }
            }
            let below = OreWorld::block(self, [pos[0], pos[1].wrapping_sub(1), pos[2]])?;
            let survives = if matches!(kind, SimplePlantKind::Carpet) {
                !is_air(&below)
            } else if matches!(kind, SimplePlantKind::Double) && state.properties["half"] == "upper"
            {
                below.name == state.name
                    && below.properties.get("half").map(String::as_str) == Some("lower")
            } else {
                let tag = if matches!(kind, SimplePlantKind::Azalea) {
                    tree_azalea_tag()
                } else {
                    tree_vegetation_tag(matches!(kind, SimplePlantKind::DryVegetation))
                };
                self.tags.contains(tag, &below.name)?
            };
            if !survives {
                return Ok(BlockState {
                    name: "minecraft:air".into(),
                    properties: BTreeMap::new(),
                });
            }
        } else {
            ensure!(
                tree_generated_log(state)
                    || (state.name == "minecraft:bee_nest"
                        && state.properties.len() == 2
                        && state.properties.get("facing").is_some_and(|v| v == "south")
                        && state
                            .properties
                            .get("honey_level")
                            .is_some_and(|v| v == "0"))
                    || (state.properties.is_empty()
                        && matches!(
                            state.name.as_str(),
                            "minecraft:air"
                                | "minecraft:cave_air"
                                | "minecraft:void_air"
                                | "minecraft:stone"
                                | "minecraft:dirt"
                                | "minecraft:rooted_dirt"
                        )),
                "unsupported tree edge shape update for {}",
                state.name
            );
        }
        Ok(state.clone())
    }
    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        self.set_tree_block(pos, state, flags)
    }
}

// Ore states are used here only for solid and fluid predicates, not as proof
// that replacing them preserves all other world-generation side effects.
fn generated_ore_state(state: &BlockState) -> Result<bool> {
    let name = state.name.as_str();
    let redstone = matches!(
        name,
        "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore"
    );
    if !redstone
        && !matches!(
            name,
            "minecraft:coal_ore"
                | "minecraft:deepslate_coal_ore"
                | "minecraft:iron_ore"
                | "minecraft:deepslate_iron_ore"
                | "minecraft:copper_ore"
                | "minecraft:deepslate_copper_ore"
                | "minecraft:gold_ore"
                | "minecraft:deepslate_gold_ore"
                | "minecraft:emerald_ore"
                | "minecraft:deepslate_emerald_ore"
                | "minecraft:lapis_ore"
                | "minecraft:deepslate_lapis_ore"
                | "minecraft:diamond_ore"
                | "minecraft:deepslate_diamond_ore"
                | "minecraft:nether_gold_ore"
                | "minecraft:nether_quartz_ore"
        )
    {
        return Ok(false);
    }
    ensure!(
        if redstone {
            state.properties.len() == 1
                && matches!(
                    state.properties.get("lit").map(String::as_str),
                    Some("true" | "false")
                )
        } else {
            state.properties.is_empty()
        },
        "invalid generated ore state"
    );
    Ok(true)
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
        | "minecraft:rooted_dirt"
        | "minecraft:mycelium"
        | "minecraft:tuff"
        | "minecraft:calcite"
        | "minecraft:smooth_basalt"
        | "minecraft:amethyst_block"
        | "minecraft:budding_amethyst"
        | "minecraft:granite"
        | "minecraft:diorite"
        | "minecraft:andesite"
        | "minecraft:bedrock"
        | "minecraft:red_sand"
        | "minecraft:moss_block" => Some(true),
        "minecraft:magma_block" | "minecraft:cactus" => Some(true),
        "minecraft:sugar_cane" | "minecraft:cactus_flower" | "minecraft:hanging_roots" => {
            Some(false)
        }
        _ => None,
    }
}

// These vanilla blocks have no block entity, POI, or setBlock postprocessing.
// Reject unknown data-pack effects until their semantics can be preserved.
fn geode_safe_state(state: &BlockState) -> Result<()> {
    if matches!(
        state.name.as_str(),
        "minecraft:amethyst_block"
            | "minecraft:budding_amethyst"
            | "minecraft:calcite"
            | "minecraft:smooth_basalt"
    ) {
        ensure!(state.properties.is_empty(), "invalid geode layer state");
        return Ok(());
    }
    if matches!(
        state.name.as_str(),
        "minecraft:small_amethyst_bud"
            | "minecraft:medium_amethyst_bud"
            | "minecraft:large_amethyst_bud"
            | "minecraft:amethyst_cluster"
    ) {
        ensure!(
            state.properties.len() == 2
                && matches!(
                    state.properties.get("facing").map(String::as_str),
                    Some("down" | "up" | "north" | "south" | "west" | "east")
                )
                && matches!(
                    state.properties.get("waterlogged").map(String::as_str),
                    Some("true" | "false")
                ),
            "invalid geode crystal state"
        );
        return Ok(());
    }
    ensure!(
        !matches!(state.name.as_str(), "minecraft:chest" | "minecraft:spawner"),
        "unsupported geode block entity"
    );
    monster_block_flags(state)?;
    Ok(())
}

fn multiface_name(face: MultifaceFace) -> &'static str {
    match face {
        MultifaceFace::Down => "down",
        MultifaceFace::Up => "up",
        MultifaceFace::North => "north",
        MultifaceFace::South => "south",
        MultifaceFace::West => "west",
        MultifaceFace::East => "east",
    }
}
fn valid_lichen(state: &BlockState) -> bool {
    state.name == "minecraft:glow_lichen"
        && state.properties.len() == 7
        && [
            "down",
            "up",
            "north",
            "south",
            "west",
            "east",
            "waterlogged",
        ]
        .into_iter()
        .all(|name| {
            matches!(
                state.properties.get(name).map(String::as_str),
                Some("true" | "false")
            )
        })
}
fn multiface_support(state: &BlockState) -> Result<bool> {
    if is_air(state)
        || matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
        || valid_lichen(state)
        || tree_cave_vines(state)?
        || generated_dripleaf(state)?
    {
        return Ok(false);
    }
    if state.name == "minecraft:vine" {
        ensure!(valid_vine(state), "invalid vine state");
        return Ok(false);
    }
    if tree_generated_log(state) || tree_generated_leaf(state)? {
        return Ok(true);
    }
    if tree_mushroom_block(state)? || tree_grass_block(state) {
        return Ok(true);
    }
    if tree_leaf_litter(state)? {
        return Ok(false);
    }
    if matches!(
        state.name.as_str(),
        "minecraft:moss_block"
            | "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:rooted_dirt"
            | "minecraft:clay"
            | "minecraft:gravel"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:sandstone"
            | "minecraft:red_sandstone"
            | "minecraft:ice"
            | "minecraft:packed_ice"
            | "minecraft:blue_ice"
            | "minecraft:snow_block"
            | "minecraft:magma_block"
            | "minecraft:netherrack"
            | "minecraft:blackstone"
    ) {
        ensure!(
            state.properties.is_empty(),
            "invalid multiface support state"
        );
        return Ok(true);
    }
    if matches!(
        state.name.as_str(),
        "minecraft:stone"
            | "minecraft:andesite"
            | "minecraft:diorite"
            | "minecraft:granite"
            | "minecraft:dripstone_block"
            | "minecraft:calcite"
            | "minecraft:tuff"
            | "minecraft:deepslate"
            | "minecraft:sulfur"
            | "minecraft:cinnabar"
            | "minecraft:bedrock"
            | "minecraft:amethyst_block"
            | "minecraft:budding_amethyst"
            | "minecraft:smooth_basalt"
            | "minecraft:glass"
    ) {
        return Ok(true);
    }
    if matches!(
        simple_plant_kind(state),
        Ok(SimplePlantKind::Vegetation
            | SimplePlantKind::DryVegetation
            | SimplePlantKind::Double
            | SimplePlantKind::Seagrass
            | SimplePlantKind::TallSeagrass
            | SimplePlantKind::SporeBlossom)
    ) {
        return Ok(false);
    }
    bail!("unsupported multiface support shape for {}", state.name)
}

fn valid_vine(state: &BlockState) -> bool {
    state.name == "minecraft:vine"
        && state.properties.len() == 5
        && ["up", "north", "south", "west", "east"]
            .into_iter()
            .all(|name| {
                matches!(
                    state.properties.get(name).map(String::as_str),
                    Some("true" | "false")
                )
            })
}

impl VinesWorld for DecorationRegion<'_> {
    type State = BlockState;
    fn vines_empty(&mut self, pos: [i32; 3]) -> Result<bool> {
        Ok(is_air(&OreWorld::block(self, pos)?))
    }
    fn vines_can_attach(&mut self, neighbor: [i32; 3], _: MultifaceFace) -> Result<bool> {
        let state = OreWorld::block(self, neighbor)?;
        // This helper accepts only blocks whose support and collision shapes
        // are known to be full or empty in all six directions.
        multiface_support(&state)
    }
    fn vine_with_face(
        &mut self,
        state: &BlockState,
        direction: MultifaceFace,
    ) -> Result<BlockState> {
        ensure!(valid_vine(state), "invalid default vine state");
        ensure!(direction != MultifaceFace::Down, "invalid vine attachment");
        let mut state = state.clone();
        state
            .properties
            .insert(multiface_name(direction).into(), "true".into());
        Ok(state)
    }
    fn set_vines(&mut self, pos: [i32; 3], state: &BlockState) -> Result<bool> {
        let result = (|| {
            if !self.writable.contains(&chunk(pos)) {
                return Ok(false);
            }
            ensure!(
                self.world.chunks.contains_key(&chunk(pos)),
                "vines chunk is missing"
            );
            if self.is_outside(pos[1]) {
                return Ok(true);
            }
            ensure!(is_air(&self.read(pos)?), "unsupported vines source effects");
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

impl native_resources::multiface_growth::MultifaceWorld for DecorationRegion<'_> {
    type State = BlockState;
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self, pos)
    }
    fn air_or_water(&mut self, state: &BlockState) -> Result<bool> {
        Ok(is_air(state) || state.name == "minecraft:water")
    }
    fn is_growth(&mut self, state: &BlockState, block: &ResourceId) -> Result<bool> {
        Ok(state
            .name
            .split_once(':')
            .is_some_and(|(namespace, path)| namespace == block.namespace && path == block.path))
    }
    fn in_block_set(&mut self, state: &BlockState, set: &MultifaceBlockSet) -> Result<bool> {
        match set {
            MultifaceBlockSet::Tag(id) => self.tags.contains(id, &state.name),
            MultifaceBlockSet::Blocks(ids) => {
                Ok(state.name.split_once(':').is_some_and(|(namespace, path)| {
                    ids.iter()
                        .any(|id| namespace == id.namespace && path == id.path)
                }))
            }
        }
    }
    fn state_for_placement(
        &mut self,
        block: &ResourceId,
        existing: &BlockState,
        pos: [i32; 3],
        face: MultifaceFace,
    ) -> Result<Option<BlockState>> {
        ensure!(
            block.namespace == "minecraft" && block.path == "glow_lichen",
            "unsupported multiface block"
        );
        if existing.name == "minecraft:glow_lichen" {
            ensure!(valid_lichen(existing), "invalid glow lichen state");
            if existing.properties[multiface_name(face)] == "true" {
                return Ok(None);
            }
        }
        let support = OreWorld::block(self, face.offset(pos))?;
        if !multiface_support(&support)? {
            return Ok(None);
        }
        let mut result = if existing.name == "minecraft:glow_lichen" {
            existing.clone()
        } else {
            let fluid = native_world::fluid::FluidState::from_block(existing)
                .map_err(anyhow::Error::msg)?;
            let waterlogged =
                fluid.is_some_and(|v| v.kind == native_world::fluid::FluidKind::Water && v.source);
            let mut properties = [
                "down",
                "up",
                "north",
                "south",
                "west",
                "east",
                "waterlogged",
            ]
            .into_iter()
            .map(|s| (s.to_owned(), "false".to_owned()))
            .collect::<BTreeMap<_, _>>();
            properties.insert("waterlogged".into(), waterlogged.to_string());
            BlockState {
                name: "minecraft:glow_lichen".into(),
                properties,
            }
        };
        result
            .properties
            .insert(multiface_name(face).into(), "true".into());
        Ok(Some(result))
    }
    fn has_face(&mut self, state: &BlockState, face: MultifaceFace) -> Result<bool> {
        ensure!(valid_lichen(state), "invalid glow lichen state");
        Ok(state.properties[multiface_name(face)] == "true")
    }
    fn can_replace_for_spread(&mut self, state: &BlockState, block: &ResourceId) -> Result<bool> {
        ensure!(
            block.namespace == "minecraft" && block.path == "glow_lichen",
            "unsupported multiface block"
        );
        if is_air(state) {
            return Ok(true);
        }
        if state.name == "minecraft:glow_lichen" {
            ensure!(valid_lichen(state), "invalid glow lichen state");
            return Ok(true);
        }
        if state.name != "minecraft:water" {
            return Ok(false);
        }
        Ok(native_world::fluid::FluidState::from_block(state)
            .map_err(anyhow::Error::msg)?
            .is_some_and(|v| v.source))
    }
    fn set_growth(&mut self, pos: [i32; 3], state: &BlockState, _initial: bool) -> Result<bool> {
        if !self.writable.contains(&chunk(pos)) {
            return Ok(false);
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(pos)),
            "multiface chunk missing"
        );
        ensure!(valid_lichen(state), "invalid glow lichen write");
        if self.is_outside(pos[1]) {
            return Ok(true);
        }
        let old = self.read(pos)?;
        ensure!(
            is_air(&old) || old.name == "minecraft:water" || valid_lichen(&old),
            "unsupported multiface replacement effect"
        );
        self.set_section_block(pos, state)?;
        self.update_final_heightmaps(pos, state)?;
        Ok(true)
    }
    fn mark_postprocessing(&mut self, pos: [i32; 3]) -> Result<()> {
        DiskWorld::mark_postprocessing(self, pos)
    }
}

impl GeodeEnvironment for DecorationRegion<'_> {
    fn set_geode_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        if !self.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.world.chunks.contains_key(&chunk(pos)),
            "geode chunk is missing"
        );
        geode_safe_state(state)?;
        if self.is_outside(pos[1]) {
            return Ok(());
        }
        geode_safe_state(&self.read(pos)?)?;
        self.set_section_block(pos, state)?;
        self.update_final_heightmaps(pos, state)
    }
    fn schedule_geode_fluid(&mut self, pos: [i32; 3]) -> Result<()> {
        use native_world::fluid::{FluidKind, FluidState};
        let state = OreWorld::block(self, pos)?;
        geode_safe_state(&state)?;
        if let Some(fluid) = FluidState::from_block(&state).map_err(anyhow::Error::msg)? {
            let name = match (fluid.kind, fluid.source) {
                (FluidKind::Water, true) => "minecraft:water",
                (FluidKind::Water, false) => "minecraft:flowing_water",
                (FluidKind::Lava, true) => "minecraft:lava",
                (FluidKind::Lava, false) => "minecraft:flowing_lava",
            };
            SpringWorld::schedule_spring_tick(self, pos, &ResourceId::parse(name)?)?;
        }
        Ok(())
    }
    fn geode_placement_state(
        &mut self,
        state: &BlockState,
        direction: GeodeDirection,
        existing: &BlockState,
    ) -> Result<BlockState> {
        geode_safe_state(state)?;
        let mut result = state.clone();
        if let Some(value) = result.properties.get_mut("facing") {
            *value = match direction {
                GeodeDirection::Down => "down",
                GeodeDirection::Up => "up",
                GeodeDirection::North => "north",
                GeodeDirection::South => "south",
                GeodeDirection::West => "west",
                GeodeDirection::East => "east",
            }
            .into();
        }
        if let Some(value) = result.properties.get_mut("waterlogged") {
            let source = native_world::fluid::FluidState::from_block(existing)
                .map_err(anyhow::Error::msg)?
                .is_some_and(|f| f.source);
            *value = source.to_string();
        }
        Ok(result)
    }
    fn geode_can_grow(&mut self, state: &BlockState) -> Result<bool> {
        if is_air(state) {
            return Ok(true);
        }
        if state.name != "minecraft:water" {
            return Ok(false);
        }
        Ok(native_world::fluid::FluidState::from_block(state)
            .map_err(anyhow::Error::msg)?
            .is_some_and(|fluid| fluid.amount == 8))
    }
}

impl DiskWorld for DecorationRegion<'_> {
    fn face_sturdy(
        &mut self,
        state: &BlockState,
        _: [i32; 3],
        direction: [i32; 3],
    ) -> Result<bool> {
        generation_face_sturdy(state, direction)
    }
    fn integer_property_bounds(
        &mut self,
        state: &BlockState,
        name: &str,
    ) -> Result<Option<(i32, i32)>> {
        let Some(raw) = state.properties.get(name) else {
            return Ok(None);
        };
        let bounds = match (state.name.as_str(), name) {
            ("minecraft:cave_vines", "age") => Some((0, 25)),
            ("minecraft:sugar_cane" | "minecraft:cactus", "age") => Some((0, 15)),
            ("minecraft:sweet_berry_bush", "age") => Some((0, 3)),
            ("minecraft:water" | "minecraft:lava", "level") => Some((0, 15)),
            (
                "minecraft:oak_leaves"
                | "minecraft:dark_oak_leaves"
                | "minecraft:azalea_leaves"
                | "minecraft:flowering_azalea_leaves",
                "distance",
            ) => Some((1, 7)),
            ("minecraft:leaf_litter", "segment_amount") => Some((1, 4)),
            ("minecraft:snow", "layers") => Some((1, 8)),
            (
                "minecraft:oak_sapling"
                | "minecraft:spruce_sapling"
                | "minecraft:birch_sapling"
                | "minecraft:jungle_sapling"
                | "minecraft:acacia_sapling"
                | "minecraft:dark_oak_sapling"
                | "minecraft:cherry_sapling"
                | "minecraft:pale_oak_sapling",
                "stage",
            ) => Some((0, 1)),
            _ if raw.parse::<i32>().is_ok() => {
                bail!("unsupported integer property {name} on {}", state.name)
            }
            _ => None,
        };
        if let Some((min, max)) = bounds {
            let current = raw
                .parse::<i32>()
                .context("invalid integer block property")?;
            ensure!(
                (min..=max).contains(&current) && current.to_string() == *raw,
                "invalid integer block property"
            );
        }
        Ok(bounds)
    }
    fn with_integer_property(
        &mut self,
        mut state: BlockState,
        name: &str,
        value: i32,
    ) -> Result<BlockState> {
        let (min, max) = self
            .integer_property_bounds(&state, name)?
            .context("integer property is absent")?;
        ensure!(
            (min..=max).contains(&value),
            "integer block property is out of range"
        );
        state.properties.insert(name.into(), value.to_string());
        Ok(state)
    }
    fn mushroom_survives(&mut self, pos: [i32; 3]) -> Result<bool> {
        self.mushroom_survival(pos)
    }
    fn small_dripleaf_survives(&mut self, value: &Value, pos: [i32; 3]) -> Result<bool> {
        let mut state = BlockState {
            name: "minecraft:small_dripleaf".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("half".into(), "lower".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        };
        if let Some(properties) = value.get("properties") {
            for (name, value) in properties
                .as_object()
                .context("invalid dripleaf properties")?
            {
                ensure!(
                    state.properties.contains_key(name),
                    "unknown dripleaf property"
                );
                state.properties.insert(
                    name.clone(),
                    value
                        .as_str()
                        .context("invalid dripleaf property value")?
                        .into(),
                );
            }
        }
        generated_dripleaf(&state)?;
        self.small_dripleaf_survival(pos, &state)
    }
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
        if geode_crystal(state)?
            || tree_generated_leaf(state)?
            || tree_generated_log(state)
            || tree_mushroom_block(state)?
            || generated_ore_state(state)?
        {
            return Ok(true);
        }
        if tree_cave_vines(state)? || generated_dripleaf(state)? || tree_glow_lichen(state)? {
            return Ok(false);
        }
        if let Some(solid) = disk_safe_solid(&state.name) {
            return Ok(solid);
        }
        if let Ok(kind) = simple_plant_kind(state) {
            return Ok(matches!(kind, SimplePlantKind::Unconditional));
        }
        bail!("unsupported disk solid predicate block {}", state.name)
    }
    fn is_liquid(&mut self, state: &BlockState) -> Result<bool> {
        if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            return Ok(true);
        }
        if geode_crystal(state)? {
            return Ok(false);
        }
        ensure!(
            disk_safe_solid(&state.name).is_some()
                || generated_ore_state(state)?
                || simple_plant_kind(state).is_ok(),
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
    Azalea,
    Carpet,
    Double,
    Unconditional,
    Mushroom,
    LeafLitter,
    Seagrass,
    TallSeagrass,
    SmallDripleaf,
    SporeBlossom,
}

// All states of these vanilla blocks have been checked for their Java
// survival implementation, POI/block-entity effects and postprocessing.
fn simple_plant_kind(state: &BlockState) -> Result<SimplePlantKind> {
    let name = state.name.as_str();
    if is_simple_carpet_id(name) {
        ensure!(state.properties.is_empty(), "invalid carpet state");
        return Ok(SimplePlantKind::Carpet);
    }
    let kind = match name {
        "minecraft:leaf_litter" => {
            tree_leaf_litter(state)?;
            SimplePlantKind::LeafLitter
        }
        "minecraft:azalea" | "minecraft:flowering_azalea" => {
            ensure!(state.properties.is_empty(), "invalid azalea state");
            SimplePlantKind::Azalea
        }
        "minecraft:seagrass" => {
            ensure!(state.properties.is_empty(), "invalid seagrass state");
            SimplePlantKind::Seagrass
        }
        "minecraft:tall_seagrass" => {
            ensure!(
                state.properties.len() == 1
                    && matches!(
                        state.properties.get("half").map(String::as_str),
                        Some("lower" | "upper")
                    ),
                "invalid tall seagrass state"
            );
            SimplePlantKind::TallSeagrass
        }
        "minecraft:small_dripleaf" => {
            generated_dripleaf(state)?;
            SimplePlantKind::SmallDripleaf
        }
        "minecraft:spore_blossom" => {
            ensure!(state.properties.is_empty(), "invalid spore blossom state");
            SimplePlantKind::SporeBlossom
        }
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
        "minecraft:brown_mushroom" | "minecraft:red_mushroom" => {
            ensure!(state.properties.is_empty(), "invalid mushroom state");
            SimplePlantKind::Mushroom
        }
        _ => bail!("unsupported simple-block survival for {name}"),
    };
    Ok(kind)
}

impl DecorationRegion<'_> {
    fn small_dripleaf_survival(&mut self, pos: [i32; 3], state: &BlockState) -> Result<bool> {
        generated_dripleaf(state)?;
        let below = [pos[0], pos[1].wrapping_sub(1), pos[2]];
        let support = self.block(below)?;
        if state.properties["half"] == "upper" {
            return Ok(support.name == state.name
                && support.properties.get("half").map(String::as_str) == Some("lower"));
        }
        if self.tags.contains(
            &ResourceId::parse("minecraft:supports_small_dripleaf")?,
            &support.name,
        )? {
            return Ok(true);
        }
        let current = self.block(pos)?;
        let source = native_world::fluid::FluidState::from_block(&current)
            .map_err(anyhow::Error::msg)?
            .is_some_and(|fluid| {
                fluid.kind == native_world::fluid::FluidKind::Water && fluid.source
            });
        Ok(source
            && self.tags.contains(
                &ResourceId::parse("minecraft:supports_vegetation")?,
                &support.name,
            )?)
    }

    fn spore_blossom_survival(&mut self, pos: [i32; 3]) -> Result<bool> {
        static UNSTABLE: OnceLock<ResourceId> = OnceLock::new();
        static WATER: OnceLock<ResourceId> = OnceLock::new();
        let above = [pos[0], pos[1].wrapping_add(1), pos[2]];
        let support = self.block(above)?;
        let unstable =
            UNSTABLE.get_or_init(|| ResourceId::parse("minecraft:unstable_bottom_center").unwrap());
        if self.tags.contains(unstable, &support.name)? {
            return Ok(false);
        }
        let center = vanilla_full_occlusion(&support.name).with_context(|| {
            format!("unsupported spore blossom center support {}", support.name)
        })?;
        if !center {
            return Ok(false);
        }
        let current = self.block(pos)?;
        let water = WATER.get_or_init(|| ResourceId::parse("minecraft:water").unwrap());
        Ok(!DiskWorld::fluid_in_tag(self, &current, water)?)
    }

    fn mushroom_survival(&mut self, pos: [i32; 3]) -> Result<bool> {
        let below = OreWorld::block(self, [pos[0], pos[1].wrapping_sub(1), pos[2]])?;
        static OVERRIDE: OnceLock<ResourceId> = OnceLock::new();
        let tag = OVERRIDE.get_or_init(|| {
            ResourceId::parse("minecraft:overrides_mushroom_light_requirement").unwrap()
        });
        if self.tags.contains(tag, &below.name)? {
            return Ok(true);
        }
        let brightness =
            self.raw_brightness
                .context("mushroom raw brightness unavailable")?(&self.world, pos)?;
        ensure!(brightness <= 15, "invalid mushroom raw brightness");
        if brightness >= 13 {
            return Ok(false);
        }
        if matches!(
            below.name.as_str(),
            "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
        ) {
            ensure!(
                below.properties.len() == 1
                    && matches!(
                        below.properties.get("snowy").map(String::as_str),
                        Some("true" | "false")
                    ),
                "invalid snowy mushroom support"
            );
            return Ok(true);
        }
        if matches!(
            below.name.as_str(),
            "minecraft:crimson_nylium" | "minecraft:warped_nylium"
        ) {
            ensure!(below.properties.is_empty(), "invalid nylium state");
            return Ok(true);
        }
        Ok(monster_block_flags(&below)?.1)
    }
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
            disk_safe_solid(&old.name).is_some()
                || generated_dripleaf(&old)?
                || simple_plant_kind(&old).is_ok(),
            "unsupported simple-block source effects"
        );
        self.set_section_block(p, state)?;
        self.update_final_heightmaps(p, state)?;
        // WorldGenRegion.setBlock records the mushroom's postprocess position.
        if matches!(
            state.name.as_str(),
            "minecraft:brown_mushroom" | "minecraft:red_mushroom"
        ) {
            DiskWorld::mark_postprocessing(self, p)?;
        }
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
                SimplePlantKind::Mushroom => self.mushroom_survival(pos)?,
                SimplePlantKind::SporeBlossom => self.spore_blossom_survival(pos)?,
                SimplePlantKind::Carpet => !is_air(&self.block(below)?),
                SimplePlantKind::LeafLitter => {
                    let state = self.block(below)?;
                    DiskWorld::face_sturdy(self, &state, below, [0, 1, 0])?
                }
                SimplePlantKind::Seagrass | SimplePlantKind::TallSeagrass => {
                    if matches!(kind, SimplePlantKind::TallSeagrass)
                        && state.properties["half"] == "upper"
                    {
                        let support = self.block(below)?;
                        support.name == state.name
                            && support.properties.get("half").map(String::as_str) == Some("lower")
                    } else {
                        let support = self.block(below)?;
                        let sturdy = DiskWorld::face_sturdy(self, &support, below, [0, 1, 0])?;
                        let allowed = sturdy
                            && !self.tags.contains(
                                &ResourceId::parse("minecraft:cannot_support_seagrass")?,
                                &support.name,
                            )?;
                        if allowed && matches!(kind, SimplePlantKind::TallSeagrass) {
                            let current = self.block(pos)?;
                            let fluid = native_world::fluid::FluidState::from_block(&current)
                                .map_err(anyhow::Error::msg)?;
                            fluid.is_some_and(|f| f.amount == 8)
                                && DiskWorld::fluid_in_tag(
                                    self,
                                    &current,
                                    &ResourceId::parse("minecraft:water")?,
                                )?
                        } else {
                            allowed
                        }
                    }
                }
                SimplePlantKind::SmallDripleaf => self.small_dripleaf_survival(pos, state)?,
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
                        SimplePlantKind::Azalea => "minecraft:supports_azalea",
                        _ => "minecraft:supports_vegetation",
                    };
                    self.tags.contains(&ResourceId::parse(tag)?, &below.name)?
                }
            };
            if !survives {
                return Ok(false);
            }
            if matches!(
                kind,
                SimplePlantKind::Double
                    | SimplePlantKind::TallSeagrass
                    | SimplePlantKind::SmallDripleaf
            ) {
                let above = [pos[0], pos[1].wrapping_add(1), pos[2]];
                let upper = self.block(above)?;
                let free = if is_air(&upper) {
                    true
                } else if let Some(upper_fluid) =
                    native_world::fluid::FluidState::from_block(&upper)
                        .map_err(anyhow::Error::msg)?
                {
                    if matches!(
                        kind,
                        SimplePlantKind::TallSeagrass | SimplePlantKind::SmallDripleaf
                    ) && Some(upper_fluid)
                        == native_world::fluid::FluidState::from_block(state)
                            .map_err(anyhow::Error::msg)?
                    {
                        match upper.name.as_str() {
                            "minecraft:water" => true,
                            "minecraft:seagrass" | "minecraft:tall_seagrass" => {
                                simple_plant_kind(&upper)?;
                                true
                            }
                            _ if disk_safe_solid(&upper.name).is_some()
                                || tree_generated_leaf(&upper)?
                                || generated_dripleaf(&upper)? =>
                            {
                                false
                            }
                            _ => bail!(
                                "unsupported fluid-filled double-plant replacement {}",
                                upper.name
                            ),
                        }
                    } else {
                        false
                    }
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
                            || tree_cave_vines(&upper)?
                            || simple_plant_kind(&upper).is_ok() =>
                        {
                            false
                        }
                        _ => bail!(
                            "unsupported double-plant replacement state {upper:?} at {above:?}"
                        ),
                    }
                };
                if !free {
                    return Ok(false);
                }
                let mut lower = state.clone();
                lower.properties.insert("half".into(), "lower".into());
                let mut upper = state.clone();
                upper.properties.insert("half".into(), "upper".into());
                if matches!(kind, SimplePlantKind::SmallDripleaf) {
                    let water_tag = ResourceId::parse("minecraft:water")?;
                    let current = self.block(pos)?;
                    lower.properties.insert(
                        "waterlogged".into(),
                        DiskWorld::fluid_in_tag(self, &current, &water_tag)?.to_string(),
                    );
                    let current_upper = self.block(above)?;
                    upper.properties.insert(
                        "waterlogged".into(),
                        DiskWorld::fluid_in_tag(self, &current_upper, &water_tag)?.to_string(),
                    );
                }
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
                "minecraft:cave_vines_plant" | "minecraft:cave_vines" => tree_cave_vines(state)?,
                "minecraft:big_dripleaf"
                | "minecraft:big_dripleaf_stem"
                | "minecraft:small_dripleaf" => generated_dripleaf(state)?,
                _ => false,
            };
            ensure!(
                valid,
                "unsupported column replacement effects at {pos:?}: {state:?}"
            );
            if self.is_outside(pos[1]) {
                return Ok(true);
            }
            let old = self.read(pos)?;
            ensure!(
                disk_safe_solid(&old.name).is_some()
                    || old.name == "minecraft:sugar_cane"
                    || matches!(
                        old.name.as_str(),
                        "minecraft:cave_vines" | "minecraft:cave_vines_plant"
                    )
                    || generated_dripleaf(&old)?
                    || simple_plant_kind(&old).is_ok(),
                "unsupported column source effects"
            );
            self.column_block_write = true;
            let write = self.set_section_block(pos, state);
            self.column_block_write = false;
            write?;
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
        let mut unchanged_registered = false;
        if let Some(guard) = &self.light_guard {
            if guard.registered.contains(&chunk(p)) {
                let old = self.read(p)?;
                if old == *state {
                    unchanged_registered = true;
                } else {
                    let old_air = super::generated_state_is_air(guard.registry, &old)
                        .context("registered light old state is unknown")?;
                    let new_air = super::generated_state_is_air(guard.registry, state)
                        .context("registered light new state is unknown")?;
                    let tree_light_update = self.tree_block_write
                        && old_air
                        && (!guard.require_unchanged_light
                            || !matches!(
                                state.name.as_str(),
                                "minecraft:birch_log" | "minecraft:birch_leaves"
                            ))
                        && ((matches!(
                            state.name.as_str(),
                            "minecraft:dark_oak_leaves" | "minecraft:birch_leaves"
                        ) && state.properties.get("waterlogged").map(String::as_str)
                            == Some("false")
                            && tree_generated_leaf(state)?)
                            || (matches!(
                                state.name.as_str(),
                                "minecraft:dark_oak_log" | "minecraft:birch_log"
                            ) && tree_generated_log(state)));
                    let fresh_patch_water = !guard.require_unchanged_light
                        && self.patch_block_write
                        && old.name == "minecraft:clay"
                        && old.properties.is_empty()
                        && state.name == "minecraft:water"
                        && state.properties.len() == 1
                        && state.properties.get("level").map(String::as_str) == Some("0");
                    let fresh_water_dripleaf = !guard.require_unchanged_light
                        && self.column_block_write
                        && old.name == "minecraft:water"
                        && old.properties.len() == 1
                        && old.properties.get("level").map(String::as_str) == Some("0")
                        && state.name == "minecraft:big_dripleaf"
                        && generated_dripleaf(state)?
                        && state.properties.get("waterlogged").map(String::as_str) == Some("false")
                        && state.properties.get("tilt").map(String::as_str) == Some("none");
                    let fresh_waterlog_dripleaf = !guard.require_unchanged_light
                        && self.patch_block_write
                        && old.name == "minecraft:big_dripleaf"
                        && state.name == old.name
                        && generated_dripleaf(&old)?
                        && generated_dripleaf(state)?
                        && old.properties.get("waterlogged").map(String::as_str) == Some("false")
                        && state.properties.get("waterlogged").map(String::as_str) == Some("true")
                        && old.properties.get("tilt").map(String::as_str) == Some("none")
                        && old.properties.get("tilt") == state.properties.get("tilt")
                        && old.properties.get("facing") == state.properties.get("facing");
                    let vine_write = guard.require_unchanged_light
                        && self.column_block_write
                        && tree_cave_vines(state)?
                        && (old_air || tree_cave_vines(&old)?);
                    let vine_check = if vine_write {
                        !native_world::block_light::skips_light_check_for_edit(&old, state)
                            .context("unsupported cave vine light check")?
                    } else {
                        false
                    };
                    if guard.require_unchanged_light {
                        ensure!(
                            tree_light_update
                                || vine_write
                                || native_world::block_light::same_light_behavior(&old, state)
                                    == Some(true),
                            "registered light behavior changed at {p:?}: {} -> {}",
                            old.name,
                            state.name
                        );
                    }
                    // Model block checks as drained before activation. With
                    // no active sources, unchanged sky behavior and section
                    // allocation, the checked stored light remains zero.
                    ensure!(
                        tree_light_update
                            || fresh_patch_water
                            || fresh_water_dripleaf
                            || fresh_waterlog_dripleaf
                            || vine_write
                            || native_world::sky_light::same_sky_behavior(&old, state)
                                == Some(true),
                        "registered light chunk has changed at {p:?}: {} -> {}",
                        old.name,
                        state.name
                    );
                    let mut new_section = false;
                    if old_air != new_air {
                        let key = (chunk(p), i8::try_from(p[1].div_euclid(16))?);
                        let count = if let Some(count) = guard.non_air_counts.get(&key) {
                            *count
                        } else {
                            let blocks = self
                                .world
                                .chunks
                                .get(&key.0)
                                .and_then(|c| c.sections.get(&key.1))
                                .and_then(|s| s.blocks.as_ref())
                                .context("registered light section is missing")?;
                            let mut count = 0;
                            for id in blocks.iter() {
                                let block =
                                    self.world.state(*id).context("unknown section block")?;
                                if !super::generated_state_is_air(guard.registry, block)
                                    .context("unknown registered section block")?
                                {
                                    count += 1;
                                }
                            }
                            count
                        };
                        new_section = old_air && count == 0 && (tree_light_update || vine_write);
                        ensure!(
                            if old_air {
                                count > 0 || new_section
                            } else {
                                count > 1
                            },
                            "registered light section occupancy changed at {p:?}"
                        );
                        let updated = if old_air { count + 1 } else { count - 1 };
                        let guard = self.light_guard.as_mut().unwrap();
                        guard.non_air_counts.insert(key, updated);
                        if new_section {
                            guard.new_nonempty_sections.push((key.0, i32::from(key.1)));
                        }
                    }
                    if tree_light_update
                        || fresh_patch_water
                        || fresh_water_dripleaf
                        || fresh_waterlog_dripleaf
                        || vine_check
                        || new_section
                    {
                        let guard = self.light_guard.as_mut().unwrap();
                        guard.light_replay_positions.insert(p);
                        if vine_write {
                            guard.vine_light_positions.insert(p);
                        }
                        if tree_light_update || vine_check {
                            guard.light_check_order.push(p);
                        }
                    }
                    self.light_guard
                        .as_mut()
                        .unwrap()
                        .changed_blocks
                        .insert(state.name.clone());
                    self.light_guard
                        .as_mut()
                        .unwrap()
                        .changed_chunks
                        .insert(chunk(p));
                }
            }
        }
        if self.bee_nests.contains_key(&p) {
            ensure!(
                self.read(p)? == *state,
                "generated bee nest replacement is not supported"
            );
        }
        if self.monster_entities.contains_key(&p) {
            ensure!(
                self.read(p)? == *state,
                "generated monster-room entity replacement is not supported"
            );
        }
        // Preserve both original maps before the first direct write in this
        // chunk; untouched neighbors need no heightmap scan.
        self.prime_heightmaps(chunk(p))?;
        if unchanged_registered {
            return Ok(());
        }
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
                "data/minecraft/tags/block/supports_azalea.json" => {
                    Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore"]}"#.to_vec())
                }
                "data/minecraft/tags/block/air.json" => Some(
                    br#"{"values":["minecraft:air","minecraft:cave_air","minecraft:void_air"]}"#
                        .to_vec(),
                ),
                "data/minecraft/tags/block/enables_bubble_column_drag_down.json"
                | "data/minecraft/tags/block/enables_bubble_column_push_up.json" => {
                    Some(br#"{"values":[]}"#.to_vec())
                }
                "data/minecraft/tags/fluid/bubble_column_can_occupy.json" => {
                    Some(br#"{"values":["minecraft:water"]}"#.to_vec())
                }
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

    #[test]
    fn generated_plant_variants_have_consistent_support_faces() {
        let mut states = Vec::new();
        for name in [
            "oak_sapling",
            "spruce_sapling",
            "birch_sapling",
            "jungle_sapling",
            "acacia_sapling",
            "dark_oak_sapling",
            "cherry_sapling",
            "pale_oak_sapling",
        ] {
            for stage in ["0", "1"] {
                let mut block = state(name);
                block.properties.insert("stage".into(), stage.into());
                states.push(block);
            }
        }
        for name in [
            "tall_grass",
            "large_fern",
            "sunflower",
            "rose_bush",
            "peony",
            "lilac",
            "tall_seagrass",
        ] {
            for half in ["lower", "upper"] {
                let mut block = state(name);
                block.properties.insert("half".into(), half.into());
                states.push(block);
            }
        }
        for age in 0..=3 {
            let mut block = state("sweet_berry_bush");
            block.properties.insert("age".into(), age.to_string());
            states.push(block);
        }
        for half in ["lower", "upper"] {
            for facing in ["north", "south", "east", "west"] {
                for waterlogged in ["true", "false"] {
                    let mut block = state("small_dripleaf");
                    block.properties = BTreeMap::from([
                        ("half".into(), half.into()),
                        ("facing".into(), facing.into()),
                        ("waterlogged".into(), waterlogged.into()),
                    ]);
                    states.push(block);
                }
            }
        }
        for block in &states {
            assert!(!tree_solid_render(block).unwrap(), "{block:?}");
            for direction in [
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, 0, 0],
                [1, 0, 0],
            ] {
                assert!(
                    !generation_face_sturdy(block, direction).unwrap(),
                    "{block:?} {direction:?}"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn tree_ground_solid_and_support_match_java() {
        use std::{path::PathBuf, process::Command};
        let names = [
            "air",
            "cave_air",
            "void_air",
            "water",
            "lava",
            "stone",
            "deepslate",
            "netherrack",
            "basalt",
            "smooth_basalt",
            "amethyst_block",
            "budding_amethyst",
            "small_amethyst_bud",
            "medium_amethyst_bud",
            "large_amethyst_bud",
            "amethyst_cluster",
            "blackstone",
            "ancient_debris",
            "dirt",
            "grass_block",
            "mud",
            "clay",
            "sand",
            "sandstone",
            "gravel",
            "snow_block",
            "podzol",
            "coarse_dirt",
            "mycelium",
            "tuff",
            "calcite",
            "granite",
            "diorite",
            "andesite",
            "bedrock",
            "red_sand",
            "moss_block",
            "magma_block",
            "oak_log",
            "birch_log",
            "oak_leaves",
            "birch_leaves",
            "dark_oak_leaves",
            "azalea_leaves",
            "flowering_azalea_leaves",
            "vine",
            "leaf_litter",
            "cave_vines",
            "cave_vines_plant",
            "short_grass",
            "fern",
            "bush",
            "dandelion",
            "poppy",
            "blue_orchid",
            "allium",
            "azure_bluet",
            "red_tulip",
            "orange_tulip",
            "white_tulip",
            "pink_tulip",
            "oxeye_daisy",
            "cornflower",
            "lily_of_the_valley",
            "firefly_bush",
            "closed_eyeblossom",
            "azalea",
            "flowering_azalea",
            "sweet_berry_bush",
            "short_dry_grass",
            "tall_dry_grass",
            "dead_bush",
            "tall_grass",
            "large_fern",
            "sunflower",
            "rose_bush",
            "peony",
            "lilac",
            "oak_sapling",
            "spruce_sapling",
            "birch_sapling",
            "jungle_sapling",
            "acacia_sapling",
            "dark_oak_sapling",
            "cherry_sapling",
            "pale_oak_sapling",
            "seagrass",
            "tall_seagrass",
            "small_dripleaf",
            "brown_mushroom",
            "red_mushroom",
            "spore_blossom",
            "moss_carpet",
            "white_carpet",
            "orange_carpet",
            "magenta_carpet",
            "light_blue_carpet",
            "yellow_carpet",
            "lime_carpet",
            "pink_carpet",
            "gray_carpet",
            "light_gray_carpet",
            "cyan_carpet",
            "purple_carpet",
            "blue_carpet",
            "brown_carpet",
            "green_carpet",
            "red_carpet",
            "black_carpet",
            "coal_ore",
            "deepslate_coal_ore",
            "iron_ore",
            "deepslate_iron_ore",
            "copper_ore",
            "deepslate_copper_ore",
            "gold_ore",
            "deepslate_gold_ore",
            "redstone_ore",
            "deepslate_redstone_ore",
            "emerald_ore",
            "deepslate_emerald_ore",
            "lapis_ore",
            "deepslate_lapis_ore",
            "diamond_ore",
            "deepslate_diamond_ore",
            "nether_gold_ore",
            "nether_quartz_ore",
        ];
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("TreeGroundSolidityOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/TreeGroundSolidityOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let result = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreeGroundSolidityOracle")
            .args(names)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        let actual: Vec<_> = text.lines().filter(|v| v.starts_with("S ")).collect();
        assert_eq!(actual.len(), names.len());
        for (line, name) in actual.iter().zip(names) {
            let mut block = state(name);
            if matches!(
                name,
                "small_amethyst_bud"
                    | "medium_amethyst_bud"
                    | "large_amethyst_bud"
                    | "amethyst_cluster"
            ) {
                block.properties = BTreeMap::from([
                    ("facing".into(), "up".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            if name == "oak_log" || name == "birch_log" {
                block.properties.insert("axis".into(), "y".into());
            }
            if name.ends_with("_leaves") {
                block.properties = BTreeMap::from([
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "false".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            if name == "leaf_litter" {
                block.properties = BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("segment_amount".into(), "1".into()),
                ]);
            }
            if name == "cave_vines" {
                block.properties = BTreeMap::from([
                    ("age".into(), "0".into()),
                    ("berries".into(), "false".into()),
                ]);
            }
            if name == "cave_vines_plant" {
                block.properties.insert("berries".into(), "false".into());
            }
            if name.ends_with("_sapling") {
                block.properties.insert("stage".into(), "0".into());
            }
            if matches!(
                name,
                "tall_grass"
                    | "large_fern"
                    | "sunflower"
                    | "rose_bush"
                    | "peony"
                    | "lilac"
                    | "tall_seagrass"
                    | "small_dripleaf"
            ) {
                block.properties.insert("half".into(), "lower".into());
            }
            if name == "small_dripleaf" {
                block.properties.insert("facing".into(), "north".into());
                block
                    .properties
                    .insert("waterlogged".into(), "false".into());
            }
            if name == "sweet_berry_bush" {
                block.properties.insert("age".into(), "0".into());
            }
            assert_eq!(
                *line,
                format!(
                    "S {name} {} {}",
                    tree_solid_render(&block).unwrap(),
                    tree_sturdy_up(&block).unwrap()
                )
            );
            if matches!(
                name,
                "smooth_basalt" | "amethyst_block" | "budding_amethyst"
            ) {
                let expected = text
                    .lines()
                    .find(|line| line.starts_with(&format!("F {name} ")))
                    .unwrap();
                let directions = [
                    [0, -1, 0],
                    [0, 1, 0],
                    [0, 0, -1],
                    [0, 0, 1],
                    [-1, 0, 0],
                    [1, 0, 0],
                ];
                for (direction, expected) in directions
                    .into_iter()
                    .zip(expected.split_whitespace().skip(2))
                {
                    assert_eq!(
                        generation_face_sturdy(&block, direction).unwrap(),
                        expected.parse::<bool>().unwrap(),
                        "{name} {direction:?}"
                    );
                }
            }
            if let Some(plant) = text
                .lines()
                .find(|line| line.starts_with(&format!("P {name} ")))
            {
                let expected: Vec<_> = plant.split_whitespace().skip(2).collect();
                assert_eq!(expected.len(), 6, "{plant}");
                for (direction, value) in [
                    [0, -1, 0],
                    [0, 1, 0],
                    [0, 0, -1],
                    [0, 0, 1],
                    [-1, 0, 0],
                    [1, 0, 0],
                ]
                .into_iter()
                .zip(expected)
                {
                    assert_eq!(
                        generation_face_sturdy(&block, direction).unwrap(),
                        value.parse::<bool>().unwrap(),
                        "{plant} {direction:?}"
                    );
                }
            }
        }
        let mut crystals = 0;
        for line in text.lines().filter_map(|line| line.strip_prefix("A ")) {
            let parts: Vec<_> = line.split_whitespace().collect();
            let mut block = state(parts[0]);
            for property in parts[1].split(',') {
                let (key, value) = property.split_once('=').unwrap();
                block.properties.insert(key.into(), value.into());
            }
            assert_eq!(
                tree_solid_render(&block).unwrap(),
                parts[2].parse::<bool>().unwrap(),
                "{line}"
            );
            assert!(parts[3].parse::<bool>().unwrap(), "{line}");
            assert!(!parts[4].parse::<bool>().unwrap(), "{line}");
            for (direction, expected) in [
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, 0, 0],
                [1, 0, 0],
            ]
            .into_iter()
            .zip(parts[5..].iter())
            {
                assert_eq!(
                    generation_face_sturdy(&block, direction).unwrap(),
                    expected.parse::<bool>().unwrap(),
                    "{line}"
                );
            }
            crystals += 1;
        }
        assert_eq!(crystals, 48);
        for name in [
            "oak_leaves",
            "birch_leaves",
            "dark_oak_leaves",
            "azalea_leaves",
            "flowering_azalea_leaves",
        ] {
            assert!(
                text.lines().any(|line| line == format!("L {name} true")),
                "leaf solid state {name}"
            );
        }
        let variants: Vec<_> = text
            .lines()
            .filter_map(|line| line.strip_prefix("V "))
            .collect();
        assert_eq!(variants.len(), 54);
        for line in variants {
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 10, "{line}");
            let mut block = state(fields[0]);
            if fields[1] != "-1" {
                block.properties.insert("age".into(), fields[1].into());
            }
            block.properties.insert("berries".into(), fields[2].into());
            assert_eq!(
                tree_solid_render(&block).unwrap(),
                fields[3].parse::<bool>().unwrap(),
                "{line}"
            );
            for face in &fields[4..] {
                assert_eq!(
                    tree_sturdy_up(&block).unwrap(),
                    face.parse::<bool>().unwrap(),
                    "{line}"
                );
            }
        }
        let mut invalid = state("cave_vines");
        invalid.properties.insert("berries".into(), "true".into());
        invalid.properties.insert("age".into(), "26".into());
        assert!(tree_sturdy_up(&invalid).is_err());
        let ores: Vec<_> = text
            .lines()
            .filter_map(|line| line.strip_prefix("R "))
            .collect();
        assert_eq!(ores.len(), 20);
        for line in ores {
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 9, "{line}");
            let mut block = state(fields[0]);
            if fields[1] != "-" {
                block.properties.insert("lit".into(), fields[1].into());
            }
            assert_eq!(
                tree_solid_render(&block).unwrap(),
                fields[2].parse::<bool>().unwrap(),
                "{line}"
            );
            for face in &fields[3..] {
                assert_eq!(
                    tree_sturdy_up(&block).unwrap(),
                    face.parse::<bool>().unwrap(),
                    "{line}"
                );
            }
        }
    }
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_dripleaf_support_matches_java_states() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("GeneratedDripleafSupportOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GeneratedDripleafSupportOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let result = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GeneratedDripleafSupportOracle")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout)
        );
        let output = String::from_utf8(result.stdout).unwrap();
        let mut counts = BTreeMap::<String, usize>::new();
        for line in output.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 9, "{line}");
            let mut block = state(fields[0]);
            for property in fields[1].split(',') {
                let (name, value) = property.split_once('=').unwrap();
                block.properties.insert(name.into(), value.into());
            }
            assert_eq!(
                tree_solid_render(&block).unwrap(),
                fields[2].parse::<bool>().unwrap(),
                "{line}"
            );
            for (direction, expected) in [
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, 0, 0],
                [1, 0, 0],
            ]
            .into_iter()
            .zip(&fields[3..])
            {
                assert_eq!(
                    generation_face_sturdy(&block, direction).unwrap(),
                    expected.parse::<bool>().unwrap(),
                    "{line} {direction:?}"
                );
            }
            assert_eq!(
                tree_sturdy_up(&block).unwrap(),
                fields[4].parse::<bool>().unwrap(),
                "{line}"
            );
            *counts.entry(fields[0].to_owned()).or_default() += 1;
        }
        assert_eq!(
            counts,
            BTreeMap::from([
                ("big_dripleaf".into(), 32),
                ("big_dripleaf_stem".into(), 8),
                ("small_dripleaf".into(), 16)
            ])
        );
        let mut invalid = state("big_dripleaf");
        invalid.properties = BTreeMap::from([
            ("facing".into(), "north".into()),
            ("waterlogged".into(), "false".into()),
            ("tilt".into(), "invalid".into()),
        ]);
        assert!(generation_face_sturdy(&invalid, [0, 1, 0]).is_err());
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
            "minecraft:torch",
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
                        status: Some("minecraft:terrain".into()),
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
            has_published_light: false,
            ore_ocean_floor_heights: BTreeMap::new(),
            feature_world_surface_heights: BTreeMap::new(),
            final_heightmaps: BTreeMap::new(),
            postprocessing: BTreeMap::new(),
            noise_postprocessing: BTreeMap::new(),
            surface_postprocessing: BTreeMap::new(),
            carver_postprocessing: BTreeMap::new(),
            feature_postprocessing: BTreeMap::new(),
            generated_bee_nests: Arc::new(BTreeMap::new()),
            generated_monster_entities: Arc::new(BTreeMap::new()),
        };
        (result, tags, motion, feature)
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn inactive_sky_check_matches_java_queue() {
        use native_world::generation_light::{VisibleLightColumn, VisibleLightSnapshot};
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        for mode in [
            "sky_inactive_shadow",
            "sky_inactive_dark",
            "sky_inactive_many",
            "sky_inactive_activate",
            "sky_inactive_activate_dark",
            "sky_inactive_activate_many",
            "sky_inactive_activate_tall",
        ] {
            let path = temp.path().join("sky.json");
            let order_path = temp.path().join("sky-order.json");
            let output = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&path)
                .arg(mode)
                .arg(&order_path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let expected: Vec<Vec<Vec<Vec<u8>>>> =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let positions: Vec<[i32; 3]> =
                serde_json::from_slice(&std::fs::read(order_path).unwrap()).unwrap();
            let expected_positions = if mode.ends_with("_many") {
                BTreeSet::from([[16, 10, 8], [16, 10, 9], [17, 10, 8]])
            } else {
                BTreeSet::from([[16, 10, 8]])
            };
            let inserted = if mode.ends_with("_many") {
                vec![[16, 10, 8], [16, 10, 9], [17, 10, 8], [16, 10, 8]]
            } else {
                vec![[16, 10, 8]]
            };
            assert_eq!(
                native_world::generation_light::order_pending_light_checks(
                    inserted.iter().copied()
                )
                .unwrap(),
                positions,
                "Java pending-node iteration order"
            );
            assert_eq!(positions.len(), expected_positions.len());
            assert_eq!(
                positions.iter().copied().collect::<BTreeSet<_>>(),
                expected_positions
            );
            let (mut terrain, _, _, _) = fixture();
            let east = ChunkPos { x: 1, z: 0 };
            let center = ChunkPos { x: 0, z: 0 };
            let world = Arc::make_mut(&mut terrain.world);
            if mode.starts_with("sky_inactive_activate") {
                let template = Arc::clone(world.chunks.get(&center).unwrap());
                for z in -2..=2 {
                    for x in -2..=2 {
                        let pos = ChunkPos { x, z };
                        world.chunks.entry(pos).or_insert_with(|| {
                            let mut chunk = (*template).clone();
                            chunk.position = pos;
                            Arc::new(chunk)
                        });
                    }
                }
            }
            let air = world.intern_state(state("air"));
            if mode.ends_with("_tall") {
                for chunk in world.chunks.values_mut() {
                    let chunk = Arc::make_mut(chunk);
                    for sy in 1..4 {
                        chunk.sections.insert(
                            sy,
                            Arc::new(Section {
                                blocks: Some(Box::new([air; BLOCK_COUNT])),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        );
                    }
                }
            }
            let section = Arc::make_mut(
                Arc::make_mut(world.chunks.get_mut(&east).unwrap())
                    .sections
                    .get_mut(&0)
                    .unwrap(),
            );
            section.blocks.as_mut().unwrap().fill(air);
            let stone = world.intern_state(state("stone"));
            if !mode.ends_with("_dark") {
                world.set_block_id(16, 11, 8, stone).unwrap();
                if mode.ends_with("_many") {
                    world.set_block_id(16, 11, 9, stone).unwrap();
                    world.set_block_id(17, 11, 8, stone).unwrap();
                }
            } else {
                world.set_block_id(31, 0, 0, stone).unwrap();
            }
            let mut log = state("dark_oak_log");
            log.properties.insert("axis".into(), "y".into());
            let log = world.intern_state(log);
            for [x, y, z] in &positions {
                world.set_block_id(*x, *y, *z, log).unwrap();
            }

            let mut before = VisibleLightSnapshot::new(false, true);
            for z in -2..=2 {
                for x in -2..=2 {
                    let index = ((z + 2) * 5 + x + 2) as usize;
                    let mut layers = BTreeMap::new();
                    for sy in -1..=1 {
                        let mut layer = [0u8; native_world::loaded::LIGHT_BYTES];
                        for i in 0..4096usize {
                            let value = expected[0][index][1][((sy + 1) as usize) * 4096 + i];
                            layer[i / 2] |= value << ((i % 2) * 4);
                        }
                        layers.insert(sy, Arc::new(layer));
                    }
                    before.insert_complete_column(
                        ChunkPos { x, z },
                        VisibleLightColumn::new(BTreeMap::new(), layers),
                    );
                }
            }
            assert_eq!(
                before.sky_light([16, 10, 8]).unwrap() > 0,
                !mode.ends_with("_dark"),
                "{mode}"
            );
            let bounds = native_world::block_light::LightBounds::new(
                0,
                if mode.ends_with("_tall") { 64 } else { 16 },
            )
            .unwrap();
            assert!(
                native_world::generation_sky_light::check_inactive_sky_block(
                    world,
                    bounds,
                    &before,
                    &BTreeSet::from([center, east]),
                    [16, 10, 8],
                )
                .is_none()
            );
            assert!(
                native_world::generation_sky_light::check_inactive_sky_block(
                    world,
                    bounds,
                    &VisibleLightSnapshot::new(false, true),
                    &BTreeSet::from([center]),
                    [16, 10, 8],
                )
                .is_none()
            );
            let after = native_world::generation_sky_light::check_inactive_sky_blocks(
                world,
                bounds,
                &before,
                &BTreeSet::from([center]),
                &inserted,
            )
            .expect("complete light inputs");
            let activated = if mode.starts_with("sky_inactive_activate") {
                let mut sources = BTreeMap::new();
                for (x, z) in [(1, 0), (0, 0), (2, 0), (1, -1), (1, 1)] {
                    let pos = ChunkPos { x, z };
                    sources.insert(
                        pos,
                        Arc::from(
                            native_world::sky_light::generated_chunk_sky_sources(
                                world, bounds, pos,
                            )
                            .unwrap(),
                        ),
                    );
                }
                assert!(
                    native_world::generation_sky_light::activate_generated_sky_light(
                        world,
                        bounds,
                        &after,
                        east,
                        &BTreeMap::new()
                    )
                    .is_none()
                );
                Some(
                    native_world::generation_sky_light::activate_generated_sky_light(
                        world, bounds, &after, east, &sources,
                    )
                    .expect("complete sky activation inputs"),
                )
            } else {
                None
            };
            for z in -2..=2 {
                for x in -2..=2 {
                    let index = ((z + 2) * 5 + x + 2) as usize;
                    for y in -16..32 {
                        for lz in 0..16 {
                            for lx in 0..16 {
                                let i = ((y + 16) * 256 + lz * 16 + lx) as usize;
                                let p = [x * 16 + lx, y, z * 16 + lz];
                                assert_eq!(
                                    before.sky_light(p),
                                    Some(expected[0][index][1][i]),
                                    "before {mode} {p:?}"
                                );
                                assert_eq!(
                                    expected[0][index][1][i], expected[1][index][1][i],
                                    "pending {mode} {p:?}"
                                );
                                assert_eq!(
                                    after.sky_light(p),
                                    Some(expected[2][index][1][i]),
                                    "after {mode} {p:?}"
                                );
                                if let Some(activated) = &activated {
                                    assert_eq!(
                                        after.sky_light(p),
                                        Some(expected[3][index][1][i]),
                                        "pending activation {mode} {p:?}"
                                    );
                                    assert_eq!(
                                        activated.sky_light(p),
                                        Some(expected[4][index][1][i]),
                                        "activation {mode} {p:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_block_light_checks_match_java_queue() {
        use native_world::generation_light::{VisibleLightColumn, VisibleLightSnapshot};
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        for mode in [
            "block_incremental_remove",
            "block_incremental_obstruct",
            "block_incremental_add_active",
            "block_incremental_add_inactive",
            "block_incremental_many",
            "block_incremental_activate",
            "block_incremental_activate_many",
        ] {
            let path = temp.path().join("block.json");
            let output = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&path)
                .arg(mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let expected: Vec<Vec<Vec<Vec<u8>>>> =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            let template = Arc::clone(world.chunks.get(&ChunkPos { x: 0, z: 0 }).unwrap());
            for z in -2..=2 {
                for x in -2..=2 {
                    let pos = ChunkPos { x, z };
                    world.chunks.entry(pos).or_insert_with(|| {
                        let mut chunk = (*template).clone();
                        chunk.position = pos;
                        Arc::new(chunk)
                    });
                }
            }
            let torch = world.intern_state(state("torch"));
            let air = world.intern_state(state("air"));
            let stone = world.intern_state(state("stone"));
            world.set_block_id(12, 10, 8, torch).unwrap();
            world.set_block_id(14, 10, 8, torch).unwrap();
            let positions: Vec<[i32; 3]> = match mode {
                "block_incremental_remove" => vec![[14, 10, 8]],
                "block_incremental_obstruct" => vec![[15, 10, 8]],
                "block_incremental_add_active" => vec![[13, 10, 9]],
                "block_incremental_add_inactive" => vec![[16, 10, 8]],
                "block_incremental_activate" => vec![[16, 10, 8]],
                "block_incremental_activate_many" => vec![[16, 10, 8], [17, 10, 8], [30, 10, 4]],
                _ => vec![[14, 10, 8], [15, 10, 8], [16, 10, 8], [14, 10, 8]],
            };
            for &[x, y, z] in &positions {
                let id = if x == 14 && z == 8 {
                    air
                } else if x == 15 && z == 8 {
                    stone
                } else {
                    torch
                };
                world.set_block_id(x, y, z, id).unwrap();
            }
            let mut before = VisibleLightSnapshot::new(true, false);
            for z in -2..=2 {
                for x in -2..=2 {
                    let index = ((z + 2) * 5 + x + 2) as usize;
                    let mut layers = BTreeMap::new();
                    for sy in -1..=1 {
                        let mut layer = [0u8; native_world::loaded::LIGHT_BYTES];
                        for i in 0..4096usize {
                            let value = expected[0][index][0][((sy + 1) as usize) * 4096 + i];
                            layer[i / 2] |= value << ((i % 2) * 4);
                        }
                        layers.insert(sy, Arc::new(layer));
                    }
                    before.insert_complete_column(
                        ChunkPos { x, z },
                        VisibleLightColumn::new(layers, BTreeMap::new()),
                    );
                }
            }
            let bounds = native_world::block_light::LightBounds::new(0, 16).unwrap();
            let active = BTreeSet::from([ChunkPos { x: 0, z: 0 }]);
            assert!(
                native_world::generation_block_light::check_generated_block_light(
                    world,
                    bounds,
                    &VisibleLightSnapshot::new(true, false),
                    &active,
                    &positions,
                )
                .is_none()
            );
            let after = native_world::generation_block_light::check_generated_block_light(
                world, bounds, &before, &active, &positions,
            )
            .expect("complete block-light inputs");
            let activated = if mode.starts_with("block_incremental_activate") {
                assert!(
                    native_world::generation_block_light::activate_generated_block_light(
                        world,
                        bounds,
                        &VisibleLightSnapshot::new(true, false),
                        ChunkPos { x: 1, z: 0 },
                    )
                    .is_none(),
                    "missing activation storage must be rejected"
                );
                Some(
                    native_world::generation_block_light::activate_generated_block_light(
                        world,
                        bounds,
                        &after,
                        ChunkPos { x: 1, z: 0 },
                    )
                    .expect("complete activation inputs"),
                )
            } else {
                None
            };
            let mut changed = 0usize;
            for z in -2..=2 {
                for x in -2..=2 {
                    let index = ((z + 2) * 5 + x + 2) as usize;
                    for y in -16..32 {
                        for lz in 0..16 {
                            for lx in 0..16 {
                                let i = ((y + 16) * 256 + lz * 16 + lx) as usize;
                                let p = [x * 16 + lx, y, z * 16 + lz];
                                assert_eq!(
                                    before.block_light(p),
                                    Some(expected[0][index][0][i]),
                                    "before {mode} {p:?}"
                                );
                                assert_eq!(
                                    expected[0][index][0][i], expected[1][index][0][i],
                                    "pending {mode} {p:?}"
                                );
                                assert_eq!(
                                    after.block_light(p),
                                    Some(expected[2][index][0][i]),
                                    "after {mode} {p:?}"
                                );
                                if let Some(activated) = &activated {
                                    assert_eq!(
                                        after.block_light(p),
                                        Some(expected[3][index][0][i]),
                                        "pending activation {mode} {p:?}"
                                    );
                                    assert_eq!(
                                        activated.block_light(p),
                                        Some(expected[4][index][0][i]),
                                        "activation {mode} {p:?}"
                                    );
                                    assert_eq!(
                                        after.block_light(p),
                                        Some(expected[2][index][0][i]),
                                        "activation must preserve previous snapshot {mode} {p:?}"
                                    );
                                }
                                changed += usize::from(
                                    expected[0][index][0][i] != expected[2][index][0][i],
                                );
                            }
                        }
                    }
                }
            }
            assert_eq!(
                changed == 0,
                mode == "block_incremental_add_inactive"
                    || mode.starts_with("block_incremental_activate"),
                "{mode}"
            );
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn empty_section_tree_light_updates_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        for mode in [
            "tree_empty",
            "tree_empty_full",
            "tree_empty_leaf_active",
            "tree_empty_no_sky",
            "tree_empty_vine_no_sky",
        ] {
            let path = temp.path().join("empty-tree.json");
            let output = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&path)
                .arg(mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let expected: Vec<Vec<Vec<u8>>> =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let (mut terrain, tags, _, _) = fixture();
            let block_names = registry.ids().map(str::to_owned).collect();
            let motion = HeightmapTags::load(&mut Data, &block_names).unwrap();
            let center = ChunkPos { x: 0, z: 0 };
            let east = ChunkPos { x: 1, z: 0 };
            let world = Arc::make_mut(&mut terrain.world);
            // Replay covers the edited chunk's two-chunk halo, whose active
            // neighbor calculations may read one further chunk.
            let template = Arc::clone(world.chunks.get(&center).unwrap());
            for z in -3..=3 {
                for x in -3..=3 {
                    let pos = ChunkPos { x, z };
                    world.chunks.entry(pos).or_insert_with(|| {
                        let mut chunk = (*template).clone();
                        chunk.position = pos;
                        Arc::new(chunk)
                    });
                }
            }
            let air = world.intern_state(state("air"));
            let chunk = Arc::make_mut(world.chunks.get_mut(&east).unwrap());
            Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
                .blocks
                .as_mut()
                .unwrap()
                .fill(air);
            let registered: Vec<_> = if mode == "tree_empty_full" {
                (-1..=1)
                    .flat_map(|x| (-1..=1).map(move |z| ChunkPos { x, z }))
                    .collect()
            } else {
                vec![east]
            };
            terrain
                .completed_decoration
                .extend(registered.iter().copied());
            let active = if mode == "tree_empty_full" {
                center
            } else {
                east
            };
            let mut tracker =
                FreshGenerationLightTracker::new(&terrain, !mode.ends_with("no_sky")).unwrap();
            for &pos in &registered {
                tracker.initialize_chunk(&terrain, pos, &registry).unwrap();
            }
            let mut engine = StagedGenerationLightEngine::new(tracker);
            let before = engine
                .activate(&terrain, active, [center], &registry)
                .unwrap();
            // Java retains its existing light engine and drains the section
            // and block updates. Rebuilding a fresh engine loses that history.
            let mut draft = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([east]))
                .unwrap();
            draft.light_guard = Some(DecorationLightGuard {
                registered: BTreeSet::from([east]),
                registry: &registry,
                require_unchanged_light: true,
                changed_blocks: BTreeSet::new(),
                changed_chunks: BTreeSet::new(),
                light_replay_positions: BTreeSet::new(),
                light_check_order: Vec::new(),
                vine_light_positions: BTreeSet::new(),
                new_nonempty_sections: Vec::new(),
                non_air_counts: BTreeMap::new(),
            });
            if mode == "tree_empty_vine_no_sky" {
                let mut vine = state("cave_vines_plant");
                vine.properties.insert("berries".into(), "true".into());
                assert!(draft.set_column_block([16, 10, 8], &vine).unwrap());
            } else if mode == "tree_empty_leaf_active" {
                let mut leaf = state("dark_oak_leaves");
                leaf.properties = BTreeMap::from([
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "false".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
                draft.set_tree_block([16, 10, 8], &leaf, 19).unwrap();
            } else {
                let mut log = state("dark_oak_log");
                log.properties.insert("axis".into(), "y".into());
                draft.set_tree_block([16, 10, 8], &log, 19).unwrap();
            }
            let mut tracking = Some(DecorationLightTracking::Staged(&mut engine, &registry));
            let result = finish_decoration_with_light(draft, &mut tracking);
            drop(tracking);
            result.unwrap();
            let after = engine.snapshot(&terrain, [center], &registry).unwrap();
            if mode == "tree_empty_vine_no_sky" {
                assert_eq!(after.block_light([15, 10, 8]), Some(13));
            }
            for (stage, actual) in [&before, &before, &after].into_iter().enumerate() {
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [x, y, z];
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                actual.block_light(p),
                                Some(expected[stage][0][index]),
                                "block {mode} {stage} {p:?}"
                            );
                            assert_eq!(
                                actual.sky_light(p),
                                Some(expected[stage][1][index]),
                                "sky {mode} {stage} {p:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn tracked_tree_light_updates_match_java_or_roll_back() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let path = temp.path().join("leaf.json");
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        for mode in [
            "tree_fresh_log",
            "tree_fresh_leaf",
            "tree_fresh_birch_log",
            "tree_fresh_birch_leaf",
            "tree_fresh_empty",
            "patch_fresh_clay_water",
            "column_fresh_water_dripleaf",
            "patch_fresh_waterlog_dripleaf",
            "patch_fresh_dripleaf_sequence",
            "log_active_sparse_column",
            "leaf_active",
            "leaf_active_full",
            "leaf_active_many",
            "log_active_full",
            "log_active_sparse",
            "log_active_sparse_twice",
            "log_active_sparse_missing",
        ] {
            let output = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&path)
                .arg(mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let expected: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let (mut terrain, tags, _, _) = fixture();
            let block_names: BTreeSet<_> = registry.ids().map(str::to_owned).collect();
            let motion = HeightmapTags::load(&mut Data, &block_names).unwrap();
            let center = ChunkPos { x: 0, z: 0 };
            let east = ChunkPos { x: 1, z: 0 };
            let world = Arc::make_mut(&mut terrain.world);
            let template = Arc::clone(world.chunks.get(&center).unwrap());
            for z in -2..=2 {
                for x in -2..=2 {
                    let pos = ChunkPos { x, z };
                    world.chunks.entry(pos).or_insert_with(|| {
                        let mut chunk = (*template).clone();
                        chunk.position = pos;
                        Arc::new(chunk)
                    });
                }
            }
            if mode == "log_active_sparse_missing" {
                world.chunks.remove(&ChunkPos { x: -2, z: 0 });
            }
            if mode == "tree_fresh_empty" {
                let air = world.intern_state(state("air"));
                let section = Arc::make_mut(
                    Arc::make_mut(world.chunks.get_mut(&east).unwrap())
                        .sections
                        .get_mut(&0)
                        .unwrap(),
                );
                section.blocks.as_mut().unwrap().fill(air);
            } else if mode.starts_with("log_active_sparse") {
                let air = world.intern_state(state("air"));
                let section = Arc::make_mut(
                    Arc::make_mut(world.chunks.get_mut(&east).unwrap())
                        .sections
                        .get_mut(&0)
                        .unwrap(),
                );
                section.blocks.as_mut().unwrap().fill(air);
                let stone = world.intern_state(state("stone"));
                let (x, z) = if mode == "log_active_sparse_column" {
                    (16, 8)
                } else {
                    (31, 0)
                };
                world.set_block_id(x, 0, z, stone).unwrap();
            } else {
                let torch = world.intern_state(state("torch"));
                world.set_block_id(17, 10, 8, torch).unwrap();
            }
            if mode == "patch_fresh_clay_water" {
                let clay = world.intern_state(state("clay"));
                world.set_block_id(16, 10, 8, clay).unwrap();
            }
            if mode == "column_fresh_water_dripleaf" || mode == "patch_fresh_dripleaf_sequence" {
                let mut water = state("water");
                water.properties.insert("level".into(), "0".into());
                let water = world.intern_state(water);
                world.set_block_id(16, 10, 8, water).unwrap();
            }
            if mode == "patch_fresh_waterlog_dripleaf" {
                let mut dripleaf = state("big_dripleaf");
                dripleaf.properties = BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("tilt".into(), "none".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
                let dripleaf = world.intern_state(dripleaf);
                world.set_block_id(16, 10, 8, dripleaf).unwrap();
            }
            let registered: Vec<_> = if mode != "leaf_active" {
                (-1..=1)
                    .flat_map(|z| (-1..=1).map(move |x| ChunkPos { x, z }))
                    .collect()
            } else {
                vec![east]
            };
            terrain
                .completed_decoration
                .extend(registered.iter().copied());
            if mode == "leaf_active" {
                let mut leaf = state("dark_oak_leaves");
                leaf.properties = BTreeMap::from([
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "false".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
                for tree_write in [false, true] {
                    let mut draft = terrain
                        .decoration_region(&tags, &motion, BTreeSet::from([east]))
                        .unwrap();
                    draft.light_guard = Some(DecorationLightGuard {
                        registered: BTreeSet::from([east]),
                        registry: &registry,
                        require_unchanged_light: true,
                        changed_blocks: BTreeSet::new(),
                        changed_chunks: BTreeSet::new(),
                        light_replay_positions: BTreeSet::new(),
                        light_check_order: Vec::new(),
                        vine_light_positions: BTreeSet::new(),
                        new_nonempty_sections: Vec::new(),
                        non_air_counts: BTreeMap::new(),
                    });
                    if tree_write {
                        draft.set_tree_block([16, 10, 8], &leaf, 19).unwrap();
                        assert!(
                            draft
                                .light_guard
                                .as_ref()
                                .unwrap()
                                .light_replay_positions
                                .contains(&[16, 10, 8])
                        );
                    } else {
                        assert!(draft.set_section_block([16, 10, 8], &leaf).is_err());
                    }
                }
            }
            let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
            for &pos in &registered {
                tracker.initialize_chunk(&terrain, pos, &registry).unwrap();
            }
            if mode.starts_with("tree_fresh_")
                || mode == "patch_fresh_clay_water"
                || mode == "column_fresh_water_dripleaf"
                || mode == "patch_fresh_waterlog_dripleaf"
                || mode == "patch_fresh_dripleaf_sequence"
            {
                let expected: Vec<Vec<Vec<Vec<u8>>>> = serde_json::from_value(expected).unwrap();
                let before = tracker
                    .snapshot(&terrain, [center, east], &registry)
                    .unwrap();
                let mut block = state(
                    if mode == "column_fresh_water_dripleaf"
                        || mode == "patch_fresh_waterlog_dripleaf"
                        || mode == "patch_fresh_dripleaf_sequence"
                    {
                        "big_dripleaf"
                    } else if mode == "patch_fresh_clay_water" {
                        "water"
                    } else if mode == "tree_fresh_birch_log" {
                        "birch_log"
                    } else if mode == "tree_fresh_birch_leaf" {
                        "birch_leaves"
                    } else if mode.ends_with("log") || mode.ends_with("empty") {
                        "dark_oak_log"
                    } else {
                        "dark_oak_leaves"
                    },
                );
                block.properties = if mode == "column_fresh_water_dripleaf"
                    || mode == "patch_fresh_waterlog_dripleaf"
                    || mode == "patch_fresh_dripleaf_sequence"
                {
                    BTreeMap::from([
                        ("facing".into(), "north".into()),
                        ("tilt".into(), "none".into()),
                        (
                            "waterlogged".into(),
                            (mode == "patch_fresh_waterlog_dripleaf").to_string(),
                        ),
                    ])
                } else if mode == "patch_fresh_clay_water" {
                    BTreeMap::from([("level".into(), "0".into())])
                } else if mode.ends_with("log") || mode.ends_with("empty") {
                    BTreeMap::from([("axis".into(), "y".into())])
                } else {
                    BTreeMap::from([
                        ("distance".into(), "7".into()),
                        ("persistent".into(), "false".into()),
                        ("waterlogged".into(), "false".into()),
                    ])
                };
                let mut draft = terrain
                    .decoration_region(&tags, &motion, BTreeSet::from([east]))
                    .unwrap();
                draft.light_guard = Some(DecorationLightGuard {
                    registered: BTreeSet::from([east]),
                    registry: &registry,
                    require_unchanged_light: false,
                    changed_blocks: BTreeSet::new(),
                    changed_chunks: BTreeSet::new(),
                    light_replay_positions: BTreeSet::new(),
                    light_check_order: Vec::new(),
                    vine_light_positions: BTreeSet::new(),
                    new_nonempty_sections: Vec::new(),
                    non_air_counts: BTreeMap::new(),
                });
                if mode == "column_fresh_water_dripleaf" || mode == "patch_fresh_dripleaf_sequence"
                {
                    assert!(draft.set_column_block([16, 10, 8], &block).unwrap());
                    if mode == "patch_fresh_dripleaf_sequence" {
                        block.properties.insert("waterlogged".into(), "true".into());
                        draft.set_patch_block([16, 10, 8], &block).unwrap();
                    }
                } else if mode == "patch_fresh_clay_water"
                    || mode == "patch_fresh_waterlog_dripleaf"
                {
                    draft.set_patch_block([16, 10, 8], &block).unwrap();
                } else {
                    draft.set_tree_block([16, 10, 8], &block, 19).unwrap();
                }
                let mut tracking = Some(DecorationLightTracking::Fresh(&mut tracker, &registry));
                finish_decoration_with_light(draft, &mut tracking).unwrap();
                drop(tracking);
                let after = tracker
                    .snapshot(&terrain, [center, east], &registry)
                    .unwrap();
                let mut engine = StagedGenerationLightEngine::new(tracker);
                let active = engine
                    .activate(&terrain, center, [center, east], &registry)
                    .unwrap();
                for (stage, snapshot) in [&before, &after, &active].into_iter().enumerate() {
                    for (index, chunk) in [center, east].into_iter().enumerate() {
                        for y in -16..32 {
                            for z in 0..16 {
                                for x in 0..16 {
                                    let pos = [chunk.x * 16 + x, y, z];
                                    let i = ((y + 16) * 256 + z * 16 + x) as usize;
                                    assert_eq!(
                                        snapshot.block_light(pos),
                                        Some(expected[stage][index][0][i]),
                                        "{mode} block {stage} {pos:?}"
                                    );
                                    assert_eq!(
                                        snapshot.sky_light(pos),
                                        Some(expected[stage][index][1][i]),
                                        "{mode} sky {stage} {pos:?}"
                                    );
                                }
                            }
                        }
                    }
                }
                continue;
            }
            let expected: Vec<Vec<Vec<u8>>> = serde_json::from_value(expected).unwrap();
            let mut engine = StagedGenerationLightEngine::new(tracker);
            let active = if mode != "leaf_active" { center } else { east };
            let before = engine
                .activate(&terrain, active, [center], &registry)
                .unwrap();
            let is_log = mode.starts_with("log_active_");
            let mut leaf = state(if is_log {
                "dark_oak_log"
            } else {
                "dark_oak_leaves"
            });
            leaf.properties = if is_log {
                BTreeMap::from([("axis".into(), "y".into())])
            } else {
                BTreeMap::from([
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "false".into()),
                    ("waterlogged".into(), "false".into()),
                ])
            };
            let previous_world = Arc::clone(&terrain.world);
            let mut draft = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([east]))
                .unwrap();
            draft.light_guard = Some(DecorationLightGuard {
                registered: BTreeSet::from([east]),
                registry: &registry,
                require_unchanged_light: true,
                changed_blocks: BTreeSet::new(),
                changed_chunks: BTreeSet::new(),
                light_replay_positions: BTreeSet::new(),
                light_check_order: Vec::new(),
                vine_light_positions: BTreeSet::new(),
                new_nonempty_sections: Vec::new(),
                non_air_counts: BTreeMap::new(),
            });
            draft.set_tree_block([16, 10, 8], &leaf, 19).unwrap();
            if mode == "leaf_active_many" {
                for [x, y, z] in [[17, 11, 8], [31, 10, 4], [16, 9, 10]] {
                    draft.set_tree_block([x, y, z], &leaf, 19).unwrap();
                }
                assert_eq!(
                    draft.light_guard.as_ref().unwrap().light_check_order,
                    vec![[16, 10, 8], [17, 11, 8], [31, 10, 4], [16, 9, 10]]
                );
            }
            let mut tracking = Some(DecorationLightTracking::Staged(&mut engine, &registry));
            let result = finish_decoration_with_light(draft, &mut tracking);
            drop(tracking);
            if mode == "log_active_sparse_missing" {
                assert!(result.is_err());
                assert!(Arc::ptr_eq(&previous_world, &terrain.world));
                assert!(engine.incremental_columns.is_empty());
                let unchanged = engine.snapshot(&terrain, [center], &registry).unwrap();
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [x, y, z];
                            assert_eq!(unchanged.block_light(p), before.block_light(p));
                            assert_eq!(unchanged.sky_light(p), before.sky_light(p));
                        }
                    }
                }
                continue;
            }
            result.unwrap();
            let after = engine.snapshot(&terrain, [center], &registry).unwrap();
            for (stage, actual) in [&before, &before, &after].into_iter().enumerate() {
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [x, y, z];
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                actual.block_light(p),
                                Some(expected[stage][0][index]),
                                "{mode} block {stage} {p:?}"
                            );
                            assert_eq!(
                                actual.sky_light(p),
                                Some(expected[stage][1][index]),
                                "{mode} sky {stage} {p:?}"
                            );
                        }
                    }
                }
            }
            if mode == "log_active_sparse_twice" {
                let mut draft = terrain
                    .decoration_region(&tags, &motion, BTreeSet::from([east]))
                    .unwrap();
                draft.light_guard = Some(DecorationLightGuard {
                    registered: BTreeSet::from([east]),
                    registry: &registry,
                    require_unchanged_light: true,
                    changed_blocks: BTreeSet::new(),
                    changed_chunks: BTreeSet::new(),
                    light_replay_positions: BTreeSet::new(),
                    light_check_order: Vec::new(),
                    vine_light_positions: BTreeSet::new(),
                    new_nonempty_sections: Vec::new(),
                    non_air_counts: BTreeMap::new(),
                });
                draft.set_tree_block([16, 10, 9], &leaf, 19).unwrap();
                let mut tracking = Some(DecorationLightTracking::Staged(&mut engine, &registry));
                finish_decoration_with_light(draft, &mut tracking).unwrap();
                drop(tracking);
                let after_second = engine.snapshot(&terrain, [center], &registry).unwrap();
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [x, y, z];
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                after.block_light(p),
                                Some(expected[3][0][index]),
                                "queued block {p:?}"
                            );
                            assert_eq!(
                                after.sky_light(p),
                                Some(expected[3][1][index]),
                                "queued sky {p:?}"
                            );
                            assert_eq!(
                                after_second.block_light(p),
                                Some(expected[4][0][index]),
                                "second block {p:?}"
                            );
                            assert_eq!(
                                after_second.sky_light(p),
                                Some(expected[4][1][index]),
                                "second sky {p:?}"
                            );
                        }
                    }
                }
            }
            if !engine.incremental_columns.is_empty() {
                let old_active = engine.active.clone();
                let error = engine
                    .activate(&terrain, east, [center], &registry)
                    .err()
                    .unwrap();
                assert!(error.to_string().contains(
                    "incremental sky light activation inputs are incomplete or unsupported"
                ));
                assert_eq!(engine.active, old_active);
                let next = ChunkPos { x: 2, z: 0 };
                terrain.completed_decoration.insert(next);
                let count = engine.fresh.registered.len();
                engine.initialize_chunk(&terrain, next, &registry).unwrap();
                assert_eq!(engine.fresh.registered.len(), count + 1);
                assert!(engine.initialize_chunk(&terrain, next, &registry).is_err());
                assert_eq!(engine.fresh.registered.len(), count + 1);
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn staged_tree_section_updates_match_java_in_25_columns() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        for mode in [
            "log_active_sparse_wide",
            "tree_empty_full_wide",
            "tree_empty_partial_wide",
            "tree_empty_partial_register",
            "tree_empty_partial_register_empty",
            "tree_empty_full_activate",
            "tree_empty_active_wide",
        ] {
            let path = temp.path().join("staged-wide.json");
            let output = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&path)
                .arg(mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let expected: Vec<Vec<Vec<Vec<u8>>>> =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let (mut terrain, tags, _, _) = fixture();
            let block_names = registry.ids().map(str::to_owned).collect();
            let motion = HeightmapTags::load(&mut Data, &block_names).unwrap();
            let center = ChunkPos { x: 0, z: 0 };
            let east = ChunkPos { x: 1, z: 0 };
            let world = Arc::make_mut(&mut terrain.world);
            let template = Arc::clone(world.chunks.get(&center).unwrap());
            let halo = if mode == "tree_empty_active_wide" {
                3
            } else {
                2
            };
            for z in -halo..=halo {
                for x in -halo..=halo {
                    let pos = ChunkPos { x, z };
                    world.chunks.entry(pos).or_insert_with(|| {
                        let mut chunk = (*template).clone();
                        chunk.position = pos;
                        Arc::new(chunk)
                    });
                }
            }
            let air = world.intern_state(state("air"));
            let section = Arc::make_mut(
                Arc::make_mut(world.chunks.get_mut(&east).unwrap())
                    .sections
                    .get_mut(&0)
                    .unwrap(),
            );
            section.blocks.as_mut().unwrap().fill(air);
            if mode == "tree_empty_partial_register_empty" {
                let west = ChunkPos { x: -1, z: 0 };
                let section = Arc::make_mut(
                    Arc::make_mut(world.chunks.get_mut(&west).unwrap())
                        .sections
                        .get_mut(&0)
                        .unwrap(),
                );
                section.blocks.as_mut().unwrap().fill(air);
            }
            if mode == "log_active_sparse_wide" {
                let stone = world.intern_state(state("stone"));
                world.set_block_id(31, 0, 0, stone).unwrap();
            }
            let registered: Vec<_> =
                if mode == "tree_empty_full_activate" || mode == "tree_empty_active_wide" {
                    (-2..=2)
                        .flat_map(|z| (-2..=2).map(move |x| ChunkPos { x, z }))
                        .collect()
                } else if mode.starts_with("tree_empty_partial_") {
                    vec![center, east]
                } else {
                    (-1..=1)
                        .flat_map(|z| (-1..=1).map(move |x| ChunkPos { x, z }))
                        .collect()
                };
            terrain
                .completed_decoration
                .extend(registered.iter().copied());
            let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
            for pos in registered {
                tracker.initialize_chunk(&terrain, pos, &registry).unwrap();
            }
            let mut engine = StagedGenerationLightEngine::new(tracker);
            let covered: Vec<_> = (-2..=2)
                .flat_map(|z| (-2..=2).map(move |x| ChunkPos { x, z }))
                .collect();
            let before = engine
                .activate(
                    &terrain,
                    if mode == "tree_empty_active_wide" {
                        east
                    } else {
                        center
                    },
                    covered.iter().copied(),
                    &registry,
                )
                .unwrap();
            let mut draft = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([east]))
                .unwrap();
            draft.light_guard = Some(DecorationLightGuard {
                registered: BTreeSet::from([east]),
                registry: &registry,
                require_unchanged_light: true,
                changed_blocks: BTreeSet::new(),
                changed_chunks: BTreeSet::new(),
                light_replay_positions: BTreeSet::new(),
                light_check_order: Vec::new(),
                vine_light_positions: BTreeSet::new(),
                new_nonempty_sections: Vec::new(),
                non_air_counts: BTreeMap::new(),
            });
            let mut log = state("dark_oak_log");
            log.properties.insert("axis".into(), "y".into());
            draft.set_tree_block([16, 10, 8], &log, 19).unwrap();
            let mut tracking = Some(DecorationLightTracking::Staged(&mut engine, &registry));
            finish_decoration_with_light(draft, &mut tracking).unwrap();
            drop(tracking);
            let after = engine
                .snapshot(&terrain, covered.iter().copied(), &registry)
                .unwrap();
            if mode.starts_with("tree_empty_partial_") {
                let outer = ChunkPos { x: 2, z: 0 };
                assert_eq!(before.block_section_allocated(outer, 0), Some(false));
                assert_eq!(after.block_section_allocated(outer, 0), Some(true));
            }
            let registered = if mode.starts_with("tree_empty_partial_register") {
                let west = ChunkPos { x: -1, z: 0 };
                terrain.completed_decoration.insert(west);
                let count = engine.incremental_columns.len();
                engine.initialize_chunk(&terrain, west, &registry).unwrap();
                if mode.ends_with("_empty") {
                    assert_eq!(engine.incremental_columns.len(), count);
                }
                Some(
                    engine
                        .snapshot(&terrain, covered.iter().copied(), &registry)
                        .unwrap(),
                )
            } else {
                None
            };
            let activated = if mode == "tree_empty_full_activate" {
                Some(
                    engine
                        .activate(&terrain, east, covered.iter().copied(), &registry)
                        .unwrap(),
                )
            } else {
                None
            };
            for (index, pos) in covered.iter().enumerate() {
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [pos.x * 16 + x, y, pos.z * 16 + z];
                            let i = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                before.block_light(p),
                                Some(expected[0][index][0][i]),
                                "{mode} before block {p:?}"
                            );
                            assert_eq!(
                                before.sky_light(p),
                                Some(expected[0][index][1][i]),
                                "{mode} before sky {p:?}"
                            );
                            assert_eq!(
                                before.block_light(p),
                                Some(expected[1][index][0][i]),
                                "{mode} pending block {p:?}"
                            );
                            assert_eq!(
                                before.sky_light(p),
                                Some(expected[1][index][1][i]),
                                "{mode} pending sky {p:?}"
                            );
                            assert_eq!(
                                after.block_light(p),
                                Some(expected[2][index][0][i]),
                                "{mode} after block {p:?}"
                            );
                            assert_eq!(
                                after.sky_light(p),
                                Some(expected[2][index][1][i]),
                                "{mode} after sky {p:?}"
                            );
                            if let Some(registered) = &registered {
                                assert_eq!(
                                    after.block_light(p),
                                    Some(expected[3][index][0][i]),
                                    "{mode} pending registration block {p:?}"
                                );
                                assert_eq!(
                                    after.sky_light(p),
                                    Some(expected[3][index][1][i]),
                                    "{mode} pending registration sky {p:?}"
                                );
                                assert_eq!(
                                    registered.block_light(p),
                                    Some(expected[4][index][0][i]),
                                    "{mode} registered block {p:?}"
                                );
                                assert_eq!(
                                    registered.sky_light(p),
                                    Some(expected[4][index][1][i]),
                                    "{mode} registered sky {p:?}"
                                );
                            }
                            if let Some(activated) = &activated {
                                assert_eq!(
                                    after.block_light(p),
                                    Some(expected[3][index][0][i]),
                                    "{mode} pending activation block {p:?}"
                                );
                                assert_eq!(
                                    after.sky_light(p),
                                    Some(expected[3][index][1][i]),
                                    "{mode} pending activation sky {p:?}"
                                );
                                assert_eq!(
                                    activated.block_light(p),
                                    Some(expected[4][index][0][i]),
                                    "{mode} activation block {p:?}"
                                );
                                assert_eq!(
                                    activated.sky_light(p),
                                    Some(expected[4][index][1][i]),
                                    "{mode} activation sky {p:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn staged_cave_vines_match_java_light_across_25_columns() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        for mode in [
            "vine_active_plain",
            "vine_active_glowing",
            "vine_active_head",
            "vine_active_existing",
            "vine_active_empty_plain",
            "vine_active_empty_glowing",
            "vine_active_many",
            "vine_active_plain_then_glowing",
            "vine_active_empty_plain_then_glowing",
        ] {
            let path = temp.path().join("vine-wide.json");
            let output = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&path)
                .arg(mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let expected: Vec<Vec<Vec<Vec<u8>>>> =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let (mut terrain, tags, _, _) = fixture();
            let block_names = registry.ids().map(str::to_owned).collect();
            let motion = HeightmapTags::load(&mut Data, &block_names).unwrap();
            let center = ChunkPos { x: 0, z: 0 };
            let east = ChunkPos { x: 1, z: 0 };
            let world = Arc::make_mut(&mut terrain.world);
            let template = Arc::clone(world.chunks.get(&center).unwrap());
            for z in -2..=2 {
                for x in -2..=2 {
                    let pos = ChunkPos { x, z };
                    world.chunks.entry(pos).or_insert_with(|| {
                        let mut chunk = (*template).clone();
                        chunk.position = pos;
                        Arc::new(chunk)
                    });
                }
            }
            if mode.contains("_empty_") {
                let air = world.intern_state(state("air"));
                let section = Arc::make_mut(
                    Arc::make_mut(world.chunks.get_mut(&east).unwrap())
                        .sections
                        .get_mut(&0)
                        .unwrap(),
                );
                section.blocks.as_mut().unwrap().fill(air);
            } else {
                let torch = world.intern_state(state("torch"));
                world.set_block_id(17, 10, 8, torch).unwrap();
            }
            if mode == "vine_active_existing" {
                let mut plain = state("cave_vines_plant");
                plain.properties.insert("berries".into(), "false".into());
                let id = world.intern_state(plain);
                world.set_block_id(16, 10, 8, id).unwrap();
            }
            let registered: Vec<_> = (-1..=1)
                .flat_map(|z| (-1..=1).map(move |x| ChunkPos { x, z }))
                .collect();
            terrain
                .completed_decoration
                .extend(registered.iter().copied());
            let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
            for pos in registered {
                tracker.initialize_chunk(&terrain, pos, &registry).unwrap();
            }
            let mut engine = StagedGenerationLightEngine::new(tracker);
            let covered: Vec<_> = (-2..=2)
                .flat_map(|z| (-2..=2).map(move |x| ChunkPos { x, z }))
                .collect();
            let before = engine
                .activate(&terrain, center, covered.iter().copied(), &registry)
                .unwrap();
            let mut draft = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([east]))
                .unwrap();
            draft.light_guard = Some(DecorationLightGuard {
                registered: BTreeSet::from([east]),
                registry: &registry,
                require_unchanged_light: true,
                changed_blocks: BTreeSet::new(),
                changed_chunks: BTreeSet::new(),
                light_replay_positions: BTreeSet::new(),
                light_check_order: Vec::new(),
                vine_light_positions: BTreeSet::new(),
                new_nonempty_sections: Vec::new(),
                non_air_counts: BTreeMap::new(),
            });
            let positions = if mode.ends_with("_plain_then_glowing") {
                vec![[16, 10, 8], [16, 10, 8]]
            } else if mode == "vine_active_many" {
                vec![[16, 10, 8], [16, 9, 8], [16, 8, 8]]
            } else {
                vec![[16, 10, 8]]
            };
            for (i, p) in positions.iter().copied().enumerate() {
                let berries = if mode.ends_with("_plain_then_glowing") {
                    i == 1
                } else if mode == "vine_active_many" {
                    i != 1
                } else {
                    !mode.ends_with("_plain")
                };
                let mut vine = state(if mode == "vine_active_head" {
                    "cave_vines"
                } else {
                    "cave_vines_plant"
                });
                vine.properties
                    .insert("berries".into(), berries.to_string());
                if mode == "vine_active_head" {
                    vine.properties.insert("age".into(), "7".into());
                }
                assert!(draft.set_column_block(p, &vine).unwrap());
            }
            let guard = draft.light_guard.as_ref().unwrap();
            let expected_checks = if mode == "vine_active_many" {
                2
            } else if mode.ends_with("_plain") {
                0
            } else {
                1
            };
            assert_eq!(guard.light_check_order.len(), expected_checks, "{mode}");
            assert_eq!(
                guard.new_nonempty_sections.len(),
                usize::from(mode.contains("_empty_")),
                "{mode}"
            );
            let mut tracking = Some(DecorationLightTracking::Staged(&mut engine, &registry));
            finish_decoration_with_light(draft, &mut tracking).unwrap();
            drop(tracking);
            let after = engine
                .snapshot(&terrain, covered.iter().copied(), &registry)
                .unwrap();
            if mode == "vine_active_glowing" {
                let original = Arc::clone(&terrain.world);
                let mut draft = terrain
                    .decoration_region(&tags, &motion, BTreeSet::from([center]))
                    .unwrap();
                draft.light_guard = Some(DecorationLightGuard {
                    registered: BTreeSet::from([center]),
                    registry: &registry,
                    require_unchanged_light: true,
                    changed_blocks: BTreeSet::new(),
                    changed_chunks: BTreeSet::new(),
                    light_replay_positions: BTreeSet::new(),
                    light_check_order: Vec::new(),
                    vine_light_positions: BTreeSet::new(),
                    new_nonempty_sections: Vec::new(),
                    non_air_counts: BTreeMap::new(),
                });
                let mut vine = state("cave_vines_plant");
                vine.properties.insert("berries".into(), "true".into());
                assert!(draft.set_column_block([0, 10, 8], &vine).unwrap());
                let mut tracking = Some(DecorationLightTracking::Staged(&mut engine, &registry));
                let error = finish_decoration_with_light(draft, &mut tracking).unwrap_err();
                drop(tracking);
                assert!(error.to_string().contains("active cave vine lighting"));
                assert!(Arc::ptr_eq(&original, &terrain.world));
                let restored = engine.snapshot(&terrain, [center], &registry).unwrap();
                for p in [[0, 10, 8], [15, 10, 8], [8, -1, 8]] {
                    assert_eq!(restored.block_light(p), after.block_light(p));
                    assert_eq!(restored.sky_light(p), after.sky_light(p));
                }
            }
            for (index, pos) in covered.iter().enumerate() {
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [pos.x * 16 + x, y, pos.z * 16 + z];
                            let i = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                before.block_light(p),
                                Some(expected[0][index][0][i]),
                                "{mode} before block {p:?}"
                            );
                            assert_eq!(
                                before.sky_light(p),
                                Some(expected[0][index][1][i]),
                                "{mode} before sky {p:?}"
                            );
                            assert_eq!(
                                before.block_light(p),
                                Some(expected[1][index][0][i]),
                                "{mode} pending block {p:?}"
                            );
                            assert_eq!(
                                before.sky_light(p),
                                Some(expected[1][index][1][i]),
                                "{mode} pending sky {p:?}"
                            );
                            assert_eq!(
                                after.block_light(p),
                                Some(expected[2][index][0][i]),
                                "{mode} after block {p:?}"
                            );
                            assert_eq!(
                                after.sky_light(p),
                                Some(expected[2][index][1][i]),
                                "{mode} after sky {p:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn staged_generation_light_snapshots_match_java() {
        use super::super::StagedGenerationLightEngine;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let temp = tempfile::tempdir().unwrap();
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
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let output = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected_path = temp.path().join("expected.json");
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StagedGenerationLightEngineOracle")
            .arg(&expected_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Vec<Vec<Vec<u8>>> =
            serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StagedGenerationLightEngineOracle")
            .arg(&expected_path)
            .arg("partial")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let partial_expected: Vec<Vec<Vec<u8>>> =
            serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        for mode in ["mixed", "equivalent", "late", "late_multi", "late_far"] {
            let output = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&expected_path)
                .arg(mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mixed_expected: Vec<Vec<Vec<Vec<u8>>>> =
                serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
            let (mut mixed_terrain, _, _, _) = fixture();
            let center = ChunkPos { x: 0, z: 0 };
            let east = ChunkPos { x: 1, z: 0 };
            let west = ChunkPos { x: -1, z: 0 };
            let north = ChunkPos { x: 0, z: -1 };
            let far = ChunkPos { x: 2, z: 0 };
            let columns = if mode == "late_far" {
                [center, east, west]
            } else {
                [center, west, north]
            };
            let world = Arc::make_mut(&mut mixed_terrain.world);
            let template = world.chunks[&center].as_ref().clone();
            for z in -2..=2 {
                for x in -2..=2 {
                    let pos = ChunkPos { x, z };
                    world.chunks.entry(pos).or_insert_with(|| {
                        let mut chunk = template.clone();
                        chunk.position = pos;
                        Arc::new(chunk)
                    });
                }
            }
            let torch = world.intern_state(state("torch"));
            world.set_block_id(0, 10, 8, torch).unwrap();
            world.set_block_id(17, 10, 8, torch).unwrap();
            if mode == "late_far" {
                world.set_block_id(31, 10, 8, torch).unwrap();
            }
            let pending_far = if mode == "late_far" {
                world.chunks.remove(&far)
            } else {
                None
            };
            mixed_terrain
                .completed_decoration
                .extend([center, east, west, far]);
            let mut mixed_tracker = FreshGenerationLightTracker::new(&mixed_terrain, true).unwrap();
            mixed_tracker
                .initialize_chunk(&mixed_terrain, center, &registry)
                .unwrap();
            if !mode.starts_with("late") {
                mixed_tracker
                    .initialize_chunk(&mixed_terrain, east, &registry)
                    .unwrap();
            }
            let mut mixed_engine = StagedGenerationLightEngine::new(mixed_tracker);
            let mixed_first = mixed_engine
                .activate(&mixed_terrain, center, [center, west, north], &registry)
                .unwrap();
            assert!(!mixed_engine.allows_writes(west, 0));
            assert!(!mixed_engine.allows_writes(ChunkPos { x: 2, z: 0 }, 1));
            assert!(mixed_engine.allows_writes(
                ChunkPos {
                    x: i32::MAX,
                    z: i32::MIN
                },
                1
            ));
            let registered = if mode.starts_with("late") {
                let frozen = Arc::clone(&mixed_terrain.world);
                let world = Arc::make_mut(&mut mixed_terrain.world);
                let stone = world.intern_state(state("stone"));
                world.set_block_id(0, 10, 8, stone).unwrap();
                assert!(
                    mixed_engine
                        .initialize_chunk(&mixed_terrain, east, &registry)
                        .is_err()
                );
                mixed_terrain.world = Arc::clone(&frozen);
                let world = Arc::make_mut(&mut mixed_terrain.world);
                let stone = world.intern_state(state("stone"));
                world.set_block_id(-1, 10, 8, stone).unwrap();
                assert!(
                    mixed_engine
                        .initialize_chunk(&mixed_terrain, east, &registry)
                        .is_err()
                );
                mixed_terrain.world = frozen;
                if mode == "late_far" {
                    let world = Arc::make_mut(&mut mixed_terrain.world);
                    assert!(world.chunks.insert(far, pending_far.unwrap()).is_none());
                    let stone = world.intern_state(state("stone"));
                    world.set_block_id(32, 10, 8, stone).unwrap();
                    world.set_block_id(33, 10, 8, torch).unwrap();
                }
                mixed_engine
                    .initialize_chunk(&mixed_terrain, east, &registry)
                    .unwrap();
                assert!(
                    mixed_engine
                        .initialize_chunk(&mixed_terrain, east, &registry)
                        .is_err()
                );
                Some(
                    mixed_engine
                        .snapshot(&mixed_terrain, columns, &registry)
                        .unwrap(),
                )
            } else {
                let world = Arc::make_mut(&mut mixed_terrain.world);
                if mode == "mixed" {
                    let stone = world.intern_state(state("stone"));
                    world.set_block_id(-1, 10, 8, stone).unwrap();
                } else {
                    let mushroom = world.intern_state(state("red_mushroom"));
                    world.set_block_id(15, 10, -1, mushroom).unwrap();
                }
                None
            };
            let mixed_second = mixed_engine
                .activate(&mixed_terrain, east, columns, &registry)
                .unwrap();
            let mut additional = Vec::new();
            if mode == "late_far" {
                mixed_engine
                    .initialize_chunk(&mixed_terrain, far, &registry)
                    .unwrap();
                additional.push(
                    mixed_engine
                        .snapshot(&mixed_terrain, columns, &registry)
                        .unwrap(),
                );
                additional.push(
                    mixed_engine
                        .activate(&mixed_terrain, far, columns, &registry)
                        .unwrap(),
                );
            }
            if mode == "late_multi" {
                mixed_engine
                    .initialize_chunk(&mixed_terrain, west, &registry)
                    .unwrap();
                additional.push(
                    mixed_engine
                        .snapshot(&mixed_terrain, [center, west, north], &registry)
                        .unwrap(),
                );
                additional.push(
                    mixed_engine
                        .activate(&mixed_terrain, west, [center, west, north], &registry)
                        .unwrap(),
                );
            }
            let actual_stages = [&mixed_first]
                .into_iter()
                .chain(registered.as_ref())
                .chain([&mixed_second])
                .chain(additional.iter());
            for (stage, actual) in actual_stages.enumerate() {
                let stage_columns = if mode == "late_far" && stage == 0 {
                    [center, west, north]
                } else {
                    columns
                };
                for (column, chunk) in stage_columns.into_iter().enumerate() {
                    let (base_x, base_z) = (chunk.x * 16, chunk.z * 16);
                    for y in -16..32 {
                        for z in 0..16 {
                            for x in 0..16 {
                                let p = [base_x + x, y, base_z + z];
                                let index = ((y + 16) * 256 + z * 16 + x) as usize;
                                assert_eq!(
                                    actual.block_light(p),
                                    Some(mixed_expected[stage][column][0][index]),
                                    "{mode} block stage {stage} at {p:?}"
                                );
                                assert_eq!(
                                    actual.sky_light(p),
                                    Some(mixed_expected[stage][column][1][index]),
                                    "{mode} sky stage {stage} at {p:?}"
                                );
                            }
                        }
                    }
                }
            }
            if mode == "mixed" {
                assert_eq!(mixed_second.block_light([-1, 10, 8]), Some(13));
            }
        }
        let (mut terrain, _, _, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let torch = world.intern_state(state("torch"));
        world.set_block_id(17, 10, 8, torch).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let east = ChunkPos { x: 1, z: 0 };
        let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                let pos = ChunkPos { x, z };
                terrain.completed_decoration.insert(pos);
                tracker.initialize_chunk(&terrain, pos, &registry).unwrap();
            }
        }
        let mut engine = StagedGenerationLightEngine::new(tracker);
        let before = engine.snapshot(&terrain, [center], &registry).unwrap();
        // East's 3x3 halo is not loaded. The attempted activation must roll back.
        assert!(
            engine
                .activate(&terrain, center, [east], &registry)
                .is_err()
        );
        use super::super::schedule::{GenerationSchedule, GenerationStage};
        let mut schedule = GenerationSchedule::new();
        schedule.request(center, GenerationStage::Light).unwrap();
        // Earlier generation steps are represented by the prepared fixture.
        while !schedule.ready(center, GenerationStage::Light) {
            let (pos, stage) = schedule.next_ready().unwrap();
            schedule.complete(pos, stage).unwrap();
        }
        terrain.completed_decoration.remove(&east);
        assert!(
            schedule
                .run_light(&terrain, center, &mut engine, &registry)
                .is_err()
        );
        assert_eq!(
            schedule.completed(center),
            Some(GenerationStage::InitializeLight)
        );
        terrain.completed_decoration.insert(east);
        let first = schedule
            .run_light(&terrain, center, &mut engine, &registry)
            .unwrap();
        assert_eq!(schedule.completed(center), Some(GenerationStage::Light));
        assert!(
            engine
                .activate(&terrain, center, [center], &registry)
                .is_err()
        );
        let second = engine
            .activate(&terrain, east, [center], &registry)
            .unwrap();
        let recaptured = engine.snapshot(&terrain, [center], &registry).unwrap();
        for (stage, actual) in [&before, &first, &second].into_iter().enumerate() {
            for y in -16..32 {
                for z in 0..16 {
                    for x in 0..16 {
                        let p = [x, y, z];
                        let index = ((y + 16) * 256 + z * 16 + x) as usize;
                        assert_eq!(
                            actual.block_light(p),
                            Some(expected[stage][0][index]),
                            "block stage {stage} at {p:?}"
                        );
                        assert_eq!(
                            actual.sky_light(p),
                            Some(expected[stage][1][index]),
                            "sky stage {stage} at {p:?}"
                        );
                    }
                }
            }
        }
        assert_eq!(before.block_light([15, 10, 8]), Some(0));
        assert_eq!(second.block_light([15, 10, 8]), Some(12));
        assert_eq!(recaptured.block_light([15, 10, 8]), Some(12));
        let mut dark_tracker = FreshGenerationLightTracker::new(&terrain, false).unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                dark_tracker
                    .initialize_chunk(&terrain, ChunkPos { x, z }, &registry)
                    .unwrap();
            }
        }
        let mut no_sky = StagedGenerationLightEngine::new(dark_tracker);
        no_sky
            .activate(&terrain, center, [center], &registry)
            .unwrap();
        let dark = no_sky
            .activate(&terrain, east, [center], &registry)
            .unwrap();
        for y in -16..32 {
            for z in 0..16 {
                for x in 0..16 {
                    let p = [x, y, z];
                    let index = ((y + 16) * 256 + z * 16 + x) as usize;
                    assert_eq!(
                        dark.block_light(p),
                        Some(expected[2][0][index]),
                        "no-sky block at {p:?}"
                    );
                    assert_eq!(dark.sky_light(p), Some(0));
                }
            }
        }
        let mut partial_tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
        partial_tracker
            .initialize_chunk(&terrain, east, &registry)
            .unwrap();
        let mut partial_engine = StagedGenerationLightEngine::new(partial_tracker);
        let partial_before = partial_engine
            .activate(&terrain, east, [center], &registry)
            .unwrap();
        let world = Arc::make_mut(&mut terrain.world);
        let stone = world.intern_state(state("stone"));
        world.set_block_id(15, 10, 8, stone).unwrap();
        let partial_after = partial_engine
            .snapshot(&terrain, [center], &registry)
            .unwrap();
        for (stage, actual) in [&partial_before, &partial_after].into_iter().enumerate() {
            for y in -16..32 {
                for z in 0..16 {
                    for x in 0..16 {
                        let p = [x, y, z];
                        let index = ((y + 16) * 256 + z * 16 + x) as usize;
                        assert_eq!(
                            actual.block_light(p),
                            Some(partial_expected[stage][0][index]),
                            "partial block stage {stage} at {p:?}"
                        );
                        assert_eq!(
                            actual.sky_light(p),
                            Some(partial_expected[stage][1][index]),
                            "partial sky stage {stage} at {p:?}"
                        );
                    }
                }
            }
        }
        let world = Arc::make_mut(&mut terrain.world);
        let air = world.intern_state(state("air"));
        world.set_block_id(17, 10, 8, air).unwrap();
        assert!(engine.snapshot(&terrain, [center], &registry).is_err());
        assert_eq!(second.block_light([15, 10, 8]), Some(12));
    }

    #[test]
    fn changed_generation_sections_require_equivalent_light_behavior() {
        let (mut terrain, _, _, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let before = Arc::clone(&terrain.world);
        let world = Arc::make_mut(&mut terrain.world);
        let red = world.intern_state(state("red_mushroom"));
        world.set_block_id(1, 10, 1, red).unwrap();
        let same = |world: &World| {
            super::super::unchanged_chunk_light_behavior(
                &before,
                world,
                &before.chunks[&center],
                &world.chunks[&center],
                0,
                16,
            )
        };
        assert!(same(&terrain.world).unwrap());
        let world = Arc::make_mut(&mut terrain.world);
        let stone = world.intern_state(state("stone"));
        world.set_block_id(2, 10, 1, stone).unwrap();
        assert!(!same(&terrain.world).unwrap());
        let world = Arc::make_mut(&mut terrain.world);
        let unknown = world.intern_state(BlockState {
            name: "example:unknown".into(),
            properties: BTreeMap::new(),
        });
        world.set_block_id(2, 10, 1, unknown).unwrap();
        assert!(same(&terrain.world).is_err());
    }

    #[test]
    fn generated_light_waits_for_writers_and_uses_current_blocks() {
        let (mut terrain, _, _, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        for z in -2..=2 {
            for x in -2..=2 {
                terrain.completed_decoration.insert(ChunkPos { x, z });
            }
        }
        // Even an outer feature center can write into the sampled halo.
        terrain
            .completed_decoration
            .remove(&ChunkPos { x: 2, z: 2 });
        assert!(terrain.calculate_generated_block_light(center).is_err());
        assert!(terrain.calculate_generated_sky_light(center).is_err());
        terrain.completed_decoration.insert(ChunkPos { x: 2, z: 2 });
        let world = Arc::make_mut(&mut terrain.world);
        let torch = world.intern_state(state("torch"));
        world.set_block_id(-1, 10, 8, torch).unwrap();
        let before = terrain.snapshot();
        let light = terrain.calculate_generated_block_light(center).unwrap();
        let sky = terrain.calculate_generated_sky_light(center).unwrap();
        let index = 8 * 16 + 10 * 256;
        assert_eq!(light[&0][index / 2] & 15, 13);
        assert_eq!(sky[&0][index / 2] & 15, 15);
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        let world = Arc::make_mut(&mut terrain.world);
        let air = world.intern_state(state("air"));
        world.set_block_id(-1, 10, 8, air).unwrap();
        let updated = terrain.calculate_generated_block_light(center).unwrap();
        assert_eq!(updated[&0][index / 2] & 15, 0);
        assert_eq!(light[&0][index / 2] & 15, 13);
    }

    #[test]
    fn generated_lighting_is_atomic_and_invalidated_by_neighbor_edits() {
        let (mut terrain, tags, motion, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        for z in -2..=2 {
            for x in -2..=2 {
                terrain.completed_decoration.insert(ChunkPos { x, z });
            }
        }
        let world = Arc::make_mut(&mut terrain.world);
        let torch = world.intern_state(state("torch"));
        world.set_block_id(-1, 10, 8, torch).unwrap();
        let before = terrain.snapshot();
        terrain.initialize_generated_lighting(center, true).unwrap();
        let lit = terrain.snapshot();
        assert!(!before.chunks[&center].block_light_correct);
        assert!(lit.chunks[&center].block_light_correct);
        assert_eq!(
            lit.chunks[&center].status.as_deref(),
            Some("minecraft:terrain")
        );
        assert_eq!(lit.light(0, 10, 8, false), Some(13));
        assert_eq!(lit.light(0, 10, 8, true), Some(15));
        assert_eq!(lit.light(0, 16, 8, true), Some(15));

        terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap()
            .finish()
            .unwrap();
        assert!(Arc::ptr_eq(
            &lit.chunks[&center],
            &terrain.snapshot().chunks[&center]
        ));

        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: -1, z: 0 }]))
            .unwrap();
        region
            .set_section_block([-1, 10, 8], &state("air"))
            .unwrap();
        region.finish().unwrap();
        let invalidated = terrain.snapshot();
        assert!(!invalidated.chunks[&center].block_light_correct);
        assert_eq!(invalidated.light(0, 10, 8, false), None);
        assert_eq!(invalidated.light(0, 10, 8, true), None);
        assert_eq!(lit.light(0, 10, 8, false), Some(13));
        terrain
            .initialize_generated_lighting(center, false)
            .unwrap();
        let no_sky = terrain.snapshot();
        assert_eq!(no_sky.light(0, 10, 8, false), Some(0));
        assert_eq!(no_sky.light(0, 10, 8, true), None);

        let world = Arc::make_mut(&mut terrain.world);
        let unknown = world.intern_state(BlockState {
            name: "example:unknown".into(),
            properties: BTreeMap::new(),
        });
        world.set_block_id(17, 10, 8, unknown).unwrap();
        let failed_input = terrain.snapshot();
        assert!(terrain.initialize_generated_lighting(center, true).is_err());
        assert!(Arc::ptr_eq(&failed_input, &terrain.snapshot()));
    }

    #[test]
    fn generated_lighting_uses_selected_dimension_and_rejects_mismatch() {
        use native_resources::dimension_lighting::GeneratedDimensionLighting;
        let (mut terrain, _, _, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        for z in -2..=2 {
            for x in -2..=2 {
                terrain.completed_decoration.insert(ChunkPos { x, z });
            }
        }
        let mut settings = GeneratedDimensionLighting {
            dimension: ResourceId::parse("minecraft:overworld").unwrap(),
            dimension_type: Some(ResourceId::parse("example:custom_sky").unwrap()),
            min_y: 0,
            height: 32,
            has_skylight: true,
            ambient_light: Some(0.0),
        };
        let before = terrain.snapshot();
        assert!(
            terrain
                .initialize_lighting_for_dimension(center, &settings)
                .is_err()
        );
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        settings.height = 16;
        terrain
            .initialize_lighting_for_dimension(center, &settings)
            .unwrap();
        let lit = terrain.snapshot();
        assert_eq!(lit.dimension_type.as_deref(), Some("example:custom_sky"));
        assert_eq!(lit.light(0, 10, 0, true), Some(15));
        settings.dimension_type = Some(ResourceId::parse("example:other").unwrap());
        assert!(
            terrain
                .initialize_lighting_for_dimension(center, &settings)
                .is_err()
        );
        assert!(Arc::ptr_eq(&lit, &terrain.snapshot()));
        settings.dimension = ResourceId::parse("example:wrong_dimension").unwrap();
        assert!(
            terrain
                .initialize_lighting_for_dimension(center, &settings)
                .is_err()
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn fresh_generation_light_reads_initialized_chunk_sections() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        let (mut terrain, _, _, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        assert!(
            terrain
                .fresh_generation_light([center], [center], &registry, true)
                .is_err()
        );
        terrain.completed_decoration.insert(center);
        let light = terrain
            .fresh_generation_light([center], [center], &registry, true)
            .unwrap();
        assert_eq!(light.sky_light([0, 0, 0]), Some(0));
        assert_eq!(light.sky_light([0, 32, 0]), Some(15));
        assert_eq!(light.block_light([0, 0, 0]), Some(0));
        let no_registered = terrain
            .fresh_generation_light([], [center], &registry, true)
            .unwrap();
        assert_eq!(no_registered.sky_light([0, 0, 0]), Some(15));
        Arc::make_mut(
            Arc::make_mut(&mut terrain.world)
                .chunks
                .get_mut(&center)
                .unwrap(),
        )
        .block_light_correct = true;
        assert!(
            terrain
                .fresh_generation_light([center], [center], &registry, true)
                .is_err()
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn tracked_fresh_light_drives_decoration_and_detects_untracked_writes() {
        struct MushroomAt(i32);
        impl ResourceProvider for MushroomAt {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"features":[[{"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:brown_mushroom"}},"placement":[{"type":"minecraft:offset","x":self.0,"y":8,"z":8}]}]]}),
                    )?));
                }
                if path == "data/minecraft/tags/block/overrides_mushroom_light_requirement.json" {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                Data.read_data(path)
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let vanilla = resources.vanilla_generated_block_registry().unwrap();
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        for x in [8, 16] {
            let schedule = FeatureSchedule::load(
                &mut MushroomAt(x),
                std::slice::from_ref(&plains),
                &BTreeSet::new(),
            )
            .unwrap();
            let (_, _, neighborhood) = scheduled_fixture(None);
            let plan = fixture_plan(&schedule, &neighborhood);
            let mut registry = batch_registry();
            registry.extend(["minecraft:brown_mushroom".into(), "minecraft:torch".into()]);
            let prepared =
                PreparedDecoration::load(&mut MushroomAt(x), &plan, &registry, &mut resolve_test)
                    .unwrap();
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            let torch = world.intern_state(state("torch"));
            world.set_block_id(17, 10, 8, torch).unwrap();
            let registered = ChunkPos { x: 1, z: 0 };
            let center = ChunkPos { x: 0, z: 0 };
            let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
            assert!(
                tracker
                    .register_chunk(&terrain, registered, &vanilla)
                    .is_err()
            );
            terrain.completed_decoration.insert(registered);
            tracker
                .register_chunk(&terrain, registered, &vanilla)
                .unwrap();
            assert!(
                tracker
                    .register_chunk(&terrain, registered, &vanilla)
                    .is_err()
            );
            let second = ChunkPos { x: 0, z: 1 };
            terrain.completed_decoration.insert(second);
            tracker.register_chunk(&terrain, second, &vanilla).unwrap();
            assert!(
                tracker
                    .calculate_block_light_for_active_chunks(
                        &terrain,
                        center,
                        &BTreeSet::from([center]),
                        &vanilla
                    )
                    .is_err()
            );
            let dark = tracker
                .calculate_block_light_for_active_chunks(
                    &terrain,
                    center,
                    &BTreeSet::new(),
                    &vanilla,
                )
                .unwrap();
            let active = tracker
                .calculate_block_light_for_active_chunks(
                    &terrain,
                    center,
                    &BTreeSet::from([registered]),
                    &vanilla,
                )
                .unwrap();
            let index = 15 + 8 * 16 + 10 * 256;
            assert_eq!(dark[&0][index / 2] >> 4, 0);
            assert_eq!(active[&0][index / 2] >> 4, 12);
            let mut covered = Vec::new();
            for z in -2..=2 {
                for x in -2..=2 {
                    covered.push(ChunkPos { x, z });
                }
            }
            let cached = tracker
                .snapshot(&terrain, covered.iter().copied(), &vanilla)
                .unwrap();
            let direct = terrain
                .fresh_generation_light(
                    [registered, second],
                    covered.iter().copied(),
                    &vanilla,
                    true,
                )
                .unwrap();
            for z in [-17, -1, 0, 15, 16, 32] {
                for x in [-17, -1, 0, 15, 16, 32] {
                    for y in [-32, 0, 8, 15, 16, 31, 32, 64] {
                        let p = [x, y, z];
                        assert_eq!(cached.block_light(p), direct.block_light(p), "{p:?}");
                        assert_eq!(cached.sky_light(p), direct.sky_light(p), "{p:?}");
                    }
                }
            }
            assert_eq!(
                tracker
                    .snapshot(&terrain, [center], &vanilla)
                    .unwrap()
                    .sky_light([8, 8, 8]),
                Some(0)
            );
            terrain.completed_decoration.remove(&second);
            assert!(tracker.snapshot(&terrain, [center], &vanilla).is_err());
            terrain.completed_decoration.insert(second);
            let mut steps = super::super::schedule::GenerationSchedule::new();
            use super::super::schedule::GenerationStage;
            steps.request(center, GenerationStage::Features).unwrap();
            while !steps.ready(center, GenerationStage::Features) {
                let (pos, stage) = steps.next_ready().unwrap();
                steps.complete(pos, stage).unwrap();
            }
            let before = terrain.snapshot();
            assert!(
                steps
                    .run_features(
                        &mut terrain,
                        &prepared,
                        true,
                        Some((&mut tracker, &vanilla))
                    )
                    .is_err()
            );
            assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
            assert_eq!(steps.completed(center), Some(GenerationStage::Terrain));
            let result = steps.run_features(
                &mut terrain,
                &prepared,
                false,
                Some((&mut tracker, &vanilla)),
            );
            if x == 8 {
                assert_eq!(result.unwrap(), [true]);
                assert_eq!(steps.completed(center), Some(GenerationStage::Features));
                assert_eq!(
                    terrain.snapshot().block(8, 8, 8).unwrap().name,
                    "minecraft:brown_mushroom"
                );
                steps
                    .request(center, GenerationStage::InitializeLight)
                    .unwrap();
                assert!(steps.ready(center, GenerationStage::InitializeLight));
                steps
                    .run_light_initialization(&terrain, center, &mut tracker, &vanilla)
                    .unwrap();
                assert_eq!(
                    steps.completed(center),
                    Some(GenerationStage::InitializeLight)
                );
                assert_eq!(
                    tracker.sky_sources(&terrain, center).unwrap()[8 + 8 * 16],
                    8
                );
                assert!(
                    tracker
                        .calculate_sky_light_for_active_chunks(
                            &terrain,
                            center,
                            &BTreeSet::from([center]),
                            &vanilla
                        )
                        .is_ok()
                );
                for z in -1..=1 {
                    for x in -1..=1 {
                        let pos = ChunkPos { x, z };
                        if pos == center {
                            continue;
                        }
                        terrain.completed_decoration.insert(pos);
                        if pos == registered || pos == second {
                            tracker
                                .initialize_registered_sources(&terrain, pos)
                                .unwrap();
                        } else {
                            tracker.initialize_chunk(&terrain, pos, &vanilla).unwrap();
                        }
                    }
                }
                let sky = tracker
                    .calculate_sky_light_for_active_chunks(
                        &terrain,
                        center,
                        &BTreeSet::from([center]),
                        &vanilla,
                    )
                    .unwrap();
                let index = 8 + 8 * 16 + 10 * 256;
                assert_eq!(sky[&0][index / 2] & 15, 15);
                assert_eq!(
                    tracker
                        .snapshot(&terrain, [center], &vanilla)
                        .unwrap()
                        .sky_light([8, 8, 8]),
                    Some(0)
                );
            } else {
                assert_eq!(result.unwrap(), [true]);
                assert_eq!(steps.completed(center), Some(GenerationStage::Features));
                assert_eq!(
                    terrain.snapshot().block(16, 8, 8).unwrap().name,
                    "minecraft:brown_mushroom"
                );
                assert!(tracker.snapshot(&terrain, [center], &vanilla).is_ok());
            }
            let world = Arc::make_mut(&mut terrain.world);
            let air = world.intern_state(state("air"));
            world.set_block_id(16, 0, 0, air).unwrap();
            assert!(tracker.snapshot(&terrain, [center], &vanilla).is_err());
            if x == 8 {
                assert!(tracker.sky_sources(&terrain, center).is_err());
            }
        }
        // A neighboring initialized chunk may already emit while the target
        // is still at FEATURES. Compare the same placement before and after
        // the source is activated, and preserve the published light after
        // edits to a chunk whose sources have not been initialized.
        for active in [false, true] {
            let schedule = FeatureSchedule::load(
                &mut MushroomAt(15),
                std::slice::from_ref(&plains),
                &BTreeSet::new(),
            )
            .unwrap();
            let (_, _, neighborhood) = scheduled_fixture(None);
            let plan = fixture_plan(&schedule, &neighborhood);
            let mut registry = batch_registry();
            registry.extend(["minecraft:brown_mushroom".into(), "minecraft:torch".into()]);
            let prepared =
                PreparedDecoration::load(&mut MushroomAt(15), &plan, &registry, &mut resolve_test)
                    .unwrap();
            let (mut terrain, _, _, _) = fixture();
            let center = ChunkPos { x: 0, z: 0 };
            let east = ChunkPos { x: 1, z: 0 };
            let west = ChunkPos { x: -1, z: 0 };
            let world = Arc::make_mut(&mut terrain.world);
            let template = world.chunks[&center].as_ref().clone();
            for z in -3..=3 {
                for x in -3..=3 {
                    let pos = ChunkPos { x, z };
                    world.chunks.entry(pos).or_insert_with(|| {
                        let mut chunk = template.clone();
                        chunk.position = pos;
                        Arc::new(chunk)
                    });
                }
            }
            let torch = world.intern_state(state("torch"));
            world.set_block_id(16, 8, 8, torch).unwrap();
            terrain.completed_decoration.insert(east);
            terrain.completed_decoration.insert(west);
            let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
            tracker.initialize_chunk(&terrain, east, &vanilla).unwrap();
            tracker.initialize_chunk(&terrain, west, &vanilla).unwrap();
            let mut engine = super::super::StagedGenerationLightEngine::new(tracker);
            if active {
                engine.activate(&terrain, east, [center], &vanilla).unwrap();
            }
            let mut steps = super::super::schedule::GenerationSchedule::new();
            use super::super::schedule::GenerationStage;
            steps.request(center, GenerationStage::Features).unwrap();
            while !steps.ready(center, GenerationStage::Features) {
                let (pos, stage) = steps.next_ready().unwrap();
                steps.complete(pos, stage).unwrap();
            }
            let result = steps
                .run_features_with_staged_light(
                    &mut terrain,
                    &prepared,
                    false,
                    &mut engine,
                    &vanilla,
                )
                .unwrap();
            assert_eq!(result, [!active]);
            assert_eq!(steps.completed(center), Some(GenerationStage::Features));
            if active {
                let old = engine.snapshot(&terrain, [center], &vanilla).unwrap();
                assert_eq!(old.block_light([15, 8, 8]), Some(13));
                let world = Arc::make_mut(&mut terrain.world);
                let stone = world.intern_state(state("stone"));
                world.set_block_id(15, 8, 8, stone).unwrap();
                let preserved = engine.snapshot(&terrain, [center], &vanilla).unwrap();
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [x, y, z];
                            assert_eq!(preserved.block_light(p), old.block_light(p), "block {p:?}");
                            assert_eq!(preserved.sky_light(p), old.sky_light(p), "sky {p:?}");
                        }
                    }
                }
                assert!(engine.activate(&terrain, west, [center], &vanilla).is_err());
                assert_eq!(
                    engine
                        .snapshot(&terrain, [center], &vanilla)
                        .unwrap()
                        .block_light([15, 8, 8]),
                    Some(13)
                );
            }
        }
        // A staged feature must roll back both a registered-chunk write and
        // an unregistered write in an active light input neighborhood.
        for x in [8, 16] {
            let schedule = FeatureSchedule::load(
                &mut MushroomAt(x),
                std::slice::from_ref(&plains),
                &BTreeSet::new(),
            )
            .unwrap();
            let (_, _, neighborhood) = scheduled_fixture(None);
            let plan = fixture_plan(&schedule, &neighborhood);
            let mut registry = batch_registry();
            registry.insert("minecraft:brown_mushroom".into());
            let prepared =
                PreparedDecoration::load(&mut MushroomAt(x), &plan, &registry, &mut resolve_test)
                    .unwrap();
            let (mut terrain, _, _, _) = fixture();
            let center = ChunkPos { x: 0, z: 0 };
            let east = ChunkPos { x: 1, z: 0 };
            let world = Arc::make_mut(&mut terrain.world);
            let template = world.chunks[&center].as_ref().clone();
            for z in -2..=2 {
                for x in -2..=2 {
                    let pos = ChunkPos { x, z };
                    world.chunks.entry(pos).or_insert_with(|| {
                        let mut chunk = template.clone();
                        chunk.position = pos;
                        Arc::new(chunk)
                    });
                }
            }
            terrain.completed_decoration.insert(east);
            let mut tracker = FreshGenerationLightTracker::new(&terrain, false).unwrap();
            tracker.initialize_chunk(&terrain, east, &vanilla).unwrap();
            let mut engine = super::super::StagedGenerationLightEngine::new(tracker);
            engine.activate(&terrain, east, [center], &vanilla).unwrap();
            let before = terrain.snapshot();
            let error = prepared
                .execute_with_staged_light(&mut terrain, false, &mut engine, &vanilla)
                .unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.contains(if x == 8 {
                    "active light neighborhood changed"
                } else {
                    "registered light behavior changed"
                }),
                "{message}"
            );
            assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
            assert!(!terrain.completed_decoration.contains(&center));
            assert!(engine.snapshot(&terrain, [center], &vanilla).is_ok());
        }
        let (mut invalid, _, _, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        invalid.completed_decoration.insert(center);
        let world = Arc::make_mut(&mut invalid.world);
        let unknown = world.intern_state(BlockState {
            name: "example:unknown".into(),
            properties: BTreeMap::new(),
        });
        world.set_block_id(0, 15, 0, unknown).unwrap();
        let before = invalid.snapshot();
        let mut failed_tracker = FreshGenerationLightTracker::new(&invalid, true).unwrap();
        let mut failed_steps = super::super::schedule::GenerationSchedule::new();
        use super::super::schedule::GenerationStage;
        failed_steps
            .request(center, GenerationStage::InitializeLight)
            .unwrap();
        while !failed_steps.ready(center, GenerationStage::InitializeLight) {
            let (pos, stage) = failed_steps.next_ready().unwrap();
            failed_steps.complete(pos, stage).unwrap();
        }
        assert!(
            failed_steps
                .run_light_initialization(&invalid, center, &mut failed_tracker, &vanilla)
                .is_err()
        );
        assert_eq!(
            failed_steps.completed(center),
            Some(GenerationStage::Features)
        );
        assert!(Arc::ptr_eq(&before, &invalid.snapshot()));
        assert!(failed_tracker.sky_sources(&invalid, center).is_err());
        assert_eq!(
            failed_tracker
                .snapshot(&invalid, [center], &vanilla)
                .unwrap()
                .sky_light([0, 0, 0]),
            Some(15)
        );
        let (mut terrain, _, _, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        terrain.completed_decoration.insert(center);
        let mut tracker = FreshGenerationLightTracker::new(&terrain, false).unwrap();
        tracker
            .initialize_chunk(&terrain, center, &vanilla)
            .unwrap();
        assert_eq!(tracker.sky_sources(&terrain, center).unwrap()[0], 8);
        assert!(
            tracker
                .calculate_sky_light_for_active_chunks(&terrain, center, &BTreeSet::new(), &vanilla)
                .is_err()
        );
        let cached = tracker.snapshot(&terrain, [center], &vanilla).unwrap();
        let direct = terrain
            .fresh_generation_light([center], [center], &vanilla, false)
            .unwrap();
        for y in [-32, 0, 8, 16, 32, 64] {
            assert_eq!(
                cached.raw_brightness([0, y, 0], 0),
                direct.raw_brightness([0, y, 0], 0)
            );
        }
        let (mut sparse, _, _, _) = fixture();
        sparse.height = 96;
        let world = Arc::make_mut(&mut sparse.world);
        let air = world.intern_state(state("air"));
        for chunk in world.chunks.values_mut() {
            let chunk = Arc::make_mut(chunk);
            for y in 1..6 {
                chunk.sections.insert(
                    y,
                    Arc::new(Section {
                        blocks: Some(Box::new([air; BLOCK_COUNT])),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                );
            }
        }
        let mut positions = Vec::new();
        for z in -1..=1 {
            for x in -1..=1 {
                let p = ChunkPos { x, z };
                sparse.completed_decoration.insert(p);
                positions.push(p);
            }
        }
        let mut tracker = FreshGenerationLightTracker::new(&sparse, true).unwrap();
        for pos in positions {
            tracker.initialize_chunk(&sparse, pos, &vanilla).unwrap();
        }
        let dark = tracker
            .calculate_sky_light_for_active_chunks(&sparse, center, &BTreeSet::new(), &vanilla)
            .unwrap();
        let lit = tracker
            .calculate_sky_light_for_active_chunks(
                &sparse,
                center,
                &BTreeSet::from([center]),
                &vanilla,
            )
            .unwrap();
        let index = 8 + 8 * 16 + 10 * 256;
        assert_eq!(dark[&0][index / 2] & 15, 0);
        assert_eq!(lit[&0][index / 2] & 15, 15);
        assert_eq!(dark[&5][index / 2] & 15, 15);
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn drained_glowing_vine_lighting_matches_java_across_chunk_halo() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let path = temp.path().join("vine_full.json");
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StagedGenerationLightEngineOracle")
            .arg(&path)
            .arg("vine_full"));
        let expected: Vec<Vec<Vec<Vec<u8>>>> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        let (mut terrain, _, _, _) = fixture();
        let east = ChunkPos { x: 1, z: 0 };
        let world = Arc::make_mut(&mut terrain.world);
        let template = world.chunks[&ChunkPos { x: 0, z: 0 }].as_ref().clone();
        for z in -2..=2 {
            for x in -2..=3 {
                let pos = ChunkPos { x, z };
                world.chunks.entry(pos).or_insert_with(|| {
                    let mut chunk = template.clone();
                    chunk.position = pos;
                    Arc::new(chunk)
                });
            }
        }
        let torch = world.intern_state(state("torch"));
        world.set_block_id(17, 10, 8, torch).unwrap();
        terrain.completed_decoration.insert(east);
        let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
        tracker.initialize_chunk(&terrain, east, &registry).unwrap();
        struct GlowingVineData;
        impl ResourceProvider for GlowingVineData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let vine = json!({"feature":{"type":"minecraft:block_column","direction":"down",
                        "allowed_placement":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"},
                        "prioritize_tip":true,"layers":[{"height":1,
                        "provider":{"id":"minecraft:cave_vines_plant","properties":{"berries":"true"}}}]},
                        "placement":[{"type":"minecraft:offset","x":16,"y":10,"z":8}]});
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[vine]]}))?));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule =
            FeatureSchedule::load(&mut GlowingVineData, &[plains], &BTreeSet::new()).unwrap();
        let (_, _, biomes) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &biomes);
        let mut blocks = batch_registry();
        blocks.extend([
            "minecraft:cave_vines_plant".into(),
            "minecraft:torch".into(),
        ]);
        let prepared =
            PreparedDecoration::load(&mut GlowingVineData, &plan, &blocks, &mut resolve_tree_test)
                .unwrap();
        assert_eq!(
            prepared
                .execute_with_tracked_light(&mut terrain, false, &mut tracker, &registry)
                .unwrap(),
            [true]
        );
        assert_eq!(
            terrain.snapshot().block(16, 10, 8).unwrap().name,
            "minecraft:cave_vines_plant"
        );
        let covered: Vec<_> = (-1..=1)
            .flat_map(|z| (0..=2).map(move |x| ChunkPos { x, z }))
            .collect();
        let dark = tracker
            .snapshot(&terrain, covered.iter().copied(), &registry)
            .unwrap();
        let mut engine = StagedGenerationLightEngine::new(tracker);
        let active = engine
            .activate(&terrain, east, covered.iter().copied(), &registry)
            .unwrap();
        for (stage, actual) in [&dark, &dark, &active].into_iter().enumerate() {
            for (chunk_index, chunk) in covered.iter().enumerate() {
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let pos = [chunk.x * 16 + x, y, chunk.z * 16 + z];
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                actual.block_light(pos),
                                Some(expected[stage][chunk_index][0][index]),
                                "block {stage} {pos:?}"
                            );
                            assert_eq!(
                                actual.sky_light(pos),
                                Some(expected[stage][chunk_index][1][index]),
                                "sky {stage} {pos:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn tracked_light_accepts_equivalent_ore_and_rejects_other_writes() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("StagedGenerationLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StagedGenerationLightEngineOracle.java"),
        )
        .unwrap();
        let compilation = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compilation.status.success(),
            "{}",
            String::from_utf8_lossy(&compilation.stderr)
        );
        let expected_path = temp.path().join("ore.json");
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StagedGenerationLightEngineOracle")
            .arg(&expected_path)
            .arg("ore")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Vec<Vec<Vec<u8>>> =
            serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StagedGenerationLightEngineOracle")
            .arg(&expected_path)
            .arg("ore_active")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let active_expected: Vec<Vec<Vec<u8>>> =
            serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_generated_block_registry().unwrap();
        let (mut data, schedule, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut blocks = batch_registry();
        blocks.insert("minecraft:torch".into());
        let prepared =
            PreparedDecoration::load(&mut data, &plan, &blocks, &mut resolve_test).unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let east = ChunkPos { x: 1, z: 0 };
        let center = ChunkPos { x: 0, z: 0 };
        let world = Arc::make_mut(&mut terrain.world);
        let torch = world.intern_state(state("torch"));
        world.set_block_id(17, 10, 8, torch).unwrap();
        terrain.completed_decoration.insert(east);
        let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
        tracker.initialize_chunk(&terrain, east, &registry).unwrap();
        let before = terrain.snapshot();
        let sky = tracker.sky_sources(&terrain, east).unwrap();
        prepared
            .execute_with_tracked_light(&mut terrain, false, &mut tracker, &registry)
            .unwrap();
        let after = terrain.snapshot();
        assert!((16..32).any(|x| (0..16).any(|z| (0..8).any(|y| {
            before.block(x, y, z) != after.block(x, y, z)
                && after.block(x, y, z).unwrap().name == "minecraft:diamond_ore"
        }))));
        assert_eq!(&*sky, &*tracker.sky_sources(&terrain, east).unwrap());
        assert!(tracker.snapshot(&terrain, [east], &registry).is_ok());
        assert!(tracker.checked_blocks.contains("minecraft:diamond_ore"));
        let before_light = tracker.snapshot(&terrain, [center], &registry).unwrap();
        let mut engine = StagedGenerationLightEngine::new(tracker);
        let active = engine
            .activate(&terrain, east, [center], &registry)
            .unwrap();
        for (stage, actual) in [&before_light, &active].into_iter().enumerate() {
            for y in -16..32 {
                for z in 0..16 {
                    for x in 0..16 {
                        let pos = [x, y, z];
                        let index = ((y + 16) * 256 + z * 16 + x) as usize;
                        assert_eq!(
                            actual.block_light(pos),
                            Some(expected[stage][0][index]),
                            "block {stage} {pos:?}"
                        );
                        assert_eq!(
                            actual.sky_light(pos),
                            Some(expected[stage][1][index]),
                            "sky {stage} {pos:?}"
                        );
                    }
                }
            }
        }
        let mut deep = state("deepslate");
        deep.properties.insert("axis".into(), "y".into());
        assert_eq!(
            native_world::block_light::same_light_behavior(&deep, &state("tuff")),
            Some(true)
        );
        for facing in ["north", "south", "east", "west"] {
            for segments in 1..=4 {
                let mut litter = state("leaf_litter");
                litter.properties = BTreeMap::from([
                    ("facing".into(), facing.into()),
                    ("segment_amount".into(), segments.to_string()),
                ]);
                assert_eq!(
                    native_world::block_light::same_light_behavior(&state("air"), &litter),
                    Some(true)
                );
            }
        }
        let (mut staged_terrain, _, _, _) = fixture();
        let world = Arc::make_mut(&mut staged_terrain.world);
        let template = world.chunks[&center].as_ref().clone();
        for z in -2..=2 {
            for x in -2..=2 {
                let pos = ChunkPos { x, z };
                world.chunks.entry(pos).or_insert_with(|| {
                    let mut chunk = template.clone();
                    chunk.position = pos;
                    Arc::new(chunk)
                });
            }
        }
        let torch = world.intern_state(state("torch"));
        world.set_block_id(17, 10, 8, torch).unwrap();
        staged_terrain.completed_decoration.insert(east);
        let mut tracker = FreshGenerationLightTracker::new(&staged_terrain, true).unwrap();
        tracker
            .initialize_chunk(&staged_terrain, east, &registry)
            .unwrap();
        let mut staged = StagedGenerationLightEngine::new(tracker);
        let first = staged
            .activate(&staged_terrain, east, [center], &registry)
            .unwrap();
        let before = staged_terrain.snapshot();
        prepared
            .execute_with_staged_light(&mut staged_terrain, false, &mut staged, &registry)
            .unwrap();
        let after = staged_terrain.snapshot();
        assert!((16..32).any(|x| (0..16).any(|z| {
            (0..8).any(|y| {
                before.block(x, y, z) != after.block(x, y, z)
                    && after.block(x, y, z).unwrap().name == "minecraft:diamond_ore"
            })
        })));
        let second = staged
            .snapshot(&staged_terrain, [center], &registry)
            .unwrap();
        for (stage, actual) in [&first, &second].into_iter().enumerate() {
            for y in -16..32 {
                for z in 0..16 {
                    for x in 0..16 {
                        let pos = [x, y, z];
                        let index = ((y + 16) * 256 + z * 16 + x) as usize;
                        assert_eq!(
                            actual.block_light(pos),
                            Some(active_expected[stage][0][index]),
                            "block {stage} {pos:?}"
                        );
                        assert_eq!(
                            actual.sky_light(pos),
                            Some(active_expected[stage][1][index]),
                            "sky {stage} {pos:?}"
                        );
                    }
                }
            }
        }
        {
            struct VineData;
            impl ResourceProvider for VineData {
                fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                    Ok(None)
                }
                fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                    if path == "data/minecraft/worldgen/biome/plains.json" {
                        let vine = json!({"feature":{"type":"minecraft:block_column","direction":"down",
                            "allowed_placement":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"},
                            "prioritize_tip":true,"layers":[{"height":1,
                            "provider":{"id":"minecraft:cave_vines_plant","properties":{"berries":"false"}}}]},
                            "placement":[{"type":"minecraft:offset","x":16,"y":10,"z":8}]});
                        return Ok(Some(serde_json::to_vec(&json!({"features":[[vine]]}))?));
                    }
                    Data.read_data(path)
                }
            }
            let plains = ResourceId::parse("minecraft:plains").unwrap();
            let schedule =
                FeatureSchedule::load(&mut VineData, &[plains], &BTreeSet::new()).unwrap();
            let (_, _, biomes) = scheduled_fixture(None);
            let plan = fixture_plan(&schedule, &biomes);
            let mut blocks = batch_registry();
            blocks.extend([
                "minecraft:cave_vines_plant".into(),
                "minecraft:torch".into(),
            ]);
            let prepared =
                PreparedDecoration::load(&mut VineData, &plan, &blocks, &mut resolve_tree_test)
                    .unwrap();
            let (mut terrain, vine_tags, _, _) = fixture();
            let vine_motion = HeightmapTags::load(&mut Data, &blocks).unwrap();
            let world = Arc::make_mut(&mut terrain.world);
            let torch = world.intern_state(state("torch"));
            world.set_block_id(17, 10, 8, torch).unwrap();
            terrain.completed_decoration.insert(east);
            let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
            tracker.initialize_chunk(&terrain, east, &registry).unwrap();
            let sources = tracker.sky_sources(&terrain, east).unwrap();
            let before = terrain.snapshot();
            {
                let mut region = terrain
                    .decoration_region(&vine_tags, &vine_motion, BTreeSet::from([east]))
                    .unwrap();
                region.light_guard = Some(DecorationLightGuard {
                    registered: BTreeSet::from([east]),
                    registry: &registry,
                    require_unchanged_light: false,
                    changed_blocks: BTreeSet::new(),
                    changed_chunks: BTreeSet::new(),
                    light_replay_positions: BTreeSet::new(),
                    light_check_order: Vec::new(),
                    vine_light_positions: BTreeSet::new(),
                    new_nonempty_sections: Vec::new(),
                    non_air_counts: BTreeMap::new(),
                });
                let mut glowing = state("cave_vines_plant");
                glowing.properties.insert("berries".into(), "true".into());
                region.set_section_block([16, 10, 8], &glowing).unwrap();
            }
            assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
            assert_eq!(
                prepared
                    .execute_with_tracked_light(&mut terrain, false, &mut tracker, &registry)
                    .unwrap(),
                [true]
            );
            assert_eq!(
                terrain.snapshot().block(16, 10, 8).unwrap().name,
                "minecraft:cave_vines_plant"
            );
            assert_eq!(&*sources, &*tracker.sky_sources(&terrain, east).unwrap());
            let before = terrain.snapshot();
            {
                let mut region = terrain
                    .decoration_region(&vine_tags, &vine_motion, BTreeSet::from([east]))
                    .unwrap();
                region.light_guard = Some(DecorationLightGuard {
                    registered: BTreeSet::from([east]),
                    registry: &registry,
                    require_unchanged_light: false,
                    changed_blocks: BTreeSet::new(),
                    changed_chunks: BTreeSet::new(),
                    light_replay_positions: BTreeSet::new(),
                    light_check_order: Vec::new(),
                    vine_light_positions: BTreeSet::new(),
                    new_nonempty_sections: Vec::new(),
                    non_air_counts: BTreeMap::new(),
                });
                let mut glowing = state("cave_vines_plant");
                glowing.properties.insert("berries".into(), "true".into());
                region.set_section_block([16, 10, 8], &glowing).unwrap();
            }
            assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
            let mut oracle = Vec::new();
            for mode in ["vine_plain", "vine_plain_queued"] {
                let path = temp.path().join(format!("{mode}.json"));
                let output = Command::new(java.join("java.exe"))
                    .arg("-classpath")
                    .arg(&cp)
                    .arg("StagedGenerationLightEngineOracle")
                    .arg(&path)
                    .arg(mode)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                oracle.push(
                    serde_json::from_slice::<Vec<Vec<Vec<u8>>>>(&std::fs::read(path).unwrap())
                        .unwrap(),
                );
            }
            assert!(oracle[0] == oracle[1], "plain vine light checks differed");
            let mut glowing_oracle = Vec::new();
            for mode in ["vine", "vine_queued"] {
                let path = temp.path().join(format!("{mode}.json"));
                let output = Command::new(java.join("java.exe"))
                    .arg("-classpath")
                    .arg(&cp)
                    .arg("StagedGenerationLightEngineOracle")
                    .arg(&path)
                    .arg(mode)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                glowing_oracle.push(
                    serde_json::from_slice::<Vec<Vec<Vec<u8>>>>(&std::fs::read(path).unwrap())
                        .unwrap(),
                );
            }
            assert!(
                glowing_oracle[0][2][1] != glowing_oracle[1][2][1],
                "expected sky light to depend on check ordering"
            );
            let mut existing_oracle = Vec::new();
            for mode in ["vine_existing", "vine_existing_queued"] {
                let path = temp.path().join(format!("{mode}.json"));
                let output = Command::new(java.join("java.exe"))
                    .arg("-classpath")
                    .arg(&cp)
                    .arg("StagedGenerationLightEngineOracle")
                    .arg(&path)
                    .arg(mode)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                existing_oracle.push(
                    serde_json::from_slice::<Vec<Vec<Vec<u8>>>>(&std::fs::read(path).unwrap())
                        .unwrap(),
                );
            }
            assert!(
                existing_oracle[0][2][1] != existing_oracle[1][2][1],
                "expected the existing vine to retain check ordering"
            );
            {
                let (mut probe, _, _, _) = fixture();
                let world = Arc::make_mut(&mut probe.world);
                let torch = world.intern_state(state("torch"));
                world.set_block_id(17, 10, 8, torch).unwrap();
                probe.completed_decoration.insert(east);
                let mut tracker = FreshGenerationLightTracker::new(&probe, true).unwrap();
                tracker.initialize_chunk(&probe, east, &registry).unwrap();
                let world = Arc::make_mut(&mut probe.world);
                let mut glowing = state("cave_vines_plant");
                glowing.properties.insert("berries".into(), "true".into());
                let glowing = world.intern_state(glowing);
                world.set_block_id(16, 10, 8, glowing).unwrap();
                tracker
                    .registered
                    .insert(east, Arc::clone(&probe.world.chunks[&east]));
                let mut engine = StagedGenerationLightEngine::new(tracker);
                let active = engine.activate(&probe, east, [center], &registry).unwrap();
                let mut mismatches = [[0usize; 2]; 2];
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let pos = [x, y, z];
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            for mode in 0..2 {
                                mismatches[mode][0] += usize::from(
                                    active.block_light(pos)
                                        != Some(glowing_oracle[mode][2][0][index]),
                                );
                                mismatches[mode][1] += usize::from(
                                    active.sky_light(pos)
                                        != Some(glowing_oracle[mode][2][1][index]),
                                );
                            }
                        }
                    }
                }
                assert_eq!(mismatches[0], [0, 0], "drained vine light");
                assert_eq!(mismatches[1][0], 0, "queued vine block light");
                assert!(mismatches[1][1] > 0, "queued vine sky light");
            }
            let dark = tracker.snapshot(&terrain, [center], &registry).unwrap();
            let mut engine = StagedGenerationLightEngine::new(tracker);
            let active = engine
                .activate(&terrain, east, [center], &registry)
                .unwrap();
            for (stage, actual) in [&dark, &dark, &active].into_iter().enumerate() {
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let pos = [x, y, z];
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                actual.block_light(pos),
                                Some(oracle[0][stage][0][index]),
                                "vine block {stage} {pos:?}"
                            );
                            assert_eq!(
                                actual.sky_light(pos),
                                Some(oracle[0][stage][1][index]),
                                "vine sky {stage} {pos:?}"
                            );
                        }
                    }
                }
            }
        }
        {
            struct MagmaData;
            impl ResourceProvider for MagmaData {
                fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                    Ok(None)
                }
                fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                    if path == "data/minecraft/worldgen/biome/plains.json" {
                        let magma = json!({"feature":{"type":"minecraft:underwater_magma", "floor_search_range":4,
                        "placement_radius_around_floor":0,"placement_probability_per_valid_position":1.0},
                        "placement":[{"type":"minecraft:offset","x":16,"y":8,"z":8}]});
                        return Ok(Some(serde_json::to_vec(&json!({"features":[[magma]]}))?));
                    }
                    Data.read_data(path)
                }
            }
            let plains = ResourceId::parse("minecraft:plains").unwrap();
            let schedule =
                FeatureSchedule::load(&mut MagmaData, &[plains], &BTreeSet::new()).unwrap();
            let (_, _, biomes) = scheduled_fixture(None);
            let plan = fixture_plan(&schedule, &biomes);
            let mut blocks = batch_registry();
            blocks.extend(["minecraft:water".into(), "minecraft:magma_block".into()]);
            let prepared =
                PreparedDecoration::load(&mut MagmaData, &plan, &blocks, &mut resolve_test)
                    .unwrap();
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            let mut water = state("water");
            water.properties.insert("level".into(), "0".into());
            let water = world.intern_state(water);
            world.set_block_id(16, 8, 8, water).unwrap();
            terrain.completed_decoration.insert(east);
            let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
            tracker.initialize_chunk(&terrain, east, &registry).unwrap();
            let sources = tracker.sky_sources(&terrain, east).unwrap();
            assert_eq!(
                prepared
                    .execute_with_tracked_light(&mut terrain, false, &mut tracker, &registry)
                    .unwrap(),
                [true]
            );
            assert_eq!(
                terrain.snapshot().block(16, 7, 8).unwrap().name,
                "minecraft:magma_block"
            );
            assert_eq!(&*sources, &*tracker.sky_sources(&terrain, east).unwrap());
            let expected_path = temp.path().join("magma.json");
            let output = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&expected_path)
                .arg("magma")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let expected: Vec<Vec<Vec<u8>>> =
                serde_json::from_slice(&std::fs::read(expected_path).unwrap()).unwrap();
            let queued_path = temp.path().join("magma_queued.json");
            let queued = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("StagedGenerationLightEngineOracle")
                .arg(&queued_path)
                .arg("magma_queued")
                .output()
                .unwrap();
            assert!(
                queued.status.success(),
                "{}",
                String::from_utf8_lossy(&queued.stderr)
            );
            let queued: Vec<Vec<Vec<u8>>> =
                serde_json::from_slice(&std::fs::read(queued_path).unwrap()).unwrap();
            assert_eq!(queued, expected, "queued and drained pre-activation checks");
            let mut exposed = Vec::new();
            for mode in ["magma_exposed", "magma_exposed_queued"] {
                let path = temp.path().join(format!("{mode}.json"));
                let output = Command::new(java.join("java.exe"))
                    .arg("-classpath")
                    .arg(&cp)
                    .arg("StagedGenerationLightEngineOracle")
                    .arg(&path)
                    .arg(mode)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                exposed.push(
                    serde_json::from_slice::<Vec<Vec<Vec<u8>>>>(&std::fs::read(path).unwrap())
                        .unwrap(),
                );
            }
            assert!(
                exposed[0] == exposed[1],
                "exposed magma light checks differed"
            );
            let dark = tracker.snapshot(&terrain, [center], &registry).unwrap();
            let mut engine = StagedGenerationLightEngine::new(tracker);
            let active = engine
                .activate(&terrain, east, [center], &registry)
                .unwrap();
            for (stage, actual) in [&dark, &dark, &active].into_iter().enumerate() {
                for y in -16..32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let pos = [x, y, z];
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                actual.block_light(pos),
                                Some(expected[stage][0][index]),
                                "magma block {stage} {pos:?}"
                            );
                            assert_eq!(
                                actual.sky_light(pos),
                                Some(expected[stage][1][index]),
                                "magma sky {stage} {pos:?}"
                            );
                        }
                    }
                }
            }
        }
        let tracker = &engine.fresh;

        // A section status update is required when its first non-air block is
        // added or its last is removed, even when the light shapes match.
        for initial in [0, 2] {
            let (mut sparse, tags, _, _) = fixture();
            let mut block_ids = batch_registry();
            block_ids.insert("minecraft:cave_vines_plant".into());
            let motion = HeightmapTags::load(&mut Data, &block_ids).unwrap();
            let world = Arc::make_mut(&mut sparse.world);
            let air = world.intern_state(state("air"));
            let mut plain_vine = state("cave_vines_plant");
            plain_vine
                .properties
                .insert("berries".into(), "false".into());
            let vine = world.intern_state(plain_vine.clone());
            let section = Arc::make_mut(
                Arc::make_mut(world.chunks.get_mut(&east).unwrap())
                    .sections
                    .get_mut(&0)
                    .unwrap(),
            );
            let data = section.blocks.as_mut().unwrap();
            data.fill(air);
            for i in 0..initial {
                data[i] = vine;
            }
            let before = sparse.snapshot();
            let mut region = sparse
                .decoration_region(&tags, &motion, BTreeSet::from([east]))
                .unwrap();
            region.light_guard = Some(DecorationLightGuard {
                registered: BTreeSet::from([east]),
                registry: &registry,
                require_unchanged_light: false,
                changed_blocks: BTreeSet::new(),
                changed_chunks: BTreeSet::new(),
                light_replay_positions: BTreeSet::new(),
                light_check_order: Vec::new(),
                vine_light_positions: BTreeSet::new(),
                new_nonempty_sections: Vec::new(),
                non_air_counts: BTreeMap::new(),
            });
            if initial == 0 {
                assert!(region.set_section_block([16, 0, 0], &plain_vine).is_err());
            } else {
                region.set_section_block([16, 0, 0], &state("air")).unwrap();
                assert!(region.set_section_block([17, 0, 0], &state("air")).is_err());
            }
            drop(region);
            assert!(Arc::ptr_eq(&before, &sparse.snapshot()));
        }

        for replacement in ["air", "torch"] {
            let before = terrain.snapshot();
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([east]))
                .unwrap();
            region.light_guard = Some(DecorationLightGuard {
                registered: BTreeSet::from([east]),
                registry: &registry,
                require_unchanged_light: false,
                changed_blocks: BTreeSet::new(),
                changed_chunks: BTreeSet::new(),
                light_replay_positions: BTreeSet::new(),
                light_check_order: Vec::new(),
                vine_light_positions: BTreeSet::new(),
                new_nonempty_sections: Vec::new(),
                non_air_counts: BTreeMap::new(),
            });
            let initial = region.read([16, 0, 0]).unwrap();
            let valid = if initial.name == "minecraft:stone" {
                state("diamond_ore")
            } else {
                state("stone")
            };
            region.set_section_block([16, 0, 0], &valid).unwrap();
            let old = region.read([16, 0, 0]).unwrap();
            let replacement = if old.name == format!("minecraft:{replacement}") {
                state("stone")
            } else {
                state(replacement)
            };
            assert!(region.set_section_block([16, 0, 0], &replacement).is_err());
            drop(region);
            assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
            assert!(tracker.snapshot(&terrain, [east], &registry).is_ok());
        }

        struct SandDisk;
        impl ResourceProvider for SandDisk {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let disk = json!({"feature":{"type":"minecraft:disk",
                        "target":{"type":"minecraft:matching_blocks","blocks":"minecraft:stone"},
                        "state_provider":{"id":"minecraft:sand"},"radius":1,"half_height":1},
                        "placement":[{"type":"minecraft:offset","x":16,"y":4,"z":8}]});
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[disk]]}))?));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(&mut SandDisk, &[plains], &BTreeSet::new()).unwrap();
        let (_, _, biomes) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &biomes);
        let mut blocks = batch_registry();
        blocks.extend(["minecraft:sand".into(), "minecraft:torch".into()]);
        let prepared =
            PreparedDecoration::load(&mut SandDisk, &plan, &blocks, &mut resolve_test).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let torch = world.intern_state(state("torch"));
        world.set_block_id(17, 10, 8, torch).unwrap();
        terrain.completed_decoration.insert(east);
        let mut tracker = FreshGenerationLightTracker::new(&terrain, true).unwrap();
        tracker.initialize_chunk(&terrain, east, &registry).unwrap();
        let sources = tracker.sky_sources(&terrain, east).unwrap();
        prepared
            .execute_with_tracked_light(&mut terrain, false, &mut tracker, &registry)
            .unwrap();
        assert_eq!(
            terrain.snapshot().block(17, 4, 8).unwrap().name,
            "minecraft:sand"
        );
        assert_eq!(&*sources, &*tracker.sky_sources(&terrain, east).unwrap());
        let expected_path = temp.path().join("normal.json");
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StagedGenerationLightEngineOracle")
            .arg(&expected_path)
            .arg("normal")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Vec<Vec<Vec<u8>>> =
            serde_json::from_slice(&std::fs::read(expected_path).unwrap()).unwrap();
        let before_light = tracker.snapshot(&terrain, [center], &registry).unwrap();
        let mut engine = StagedGenerationLightEngine::new(tracker);
        let active = engine
            .activate(&terrain, east, [center], &registry)
            .unwrap();
        for (stage, actual) in [&before_light, &active].into_iter().enumerate() {
            for y in -16..32 {
                for z in 0..16 {
                    for x in 0..16 {
                        let pos = [x, y, z];
                        let index = ((y + 16) * 256 + z * 16 + x) as usize;
                        assert_eq!(
                            actual.block_light(pos),
                            Some(expected[stage][0][index]),
                            "normal block {stage} {pos:?}"
                        );
                        assert_eq!(
                            actual.sky_light(pos),
                            Some(expected[stage][1][index]),
                            "normal sky {stage} {pos:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn tree_mushroom_edge_updates_match_java() {
        use native_resources::tree_leaf_update::ShapeEdgeWorld;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("TreeEdgeMushroomOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/TreeEdgeMushroomOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreeEdgeMushroomOracle")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout).unwrap();
        let (mut terrain, tags, _, _) = fixture();
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let names = [
            "red_mushroom_block",
            "brown_mushroom_block",
            "mushroom_stem",
        ];
        let faces = ["down", "up", "north", "south", "west", "east"];
        let directions = [
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
            [-1, 0, 0],
            [1, 0, 0],
        ];
        let mut supports = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("S ")) {
            let values: Vec<_> = line.split_whitespace().collect();
            assert_eq!(values.len(), 5);
            let kind: usize = values[0].parse().unwrap();
            let mask: usize = values[1].parse().unwrap();
            let direction: usize = values[2].parse().unwrap();
            let mut block = state(names[kind]);
            for (bit, face) in faces.iter().enumerate() {
                block
                    .properties
                    .insert((*face).into(), ((mask & (1 << bit)) != 0).to_string());
            }
            assert_eq!(
                DiskWorld::face_sturdy(&mut region, &block, [8, 8, 8], directions[direction])
                    .unwrap(),
                values[3].parse::<bool>().unwrap(),
                "{line}"
            );
            assert_eq!(
                tree_solid_render(&block).unwrap(),
                values[4].parse::<bool>().unwrap(),
                "{line}"
            );
            supports += 1;
        }
        assert_eq!(supports, 3 * 64 * 6);
        for name in names {
            let mut block = state(name);
            block.properties = faces
                .iter()
                .map(|face| ((*face).into(), "false".into()))
                .collect();
            region.set_section_block([8, 9, 8], &block).unwrap();
            for face in faces {
                let predicate = BlockPredicate::decode(&json!({
                    "type":"minecraft:has_sturdy_face", "offset":[0,1,0], "direction":face
                }))
                .unwrap();
                assert!(
                    predicate.test(&mut region, [8, 8, 8]).unwrap(),
                    "{name} {face}"
                );
            }
        }
        let mut count = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("M ")) {
            let values: Vec<usize> = line
                .split_whitespace()
                .map(|part| part.parse().unwrap())
                .collect();
            assert_eq!(values.len(), 5);
            let [kind, mask, direction, neighbor, expected] =
                <[usize; 5]>::try_from(values).unwrap();
            let mut original = state(names[kind]);
            for (bit, face) in faces.iter().enumerate() {
                original
                    .properties
                    .insert((*face).into(), ((mask & (1 << bit)) != 0).to_string());
            }
            let neighbor_name =
                ["air", "oak_log", names[kind], names[0], names[1], names[2]][neighbor];
            let mut neighbor = state(neighbor_name);
            if names.contains(&neighbor_name) {
                neighbor.properties = faces
                    .iter()
                    .map(|face| ((*face).into(), "true".into()))
                    .collect();
            }
            let actual = region
                .update_shape(
                    [8, 8, 8],
                    &original,
                    directions[direction],
                    [0, 0, 0],
                    &neighbor,
                )
                .unwrap();
            let actual_mask = faces.iter().enumerate().fold(0usize, |value, (bit, face)| {
                value | (usize::from(actual.properties[*face] == "true") << bit)
            });
            assert_eq!(actual_mask, expected, "{line}");
            count += 1;
        }
        assert_eq!(count, 3 * 64 * 6 * 6);
        let mut block = state("red_mushroom_block");
        block.properties = faces
            .iter()
            .map(|face| ((*face).into(), "true".into()))
            .collect();
        region.set_section_block([8, 8, 8], &block).unwrap();
        let updated = region
            .update_shape([8, 8, 8], &block, [1, 0, 0], [9, 8, 8], &block)
            .unwrap();
        ShapeEdgeWorld::set_block(&mut region, [8, 8, 8], &updated, 2).unwrap();
        region.finish().unwrap();
        assert_eq!(
            terrain.snapshot().block(8, 8, 8).unwrap().properties["east"],
            "false"
        );
        let (mut terrain, _, _, _) = fixture();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        region.set_section_block([8, 8, 8], &block).unwrap();
        region.set_section_block([9, 8, 8], &block).unwrap();
        native_resources::tree_leaf_update::update_shape_at_edges(
            &mut region,
            native_resources::tree_leaf_update::TreeBounds {
                min: [8, 8, 8],
                max: [8, 8, 8],
            },
            &BTreeSet::from([[8, 8, 8]]),
            3,
        )
        .unwrap();
        region.finish().unwrap();
        let snapshot = terrain.snapshot();
        assert_eq!(snapshot.block(8, 8, 8).unwrap().properties["east"], "false");
        assert_eq!(snapshot.block(9, 8, 8).unwrap().properties["west"], "false");
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generation_solid_predicate_matches_java_states() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let source = temp.path().join("GenerationSolidPredicateOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GenerationSolidPredicateOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GenerationSolidPredicateOracle")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout).unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let predicate =
            BlockPredicate::decode(&json!({"type":"minecraft:solid","offset":[1,-2,3]})).unwrap();
        let mut count = 0;
        let mut families = BTreeMap::<String, usize>::new();
        for line in output.lines().filter_map(|line| line.strip_prefix("S ")) {
            let parts: Vec<_> = line.split_whitespace().collect();
            assert_eq!(parts.len(), 3, "{line}");
            let mut block = state(parts[0]);
            if parts[1] != "-" {
                for property in parts[1].split(',') {
                    let (key, value) = property.split_once('=').unwrap();
                    block.properties.insert(key.into(), value.into());
                }
            }
            let expected = parts[2].parse::<bool>().unwrap();
            assert_eq!(
                DiskWorld::is_solid(&mut region, &block).unwrap(),
                expected,
                "{line}"
            );
            region.set_section_block([9, 6, 11], &block).unwrap();
            assert_eq!(
                predicate.test(&mut region, [8, 8, 8]).unwrap(),
                expected,
                "{line}"
            );
            count += 1;
            *families.entry(parts[0].to_owned()).or_default() += 1;
        }
        assert!(count >= 300, "missing Java state comparisons: {count}");
        for (name, expected) in [
            ("cave_vines", 52),
            ("cave_vines_plant", 2),
            ("big_dripleaf", 32),
            ("big_dripleaf_stem", 8),
            ("small_dripleaf", 16),
            ("glow_lichen", 128),
            ("coal_ore", 1),
            ("deepslate_coal_ore", 1),
            ("redstone_ore", 2),
            ("deepslate_redstone_ore", 2),
            ("nether_gold_ore", 1),
            ("nether_quartz_ore", 1),
        ] {
            assert_eq!(
                families.get(name),
                Some(&expected),
                "missing Java states for {name}"
            );
        }
        let mut ore_states = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("O ")) {
            let parts: Vec<_> = line.split_whitespace().collect();
            assert_eq!(parts.len(), 3, "{line}");
            let mut block = state(parts[0]);
            if parts[1] != "-" {
                for property in parts[1].split(',') {
                    let (key, value) = property.split_once('=').unwrap();
                    block.properties.insert(key.into(), value.into());
                }
            }
            assert_eq!(
                DiskWorld::is_liquid(&mut region, &block).unwrap(),
                !parts[2].parse::<bool>().unwrap(),
                "{line}"
            );
            ore_states += 1;
        }
        assert_eq!(ore_states, 20);
        let mut invalid = state("coal_ore");
        invalid
            .properties
            .insert("unexpected".into(), "true".into());
        assert!(DiskWorld::is_solid(&mut region, &invalid).is_err());
        assert!(DiskWorld::is_liquid(&mut region, &invalid).is_err());
        assert!(DiskWorld::is_solid(&mut region, &state("redstone_ore")).is_err());
        let directions = [
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
            [-1, 0, 0],
            [1, 0, 0],
        ];
        let mut lichen_states = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("G ")) {
            let parts: Vec<_> = line.split_whitespace().collect();
            assert_eq!(parts.len(), 8, "{line}");
            let mut block = state("glow_lichen");
            for property in parts[0].split(',') {
                let (name, value) = property.split_once('=').unwrap();
                block.properties.insert(name.into(), value.into());
            }
            assert_eq!(
                tree_solid_render(&block).unwrap(),
                parts[1].parse::<bool>().unwrap(),
                "{line}"
            );
            for (direction, expected) in directions.into_iter().zip(&parts[2..]) {
                let expected = expected.parse::<bool>().unwrap();
                assert_eq!(tree_sturdy_up(&block).unwrap(), expected, "{line}");
                assert_eq!(
                    DiskWorld::face_sturdy(&mut region, &block, [8, 8, 8], direction).unwrap(),
                    expected,
                    "{line}"
                );
                assert_eq!(
                    VegetationPatchWorld::patch_face_sturdy(
                        &mut region,
                        &block,
                        [8, 8, 8],
                        direction
                    )
                    .unwrap(),
                    expected,
                    "{line}"
                );
            }
            lichen_states += 1;
        }
        assert_eq!(lichen_states, 128);
        let invalid = state("glow_lichen");
        assert!(tree_sturdy_up(&invalid).is_err());
        assert!(DiskWorld::is_solid(&mut region, &invalid).is_err());
    }

    #[test]
    fn fresh_light_tracker_reports_changed_block_and_preserves_rejection() {
        let (mut terrain, tags, motion, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let mut tracker = FreshGenerationLightTracker::new(&terrain, false).unwrap();
        tracker
            .registered
            .insert(center, Arc::clone(&terrain.world.chunks[&center]));
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        region.set_section_block([8, 5, 8], &state("air")).unwrap();
        let error = tracker
            .validate_world(&region.world)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("registered light chunk has changed: [8, 5, 8]"),
            "{error}"
        );
        assert!(error.contains("minecraft:stone"), "{error}");
        assert!(error.contains("minecraft:air"), "{error}");
        assert!(tracker.validate_world(&region.target.world).is_ok());
    }

    #[test]
    fn fresh_light_tracker_stays_invalid_after_published_light_is_cleared() {
        let (mut terrain, tags, motion, _) = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        for z in -2..=2 {
            for x in -2..=2 {
                terrain.completed_decoration.insert(ChunkPos { x, z });
            }
        }
        let tracker = FreshGenerationLightTracker::new(&terrain, false).unwrap();
        terrain
            .initialize_generated_lighting(center, false)
            .unwrap();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        region.set_section_block([0, 0, 0], &state("air")).unwrap();
        region.finish().unwrap();
        assert!(!terrain.snapshot().chunks[&center].block_light_correct);
        assert!(tracker.validate_terrain(&terrain).is_err());
        assert!(FreshGenerationLightTracker::new(&terrain, false).is_err());
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
    fn unsupported_geode_write_rolls_back_terrain_and_random() {
        let (mut terrain, tags, motion, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let stone = world.intern_state(state("stone"));
        for chunk in world.chunks.values_mut() {
            let chunk = Arc::make_mut(chunk);
            Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
                .blocks
                .as_mut()
                .unwrap()
                .fill(stone);
        }
        let value = json!({
            "type":"minecraft:geode",
            "blocks":{
                "filling_provider":{"id":"minecraft:diamond_block"},
                "inner_layer_provider":{"id":"minecraft:amethyst_block"},
                "alternate_inner_layer_provider":{"id":"minecraft:budding_amethyst"},
                "middle_layer_provider":{"id":"minecraft:calcite"},
                "outer_layer_provider":{"id":"minecraft:smooth_basalt"},
                "inner_placements":["minecraft:amethyst_block"],
                "cannot_replace":[], "invalid_blocks":[]
            },
            "layers":{}, "crack":{"generate_crack_chance":0.0},
            "invalid_blocks_threshold":1, "min_gen_offset":-3, "max_gen_offset":3
        });
        let feature = GeodeFeature::decode(&value, &mut resolve_test).unwrap();
        let placed = PlacedDecorationFeature {
            placement: PlacedFeature::decode(&json!({"feature":"minecraft:test","placement":[]}))
                .unwrap(),
            feature: ConfiguredDecoration::Geode(feature),
        };
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(123);
        let mut original = random.clone();
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: None,
            calls: 0,
        };
        assert!(
            region
                .place_placed(&placed, &mut random, [0, 4, 0], 0, 16, &mut env)
                .is_err()
        );
        assert_eq!(random.next_long(), original.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_vine_support_matches_java_states_and_placement() {
        use native_resources::multiface_growth::MultifaceWorld;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let source = temp.path().join("GenerationMultifaceSupportOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GenerationMultifaceSupportOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GenerationMultifaceSupportOracle"));
        let (mut terrain, tags, _, _) = fixture();
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let origin = [8, 8, 8];
        let lichen = ResourceId::parse("minecraft:glow_lichen").unwrap();
        let mut checked = 0;
        let mut families = BTreeMap::<String, usize>::new();
        for line in output.lines().filter_map(|line| line.strip_prefix("S ")) {
            let p: Vec<_> = line.split_whitespace().collect();
            assert_eq!(p.len(), 4, "{line}");
            let mut support = state(p[0]);
            if p[1] != "-" {
                for property in p[1].split(',') {
                    let (k, v) = property.split_once('=').unwrap();
                    support.properties.insert(k.into(), v.into());
                }
            }
            let face = match p[2] {
                "down" => MultifaceFace::Down,
                "up" => MultifaceFace::Up,
                "north" => MultifaceFace::North,
                "south" => MultifaceFace::South,
                "west" => MultifaceFace::West,
                "east" => MultifaceFace::East,
                _ => panic!("{line}"),
            };
            let expected = p[3] == "true";
            assert_eq!(multiface_support(&support).unwrap(), expected, "{line}");
            let pos = face.offset(origin);
            region.set_section_block(pos, &support).unwrap();
            assert_eq!(
                region.vines_can_attach(pos, face).unwrap(),
                expected,
                "{line}"
            );
            assert_eq!(
                region
                    .state_for_placement(&lichen, &state("air"), origin, face)
                    .unwrap()
                    .is_some(),
                expected,
                "{line}"
            );
            checked += 1;
            *families.entry(p[0].to_owned()).or_default() += 1;
        }
        assert!(checked > 2_000, "missing Java state comparisons: {checked}");
        for (name, states) in [
            ("cave_vines", 52),
            ("cave_vines_plant", 2),
            ("big_dripleaf", 32),
            ("big_dripleaf_stem", 8),
            ("small_dripleaf", 16),
            ("glow_lichen", 128),
        ] {
            assert_eq!(
                families.get(name),
                Some(&(states * 6)),
                "missing Java faces for {name}"
            );
        }
        let plants = [
            "short_grass",
            "fern",
            "bush",
            "dandelion",
            "poppy",
            "blue_orchid",
            "allium",
            "azure_bluet",
            "red_tulip",
            "orange_tulip",
            "white_tulip",
            "pink_tulip",
            "oxeye_daisy",
            "cornflower",
            "lily_of_the_valley",
            "firefly_bush",
            "closed_eyeblossom",
            "sweet_berry_bush",
            "short_dry_grass",
            "tall_dry_grass",
            "dead_bush",
            "sunflower",
            "tall_grass",
            "large_fern",
            "rose_bush",
            "peony",
            "lilac",
            "seagrass",
            "tall_seagrass",
            "spore_blossom",
        ];
        assert_eq!(plants.len(), 30);
        assert_eq!(
            plants
                .iter()
                .map(|name| families.get(*name).copied().expect(name))
                .sum::<usize>(),
            40 * 6
        );
        let vine = VinesFeature::decode(&json!({"type":"minecraft:vines"}), &mut |_| {
            let mut value = state("vine");
            value.properties = ["up", "north", "south", "west", "east"]
                .into_iter()
                .map(|face| (face.into(), "false".into()))
                .collect();
            Ok(value)
        })
        .unwrap();
        let faces = [
            MultifaceFace::Up,
            MultifaceFace::North,
            MultifaceFace::South,
            MultifaceFace::West,
            MultifaceFace::East,
        ];
        let mut cases = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("V ")) {
            let (head, rest) = line.split_once(" W ").unwrap();
            let (writes, next) = rest.split_once(" R ").unwrap();
            let values: Vec<_> = head.split_whitespace().collect();
            let mask: usize = values[0].parse().unwrap();
            let occupied = values[1] == "true";
            let expected = values[2] == "true";
            region
                .set_section_block(origin, &state(if occupied { "stone" } else { "air" }))
                .unwrap();
            for (bit, face) in faces.iter().enumerate() {
                region
                    .set_section_block(
                        face.offset(origin),
                        &state(if mask & (1 << bit) != 0 {
                            "moss_block"
                        } else {
                            "air"
                        }),
                    )
                    .unwrap();
            }
            let mut random = DecorationRandom::new(42);
            assert_eq!(vine.place(&mut region, origin).unwrap(), expected, "{line}");
            assert_eq!(random.next_long(), next.parse::<i64>().unwrap(), "{line}");
            if expected {
                let actual = region.read(origin).unwrap();
                let props = actual
                    .properties
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",");
                assert_eq!(props, writes, "{line}");
            } else {
                assert!(writes.is_empty());
                assert_eq!(
                    region.read(origin).unwrap().name,
                    if occupied {
                        "minecraft:stone"
                    } else {
                        "minecraft:air"
                    }
                );
            }
            cases += 1;
        }
        assert_eq!(cases, 64);
    }

    #[test]
    fn unknown_multiface_support_rolls_back_random_and_terrain() {
        let (mut terrain, tags, motion, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let air = world.intern_state(state("air"));
        let unknown = world.intern_state(state("diamond_block"));
        let center = ChunkPos { x: 0, z: 0 };
        let chunk = Arc::make_mut(world.chunks.get_mut(&center).unwrap());
        let blocks = Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
            .blocks
            .as_mut()
            .unwrap();
        blocks.fill(air);
        for [x, y, z] in [
            [8, 7, 8],
            [8, 9, 8],
            [8, 8, 7],
            [8, 8, 9],
            [7, 8, 8],
            [9, 8, 8],
        ] {
            blocks[(x + z * 16 + y * 256) as usize] = unknown;
        }
        let value = json!({"type":"minecraft:multiface_growth","block":"minecraft:glow_lichen","can_place_on_floor":true,"can_place_on_ceiling":true,"can_place_on_wall":true,"can_be_placed_on":["minecraft:diamond_block"]});
        let placed = PlacedDecorationFeature {
            placement: PlacedFeature::decode(&json!({"feature":"minecraft:test","placement":[]}))
                .unwrap(),
            feature: ConfiguredDecoration::Multiface(MultifaceGrowth::decode(&value).unwrap()),
        };
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        let mut random = DecorationRandom::new(7);
        let mut original = random.clone();
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
        assert_eq!(random.next_long(), original.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    #[test]
    fn geode_fluid_ticks_distinguish_source_and_flowing_water_and_lava() {
        let (mut terrain, tags, motion, _) = fixture();
        let chunk = ChunkPos { x: 0, z: 0 };
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([chunk]))
            .unwrap();
        for (x, name, level, expected) in [
            (0, "water", "0", "minecraft:water"),
            (1, "water", "8", "minecraft:flowing_water"),
            (2, "lava", "0", "minecraft:lava"),
            (3, "lava", "3", "minecraft:flowing_lava"),
        ] {
            let mut block = state(name);
            block.properties.insert("level".into(), level.into());
            region.set_section_block([x, 4, 0], &block).unwrap();
            region.schedule_geode_fluid([x, 4, 0]).unwrap();
            region.schedule_geode_fluid([x, 4, 0]).unwrap();
            assert_eq!(
                region.world.saved_fluid_ticks[&chunk].last().unwrap().fluid,
                expected
            );
            if name == "water" {
                assert_eq!(
                    region.geode_can_grow(&block).unwrap(),
                    level == "0" || level == "8"
                );
            }
        }
        assert_eq!(region.world.saved_fluid_ticks[&chunk].len(), 4);
        region.finish().unwrap();
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
        let original = region.final_heights[&ChunkPos { x: 0, z: 0 }].clone();
        let converted = native_persistence::heightmap::SpawnHeightmaps::from_first_available(
            native_persistence::heightmap::HeightBounds {
                min_y: 0,
                max_y: 16,
            },
            &original.values[2],
            &original.values[0],
            &original.values[1],
        )
        .unwrap();
        use native_persistence::heightmap::SpawnHeightmapKind;
        assert_eq!(
            converted.highest(SpawnHeightmapKind::WorldSurface, 0, 0),
            Some(12)
        );
        assert_eq!(
            converted.highest(SpawnHeightmapKind::OceanFloor, 0, 0),
            Some(6)
        );
        assert_eq!(
            converted.highest(SpawnHeightmapKind::MotionBlocking, 0, 0),
            Some(6)
        );
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
        use native_resources::mob_spawn_placement::MobSpawnHeightmap;
        let (mut terrain, tags, motion, _) = fixture();
        assert_eq!(
            terrain
                .primed_mob_spawn_height(0, 0, MobSpawnHeightmap::MotionBlockingNoLeaves)
                .unwrap(),
            None
        );
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        region
            .set_section_block([0, 12, 0], &state("stone"))
            .unwrap();
        for kind in 0..4 {
            assert_eq!(region.final_height(kind, 0, 0).unwrap(), 8);
        }
        assert_eq!(region.final_height(3, -1, -1).unwrap(), 8);
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
        region.finish().unwrap();
        assert_eq!(
            terrain
                .primed_mob_spawn_height(0, 0, MobSpawnHeightmap::WorldSurface)
                .unwrap(),
            Some(13)
        );
        assert_eq!(
            terrain
                .primed_mob_spawn_height(0, 0, MobSpawnHeightmap::MotionBlockingNoLeaves)
                .unwrap(),
            Some(7)
        );
        assert_eq!(
            terrain
                .primed_mob_spawn_height(-1, -1, MobSpawnHeightmap::MotionBlockingNoLeaves)
                .unwrap(),
            Some(8)
        );
        assert!(
            terrain
                .primed_mob_spawn_height(1000, 0, MobSpawnHeightmap::OceanFloor)
                .is_err()
        );
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
            sea_level: 8,
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
    fn weighted_selector_validates_weights_and_keeps_zero_weight_dependencies() {
        let vine = json!({"feature":{"type":"minecraft:vines"},"placement":[]});
        let make = |features: Value| json!({"feature":{"type":"minecraft:weighted_random_selector","features":features},"placement":[]});
        let placed = PlacedDecorationFeature::from_placement(
            &mut Data,
            PlacedFeature::decode(&make(json!([{"weight":0,"data":vine}]))).unwrap(),
            &mut resolve_test,
        )
        .unwrap();
        assert!(
            placed
                .required_blocks()
                .contains(&ResourceId::parse("minecraft:vine").unwrap())
        );
        for features in [
            json!([{"weight":-1,"data":vine}]),
            json!([{"weight":2147483647i64,"data":vine},{"weight":1,"data":vine}]),
            json!([{"data":vine}]),
        ] {
            assert!(
                PlacedDecorationFeature::from_placement(
                    &mut Data,
                    PlacedFeature::decode(&make(features)).unwrap(),
                    &mut resolve_test
                )
                .is_err()
            );
        }
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
            } else if kind == 1 {
                json!({"type":"minecraft:random_selector","features":[{"chance":0.4,"feature":inner},{"chance":0.3,"feature":b}],"default":a})
            } else if kind == 2 {
                json!({"type":"minecraft:random_boolean_selector","feature_true":inner,"feature_false":b})
            } else {
                let entries = match kind {
                    3 => json!([{"weight":80,"data":a},{"weight":20,"data":b}]),
                    4 => {
                        json!([{"weight":1,"data":a},{"weight":0,"data":inner},{"weight":62,"data":b}])
                    }
                    5 => json!([{"weight":0,"data":a},{"weight":0,"data":b}]),
                    6 => json!([]),
                    7 => json!([{"weight":1,"data":inner}]),
                    _ => unreachable!(),
                };
                json!({"type":"minecraft:weighted_random_selector","features":entries})
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
        assert_eq!(cases.len(), 8 * 2 * 2 * 16);
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
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn waterlogged_dripleaf_patches_match_java() {
        use std::{path::PathBuf, process::Command};
        fn key(block: &BlockState) -> String {
            let mut result = format!("{}|", block.name);
            for (name, value) in &block.properties {
                result.push_str(&format!("{name}={value};"));
            }
            result
        }
        fn parse(value: &str) -> BlockState {
            let (name, properties) = value.split_once('|').unwrap();
            let properties = properties
                .split(';')
                .filter(|v| !v.is_empty())
                .map(|pair| {
                    let (name, value) = pair.split_once('=').unwrap();
                    (name.into(), value.into())
                })
                .collect();
            BlockState {
                name: name.into(),
                properties,
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let helper = temp.path().join("VegetationPatchOracle.java");
        std::fs::write(
            &helper,
            include_str!("../../../resources/tests/java/VegetationPatchOracle.java"),
        )
        .unwrap();
        let source = temp.path().join("WaterloggedDripleafPatchOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/WaterloggedDripleafPatchOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&helper)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("WaterloggedDripleafPatchOracle"));
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:water".into(),
            "minecraft:big_dripleaf".into(),
            "minecraft:big_dripleaf_stem".into(),
            "minecraft:small_dripleaf".into(),
        ]);
        let tags = OreBlockTags::load(&mut Data, &registry, &BTreeSet::new()).unwrap();
        let motion = HeightmapTags::load(&mut Data, &registry).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let (mut terrain, _, _, _) = fixture();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        let mut states = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("S ")) {
            let parts: Vec<_> = line.split_whitespace().collect();
            assert_eq!(parts.len(), 3, "{line}");
            let old = parse(parts[0]);
            region.set_section_block([8, 8, 8], &old).unwrap();
            let next = region.patch_waterlogged(old.clone()).unwrap();
            assert_eq!(next.is_some(), parts[2].parse::<bool>().unwrap(), "{line}");
            if let Some(next) = next {
                assert_eq!(key(&next), parts[1], "{line}");
                region.set_patch_block([8, 8, 8], &next).unwrap();
                assert_eq!(key(&region.read([8, 8, 8]).unwrap()), parts[1], "{line}");
            } else {
                assert_eq!(key(&old), parts[1], "{line}");
            }
            states += 1;
        }
        assert_eq!(states, 56);
        assert!(region.feature_marks.is_empty());
        assert!(region.world.saved_block_ticks.is_empty());
        assert!(region.world.saved_fluid_ticks.is_empty());
        drop(region);

        let mut rows = output
            .lines()
            .filter(|line| line.starts_with("P ") || line.starts_with("B "));
        for _ in 0..18 {
            let line = rows.next().unwrap();
            let p: Vec<_> = line.split_whitespace().collect();
            assert_eq!(p.len(), 6, "{line}");
            let kind: usize = p[1].parse().unwrap();
            let wet = p[2];
            let seed: i64 = p[3].parse().unwrap();
            let name = ["big_dripleaf", "big_dripleaf_stem", "small_dripleaf"][kind];
            let mut props = BTreeMap::from([
                ("facing".to_owned(), "north".to_owned()),
                ("waterlogged".to_owned(), wet.to_owned()),
            ]);
            if kind == 0 {
                props.insert("tilt".into(), "none".into());
            }
            if kind == 2 {
                props.insert("half".into(), "lower".into());
            }
            let definition = json!({"feature":{"type":"minecraft:waterlogged_vegetation_patch",
                "replaceable":["minecraft:stone"],"ground_state":{"id":"minecraft:clay"},
                "vegetation_feature":{"feature":{"type":"minecraft:block_column","direction":"up",
                    "allowed_placement":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"},
                    "prioritize_tip":true,"layers":[{"height":1,"provider":{"id":format!("minecraft:{name}"),"properties":props}}]},"placement":[]},
                "surface":"floor","depth":1,"extra_bottom_block_chance":0.0,"vertical_range":3,
                "vegetation_chance":1.0,"xz_radius":0,"extra_edge_column_chance":0.0},"placement":[]});
            let placed = PlacedDecorationFeature::from_placement(
                &mut Data,
                PlacedFeature::decode(&definition).unwrap(),
                &mut |value| {
                    let mut state = resolve_tree_test(value)?;
                    if state.name == "minecraft:water" {
                        state.properties.insert("level".into(), "0".into());
                    }
                    Ok(state)
                },
            )
            .unwrap();
            let (mut terrain, _, _, _) = fixture();
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([center]))
                .unwrap();
            let mut env = PlacementTestEnv {
                allow: true,
                fail_after: None,
                calls: 0,
            };
            let mut random = DecorationRandom::new(seed);
            assert_eq!(
                region
                    .place_placed(&placed, &mut random, [8, 8, 8], 0, 16, &mut env)
                    .unwrap(),
                p[4].parse::<bool>().unwrap(),
                "{line}"
            );
            assert_eq!(random.next_long(), p[5].parse::<i64>().unwrap(), "{line}");
            let b = rows.next().unwrap();
            let fields: Vec<_> = b.split_whitespace().collect();
            assert_eq!(fields.len(), 5, "{b}");
            let pos = [
                fields[1].parse().unwrap(),
                fields[2].parse().unwrap(),
                fields[3].parse().unwrap(),
            ];
            assert_eq!(key(&region.read(pos).unwrap()), fields[4], "{b}");
            assert!(region.feature_marks.is_empty());
            assert!(region.world.saved_block_ticks.is_empty());
            assert!(region.world.saved_fluid_ticks.is_empty());
            region.finish().unwrap();
        }
        assert!(rows.next().is_none());
    }

    #[test]
    fn vegetation_patch_executes_nested_feature_in_generated_chunk() {
        struct PatchData;
        impl ResourceProvider for PatchData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/minecraft/worldgen/biome/plains.json" => json!({"features":[[{"feature":{
                        "type":"minecraft:vegetation_patch","replaceable":"#minecraft:base_stone_overworld","ground_state":{"id":"minecraft:moss_block"},
                        "vegetation_feature":{"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:short_grass"}},"placement":[]},
                        "surface":"floor","depth":1,"extra_bottom_block_chance":0.0,"vertical_range":3,"vegetation_chance":1.0,"xz_radius":0,"extra_edge_column_chance":0.0
                    },"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8}]}]]}),
                    "data/minecraft/tags/block/supports_vegetation.json" => {
                        json!({"values":["minecraft:moss_block"]})
                    }
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        json!({"values":["minecraft:stone","minecraft:diamond_ore","minecraft:moss_block"]})
                    }
                    _ => return Data.read_data(path),
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(&mut PatchData, &[plains], &BTreeSet::new()).unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.insert("minecraft:moss_block".into());
        let prepared =
            PreparedDecoration::load(&mut PatchData, &plan, &registry, &mut resolve_test).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
        let after = terrain.snapshot();
        assert_eq!(after.block(8, 7, 8).unwrap().name, "minecraft:moss_block");
        assert_eq!(after.block(8, 8, 8).unwrap().name, "minecraft:short_grass");
        assert_eq!(before.block(8, 7, 8).unwrap().name, "minecraft:stone");
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn carpet_generation_matches_java_survival_and_edges() {
        use native_resources::tree_leaf_update::ShapeEdgeWorld;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let source = temp.path().join("CarpetFeatureOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/CarpetFeatureOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("CarpetFeatureOracle"));
        let names = [
            "moss_carpet",
            "white_carpet",
            "orange_carpet",
            "magenta_carpet",
            "light_blue_carpet",
            "yellow_carpet",
            "lime_carpet",
            "pink_carpet",
            "gray_carpet",
            "light_gray_carpet",
            "cyan_carpet",
            "purple_carpet",
            "blue_carpet",
            "brown_carpet",
            "green_carpet",
            "red_carpet",
            "black_carpet",
        ];
        let mut registry = batch_registry();
        registry.extend(names.iter().map(|name| format!("minecraft:{name}")));
        registry.extend(["minecraft:water".into(), "minecraft:lava".into()]);
        let tags = OreBlockTags::load(&mut Data, &registry, &BTreeSet::new()).unwrap();
        let motion = HeightmapTags::load(&mut Data, &registry).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let mut cases = 0;
        let mut edges = 0;
        for line in output.lines().filter(|line| line.starts_with("C ")) {
            let p: Vec<_> = line.split_whitespace().collect();
            assert_eq!(p.len(), 11, "{line}");
            let kind: usize = p[1].parse().unwrap();
            let base: usize = p[2].parse().unwrap();
            let here: usize = p[3].parse().unwrap();
            let mut support = state(
                [
                    "air",
                    "cave_air",
                    "stone",
                    "water",
                    "lava",
                    "short_grass",
                    "moss_carpet",
                ][base],
            );
            if matches!(support.name.as_str(), "minecraft:water" | "minecraft:lava") {
                support.properties.insert("level".into(), "0".into());
            }
            let mut current = state(["air", "stone", "water", "moss_carpet"][here]);
            if current.name == "minecraft:water" {
                current.properties.insert("level".into(), "0".into());
            }
            region.set_section_block([8, 7, 8], &support).unwrap();
            region.set_section_block([8, 8, 8], &current).unwrap();
            let target = state(names[kind]);
            let predicate = BlockPredicate::decode(
                &json!({"type":"minecraft:would_survive","state":{"id":target.name}}),
            )
            .unwrap();
            assert!(predicate.required_tags().is_empty());
            assert_eq!(
                predicate.test(&mut region, [8, 8, 8]).unwrap(),
                p[5] == "true",
                "{line}"
            );
            assert_eq!(
                DiskWorld::is_solid(&mut region, &target).unwrap(),
                p[10] == "true",
                "{line}"
            );
            let feature = SimpleBlockFeature::decode(
                &json!({"type":"minecraft:simple_block","to_place":{"id":target.name}}),
                &mut resolve_tree_test,
            )
            .unwrap();
            let mut random = DecorationRandom::new(42);
            assert_eq!(
                feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                p[6] == "true",
                "{line}"
            );
            assert_eq!(
                feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                p[7] == "true",
                "{line}"
            );
            assert_eq!(region.read([8, 8, 8]).unwrap().name, p[8], "{line}");
            assert_eq!(random.next_long(), p[9].parse::<i64>().unwrap(), "{line}");
            assert_eq!(p[4], p[5], "{line}");
            for edge in output
                .lines()
                .filter(|edge| edge.starts_with(&format!("U {kind} {base} {here} ")))
            {
                let u: Vec<_> = edge.split_whitespace().collect();
                let direction = match u[4] {
                    "down" => [0, -1, 0],
                    "up" => [0, 1, 0],
                    "north" => [0, 0, -1],
                    "south" => [0, 0, 1],
                    "west" => [-1, 0, 0],
                    "east" => [1, 0, 0],
                    _ => panic!("{edge}"),
                };
                assert_eq!(
                    region
                        .update_shape([8, 8, 8], &target, direction, [0, 0, 0], &state("stone"))
                        .unwrap()
                        .name,
                    u[5],
                    "{edge}"
                );
                edges += 1;
            }
            cases += 1;
        }
        assert_eq!(cases, 17 * 7 * 4);
        assert_eq!(edges, cases * 6);
        for name in ["moss_carpet", "minecraft:moss_carpet"] {
            let predicate = BlockPredicate::decode(
                &json!({"type":"minecraft:would_survive","state":{"id":name}}),
            )
            .unwrap();
            assert!(predicate.required_tags().is_empty());
            assert!(predicate.test(&mut region, [8, 8, 8]).unwrap());
        }
        assert!(!is_simple_carpet_id("minecraft:pale_moss_carpet"));
        assert!(!is_simple_carpet_id("custom:moss_carpet"));
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn azalea_generation_matches_java_tags_and_edges() {
        use native_resources::tree_leaf_update::ShapeEdgeWorld;
        use std::{path::PathBuf, process::Command};
        struct AzaleaData(u8);
        impl ResourceProvider for AzaleaData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/supports_azalea.json" {
                    let mut values = Vec::new();
                    if self.0 & 1 != 0 {
                        values.push("minecraft:stone");
                    }
                    if self.0 & 2 != 0 {
                        values.push("minecraft:dirt");
                    }
                    return Ok(Some(serde_json::to_vec(&json!({"values":values}))?));
                }
                if path == "data/minecraft/tags/block/supports_vegetation.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"values":if self.0 & 4 != 0 { vec!["minecraft:stone"] } else { vec![] }}),
                    )?));
                }
                Data.read_data(path)
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let source = temp.path().join("AzaleaFeatureOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/AzaleaFeatureOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("AzaleaFeatureOracle"));
        let mut registry = batch_registry();
        registry.extend(
            [
                "minecraft:azalea",
                "minecraft:flowering_azalea",
                "minecraft:dirt",
                "minecraft:sand",
                "minecraft:water",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        let required = BTreeSet::from([
            ResourceId::parse("minecraft:supports_azalea").unwrap(),
            ResourceId::parse("minecraft:supports_vegetation").unwrap(),
        ]);
        let motion = HeightmapTags::load(&mut AzaleaData(0), &registry).unwrap();
        let mut cases = 0;
        let mut edges = 0;
        for mode in 0..8u8 {
            let tags = OreBlockTags::load(&mut AzaleaData(mode), &registry, &required).unwrap();
            let (mut terrain, _, _, _) = fixture();
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
                .unwrap();
            for line in output
                .lines()
                .filter(|line| line.starts_with(&format!("A {mode} ")))
            {
                let p: Vec<_> = line.split_whitespace().collect();
                assert_eq!(p.len(), 12, "{line}");
                let kind: usize = p[2].parse().unwrap();
                let base: usize = p[3].parse().unwrap();
                let here: usize = p[4].parse().unwrap();
                let mut support = state(["stone", "dirt", "sand", "air"][base]);
                let mut current =
                    state(["air", "stone", "water", "azalea", "flowering_azalea"][here]);
                if current.name == "minecraft:water" {
                    current.properties.insert("level".into(), "0".into());
                }
                if support.name == "minecraft:water" {
                    support.properties.insert("level".into(), "0".into());
                }
                region.set_section_block([8, 7, 8], &support).unwrap();
                region.set_section_block([8, 8, 8], &current).unwrap();
                let target = state(["azalea", "flowering_azalea"][kind]);
                let pred = BlockPredicate::decode(
                    &json!({"type":"minecraft:would_survive","state":{"id":target.name}}),
                )
                .unwrap();
                assert_eq!(
                    pred.test(&mut region, [8, 8, 8]).unwrap(),
                    p[6] == "true",
                    "{line}"
                );
                assert_eq!(
                    DiskWorld::is_solid(&mut region, &target).unwrap(),
                    p[11] == "true",
                    "{line}"
                );
                let feature = SimpleBlockFeature::decode(
                    &json!({"type":"minecraft:simple_block","to_place":{"id":target.name}}),
                    &mut resolve_tree_test,
                )
                .unwrap();
                let mut random = DecorationRandom::new(42);
                assert_eq!(
                    feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                    p[7] == "true",
                    "{line}"
                );
                assert_eq!(
                    feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                    p[8] == "true",
                    "{line}"
                );
                assert_eq!(region.read([8, 8, 8]).unwrap().name, p[9], "{line}");
                assert_eq!(random.next_long(), p[10].parse::<i64>().unwrap(), "{line}");
                assert_eq!(p[5], p[6], "{line}");
                for edge in output
                    .lines()
                    .filter(|edge| edge.starts_with(&format!("U {mode} {kind} {base} {here} ")))
                {
                    let u: Vec<_> = edge.split_whitespace().collect();
                    let direction = match u[5] {
                        "down" => [0, -1, 0],
                        "up" => [0, 1, 0],
                        "north" => [0, 0, -1],
                        "south" => [0, 0, 1],
                        "west" => [-1, 0, 0],
                        "east" => [1, 0, 0],
                        _ => panic!("{edge}"),
                    };
                    let result = region
                        .update_shape([8, 8, 8], &target, direction, [0, 0, 0], &state("stone"))
                        .unwrap();
                    assert_eq!(result.name, u[6], "{edge}");
                    edges += 1;
                }
                cases += 1;
            }
        }
        assert_eq!(cases, 8 * 2 * 4 * 5);
        assert_eq!(edges, cases * 6);
        let feature=PlacedDecorationFeature::from_placement(&mut AzaleaData(0),PlacedFeature::decode(&json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:azalea"}},"placement":[]})).unwrap(),&mut resolve_tree_test).unwrap();
        assert!(
            feature
                .required_tags()
                .contains(&ResourceId::parse("minecraft:supports_azalea").unwrap())
        );
        let predicate = BlockPredicate::decode(&json!({
            "type":"minecraft:would_survive", "state":{"id":"minecraft:flowering_azalea"}
        }))
        .unwrap();
        assert!(
            predicate
                .required_tags()
                .contains(&ResourceId::parse("minecraft:supports_azalea").unwrap())
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn spore_blossom_feature_matches_java_placement() {
        use std::{path::PathBuf, process::Command};
        struct SporeData(bool);
        impl ResourceProvider for SporeData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/unstable_bottom_center.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"values":if self.0 { vec!["minecraft:stone"] } else { vec![] }}),
                    )?));
                }
                Data.read_data(path)
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let source = temp.path().join("SporeBlossomOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/SporeBlossomOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SporeBlossomOracle"));
        let mut registry = batch_registry();
        registry.extend(
            [
                "minecraft:spore_blossom",
                "minecraft:dirt",
                "minecraft:water",
                "minecraft:lava",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        let required =
            BTreeSet::from([ResourceId::parse("minecraft:unstable_bottom_center").unwrap()]);
        let motion = HeightmapTags::load(&mut SporeData(false), &registry).unwrap();
        let feature = SimpleBlockFeature::decode(
            &json!({"type":"minecraft:simple_block", "to_place":{"id":"minecraft:spore_blossom"}}),
            &mut resolve_tree_test,
        )
        .unwrap();
        let mut cases = 0;
        for mode in 0..4u8 {
            let tags =
                OreBlockTags::load(&mut SporeData(mode & 1 != 0), &registry, &required).unwrap();
            let water: BTreeSet<String> = if mode & 2 != 0 {
                ["minecraft:water".into(), "minecraft:flowing_water".into()]
                    .into_iter()
                    .collect()
            } else {
                BTreeSet::new()
            };
            let (mut terrain, _, _, _) = fixture();
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
                .unwrap();
            region.lake_water_fluids = Some(&water);
            assert!(!DiskWorld::is_solid(&mut region, &state("spore_blossom")).unwrap());
            for line in output
                .lines()
                .filter(|line| line.starts_with(&format!("S {mode} ")))
            {
                let p: Vec<_> = line.split_whitespace().collect();
                assert_eq!(p.len(), 9, "{line}");
                let support: usize = p[2].parse().unwrap();
                let current: usize = p[3].parse().unwrap();
                let mut above = state(["air", "stone", "dirt", "water"][support]);
                let mut here = state(["air", "water", "lava"][current]);
                for block in [&mut above, &mut here] {
                    if matches!(block.name.as_str(), "minecraft:water" | "minecraft:lava") {
                        block.properties.insert("level".into(), "0".into());
                    }
                }
                region.set_section_block([8, 9, 8], &above).unwrap();
                region.set_section_block([8, 8, 8], &here).unwrap();
                assert_eq!(
                    region.spore_blossom_survival([8, 8, 8]).unwrap(),
                    p[4] == "true",
                    "{line}"
                );
                let mut random = DecorationRandom::new(42);
                assert_eq!(
                    feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                    p[5] == "true",
                    "{line}"
                );
                assert_eq!(
                    feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                    p[6] == "true",
                    "{line}"
                );
                assert_eq!(region.read([8, 8, 8]).unwrap().name, p[7], "{line}");
                assert_eq!(random.next_long(), p[8].parse::<i64>().unwrap(), "{line}");
                cases += 1;
            }
        }
        assert_eq!(cases, 48);
        let placed = PlacedDecorationFeature::from_placement(&mut SporeData(false), PlacedFeature::decode(&json!({"feature":{"type":"minecraft:simple_block", "to_place":{"id":"minecraft:spore_blossom"}}, "placement":[]})).unwrap(), &mut resolve_tree_test).unwrap();
        assert!(placed.needs_water_fluids());
        assert!(
            placed
                .required_tags()
                .contains(&ResourceId::parse("minecraft:unstable_bottom_center").unwrap())
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn small_dripleaf_feature_matches_java_placement() {
        use std::{path::PathBuf, process::Command};
        struct DripleafData(i32);
        impl ResourceProvider for DripleafData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let enabled = match path {
                    "data/minecraft/tags/block/supports_small_dripleaf.json" => {
                        Some(self.0 & 1 != 0)
                    }
                    "data/minecraft/tags/block/supports_vegetation.json" => Some(self.0 & 2 != 0),
                    _ => None,
                };
                if let Some(enabled) = enabled {
                    return Ok(Some(if enabled {
                        br#"{"values":["minecraft:stone"]}"#.to_vec()
                    } else {
                        br#"{"values":[]}"#.to_vec()
                    }));
                }
                Data.read_data(path)
            }
        }
        fn sample(i: i32) -> BlockState {
            let mut block = state(match i {
                0 => "air",
                1 | 2 => "water",
                3 => "lava",
                4 | 5 => "small_dripleaf",
                6 => "seagrass",
                7 => "stone",
                8 | 9 => "cave_vines",
                10 | 11 => "cave_vines_plant",
                _ => unreachable!(),
            });
            match i {
                1 | 2 | 3 => {
                    block
                        .properties
                        .insert("level".into(), if i == 2 { "1" } else { "0" }.into());
                }
                4 | 5 => {
                    block
                        .properties
                        .insert("half".into(), if i == 4 { "lower" } else { "upper" }.into());
                    block.properties.insert("facing".into(), "north".into());
                    block
                        .properties
                        .insert("waterlogged".into(), (i == 5).to_string());
                }
                8..=11 => {
                    if i < 10 {
                        block.properties.insert("age".into(), "0".into());
                    }
                    block
                        .properties
                        .insert("berries".into(), (i % 2 == 1).to_string());
                }
                _ => {}
            }
            block
        }
        fn key(block: &BlockState) -> String {
            let mut value = format!("{}|", block.name);
            for (k, v) in &block.properties {
                value.push_str(&format!("{k}={v};"));
            }
            value
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let source = temp.path().join("SmallDripleafFeatureOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/SmallDripleafFeatureOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
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
            .arg("SmallDripleafFeatureOracle"));
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:water".into(),
            "minecraft:lava".into(),
            "minecraft:seagrass".into(),
            "minecraft:small_dripleaf".into(),
        ]);
        let motion = HeightmapTags::load(&mut DripleafData(0), &registry).unwrap();
        let mut checked = 0;
        for mode in 0..32 {
            let required = BTreeSet::from([
                ResourceId::parse("minecraft:supports_small_dripleaf").unwrap(),
                ResourceId::parse("minecraft:supports_vegetation").unwrap(),
            ]);
            let tags = OreBlockTags::load(&mut DripleafData(mode), &registry, &required).unwrap();
            let (mut terrain, _, _, _) = fixture();
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
                .unwrap();
            let mut water = BTreeSet::new();
            if mode & 4 != 0 {
                water.insert("minecraft:water".into());
            }
            if mode & 8 != 0 {
                water.insert("minecraft:flowing_water".into());
            }
            if mode & 16 != 0 {
                water.insert("minecraft:lava".into());
            }
            region.lake_water_fluids = Some(&water);
            for line in output
                .lines()
                .filter(|line| line.starts_with(&format!("S {mode} ")))
            {
                let p: Vec<_> = line.split_whitespace().collect();
                assert_eq!(p.len(), 13, "{line}");
                let half: i32 = p[2].parse().unwrap();
                let wet: i32 = p[3].parse().unwrap();
                let base: i32 = p[4].parse().unwrap();
                let here: i32 = p[5].parse().unwrap();
                let above: i32 = p[6].parse().unwrap();
                let support = match base {
                    0 => state("stone"),
                    1 => state("air"),
                    2 => sample(4),
                    3 => sample(5),
                    _ => unreachable!(),
                };
                region.set_section_block([8, 7, 8], &support).unwrap();
                region.set_section_block([8, 8, 8], &sample(here)).unwrap();
                region.set_section_block([8, 9, 8], &sample(above)).unwrap();
                let mut target = sample(4);
                target.properties.insert(
                    "half".into(),
                    if half == 0 { "lower" } else { "upper" }.into(),
                );
                target
                    .properties
                    .insert("waterlogged".into(), (wet != 0).to_string());
                let predicate = BlockPredicate::decode(&json!({
                    "type":"minecraft:would_survive",
                    "state":{"id":target.name,"properties":target.properties}
                }))
                .unwrap();
                assert_eq!(
                    predicate.test(&mut region, [8, 8, 8]).unwrap(),
                    p[7] == "true",
                    "survival {line}"
                );
                let feature=SimpleBlockFeature::decode(&json!({"type":"minecraft:simple_block","to_place":{"id":target.name,"properties":target.properties}}),&mut resolve_tree_test).unwrap();
                let mut random = DecorationRandom::new(42);
                assert_eq!(
                    feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                    p[8] == "true",
                    "{line}"
                );
                assert_eq!(
                    feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                    p[9] == "true",
                    "second {line}"
                );
                assert_eq!(key(&region.read([8, 8, 8]).unwrap()), p[10], "{line}");
                assert_eq!(key(&region.read([8, 9, 8]).unwrap()), p[11], "{line}");
                assert_eq!(random.next_long(), p[12].parse::<i64>().unwrap(), "{line}");
                checked += 1;
            }
        }
        assert_eq!(checked, 32 * 2 * 2 * 4 * 8 * 12);
        let predicate = BlockPredicate::decode(
            &json!({"type":"minecraft:would_survive","state":{"id":"minecraft:small_dripleaf"}}),
        )
        .unwrap();
        assert_eq!(
            predicate.required_tags(),
            BTreeSet::from([
                ResourceId::parse("minecraft:supports_small_dripleaf").unwrap(),
                ResourceId::parse("minecraft:supports_vegetation").unwrap(),
            ])
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn seagrass_features_match_java_placement() {
        use std::{path::PathBuf, process::Command};
        struct GrassData(bool);
        impl ResourceProvider for GrassData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/cannot_support_seagrass.json" {
                    return Ok(Some(if self.0 {
                        br#"{"values":["minecraft:magma_block","minecraft:stone"]}"#.to_vec()
                    } else {
                        br#"{"values":["minecraft:magma_block"]}"#.to_vec()
                    }));
                }
                Data.read_data(path)
            }
        }
        fn sample(i: i32) -> BlockState {
            let mut s = state(match i {
                0 => "air",
                1 | 2 | 3 => "water",
                4 => "lava",
                5 => "seagrass",
                6 | 7 => "tall_seagrass",
                _ => unreachable!(),
            });
            match i {
                1 => {
                    s.properties.insert("level".into(), "0".into());
                }
                2 => {
                    s.properties.insert("level".into(), "1".into());
                }
                3 => {
                    s.properties.insert("level".into(), "8".into());
                }
                4 => {
                    s.properties.insert("level".into(), "0".into());
                }
                6 | 7 => {
                    s.properties
                        .insert("half".into(), if i == 6 { "lower" } else { "upper" }.into());
                }
                _ => {}
            }
            s
        }
        fn key(s: &BlockState) -> String {
            let mut text = format!("{}|", s.name);
            for (k, v) in &s.properties {
                text.push_str(&format!("{k}={v};"));
            }
            text
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let source = temp.path().join("SeagrassFeatureOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/SeagrassFeatureOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let o = c.output().unwrap();
            assert!(
                o.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            );
            String::from_utf8(o.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SeagrassFeatureOracle"));
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:water".into(),
            "minecraft:lava".into(),
            "minecraft:sand".into(),
            "minecraft:magma_block".into(),
            "minecraft:seagrass".into(),
            "minecraft:tall_seagrass".into(),
        ]);
        let motion = HeightmapTags::load(&mut GrassData(false), &registry).unwrap();
        let mut checked = 0;
        for mode in 0..8 {
            let tags = OreBlockTags::load(
                &mut GrassData(mode & 4 != 0),
                &registry,
                &BTreeSet::from([ResourceId::parse("minecraft:cannot_support_seagrass").unwrap()]),
            )
            .unwrap();
            let (mut terrain, _, _, _) = fixture();
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
                .unwrap();
            let mut water = BTreeSet::new();
            if mode & 1 != 0 {
                water.insert("minecraft:water".into());
            }
            if mode & 2 != 0 {
                water.insert("minecraft:flowing_water".into());
            }
            region.lake_water_fluids = Some(&water);
            for line in output
                .lines()
                .filter(|line| line.starts_with(&format!("S {mode} ")))
            {
                let p: Vec<_> = line.split_whitespace().collect();
                assert_eq!(p.len(), 11, "{line}");
                let kind: i32 = p[2].parse().unwrap();
                let base: i32 = p[3].parse().unwrap();
                let here: i32 = p[4].parse().unwrap();
                let above: i32 = p[5].parse().unwrap();
                let support = match base {
                    0 => state("stone"),
                    1 => state("sand"),
                    2 => state("magma_block"),
                    3 => state("air"),
                    _ => sample(6),
                };
                region.set_section_block([8, 7, 8], &support).unwrap();
                region.set_section_block([8, 8, 8], &sample(here)).unwrap();
                region.set_section_block([8, 9, 8], &sample(above)).unwrap();
                let target = if kind == 0 {
                    sample(5)
                } else if kind == 1 {
                    sample(6)
                } else {
                    sample(7)
                };
                let mut random = DecorationRandom::new(42);
                let feature=SimpleBlockFeature::decode(&json!({"type":"minecraft:simple_block","to_place":{"id":target.name,"properties":target.properties}}),&mut resolve_tree_test).unwrap();
                assert_eq!(
                    feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                    p[6] == "true",
                    "{line}"
                );
                assert_eq!(
                    feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                    p[7] == "true",
                    "second {line}"
                );
                assert_eq!(key(&region.read([8, 8, 8]).unwrap()), p[8], "{line}");
                assert_eq!(key(&region.read([8, 9, 8]).unwrap()), p[9], "{line}");
                assert_eq!(random.next_long(), p[10].parse::<i64>().unwrap(), "{line}");
                checked += 1;
            }
        }
        assert_eq!(checked, 8 * 3 * 5 * 8 * 8);
    }

    #[test]
    fn weighted_tall_seagrass_writes_both_halves_in_chunk() {
        struct GrassData;
        impl ResourceProvider for GrassData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/cannot_support_seagrass.json" {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                Data.read_data(path)
            }
        }
        let value = json!({"feature":{"type":"minecraft:weighted_random_selector","features":[{"weight":1,"data":{"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:tall_seagrass","properties":{"half":"lower"}}},"placement":[]}}]},"placement":[]});
        let placed = PlacedDecorationFeature::from_placement(
            &mut GrassData,
            PlacedFeature::decode(&value).unwrap(),
            &mut resolve_tree_test,
        )
        .unwrap();
        assert!(placed.needs_water_fluids());
        assert!(
            placed
                .required_tags()
                .contains(&ResourceId::parse("minecraft:cannot_support_seagrass").unwrap())
        );
        let mut registry = batch_registry();
        registry.extend(["minecraft:water".into(), "minecraft:tall_seagrass".into()]);
        let tags = OreBlockTags::load(&mut GrassData, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut GrassData, &registry).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let water_tags = BTreeSet::from([
            "minecraft:water".to_string(),
            "minecraft:flowing_water".to_string(),
        ]);
        region.lake_water_fluids = Some(&water_tags);
        let mut water = state("water");
        water.properties.insert("level".into(), "0".into());
        for y in [8, 9] {
            region.set_section_block([8, y, 8], &water).unwrap();
            region.update_final_heightmaps([8, y, 8], &water).unwrap();
        }
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: None,
            calls: 0,
        };
        assert!(
            region
                .place_placed(
                    &placed,
                    &mut DecorationRandom::new(42),
                    [8, 8, 8],
                    0,
                    16,
                    &mut env
                )
                .unwrap()
        );
        region.finish().unwrap();
        let after = terrain.snapshot();
        assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:air");
        for (y, half) in [(8, "lower"), (9, "upper")] {
            let state = after.block(8, y, 8).unwrap();
            assert_eq!(state.name, "minecraft:tall_seagrass");
            assert_eq!(state.properties["half"], half);
        }
    }

    #[test]
    fn vines_feature_writes_first_supported_face_to_chunk() {
        let value = json!({"feature":{"type":"minecraft:vines"},"placement":[]});
        let placed = PlacedDecorationFeature::from_placement(
            &mut Data,
            PlacedFeature::decode(&value).unwrap(),
            &mut |value| {
                let mut block = resolve_test(value)?;
                if block.name == "minecraft:vine" {
                    block.properties = ["up", "north", "south", "west", "east"]
                        .into_iter()
                        .map(|s| (s.into(), "false".into()))
                        .collect();
                }
                Ok(block)
            },
        )
        .unwrap();
        let mut registry = batch_registry();
        registry.insert("minecraft:vine".into());
        let tags = OreBlockTags::load(&mut Data, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut Data, &registry).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let air = state("air");
        region.set_section_block([8, 7, 8], &air).unwrap();
        region.update_final_heightmaps([8, 7, 8], &air).unwrap();
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: None,
            calls: 0,
        };
        assert!(
            region
                .place_placed(
                    &placed,
                    &mut DecorationRandom::new(42),
                    [8, 7, 8],
                    0,
                    16,
                    &mut env
                )
                .unwrap()
        );
        region.finish().unwrap();
        let after = terrain.snapshot();
        let vine = after.block(8, 7, 8).unwrap();
        assert_eq!(before.block(8, 7, 8).unwrap().name, "minecraft:stone");
        assert_eq!(vine.name, "minecraft:vine");
        assert_eq!(vine.properties["north"], "true");
        for direction in ["up", "south", "west", "east"] {
            assert_eq!(vine.properties[direction], "false");
        }
    }

    #[test]
    fn root_system_executes_nested_feature_and_writes_generated_chunk() {
        struct RootData;
        impl ResourceProvider for RootData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                match path {
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" =>
                        Ok(Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore","minecraft:rooted_dirt","minecraft:melon"]}"#.to_vec())),
                    _ => Data.read_data(path),
                }
            }
        }
        let value = json!({"feature":{"type":"minecraft:root_system",
            "feature":{"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:melon"}},"placement":[]},
            "required_vertical_space_for_tree":1,"level_test_distance":0,"max_level_deviation":0,
            "root_radius":1,"root_replaceable":["minecraft:stone"],"root_state_provider":{"id":"minecraft:rooted_dirt"},
            "root_placement_attempts":1,"root_column_max_height":10,"hanging_root_radius":1,
            "hanging_roots_vertical_span":1,"hanging_root_state_provider":{"id":"minecraft:hanging_roots","properties":{"waterlogged":"false"}},
            "hanging_root_placement_attempts":1,"allowed_vertical_water_for_tree":1,
            "allowed_tree_position":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"}},"placement":[]});
        let placed = PlacedDecorationFeature::from_placement(
            &mut RootData,
            PlacedFeature::decode(&value).unwrap(),
            &mut resolve_tree_test,
        )
        .unwrap();
        assert!(placed.feature.needs_root_system());
        assert!(placed.needs_water_fluids());
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:rooted_dirt".into(),
            "minecraft:hanging_roots".into(),
            "minecraft:melon".into(),
        ]);
        let tags = OreBlockTags::load(&mut RootData, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut RootData, &registry).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        let water = BTreeSet::from(["minecraft:water".into(), "minecraft:flowing_water".into()]);
        let lava = BTreeSet::from(["minecraft:lava".into(), "minecraft:flowing_lava".into()]);
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        region.lake_water_fluids = Some(&water);
        region.cactus_lava_fluids = Some(&lava);
        let air = state("air");
        region.set_section_block([8, 5, 8], &air).unwrap();
        region.update_final_heightmaps([8, 5, 8], &air).unwrap();
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: None,
            calls: 0,
        };
        assert!(
            region
                .place_placed(
                    &placed,
                    &mut DecorationRandom::new(42),
                    [8, 5, 8],
                    0,
                    16,
                    &mut env
                )
                .unwrap()
        );
        assert_eq!(region.final_height(0, 8, 8).unwrap(), 9);
        region.finish().unwrap();
        let after = terrain.snapshot();
        assert_eq!(before.block(8, 6, 8).unwrap().name, "minecraft:stone");
        assert_eq!(after.block(8, 6, 8).unwrap().name, "minecraft:rooted_dirt");
        assert_eq!(
            after.block(8, 5, 8).unwrap().name,
            "minecraft:hanging_roots"
        );
        assert_eq!(after.block(8, 8, 8).unwrap().name, "minecraft:melon");
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn dripleaf_column_states_and_placement_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let source = temp.path().join("DripleafColumnOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/DripleafColumnOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
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
            .arg("DripleafColumnOracle"));
        let mut blocks = batch_registry();
        blocks.extend([
            "minecraft:big_dripleaf".into(),
            "minecraft:big_dripleaf_stem".into(),
            "minecraft:small_dripleaf".into(),
        ]);
        let tags = OreBlockTags::load(&mut Data, &blocks, &BTreeSet::new()).unwrap();
        let motion = HeightmapTags::load(&mut Data, &blocks).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        let mut count = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("S ")) {
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 11, "{line}");
            let properties = fields[1]
                .split(',')
                .map(|pair| {
                    let (key, value) = pair.split_once('=').unwrap();
                    (key.to_owned(), value.to_owned())
                })
                .collect();
            let block = BlockState {
                name: fields[0].into(),
                properties,
            };
            assert!(generated_dripleaf(&block).unwrap(), "{line}");
            assert_eq!(
                &fields[2..6],
                &["false", "false", "false", "none"],
                "{line}"
            );
            let fluid = native_world::fluid::FluidState::from_block(&block).unwrap();
            assert_eq!(
                fluid.is_none(),
                fields[6].parse::<bool>().unwrap(),
                "{line}"
            );
            let height_flags = region.final_predicates(&block).unwrap();
            for kind in 0..4 {
                assert_eq!(
                    height_flags[kind],
                    fields[7 + kind].parse::<bool>().unwrap(),
                    "{line}"
                );
            }
            assert!(region.set_column_block([8, 8, 8], &block).unwrap());
            assert_eq!(region.read([8, 8, 8]).unwrap(), block, "{line}");
            count += 1;
        }
        assert_eq!(count, 56);
        assert!(region.feature_marks.is_empty());
        assert!(region.world.saved_block_ticks.is_empty());
        assert!(region.world.saved_fluid_ticks.is_empty());
        let mut invalid = state("big_dripleaf");
        invalid.properties = BTreeMap::from([
            ("facing".into(), "east".into()),
            ("tilt".into(), "none".into()),
            ("waterlogged".into(), "invalid".into()),
        ]);
        assert!(generated_dripleaf(&invalid).is_err());
        assert!(region.set_column_block([8, 8, 8], &invalid).is_err());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));

        let mut rows = output
            .lines()
            .filter(|line| line.starts_with("C ") || line.starts_with("B "));
        for _ in 0..8 {
            let line = rows.next().unwrap();
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 5, "{line}");
            let facing = fields[1];
            let wet = fields[2];
            let (mut terrain, _, _, _) = fixture();
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([center]))
                .unwrap();
            let feature=BlockColumnFeature::decode(&json!({"type":"minecraft:block_column","direction":"up",
                "allowed_placement":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"},
                "prioritize_tip":true,"layers":[
                    {"height":2,"provider":{"id":"minecraft:big_dripleaf_stem","properties":{"facing":facing,"waterlogged":wet}}},
                    {"height":1,"provider":{"id":"minecraft:big_dripleaf","properties":{"facing":facing,"tilt":"none","waterlogged":wet}}}
                ]}),&mut resolve_tree_test).unwrap();
            let mut random = DecorationRandom::new(42);
            assert_eq!(
                feature.place(&mut region, &mut random, [8, 8, 8]).unwrap(),
                fields[3].parse::<bool>().unwrap(),
                "{line}"
            );
            assert_eq!(
                random.next_long(),
                fields[4].parse::<i64>().unwrap(),
                "{line}"
            );
            for _ in 0..3 {
                let line = rows.next().unwrap();
                let fields: Vec<_> = line.split_whitespace().collect();
                assert_eq!(fields.len(), 6, "{line}");
                let pos = [
                    fields[1].parse().unwrap(),
                    fields[2].parse().unwrap(),
                    fields[3].parse().unwrap(),
                ];
                let block = region.read(pos).unwrap();
                let key = block
                    .properties
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",");
                assert_eq!(block.name, fields[4], "{line}");
                assert_eq!(key, fields[5], "{line}");
            }
            assert!(region.feature_marks.is_empty());
            assert!(region.world.saved_block_ticks.is_empty());
            assert!(region.world.saved_fluid_ticks.is_empty());
            region.finish().unwrap();
        }
        assert!(rows.next().is_none());
    }

    #[test]
    fn randomized_cave_vines_write_to_generated_chunk() {
        struct VinesData;
        impl ResourceProvider for VinesData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/example/worldgen/block_state_provider/head.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"type":"minecraft:randomized_int","property":"age","source":{"id":"minecraft:cave_vines","properties":{"age":"0","berries":"true"}},"values":{"type":"minecraft:uniform","min_inclusive":23,"max_inclusive":25}}),
                    )?));
                }
                Data.read_data(path)
            }
        }
        let value = json!({"feature":{"type":"minecraft:block_column","direction":"down","allowed_placement":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"},"prioritize_tip":true,"layers":[
            {"height":2,"provider":{"id":"minecraft:cave_vines_plant","properties":{"berries":"true"}}},
            {"height":1,"provider":"example:head"}
        ]},"placement":[]});
        let placed = PlacedDecorationFeature::from_placement(
            &mut VinesData,
            PlacedFeature::decode(&value).unwrap(),
            &mut resolve_tree_test,
        )
        .unwrap();
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:cave_vines".into(),
            "minecraft:cave_vines_plant".into(),
        ]);
        let tags = OreBlockTags::load(&mut VinesData, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut VinesData, &registry).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: None,
            calls: 0,
        };
        assert!(
            region
                .place_placed(
                    &placed,
                    &mut DecorationRandom::new(42),
                    [8, 12, 8],
                    0,
                    16,
                    &mut env
                )
                .unwrap()
        );
        region.finish().unwrap();
        let after = terrain.snapshot();
        for y in [12, 11] {
            let body = after.block(8, y, 8).unwrap();
            assert_eq!(body.name, "minecraft:cave_vines_plant");
            assert_eq!(body.properties["berries"], "true");
        }
        let head = after.block(8, 10, 8).unwrap();
        assert_eq!(head.name, "minecraft:cave_vines");
        assert_eq!(head.properties["berries"], "true");
        assert!((23..=25).contains(&head.properties["age"].parse::<i32>().unwrap()));
        assert_eq!(before.block(8, 10, 8).unwrap().name, "minecraft:air");
    }

    #[test]
    fn huge_mushroom_updates_generated_terrain_and_heightmaps() {
        struct MushroomData;
        impl ResourceProvider for MushroomData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                match path {
                    "data/minecraft/tags/block/leaves.json" | "data/minecraft/tags/block/replaceable_by_mushrooms.json" => Ok(Some(br#"{"values":[]}"#.to_vec())),
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json" | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => Ok(Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore","minecraft:brown_mushroom_block","minecraft:mushroom_stem"]}"#.to_vec())),
                    _ => Data.read_data(path),
                }
            }
        }
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:brown_mushroom_block".into(),
            "minecraft:mushroom_stem".into(),
        ]);
        let definition = json!({
            "type":"minecraft:huge_brown_mushroom", "foliage_radius":2,
            "cap_provider":{"id":"minecraft:brown_mushroom_block"},
            "stem_provider":{"id":"minecraft:mushroom_stem"},
            "can_place_on":{"type":"minecraft:matching_blocks","blocks":["minecraft:stone"]}
        });
        let feature = HugeMushroomFeature::decode(&mut MushroomData, &definition, &mut |value| {
            let mut result = resolve_test(value)?;
            result.properties = ["up", "down", "north", "south", "west", "east"]
                .into_iter()
                .map(|name| (name.into(), "true".into()))
                .collect();
            Ok(result)
        })
        .unwrap();
        let tags =
            OreBlockTags::load(&mut MushroomData, &registry, &feature.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut MushroomData, &registry).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let seed = (0..100)
            .find(|&seed| {
                let mut random = DecorationRandom::new(seed);
                let _ = random.next_int_bound(3);
                random.next_int_bound(12) != Some(0)
            })
            .unwrap();
        let mut expected_random = DecorationRandom::new(seed);
        let height = expected_random.next_int_bound(3).unwrap() + 4;
        assert!(
            feature
                .place(&mut region, &mut DecorationRandom::new(seed), [8, 8, 8])
                .unwrap()
        );
        assert_eq!(region.final_height(0, 8, 8).unwrap(), 9 + height);
        assert_eq!(region.final_height(1, 8, 8).unwrap(), 9 + height);
        region.finish().unwrap();
        let after = terrain.snapshot();
        assert_eq!(
            after.block(8, 8, 8).unwrap().name,
            "minecraft:mushroom_stem"
        );
        assert_eq!(
            after.block(8, 8 + height, 8).unwrap().name,
            "minecraft:brown_mushroom_block"
        );
        assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:air");
    }

    struct TreeData;
    impl ResourceProvider for TreeData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            let value = match path {
                "data/minecraft/worldgen/biome/plains.json" => json!({"features":[[{"feature":{
                    "type":"minecraft:tree",
                    "trunk_provider":{"id":"minecraft:oak_log","properties":{"axis":"y"}},
                    "trunk_placer":{"type":"minecraft:straight_trunk_placer","base_height":4,"height_rand_a":2,"height_rand_b":0},
                    "foliage_provider":{"id":"minecraft:oak_leaves","properties":{"distance":"7","persistent":"false","waterlogged":"false"}},
                    "foliage_placer":{"type":"minecraft:blob_foliage_placer","height":3,"radius":2,"offset":0},
                    "below_trunk_provider":{"id":"minecraft:dirt"},"minimum_size":{"type":"minecraft:two_layers_feature_size"},"decorators":[],"ignore_vines":true
                },"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8}]}]]}),
                "data/minecraft/tags/block/replaceable_by_trees.json" => {
                    json!({"values":["minecraft:oak_leaves"]})
                }
                "data/minecraft/tags/block/snow.json" => json!({"values":[]}),
                "data/minecraft/tags/block/supports_dry_vegetation.json" => {
                    json!({"values":["minecraft:stone"]})
                }
                "data/minecraft/tags/block/logs.json"
                | "data/minecraft/tags/block/prevents_nearby_leaf_decay.json" => {
                    json!({"values":["minecraft:oak_log"]})
                }
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json" => {
                    json!({"values":["minecraft:stone","minecraft:diamond_ore","minecraft:dirt","minecraft:grass_block","minecraft:oak_log","minecraft:oak_leaves"]})
                }
                "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                    json!({"values":["minecraft:stone","minecraft:diamond_ore","minecraft:dirt","minecraft:grass_block","minecraft:oak_log"]})
                }
                _ => return Data.read_data(path),
            };
            Ok(Some(serde_json::to_vec(&value)?))
        }
    }
    fn resolve_tree_test(v: &Value) -> Result<BlockState> {
        let mut block = resolve_test(v)?;
        if let Some(values) = v.get("properties").and_then(Value::as_object) {
            for (key, value) in values {
                block.properties.insert(
                    key.clone(),
                    value
                        .as_str()
                        .context("invalid test state property")?
                        .into(),
                );
            }
        }
        Ok(block)
    }
    fn prepare_tree_fixture() -> (
        FeatureSchedule,
        super::super::BiomeNeighborhood,
        BTreeSet<String>,
    ) {
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut TreeData,
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:dirt".into(),
            "minecraft:grass_block".into(),
            "minecraft:oak_log".into(),
            "minecraft:oak_leaves".into(),
        ]);
        (schedule, neighborhood, registry)
    }
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn monster_room_block_flags_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
                let p = game.join("libraries").join(p);
                if p.is_file() {
                    paths.push(p)
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("MonsterRoomBlockOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/MonsterRoomBlockOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let o = c.output().unwrap();
            assert!(
                o.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            );
            String::from_utf8(o.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("MonsterRoomBlockOracle"));
        let mut count = 0;
        for line in output.lines().filter_map(|v| v.strip_prefix("C ")) {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            assert_eq!(parts.len(), 6);
            let mut block = state(parts[0]);
            if parts[1] != "-" {
                for entry in parts[1].split(',') {
                    let (key, value) = entry.split_once('=').unwrap();
                    block.properties.insert(key.into(), value.into());
                }
            }
            let expected = (
                parts[2].parse::<bool>().unwrap(),
                parts[3].parse::<bool>().unwrap(),
            );
            assert_eq!(monster_block_flags(&block).unwrap(), expected, "{line}");
            assert_eq!(
                parts[4].parse::<bool>().unwrap(),
                matches!(parts[0], "chest" | "spawner"),
                "{line}"
            );
            assert!(
                !parts[5].parse::<bool>().unwrap(),
                "unexpected postprocessing: {line}"
            );
            count += 1;
        }
        assert!(count >= 70, "missing block states");
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn installed_monster_room_matches_java_and_preserves_entities() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
                let p = game.join("libraries").join(p);
                if p.is_file() {
                    paths.push(p)
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("MonsterRoomOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../resources/tests/java/MonsterRoomOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let o = c.output().unwrap();
            assert!(
                o.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            );
            String::from_utf8(o.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("MonsterRoomOracle")
            .arg("chunk"));
        let grassy_output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("MonsterRoomOracle")
            .arg("chunk_grass"));
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let generated = resources.vanilla_generated_block_registry().unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let placement =
            PlacedFeature::decode(&json!({"feature":"minecraft:monster_room","placement":[]}))
                .unwrap();
        let placed = PlacedDecorationFeature::from_placement(&mut resources, placement, &mut |v| {
            super::super::resolve_generated_terrain_state(v, &generated)
        })
        .unwrap();
        let tags = OreBlockTags::load(&mut resources, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        let mut saw_chest = false;
        let mut saw_spawner = false;
        for (grassy, seed) in [false, true].into_iter().flat_map(|grassy| {
            [0, 1, -1, 987654321]
                .into_iter()
                .map(move |seed| (grassy, seed))
        }) {
            let output = if grassy { &grassy_output } else { &output };
            for mode in 0..4 {
                let mut preview = DecorationRandom::new(seed);
                let rx = preview.next_int_bound(2).unwrap() + 2;
                let rz = preview.next_int_bound(2).unwrap() + 2;
                let (mut terrain, _, _, _) = fixture();
                let world = Arc::make_mut(&mut terrain.world);
                let stone = world.intern_state(state("stone"));
                let grass = world.intern_state(BlockState {
                    name: "minecraft:grass_block".into(),
                    properties: BTreeMap::from([("snowy".into(), "false".into())]),
                });
                let air = world.intern_state(state("air"));
                let bedrock = world.intern_state(state("bedrock"));
                let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
                let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
                let blocks = section.blocks.as_mut().unwrap();
                blocks.fill(if grassy { grass } else { stone });
                let index = |x: i32, y: i32, z: i32| -> usize { (x + z * 16 + y * 256) as usize };
                blocks[index(8 - rx - 1, 8, 8)] = air;
                blocks[index(8 - rx - 1, 9, 8)] = air;
                if mode == 1 {
                    blocks[index(8 - rx - 1, 7, 8 - rz - 1)] = air;
                }
                if mode == 2 {
                    for x in [8 - rx - 1, 8 + rx + 1] {
                        for z in 8 - rz..=8 + rz {
                            blocks[index(x, 8, z)] = air;
                            blocks[index(x, 9, z)] = air;
                        }
                    }
                }
                if mode == 3 {
                    blocks[index(8 - rx - 1, 7, 9)] = bedrock;
                    blocks[index(9, 8, 9)] = bedrock;
                }
                let before = terrain.snapshot();
                let writable = before.chunks.keys().copied().collect();
                let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
                let mut random = DecorationRandom::new(seed);
                let mut env = PlacementTestEnv {
                    allow: true,
                    fail_after: None,
                    calls: 0,
                };
                let ok = region
                    .place_placed(&placed, &mut random, [8, 8, 8], 0, 16, &mut env)
                    .unwrap();
                region.finish().unwrap();
                let actual = terrain.snapshot();
                let prefix = format!("C {seed} {mode} ");
                let line = output
                    .lines()
                    .find_map(|l| l.strip_prefix(&prefix))
                    .unwrap();
                let (java_ok, rest) = line.split_once(" W ").unwrap();
                let (writes, rest) = rest.split_once(" T ").unwrap();
                let (java_chests, rest) = rest.split_once(" M ").unwrap();
                let (java_mob, java_random) = rest.split_once(" R ").unwrap();
                assert_eq!(ok, java_ok.parse::<bool>().unwrap());
                assert_eq!(random.next_long(), java_random.parse::<i64>().unwrap());
                let mut expected = BTreeMap::new();
                if !writes.is_empty() {
                    for item in writes.split(';') {
                        let parts = item.split(',').collect::<Vec<_>>();
                        let pos = [
                            parts[0].parse::<i32>().unwrap(),
                            parts[1].parse::<i32>().unwrap(),
                            parts[2].parse::<i32>().unwrap(),
                        ];
                        let replacement = match parts[3] {
                            "air" => state("cave_air"),
                            "cobble" => state("cobblestone"),
                            "mossy" => state("mossy_cobblestone"),
                            "spawner" => state("spawner"),
                            value if value.starts_with("chest:") => {
                                let mut s = state("chest");
                                s.properties = BTreeMap::from([
                                    ("facing".into(), value[6..].into()),
                                    ("type".into(), "single".into()),
                                    ("waterlogged".into(), "false".into()),
                                ]);
                                s
                            }
                            _ => panic!("unknown Java write"),
                        };
                        expected.insert(pos, replacement);
                    }
                }
                for x in 0..16 {
                    for y in 0..16 {
                        for z in 0..16 {
                            let p = [x, y, z];
                            assert_eq!(
                                actual.block(x, y, z),
                                expected.get(&p).or_else(|| before.block(x, y, z)),
                                "seed {seed} mode {mode} block {p:?}"
                            );
                        }
                    }
                }
                let mut chest_data = Vec::new();
                let mut mob = "-";
                for entity in terrain.generated_monster_entities() {
                    match entity {
                        super::super::GeneratedMonsterRoomEntity::Chest(chest) => {
                            saw_chest = true;
                            let [x, y, z] = chest.position;
                            chest_data.push(format!("{x},{y},{z},{}", chest.loot_seed));
                            assert!(
                                matches!(chest.block_entity().get("LootTable"),Some(native_persistence::nbt::Tag::String(value)) if value.matches("minecraft:chests/simple_dungeon"))
                            );
                        }
                        super::super::GeneratedMonsterRoomEntity::Spawner(spawner) => {
                            saw_spawner = true;
                            mob = match spawner.mob {
                                RoomMob::Skeleton => "minecraft:skeleton",
                                RoomMob::Zombie => "minecraft:zombie",
                                RoomMob::Spider => "minecraft:spider",
                            };
                        }
                    }
                }
                chest_data.sort();
                assert_eq!(chest_data.join(";"), java_chests);
                assert_eq!(mob, java_mob);
            }
        }
        assert!(
            saw_chest && saw_spawner,
            "oracle seeds must exercise block entities"
        );
        // Unsupported data encountered after wall writes must discard the draft.
        let mut probe = DecorationRandom::new(0);
        let rx = probe.next_int_bound(2).unwrap() + 2;
        let rz = probe.next_int_bound(2).unwrap() + 2;
        let (mut terrain, _, _, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let stone = world.intern_state(state("stone"));
        let air = world.intern_state(state("air"));
        let unknown = world.intern_state(state("beacon"));
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        let blocks = section.blocks.as_mut().unwrap();
        blocks.fill(stone);
        let index = |x: i32, y: i32, z: i32| -> usize { (x + z * 16 + y * 256) as usize };
        blocks[index(8 - rx - 1, 8, 8)] = air;
        blocks[index(8 - rx - 1, 9, 8)] = air;
        blocks[index(8 - rx - 1, 6, 8 - rz - 1)] = unknown;
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(0);
        let mut unchanged = random.clone();
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
        assert_eq!(random.next_long(), unchanged.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        assert_eq!(terrain.generated_monster_entities().count(), 0);
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn installed_geode_matches_java_in_generated_chunks() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("GeodeOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../resources/tests/java/GeodeOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let oracle = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GeodeOracle"));
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let generated = resources.vanilla_generated_block_registry().unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let placement =
            PlacedFeature::decode(&json!({"feature":"minecraft:amethyst_geode","placement":[]}))
                .unwrap();
        let placed = PlacedDecorationFeature::from_placement(&mut resources, placement, &mut |v| {
            super::super::resolve_generated_terrain_state(v, &generated)
        })
        .unwrap();
        let tags = OreBlockTags::load(&mut resources, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        let mut wrote_crystal = false;
        let mut saw_tick = false;
        for seed in [0, 1, -1, 987654321] {
            for mode in 0..3 {
                let (mut terrain, _, _, _) = fixture();
                terrain.seed = seed + 123;
                terrain.height = 48;
                let world = Arc::make_mut(&mut terrain.world);
                let stone = world.intern_state(state("stone"));
                let bedrock = world.intern_state(state("bedrock"));
                let air = world.intern_state(state("air"));
                let mut water_state = state("water");
                water_state.properties.insert("level".into(), "0".into());
                let water = world.intern_state(water_state);
                for (pos, chunk) in &mut world.chunks {
                    let chunk = Arc::make_mut(chunk);
                    chunk.sections.clear();
                    for sy in 0..3 {
                        let mut blocks = Box::new([stone; BLOCK_COUNT]);
                        for y in 0..16 {
                            for z in 0..16 {
                                for x in 0..16 {
                                    let (ax, ay, az) =
                                        (pos.x * 16 + x, sy * 16 + y, pos.z * 16 + z);
                                    blocks[(x + z * 16 + y * 256) as usize] = if mode == 2 {
                                        air
                                    } else if mode == 1 && (ax + ay + az).rem_euclid(17) == 0 {
                                        bedrock
                                    } else if mode == 1 && (ax * 3 + ay + az).rem_euclid(23) == 0 {
                                        water
                                    } else {
                                        stone
                                    };
                                }
                            }
                        }
                        chunk.sections.insert(
                            sy as i8,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        );
                    }
                }
                let before = terrain.snapshot();
                let writable = before.chunks.keys().copied().collect();
                let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
                let mut random = DecorationRandom::new(seed);
                let mut env = PlacementTestEnv {
                    allow: true,
                    fail_after: None,
                    calls: 0,
                };
                let ok = region
                    .place_placed(&placed, &mut random, [0, 20, 0], 0, 48, &mut env)
                    .unwrap();
                region.finish().unwrap();
                let after = terrain.snapshot();
                let prefix = format!("C {seed} {mode} ");
                let line = oracle
                    .lines()
                    .find_map(|l| l.strip_prefix(&prefix))
                    .unwrap();
                let (java_ok, rest) = line.split_once(" W ").unwrap();
                let (writes, rest) = rest.split_once(" T ").unwrap();
                let (ticks, next) = rest.split_once(" R ").unwrap();
                assert_eq!(ok, java_ok.parse::<bool>().unwrap());
                assert_eq!(random.next_long(), next.parse::<i64>().unwrap());
                let mut expected = BTreeMap::new();
                if !writes.is_empty() {
                    for item in writes.split(';') {
                        let parts = item.split(',').collect::<Vec<_>>();
                        let pos = [
                            parts[0].parse::<i32>().unwrap(),
                            parts[1].parse::<i32>().unwrap(),
                            parts[2].parse::<i32>().unwrap(),
                        ];
                        let mut kind = parts[3].split(':');
                        let name = kind.next().unwrap();
                        let mut block = state(name);
                        if let Some(facing) = kind.next() {
                            block.properties.insert("facing".into(), facing.into());
                            block
                                .properties
                                .insert("waterlogged".into(), kind.next().unwrap().into());
                            wrote_crystal = true;
                        }
                        expected.insert(pos, block);
                    }
                }
                for z in -16..=16 {
                    for y in 4..=36 {
                        for x in -16..=16 {
                            let p = [x, y, z];
                            assert_eq!(
                                after.block(x, y, z),
                                expected.get(&p).or_else(|| before.block(x, y, z)),
                                "seed {seed} mode {mode} {p:?}"
                            );
                        }
                    }
                }
                let expected_ticks: BTreeSet<_> = if ticks.is_empty() {
                    BTreeSet::new()
                } else {
                    ticks
                        .split(';')
                        .map(|v| {
                            let a = v
                                .split(',')
                                .map(|v| v.parse::<i32>().unwrap())
                                .collect::<Vec<_>>();
                            [a[0], a[1], a[2]]
                        })
                        .collect()
                };
                let actual_ticks: BTreeSet<_> = after
                    .saved_fluid_ticks
                    .values()
                    .flatten()
                    .map(|v| v.position)
                    .collect();
                saw_tick |= !actual_ticks.is_empty();
                assert_eq!(actual_ticks, expected_ticks, "seed {seed} mode {mode}");
            }
        }
        assert!(
            wrote_crystal && saw_tick,
            "oracle must cover crystal and fluid effects"
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn installed_glow_lichen_matches_java_in_generated_chunks() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("MultifaceOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../resources/tests/java/MultifaceOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let output = c.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let oracle = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("MultifaceOracle")
            .arg("chunk"));
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let generated = resources.vanilla_generated_block_registry().unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let placement =
            PlacedFeature::decode(&json!({"feature":"minecraft:glow_lichen","placement":[]}))
                .unwrap();
        let placed = PlacedDecorationFeature::from_placement(&mut resources, placement, &mut |v| {
            super::super::resolve_generated_terrain_state(v, &generated)
        })
        .unwrap();
        let tags = OreBlockTags::load(&mut resources, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let mut saw_write = false;
        let mut saw_spread = false;
        for mode in 0..6 {
            for seed in [0, 1, -1, 123456789] {
                let (mut terrain, _, _, _) = fixture();
                let world = Arc::make_mut(&mut terrain.world);
                let air = world.intern_state(state("air"));
                let stone = world.intern_state(state("stone"));
                let mut water = state("water");
                water.properties.insert("level".into(), "0".into());
                let source_water = world.intern_state(water.clone());
                water.properties.insert("level".into(), "5".into());
                let flowing_water = world.intern_state(water);
                let mut lichen = super::super::resolve_generated_terrain_state(
                    &json!("minecraft:glow_lichen"),
                    &generated,
                )
                .unwrap();
                lichen.properties.insert("east".into(), "true".into());
                let lichen = world.intern_state(lichen);
                let chunk = Arc::make_mut(world.chunks.get_mut(&center).unwrap());
                let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
                let blocks = section.blocks.as_mut().unwrap();
                for y in 0..16i32 {
                    for z in 0..16i32 {
                        for x in 0..16i32 {
                            let (dx, dy, dz) = (x - 8, y - 8, z - 8);
                            blocks[(x + z * 16 + y * 256) as usize] = if mode == 0 {
                                air
                            } else if mode == 1 && dy >= 1 {
                                stone
                            } else if mode >= 2
                                && (dx >= 2
                                    || dy >= 2
                                    || dz >= 2
                                    || dx <= -2
                                    || dy <= -2
                                    || dz <= -2)
                            {
                                stone
                            } else if mode == 3 {
                                source_water
                            } else if mode == 4 {
                                flowing_water
                            } else if mode == 5 && (dx, dy, dz) == (1, 0, 0) {
                                lichen
                            } else {
                                air
                            };
                        }
                    }
                }
                let before = terrain.snapshot();
                let mut region = terrain
                    .decoration_region(&tags, &motion, BTreeSet::from([center]))
                    .unwrap();
                let mut random = DecorationRandom::new(seed);
                let mut env = PlacementTestEnv {
                    allow: true,
                    fail_after: None,
                    calls: 0,
                };
                let ok = region
                    .place_placed(&placed, &mut random, [8, 8, 8], 0, 16, &mut env)
                    .unwrap();
                region.finish().unwrap();
                let prefix = format!("C {mode} 0 {seed} ");
                let line = oracle
                    .lines()
                    .find_map(|l| l.strip_prefix(&prefix))
                    .unwrap();
                let (java_ok, rest) = line.split_once(" W ").unwrap();
                let (writes, rest) = rest.split_once(" M ").unwrap();
                let (marks, next) = rest.split_once(" R ").unwrap();
                assert_eq!(ok, java_ok.parse::<bool>().unwrap());
                assert_eq!(random.next_long(), next.parse::<i64>().unwrap());
                let mut expected = BTreeMap::new();
                if !writes.is_empty() {
                    for item in writes.split(';') {
                        let a = item.split(',').collect::<Vec<_>>();
                        let p = [
                            a[0].parse::<i32>().unwrap(),
                            a[1].parse::<i32>().unwrap(),
                            a[2].parse::<i32>().unwrap(),
                        ];
                        let (mask, wet) = a[3].split_once(':').unwrap();
                        let mask = mask.parse::<u8>().unwrap();
                        let mut block = super::super::resolve_generated_terrain_state(
                            &json!("minecraft:glow_lichen"),
                            &generated,
                        )
                        .unwrap();
                        for (i, name) in ["down", "up", "north", "south", "west", "east"]
                            .into_iter()
                            .enumerate()
                        {
                            block
                                .properties
                                .insert(name.into(), (mask & (1 << i) != 0).to_string());
                        }
                        block.properties.insert("waterlogged".into(), wet.into());
                        expected.insert(p, block);
                        saw_write = true;
                        saw_spread |= a[4] == "false";
                    }
                }
                let after = terrain.snapshot();
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [x, y, z];
                            assert_eq!(
                                after.block(x, y, z),
                                expected.get(&p).or_else(|| before.block(x, y, z)),
                                "mode {mode} seed {seed} {p:?}"
                            );
                        }
                    }
                }
                let expected_marks: Vec<i16> = if marks.is_empty() {
                    vec![]
                } else {
                    marks
                        .split(';')
                        .map(|v| {
                            let a = v
                                .split(',')
                                .map(|x| x.parse::<i32>().unwrap())
                                .collect::<Vec<_>>();
                            (a[0] | a[1] << 4 | a[2] << 8) as i16
                        })
                        .collect()
                };
                let actual_marks = terrain
                    .feature_postprocessing(center)
                    .map(|p| p.sections()[0].clone())
                    .unwrap_or_default();
                assert_eq!(actual_marks, expected_marks, "mode {mode} seed {seed}");
            }
        }
        assert!(
            saw_write && saw_spread,
            "oracle must cover placement and spread"
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn installed_flower_noise_provider_matches_java_in_generated_chunk() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("NoiseThresholdOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../resources/tests/java/NoiseThresholdOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let output = c.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let oracle = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("NoiseThresholdOracle")
            .arg("chunk"));
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let generated = resources.vanilla_generated_block_registry().unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let placement =
            PlacedFeature::decode(&json!({"feature":"minecraft:flower_plain","placement":[]}))
                .unwrap();
        let placed = PlacedDecorationFeature::from_placement(&mut resources, placement, &mut |v| {
            super::super::resolve_generated_terrain_state(v, &generated)
        })
        .unwrap();
        let tags = OreBlockTags::load(&mut resources, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        for seed in [0, 1] {
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            let grass = world.intern_state(
                super::super::resolve_generated_terrain_state(
                    &json!("minecraft:grass_block"),
                    &generated,
                )
                .unwrap(),
            );
            let center = ChunkPos { x: 0, z: 0 };
            let blocks = Arc::make_mut(
                Arc::make_mut(world.chunks.get_mut(&center).unwrap())
                    .sections
                    .get_mut(&0)
                    .unwrap(),
            )
            .blocks
            .as_mut()
            .unwrap();
            for z in 0..16 {
                for x in 0..16 {
                    blocks[x + z * 16 + 7 * 256] = grass;
                }
            }
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([center]))
                .unwrap();
            let mut checked = 0;
            for line in oracle.lines().filter(|v| v.starts_with("C ")) {
                let a = line.split_whitespace().collect::<Vec<_>>();
                if a[4].parse::<i64>().unwrap() != seed {
                    continue;
                }
                let p = [
                    a[1].parse().unwrap(),
                    a[2].parse().unwrap(),
                    a[3].parse().unwrap(),
                ];
                let mut random = DecorationRandom::new(seed);
                let mut env = PlacementTestEnv {
                    allow: true,
                    fail_after: None,
                    calls: 0,
                };
                assert!(
                    region
                        .place_placed(&placed, &mut random, p, 0, 16, &mut env)
                        .unwrap()
                );
                assert_eq!(random.next_long(), a[6].parse::<i64>().unwrap());
                assert_eq!(region.read(p).unwrap().name, a[5]);
                checked += 1;
            }
            assert_eq!(checked, 256);
            region.finish().unwrap();
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn installed_mushroom_features_match_java_brightness_and_support() {
        use std::{cell::Cell, path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("MushroomGenerationOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/MushroomGenerationOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let output = c.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let oracle = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("MushroomGenerationOracle"));
        let effects = oracle
            .lines()
            .filter(|line| line.starts_with("E "))
            .collect::<Vec<_>>();
        assert_eq!(effects.len(), 2);
        assert!(
            effects
                .iter()
                .all(|line| line.ends_with("false false BlockPos{x=0, y=0, z=0}")),
            "{effects:?}"
        );
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let generated = resources.vanilla_generated_block_registry().unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        let mut features = BTreeMap::new();
        for name in ["brown_mushroom", "red_mushroom"] {
            let placement = PlacedFeature::decode(
                &json!({"feature":format!("minecraft:{name}"),"placement":[]}),
            )
            .unwrap();
            let placed =
                PlacedDecorationFeature::from_placement(&mut resources, placement, &mut |v| {
                    super::super::resolve_generated_terrain_state(v, &generated)
                })
                .unwrap();
            features.insert(format!("minecraft:{name}"), placed);
        }
        struct TagOverride<'a> {
            resources: &'a mut native_resources::packs::ResourceStack,
            base: &'a str,
            enabled: bool,
        }
        impl ResourceProvider for TagOverride<'_> {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.resources.read_asset(path)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/overrides_mushroom_light_requirement.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"values":if self.enabled {vec![self.base]} else {vec![]}}),
                    )?));
                }
                self.resources.read_data(path)
            }
        }
        let required =
            BTreeSet::from([
                ResourceId::parse("minecraft:overrides_mushroom_light_requirement").unwrap(),
            ]);
        let mut tag_cache = BTreeMap::new();
        let mut count = 0;
        for line in oracle.lines().filter(|v| v.starts_with("C ")) {
            let a = line.split_whitespace().collect::<Vec<_>>();
            assert_eq!(a.len(), 10);
            let mushroom = a[1];
            let base = a[2];
            let overridden = a[3].parse::<bool>().unwrap();
            let light = a[4].parse::<u8>().unwrap();
            let expected = a[5].parse::<bool>().unwrap();
            let expected_calls = a[6].parse::<u32>().unwrap();
            let key = (base.to_owned(), overridden);
            if !tag_cache.contains_key(&key) {
                let tags = OreBlockTags::load(
                    &mut TagOverride {
                        resources: &mut resources,
                        base,
                        enabled: overridden,
                    },
                    &registry,
                    &required,
                )
                .unwrap();
                tag_cache.insert(key.clone(), tags);
            }
            let tags = &tag_cache[&key];
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            let mut support = state(base.strip_prefix("minecraft:").unwrap());
            if matches!(
                base,
                "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
            ) {
                support.properties.insert("snowy".into(), "false".into());
            }
            if base == "minecraft:water" {
                support.properties.insert("level".into(), "0".into());
            }
            if base == "minecraft:deepslate" {
                support.properties.insert("axis".into(), "y".into());
            }
            if base == "minecraft:chest" {
                support.properties = BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("type".into(), "single".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            let id = world.intern_state(support);
            let center = ChunkPos { x: 0, z: 0 };
            let chunk = Arc::make_mut(world.chunks.get_mut(&center).unwrap());
            Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
                .blocks
                .as_mut()
                .unwrap()[8 + 8 * 16 + 7 * 256] = id;
            let calls = Cell::new(0u32);
            let sampler = |_: &World, _: [i32; 3]| {
                calls.set(calls.get() + 1);
                Ok(light)
            };
            let mut region = terrain
                .decoration_region(tags, &motion, BTreeSet::from([center]))
                .unwrap();
            region.raw_brightness = Some(&sampler);
            let mut random = DecorationRandom::new(33);
            let mut env = PlacementTestEnv {
                allow: true,
                fail_after: None,
                calls: 0,
            };
            let placed = region
                .place_placed(&features[mushroom], &mut random, [8, 8, 8], 0, 16, &mut env)
                .unwrap();
            assert_eq!(placed, expected, "{line}");
            assert_eq!(calls.get(), expected_calls, "{line}");
            assert_eq!(random.next_long(), a[8].parse::<i64>().unwrap());
            assert_eq!(
                region.read([8, 8, 8]).unwrap().name,
                if expected { mushroom } else { "minecraft:air" }
            );
            assert_eq!(a[7], if expected { mushroom } else { "-" });
            if !overridden && light == 0 {
                assert_eq!(
                    expected,
                    a[9].parse::<bool>().unwrap(),
                    "solid render {base}"
                );
            }
            region.finish().unwrap();
            let marks = terrain
                .feature_postprocessing(center)
                .map(|v| v.sections()[0].clone())
                .unwrap_or_default();
            assert_eq!(
                marks,
                if expected { vec![0x888] } else { vec![] },
                "postprocessing {line}"
            );
            count += 1;
        }
        assert_eq!(count, 192);
    }

    #[test]
    fn unfinished_generated_spawner_cannot_be_published() {
        let (mut terrain, tags, _, _) = fixture();
        let mut registry = batch_registry();
        registry.insert("minecraft:spawner".into());
        let motion = HeightmapTags::load(&mut Data, &registry).unwrap();
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        native_resources::monster_room::MonsterRoomWorld::set_block(
            &mut region,
            [0, 8, 0],
            RoomBlock::Spawner,
        )
        .unwrap();
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        assert_eq!(terrain.generated_monster_entities().count(), 0);
    }

    #[test]
    fn planned_oak_tree_is_written_to_generated_chunk() {
        let (schedule, neighborhood, registry) = prepare_tree_fixture();
        let plan = fixture_plan(&schedule, &neighborhood);
        let prepared =
            PreparedDecoration::load(&mut TreeData, &plan, &registry, &mut resolve_tree_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
        let world = terrain.snapshot();
        assert_eq!(world.block(8, 7, 8).unwrap().name, "minecraft:dirt");
        let mut logs = 0;
        let mut leaves = 0;
        for x in 4..=12 {
            for y in 8..16 {
                for z in 4..=12 {
                    let block = world.block(x, y, z).unwrap();
                    if block.name == "minecraft:oak_log" {
                        logs += 1;
                    }
                    if block.name == "minecraft:oak_leaves" {
                        leaves += 1;
                        assert!(block.properties["distance"].parse::<u8>().unwrap() < 7);
                    }
                }
            }
        }
        assert!((4..=6).contains(&logs));
        assert!(leaves > 10);
        assert!(
            !world
                .saved_block_ticks
                .values()
                .flatten()
                .collect::<Vec<_>>()
                .is_empty()
        );
    }

    #[test]
    fn planned_azalea_tree_updates_generated_leaves_and_soil() {
        struct AzaleaData;
        impl ResourceProvider for AzaleaData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/minecraft/worldgen/biome/plains.json" => {
                        let mut v: Value =
                            serde_json::from_slice(&TreeData.read_data(path)?.unwrap())?;
                        let tree = &mut v["features"][0][0]["feature"];
                        tree["below_trunk_provider"] = json!({"id":"minecraft:rooted_dirt"});
                        tree["trunk_placer"] = json!({"type":"minecraft:bending_trunk_placer","base_height":4,"height_rand_a":2,"height_rand_b":0,"min_height_for_leaves":3,"bend_length":{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":2}});
                        tree["foliage_provider"] = json!({"type":"minecraft:weighted","entries":[
                            {"data":{"id":"minecraft:azalea_leaves","properties":{"distance":"7","persistent":"false","waterlogged":"false"}},"weight":3},
                            {"data":{"id":"minecraft:flowering_azalea_leaves","properties":{"distance":"7","persistent":"false","waterlogged":"false"}},"weight":1}]});
                        tree["foliage_placer"] = json!({"type":"minecraft:random_spread_foliage_placer","radius":3,"offset":0,"foliage_height":2,"leaf_placement_attempts":50});
                        v
                    }
                    "data/minecraft/tags/block/replaceable_by_trees.json" => {
                        json!({"values":["minecraft:azalea_leaves","minecraft:flowering_azalea_leaves"]})
                    }
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json" => {
                        json!({"values":["minecraft:stone","minecraft:diamond_ore","minecraft:rooted_dirt","minecraft:grass_block","minecraft:oak_log","minecraft:azalea_leaves","minecraft:flowering_azalea_leaves"]})
                    }
                    "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        json!({"values":["minecraft:stone","minecraft:diamond_ore","minecraft:rooted_dirt","minecraft:grass_block","minecraft:oak_log"]})
                    }
                    _ => return TreeData.read_data(path),
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(&mut AzaleaData, &[plains], &BTreeSet::new()).unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let (_, _, mut registry) = prepare_tree_fixture();
        registry.extend([
            "minecraft:rooted_dirt".into(),
            "minecraft:azalea_leaves".into(),
            "minecraft:flowering_azalea_leaves".into(),
        ]);
        let prepared =
            PreparedDecoration::load(&mut AzaleaData, &plan, &registry, &mut resolve_tree_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
        let world = terrain.snapshot();
        assert_eq!(world.block(8, 7, 8).unwrap().name, "minecraft:rooted_dirt");
        let mut leaves = 0;
        for x in 2..=14 {
            for y in 8..16 {
                for z in 2..=14 {
                    let state = world.block(x, y, z).unwrap();
                    if matches!(
                        state.name.as_str(),
                        "minecraft:azalea_leaves" | "minecraft:flowering_azalea_leaves"
                    ) {
                        leaves += 1;
                        assert!(
                            (1..=7).contains(&state.properties["distance"].parse::<u8>().unwrap())
                        );
                    }
                }
            }
        }
        assert!(leaves > 10);
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_dark_oak_chunk_and_ticks_match_java() {
        use std::{path::PathBuf, process::Command};
        struct DarkData;
        impl ResourceProvider for DarkData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/minecraft/worldgen/biome/plains.json" => {
                        let mut v: Value =
                            serde_json::from_slice(&TreeData.read_data(path)?.unwrap())?;
                        let tree = &mut v["features"][0][0]["feature"];
                        tree["trunk_provider"] =
                            json!({"id":"minecraft:dark_oak_log","properties":{"axis":"y"}});
                        tree["trunk_placer"] = json!({"type":"minecraft:dark_oak_trunk_placer","base_height":6,"height_rand_a":2,"height_rand_b":1});
                        tree["foliage_provider"] = json!({"id":"minecraft:dark_oak_leaves","properties":{"distance":"7","persistent":"false","waterlogged":"false"}});
                        tree["foliage_placer"] = json!({"type":"minecraft:dark_oak_foliage_placer","radius":0,"offset":0});
                        tree["minimum_size"] =
                            json!({"type":"minecraft:three_layers_feature_size","upper_size":2});
                        v["features"][0][0]["placement"][0]["y"] = json!(5);
                        v
                    }
                    "data/minecraft/tags/block/replaceable_by_trees.json"
                    | "data/minecraft/tags/block/leaves.json" => {
                        json!({"values":["minecraft:dark_oak_leaves"]})
                    }
                    "data/minecraft/tags/block/logs.json"
                    | "data/minecraft/tags/block/prevents_nearby_leaf_decay.json" => {
                        json!({"values":["minecraft:dark_oak_log"]})
                    }
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json" => {
                        json!({"values":["minecraft:stone","minecraft:diamond_ore","minecraft:dirt","minecraft:grass_block","minecraft:dark_oak_log","minecraft:dark_oak_leaves"]})
                    }
                    "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        json!({"values":["minecraft:stone","minecraft:diamond_ore","minecraft:dirt","minecraft:grass_block","minecraft:dark_oak_log"]})
                    }
                    _ => return TreeData.read_data(path),
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("DarkOakChunkOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/DarkOakChunkOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let o = c.output().unwrap();
            assert!(
                o.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            );
            String::from_utf8(o.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("DarkOakChunkOracle"));
        let actual = output.lines().find(|v| v.starts_with("RESULT ")).unwrap();
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(&mut DarkData, &[plains], &BTreeSet::new()).unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let (_, _, mut registry) = prepare_tree_fixture();
        registry.extend([
            "minecraft:dark_oak_log".into(),
            "minecraft:dark_oak_leaves".into(),
        ]);
        let prepared =
            PreparedDecoration::load(&mut DarkData, &plan, &registry, &mut resolve_tree_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let air = world.intern_state(state("air"));
        for chunk in world.chunks.values_mut() {
            let chunk = Arc::make_mut(chunk);
            let blocks = Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
                .blocks
                .as_mut()
                .unwrap();
            for y in 5..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        blocks[x + z * 16 + y * 256] = air;
                    }
                }
            }
        }
        let success = prepared.execute(&mut terrain, false).unwrap();
        let world = terrain.snapshot();
        let mut blocks = Vec::new();
        for x in 0..16 {
            for y in 4..16 {
                for z in 0..16 {
                    let block = world.block(x, y, z).unwrap();
                    if !matches!(
                        block.name.as_str(),
                        "minecraft:dirt" | "minecraft:dark_oak_log" | "minecraft:dark_oak_leaves"
                    ) {
                        continue;
                    }
                    let extra = match block.name.as_str() {
                        "minecraft:dark_oak_log" => format!(",axis={}", block.properties["axis"]),
                        "minecraft:dark_oak_leaves" => format!(
                            ",distance={},persistent={},waterlogged={}",
                            block.properties["distance"],
                            block.properties["persistent"],
                            block.properties["waterlogged"]
                        ),
                        _ => String::new(),
                    };
                    blocks.push(format!("{x},{y},{z},{}{extra}", block.name));
                }
            }
        }
        let ticks = world
            .saved_block_ticks
            .get(&ChunkPos { x: 0, z: 0 })
            .into_iter()
            .flatten()
            .map(|t| {
                format!(
                    "{},{},{},{}",
                    t.position[0], t.position[1], t.position[2], t.block
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            format!(
                "RESULT {} B {} T {}",
                success[0],
                blocks.join(";"),
                ticks.join(";")
            )
        );
        assert!(success[0]);
    }

    #[test]
    fn tree_ground_decorator_writes_litter_and_finalizes_tree() {
        struct LitterData;
        impl ResourceProvider for LitterData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let Some(bytes) = TreeData.read_data(path)? else {
                    return Ok(None);
                };
                if path != "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(bytes));
                }
                let mut biome: Value = serde_json::from_slice(&bytes)?;
                biome["features"][0][0]["feature"]["decorators"] = json!([{
                    "type":"minecraft:place_on_ground","tries":150,"height":2,
                    "block_state_provider":{"id":"minecraft:leaf_litter","properties":{"facing":"north","segment_amount":"1"}}
                }]);
                Ok(Some(serde_json::to_vec(&biome)?))
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut LitterData,
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let (_, _, mut registry) = prepare_tree_fixture();
        registry.insert("minecraft:leaf_litter".into());
        let prepared =
            PreparedDecoration::load(&mut LitterData, &plan, &registry, &mut resolve_tree_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
        let world = terrain.snapshot();
        let mut litter = 0;
        for x in 5..=11 {
            for z in 5..=11 {
                if world.block(x, 8, z).unwrap().name == "minecraft:leaf_litter" {
                    litter += 1;
                }
            }
        }
        assert!(litter > 0);
        assert_eq!(world.block(8, 8, 8).unwrap().name, "minecraft:oak_log");
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_freeze_chunk_matches_java_and_requires_light() {
        use std::{path::PathBuf, process::Command};
        struct FreezeData(bool);
        impl ResourceProvider for FreezeData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/minecraft/worldgen/biome/plains.json" => {
                        json!({"has_precipitation":true,"temperature":if self.0 {0.8} else {0.0},"features":[[{"feature":{"type":"minecraft:freeze_top_layer"},"placement":[]}]]})
                    }
                    "data/minecraft/tags/block/cannot_support_snow_layer.json" => {
                        json!({"values":["minecraft:ice"]})
                    }
                    "data/minecraft/tags/block/support_override_snow_layer.json" => {
                        json!({"values":[]})
                    }
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        json!({"values":["minecraft:stone","minecraft:grass_block","minecraft:ice"]})
                    }
                    _ => return Data.read_data(path),
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
                let p = game.join("libraries").join(p);
                if p.is_file() {
                    paths.push(p)
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("FreezeChunkOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/FreezeChunkOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let o = c.output().unwrap();
            assert!(
                o.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            );
            String::from_utf8(o.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let java_output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("FreezeChunkOracle"));
        let actual = java_output
            .lines()
            .find(|v| v.starts_with("RESULT "))
            .unwrap();
        let mut installed = native_resources::packs::ResourceStack::open(&game).unwrap();
        let vanilla = PlacedDecorationFeature::load(
            &mut installed,
            &ResourceId::parse("minecraft:freeze_top_layer").unwrap(),
            &mut resolve_test,
        )
        .unwrap();
        assert!(matches!(vanilla.feature, ConfiguredDecoration::Freeze));
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut FreezeData(false),
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:ice".into(),
            "minecraft:snow".into(),
            "minecraft:water".into(),
            "minecraft:grass_block".into(),
        ]);
        let prepared = PreparedDecoration::load(
            &mut FreezeData(false),
            &plan,
            &registry,
            &mut resolve_tree_test,
        )
        .unwrap();
        let make_terrain = || {
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            let stone = world.intern_state(state("stone"));
            let mut grass = state("grass_block");
            grass.properties.insert("snowy".into(), "false".into());
            let grass = world.intern_state(grass);
            let mut water = state("water");
            water.properties.insert("level".into(), "0".into());
            let source = world.intern_state(water.clone());
            water.properties.insert("level".into(), "3".into());
            let flowing = world.intern_state(water);
            let mut snow = state("snow");
            snow.properties.insert("layers".into(), "2".into());
            let snow = world.intern_state(snow);
            let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
            let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
            let blocks = section.blocks.as_mut().unwrap();
            for x in 0..16 {
                for z in 0..16 {
                    let i = x + z * 16;
                    blocks[i + 7 * 256] = [stone, grass, source, flowing][(x + z * 3) % 4];
                    if (x * 7 + z * 3) % 17 == 0 {
                        blocks[i + 8 * 256] = snow;
                    }
                }
            }
            terrain
        };
        let mut unknown = make_terrain();
        let before = unknown.snapshot();
        assert!(prepared.execute(&mut unknown, false).is_err());
        assert!(Arc::ptr_eq(&before, &unknown.snapshot()));
        let mut terrain = make_terrain();
        let light = |p: [i32; 3]| {
            Ok(if (p[0] + 2 * p[2]).rem_euclid(5) == 0 {
                10
            } else {
                9
            })
        };
        assert_eq!(
            prepared
                .execute_with_block_light(&mut terrain, false, &light)
                .unwrap(),
            [true]
        );
        let world = terrain.snapshot();
        let name = |b: &BlockState| -> String {
            match b.name.as_str() {
                "minecraft:ice" => "ice".into(),
                "minecraft:water" => format!("water{}", b.properties["level"]),
                "minecraft:grass_block" => format!("grass{}", b.properties["snowy"]),
                "minecraft:stone" => "stone".into(),
                "minecraft:snow" => format!("snow{}", b.properties["layers"]),
                "minecraft:air" => "air".into(),
                _ => panic!("unexpected freeze state {}", b.name),
            }
        };
        let mut values = Vec::new();
        for x in 0..16 {
            for z in 0..16 {
                values.push(format!(
                    "{x},{z},{},{}",
                    name(world.block(x, 7, z).unwrap()),
                    name(world.block(x, 8, z).unwrap())
                ));
            }
        }
        assert_eq!(actual, format!("RESULT true {}", values.join(";")));
        // The same published engine layers must drive the same freeze result.
        let mut block_layer = [0u8; native_world::loaded::LIGHT_BYTES];
        for y in 0..16usize {
            for z in 0..16usize {
                for x in 0..16usize {
                    let i = x + z * 16 + y * 256;
                    let value = if (x + 2 * z) % 5 == 0 { 10 } else { 9 };
                    if i % 2 == 0 {
                        block_layer[i / 2] |= value;
                    } else {
                        block_layer[i / 2] |= value << 4;
                    }
                }
            }
        }
        let mut block = native_world::generation_light::VisibleLayers::new();
        block.insert(0, Arc::new(block_layer));
        let mut light = native_world::generation_light::VisibleLightSnapshot::new(true, true);
        light.insert_complete_column(
            ChunkPos { x: 0, z: 0 },
            native_world::generation_light::VisibleLightColumn::new(block, Default::default()),
        );
        let mut via_snapshot = make_terrain();
        assert_eq!(
            prepared
                .execute_with_visible_light(&mut via_snapshot, false, &light)
                .unwrap(),
            [true]
        );
        let via_snapshot = via_snapshot.snapshot();
        for x in 0..16 {
            for z in 0..16 {
                for y in [7, 8] {
                    assert_eq!(
                        via_snapshot.block(x, y, z),
                        world.block(x, y, z),
                        "{x},{y},{z}"
                    );
                }
            }
        }
        let warm_schedule = FeatureSchedule::load(
            &mut FreezeData(true),
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let warm_plan = fixture_plan(&warm_schedule, &neighborhood);
        let warm = PreparedDecoration::load(
            &mut FreezeData(true),
            &warm_plan,
            &registry,
            &mut resolve_tree_test,
        )
        .unwrap();
        let mut warm_terrain = make_terrain();
        assert_eq!(warm.execute(&mut warm_terrain, false).unwrap(), [true]);
    }

    #[test]
    fn generated_bee_nest_cannot_be_overwritten_without_entity_handling() {
        let (mut terrain, tags, motion, _) = fixture();
        let pos = [2, 8, 2];
        let nest = BlockState {
            name: "minecraft:bee_nest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "south".into()),
                ("honey_level".into(), "0".into()),
            ]),
        };
        let world = Arc::make_mut(&mut terrain.world);
        let id = world.intern_state(nest.clone());
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[2 + 2 * 16 + 8 * 256] = id;
        Arc::make_mut(&mut terrain.generated_bee_nests).insert(
            pos,
            super::super::GeneratedBeeNest::new(pos, vec![1, 2]).unwrap(),
        );
        let writable = BTreeSet::from([ChunkPos { x: 0, z: 0 }]);
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        assert!(region.set_section_block(pos, &state("air")).is_err());
        assert_eq!(region.read(pos).unwrap(), nest);
        drop(region);
        assert_eq!(terrain.generated_bee_nests().count(), 1);
        assert_eq!(
            terrain.snapshot().block(pos[0], pos[1], pos[2]).unwrap(),
            &nest
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn installed_fallen_oak_chunk_matches_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("FallenTreeChunkOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/FallenTreeChunkOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let output = c.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let actual = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("FallenTreeChunkOracle"));
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let generated = resources.vanilla_generated_block_registry().unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let placement =
            PlacedFeature::decode(&json!({"feature":"minecraft:fallen_oak_tree","placement":[]}))
                .unwrap();
        let placed =
            PlacedDecorationFeature::from_placement(&mut resources, placement, &mut |value| {
                super::super::resolve_generated_terrain_state(value, &generated)
            })
            .unwrap();
        // This selector eagerly loads fancy, fallen and ordinary oak branches.
        let plains_selector =
            PlacedFeature::decode(&json!({"feature":"minecraft:trees_plains","placement":[]}))
                .unwrap();
        let plains_selector = PlacedDecorationFeature::from_placement(
            &mut resources,
            plains_selector,
            &mut |value| super::super::resolve_generated_terrain_state(value, &generated),
        )
        .unwrap();
        let tags = OreBlockTags::load(&mut resources, &registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        let mut expected = Vec::new();
        let mut saw_horizontal_log = false;
        let mut saw_vine = false;
        let mut saw_mushroom = false;
        for seed in [0, 1, -1, 987654321, 11, 42, 100, 218] {
            let (mut terrain, _, _, _) = fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
            let mut random = DecorationRandom::new(seed);
            let mut env = PlacementTestEnv {
                allow: true,
                fail_after: None,
                calls: 0,
            };
            let ok = region
                .place_placed(&placed, &mut random, [8, 8, 8], 0, 16, &mut env)
                .unwrap();
            region.finish().unwrap();
            let world = terrain.snapshot();
            let mut blocks = Vec::new();
            for x in 0..16 {
                for y in 8..16 {
                    for z in 0..16 {
                        let block = world.block(x, y, z).unwrap();
                        let name = block.name.strip_prefix("minecraft:").unwrap();
                        let extra = match name {
                            "air" => continue,
                            "oak_log" => {
                                saw_horizontal_log |= block.properties["axis"] != "y";
                                format!(",{}", block.properties["axis"])
                            }
                            "vine" => {
                                saw_vine = true;
                                format!(
                                    ",{},{},{},{},{}",
                                    block.properties["up"],
                                    block.properties["north"],
                                    block.properties["south"],
                                    block.properties["west"],
                                    block.properties["east"]
                                )
                            }
                            "red_mushroom" | "brown_mushroom" => {
                                saw_mushroom = true;
                                String::new()
                            }
                            _ => panic!("unexpected generated block {}", block.name),
                        };
                        blocks.push(format!("{x},{y},{z},{name}{extra}"));
                    }
                }
            }
            expected.push(format!(
                "C {seed} {ok} B {} R {}",
                blocks.join(";"),
                random.next_long()
            ));
        }
        assert!(
            saw_horizontal_log && saw_vine && saw_mushroom,
            "oracle seeds must exercise logs, vines and mushrooms"
        );
        let actual: Vec<_> = actual
            .lines()
            .filter(|line| line.starts_with("C "))
            .collect();
        assert_eq!(actual.len(), expected.len());
        for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
            assert_eq!(*a, b, "seed case {i}");
        }
        let selector_tags =
            OreBlockTags::load(&mut resources, &registry, &plains_selector.required_tags())
                .unwrap();
        let mut seen = [false; 3];
        for seed in 0..10_000i64 {
            let mut selection = DecorationRandom::new(seed);
            let branch = if selection.next_float() < 0.33333334 {
                0
            } else if selection.next_float() < 0.0125 {
                1
            } else {
                2
            };
            if seen[branch] {
                continue;
            }
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            let dirt = world.intern_state(state("dirt"));
            let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
            let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
            section.blocks.as_mut().unwrap()[8 + 8 * 16 + 7 * 256] = dirt;
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain
                .decoration_region(&selector_tags, &motion, writable)
                .unwrap();
            let mut random = DecorationRandom::new(seed);
            let mut env = PlacementTestEnv {
                allow: true,
                fail_after: None,
                calls: 0,
            };
            if region
                .place_placed(&plains_selector, &mut random, [8, 8, 8], 0, 16, &mut env)
                .unwrap()
            {
                region.finish().unwrap();
                seen[branch] = true;
            }
            if seen.iter().all(|v| *v) {
                break;
            }
        }
        assert_eq!(seen, [true, true, true], "real plains selector branches");
        let (mut terrain, _, _, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let unknown_support = world.intern_state(state("diamond_ore"));
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[7 * 256..8 * 256].fill(unknown_support);
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(0);
        let mut unchanged = random.clone();
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
        assert_eq!(random.next_long(), unchanged.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    #[test]
    fn repeated_oak_placements_reset_and_reuse_position_buffers() {
        let (schedule, neighborhood, registry) = prepare_tree_fixture();
        let plan = fixture_plan(&schedule, &neighborhood);
        let prepared =
            PreparedDecoration::load(&mut TreeData, &plan, &registry, &mut resolve_tree_test)
                .unwrap();
        let feature = &prepared.features.values().next().unwrap().placed;
        let (mut terrain, _, _, _) = fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain
            .decoration_region(&prepared.tags, &prepared.motion, writable)
            .unwrap();
        let mut random = DecorationRandom::new(0);
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: None,
            calls: 0,
        };
        assert!(
            region
                .place_placed(feature, &mut random, [-4, 0, -4], 0, 16, &mut env)
                .unwrap()
        );
        let log_capacity = region.tree_logs.capacity();
        let leaf_capacity = region.tree_leaves.capacity();
        assert!(region.tree_logs.is_empty() && region.tree_leaves.is_empty());
        assert!(log_capacity > 0 && leaf_capacity > 0);
        assert!(
            region
                .place_placed(feature, &mut random, [4, 0, 4], 0, 16, &mut env)
                .unwrap()
        );
        assert!(region.tree_logs.is_empty() && region.tree_leaves.is_empty());
        assert!(
            region.tree_logs.capacity() >= log_capacity
                && region.tree_leaves.capacity() >= leaf_capacity
        );
        region.finish().unwrap();
        let world = terrain.snapshot();
        assert_eq!(world.block(4, 8, 4).unwrap().name, "minecraft:oak_log");
        assert_eq!(world.block(12, 8, 12).unwrap().name, "minecraft:oak_log");
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn tree_edge_falling_updates_match_java() {
        use native_resources::tree_leaf_update::ShapeEdgeWorld;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let source = temp.path().join("TreeEdgeFallingOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/TreeEdgeFallingOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
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
            .arg("TreeEdgeFallingOracle"));
        let (mut terrain, tags, _, _) = fixture();
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let motion = HeightmapTags::load(&mut resources, &registry).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([center]))
            .unwrap();
        let names = ["sand", "red_sand", "gravel"];
        let neighbors = ["air", "stone", "oak_log"];
        let mut count = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("F ")) {
            let parts: Vec<_> = line.split_whitespace().collect();
            assert_eq!(parts.len(), 5, "{line}");
            let kind: usize = parts[0].parse().unwrap();
            let direction = match parts[1] {
                "down" => [0, -1, 0],
                "up" => [0, 1, 0],
                "north" => [0, 0, -1],
                "south" => [0, 0, 1],
                "west" => [-1, 0, 0],
                "east" => [1, 0, 0],
                _ => panic!("unexpected direction"),
            };
            let neighbor: usize = parts[2].parse().unwrap();
            let block = state(names[kind]);
            let pos = [8, 8, 8];
            region.set_section_block(pos, &block).unwrap();
            region.world.saved_block_ticks.clear();
            region.block_tick_keys.clear();
            let updated = ShapeEdgeWorld::update_shape(
                &mut region,
                pos,
                &block,
                direction,
                [8 + direction[0], 8 + direction[1], 8 + direction[2]],
                &state(neighbors[neighbor]),
            )
            .unwrap();
            assert_eq!(
                updated == block,
                parts[3].parse::<bool>().unwrap(),
                "{line}"
            );
            let ticks = region.world.saved_block_ticks.get(&center).unwrap();
            assert_eq!(ticks.len(), 1, "{line}");
            assert_eq!(parts[4], format!("B:minecraft:{}:2", names[kind]), "{line}");
            assert_eq!(ticks[0].block, block.name, "{line}");
            assert_eq!(ticks[0].position, pos, "{line}");
            // ProtoChunkTicks stores zero rather than the requested delay.
            assert_eq!(ticks[0].delay, 0, "{line}");
            count += 1;
        }
        assert_eq!(count, 3 * 6 * 3);
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn tree_edge_fluid_updates_match_java() {
        use std::{path::PathBuf, process::Command};
        struct BubbleData;
        impl ResourceProvider for BubbleData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                match path {
                    "data/minecraft/tags/block/enables_bubble_column_drag_down.json" => {
                        Ok(Some(br#"{"values":["minecraft:magma_block"]}"#.to_vec()))
                    }
                    "data/minecraft/tags/block/enables_bubble_column_push_up.json" => Ok(Some(
                        br#"{"values":["minecraft:soul_sand","minecraft:stone"]}"#.to_vec(),
                    )),
                    _ => Data.read_data(path),
                }
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let source = temp.path().join("TreeEdgeFluidOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/TreeEdgeFluidOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let out = c.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreeEdgeFluidOracle"));
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:water".into(),
            "minecraft:lava".into(),
            "minecraft:magma_block".into(),
            "minecraft:soul_sand".into(),
        ]);
        let tags = OreBlockTags::load(
            &mut BubbleData,
            &registry,
            &[
                ResourceId::parse("minecraft:enables_bubble_column_drag_down").unwrap(),
                ResourceId::parse("minecraft:enables_bubble_column_push_up").unwrap(),
            ]
            .into_iter()
            .collect(),
        )
        .unwrap();
        let motion = HeightmapTags::load(&mut BubbleData, &registry).unwrap();
        let mut checked = 0;
        for mode in 0..4 {
            let (mut terrain, _, _, _) = fixture();
            let pos = [8, 8, 8];
            let center = ChunkPos { x: 0, z: 0 };
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([center]))
                .unwrap();
            let mut bubble = BTreeSet::new();
            if mode & 1 != 0 {
                bubble.insert("minecraft:water".into());
            }
            if mode & 2 != 0 {
                bubble.insert("minecraft:lava".into());
            }
            region.bubble_column_fluids = Some(&bubble);
            for line in output
                .lines()
                .filter(|line| line.starts_with(&format!("E {mode} ")))
            {
                let parts: Vec<_> = line.split_whitespace().collect();
                assert!((7..=8).contains(&parts.len()), "{line}");
                let kind: i32 = parts[2].parse().unwrap();
                let level: i32 = parts[3].parse().unwrap();
                let neighbor: i32 = parts[4].parse().unwrap();
                let direction = match parts[5] {
                    "down" => [0, -1, 0],
                    "up" => [0, 1, 0],
                    "north" => [0, 0, -1],
                    "south" => [0, 0, 1],
                    "west" => [-1, 0, 0],
                    "east" => [1, 0, 0],
                    _ => panic!("direction"),
                };
                let mut liquid = state(if kind == 0 { "water" } else { "lava" });
                liquid.properties.insert("level".into(), level.to_string());
                let mut other = state(match neighbor {
                    0 => "air",
                    1 | 2 => "water",
                    3 => "lava",
                    4 => "magma_block",
                    5 => "soul_sand",
                    _ => "stone",
                });
                if matches!(neighbor, 1 | 2 | 3) {
                    other
                        .properties
                        .insert("level".into(), if neighbor == 2 { "1" } else { "0" }.into());
                }
                region.set_section_block(pos, &liquid).unwrap();
                region.world.saved_fluid_ticks.clear();
                region.world.saved_block_ticks.clear();
                region.fluid_tick_keys.clear();
                region.block_tick_keys.clear();
                let updated = native_resources::tree_leaf_update::ShapeEdgeWorld::update_shape(
                    &mut region,
                    pos,
                    &liquid,
                    direction,
                    [8 + direction[0], 8 + direction[1], 8 + direction[2]],
                    &other,
                )
                .unwrap();
                assert_eq!(updated == liquid, parts[6] == "true", "{line}");
                let mut events = Vec::new();
                if let Some(ticks) = region.world.saved_fluid_ticks.get(&center) {
                    events.extend(ticks.iter().map(|t| format!("F:{}", t.fluid)));
                }
                if let Some(ticks) = region.world.saved_block_ticks.get(&center) {
                    events.extend(ticks.iter().map(|t| format!("B:{}", t.block)));
                }
                assert_eq!(
                    events.join(","),
                    parts.get(7).copied().unwrap_or(""),
                    "{line}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 4 * 2 * 4 * 7 * 6);
    }

    #[test]
    fn tree_edges_remove_only_vegetation_without_active_support() {
        struct Support(bool);
        impl ResourceProvider for Support {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/supports_vegetation.json" && !self.0 {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                TreeData.read_data(path)
            }
        }
        let (schedule, neighborhood, mut registry) = prepare_tree_fixture();
        registry.extend([
            "minecraft:oak_sapling".into(),
            "minecraft:sweet_berry_bush".into(),
        ]);
        let plan = fixture_plan(&schedule, &neighborhood);
        for supported in [false, true] {
            let prepared = PreparedDecoration::load(
                &mut Support(supported),
                &plan,
                &registry,
                &mut resolve_tree_test,
            )
            .unwrap();
            for name in ["short_grass", "oak_sapling", "sweet_berry_bush"] {
                let (mut terrain, _, _, _) = fixture();
                let world = Arc::make_mut(&mut terrain.world);
                let mut plant = state(name);
                if name == "oak_sapling" {
                    plant.properties.insert("stage".into(), "1".into());
                }
                if name == "sweet_berry_bush" {
                    plant.properties.insert("age".into(), "3".into());
                }
                let id = world.intern_state(plant.clone());
                let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
                let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
                section.blocks.as_mut().unwrap()[8 + 9 * 16 + 8 * 256] = id;
                assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
                let expected = if supported { plant } else { state("air") };
                assert_eq!(
                    terrain.snapshot().block(8, 8, 9).unwrap(),
                    &expected,
                    "{name}, support {supported}"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn tree_edge_vegetation_survival_matches_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("TreeVegetationOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/TreeVegetationOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let actual = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreeVegetationOracle"));
        struct PlantData(bool);
        impl ResourceProvider for PlantData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if matches!(
                    path,
                    "data/minecraft/tags/block/supports_vegetation.json"
                        | "data/minecraft/tags/block/supports_dry_vegetation.json"
                ) {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"values":if self.0 {vec!["minecraft:stone"]}else{vec![]}}),
                    )?));
                }
                TreeData.read_data(path)
            }
        }
        let names = [
            "short_grass",
            "fern",
            "bush",
            "dandelion",
            "poppy",
            "blue_orchid",
            "allium",
            "azure_bluet",
            "red_tulip",
            "orange_tulip",
            "white_tulip",
            "pink_tulip",
            "oxeye_daisy",
            "cornflower",
            "lily_of_the_valley",
            "firefly_bush",
            "closed_eyeblossom",
            "sweet_berry_bush",
            "short_dry_grass",
            "tall_dry_grass",
            "dead_bush",
            "tall_grass",
            "large_fern",
            "sunflower",
            "rose_bush",
            "peony",
            "lilac",
            "oak_sapling",
            "spruce_sapling",
            "birch_sapling",
            "jungle_sapling",
            "acacia_sapling",
            "dark_oak_sapling",
            "cherry_sapling",
            "pale_oak_sapling",
        ];
        let (_, _, mut registry) = prepare_tree_fixture();
        for name in names {
            registry.insert(format!("minecraft:{name}"));
        }
        let mut expected = Vec::new();
        for name in names {
            let doubled = matches!(
                name,
                "tall_grass" | "large_fern" | "sunflower" | "rose_bush" | "peony" | "lilac"
            );
            for support in [false, true] {
                let required = BTreeSet::from([
                    ResourceId::parse("minecraft:supports_vegetation").unwrap(),
                    ResourceId::parse("minecraft:supports_dry_vegetation").unwrap(),
                ]);
                let tags =
                    OreBlockTags::load(&mut PlantData(support), &registry, &required).unwrap();
                let motion = HeightmapTags::load(&mut PlantData(support), &registry).unwrap();
                let (mut terrain, _, _, _) = fixture();
                let writable = terrain.snapshot().chunks.keys().copied().collect();
                let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
                let stages = if name.ends_with("_sapling") {
                    2
                } else if name == "sweet_berry_bush" {
                    4
                } else {
                    1
                };
                for stage in 0..stages {
                    for half in 0..if doubled { 2 } else { 1 } {
                        for below_pair in 0..if doubled && half == 1 { 2 } else { 1 } {
                            let mut plant = state(name);
                            if name.ends_with("_sapling") {
                                plant.properties.insert("stage".into(), stage.to_string());
                            }
                            if name == "sweet_berry_bush" {
                                plant.properties.insert("age".into(), stage.to_string());
                            }
                            if doubled {
                                plant.properties.insert(
                                    "half".into(),
                                    if half == 0 { "lower" } else { "upper" }.into(),
                                );
                            }
                            let mut below = if below_pair == 1 {
                                state(name)
                            } else {
                                state("stone")
                            };
                            if below_pair == 1 {
                                below.properties.insert("half".into(), "lower".into());
                            }
                            let id = region.world.intern_state(below);
                            let chunk = Arc::make_mut(
                                region
                                    .world
                                    .chunks
                                    .get_mut(&ChunkPos { x: 0, z: 0 })
                                    .unwrap(),
                            );
                            let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
                            section.blocks.as_mut().unwrap()[8 + 8 * 16 + 7 * 256] = id;
                            for (direction_name, direction) in
                                [("down", [0, -1, 0]), ("up", [0, 1, 0]), ("east", [1, 0, 0])]
                            {
                                for neighbor_kind in 0..if doubled { 3 } else { 1 } {
                                    let mut neighbor = if neighbor_kind == 0 {
                                        state("air")
                                    } else {
                                        state(name)
                                    };
                                    if neighbor_kind != 0 {
                                        let other =
                                            if neighbor_kind == 1 { 1 - half } else { half };
                                        neighbor.properties.insert(
                                            "half".into(),
                                            if other == 0 { "lower" } else { "upper" }.into(),
                                        );
                                    }
                                    let updated=native_resources::tree_leaf_update::ShapeEdgeWorld::update_shape(&mut region,[8,8,8],&plant,direction,[8+direction[0],8+direction[1],8+direction[2]],&neighbor).unwrap();
                                    let extra = updated
                                        .properties
                                        .get("half")
                                        .map(|v| format!(",{v}"))
                                        .or_else(|| {
                                            updated
                                                .properties
                                                .get("stage")
                                                .map(|v| format!(",stage={v}"))
                                        })
                                        .or_else(|| {
                                            updated
                                                .properties
                                                .get("age")
                                                .map(|v| format!(",age={v}"))
                                        })
                                        .unwrap_or_default();
                                    expected.push(format!("C {name} {support} {stage} {half} {below_pair} {direction_name} {neighbor_kind} {}{extra}",updated.name));
                                }
                            }
                        }
                    }
                }
            }
        }
        let actual: Vec<_> = actual
            .lines()
            .filter(|line| line.starts_with("C "))
            .collect();
        assert_eq!(actual.len(), 564);
        assert_eq!(actual.len(), expected.len());
        for (i, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
            assert_eq!(*actual, expected, "case {i}");
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_oak_chunk_and_ticks_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
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
        let source = temp.path().join("TreeChunkOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/TreeChunkOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stderr),
                String::from_utf8_lossy(&result.stdout)
            );
            String::from_utf8(result.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let actual = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreeChunkOracle"));
        let actual = actual
            .lines()
            .find(|line| line.starts_with("RESULT "))
            .unwrap();
        let (schedule, neighborhood, registry) = prepare_tree_fixture();
        let plan = fixture_plan(&schedule, &neighborhood);
        let prepared =
            PreparedDecoration::load(&mut TreeData, &plan, &registry, &mut resolve_tree_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        let success = prepared.execute(&mut terrain, false).unwrap();
        let world = terrain.snapshot();
        let mut blocks = Vec::new();
        for x in 0..16 {
            for y in 7..16 {
                for z in 0..16 {
                    let block = world.block(x, y, z).unwrap();
                    if !matches!(
                        block.name.as_str(),
                        "minecraft:dirt" | "minecraft:oak_log" | "minecraft:oak_leaves"
                    ) {
                        continue;
                    }
                    let extra = match block.name.as_str() {
                        "minecraft:oak_log" => format!(",axis={}", block.properties["axis"]),
                        "minecraft:oak_leaves" => format!(
                            ",distance={},persistent={},waterlogged={}",
                            block.properties["distance"],
                            block.properties["persistent"],
                            block.properties["waterlogged"]
                        ),
                        _ => String::new(),
                    };
                    blocks.push(format!("{x},{y},{z},{}{extra}", block.name));
                }
            }
        }
        let ticks = world
            .saved_block_ticks
            .get(&ChunkPos { x: 0, z: 0 })
            .into_iter()
            .flatten()
            .map(|tick| {
                format!(
                    "{},{},{},{}",
                    tick.position[0], tick.position[1], tick.position[2], tick.block
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            format!(
                "RESULT {} B {} T {}",
                success[0],
                blocks.join(";"),
                ticks.join(";")
            )
        );

        struct FancyTreeData(bool);
        impl ResourceProvider for FancyTreeData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let Some(bytes) = TreeData.read_data(path)? else {
                    return Ok(None);
                };
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let mut v: Value = serde_json::from_slice(&bytes)?;
                    v["features"][0][0]["feature"]["foliage_placer"]["type"] =
                        json!("minecraft:fancy_foliage_placer");
                    if self.0 {
                        v["features"][0][0]["feature"]["trunk_placer"]["type"] =
                            json!("minecraft:fancy_trunk_placer");
                    }
                    return Ok(Some(serde_json::to_vec(&v)?));
                }
                Ok(Some(bytes))
            }
        }
        for branching in [false, true] {
            let fancy_output = run(Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("TreeChunkOracle")
                .arg(if branching { "branching" } else { "fancy" }));
            let fancy_actual = fancy_output
                .lines()
                .find(|line| line.starts_with("RESULT "))
                .unwrap();
            let plains = ResourceId::parse("minecraft:plains").unwrap();
            let fancy_schedule =
                FeatureSchedule::load(&mut FancyTreeData(branching), &[plains], &BTreeSet::new())
                    .unwrap();
            let fancy_plan = fixture_plan(&fancy_schedule, &neighborhood);
            let fancy_prepared = PreparedDecoration::load(
                &mut FancyTreeData(branching),
                &fancy_plan,
                &registry,
                &mut resolve_tree_test,
            )
            .unwrap();
            let (mut fancy_terrain, _, _, _) = fixture();
            let fancy_success = fancy_prepared.execute(&mut fancy_terrain, false).unwrap();
            let fancy_world = fancy_terrain.snapshot();
            let mut fancy_blocks = Vec::new();
            for x in 0..16 {
                for y in 7..16 {
                    for z in 0..16 {
                        let b = fancy_world.block(x, y, z).unwrap();
                        let extra = match b.name.as_str() {
                            "minecraft:oak_log" => format!(",axis={}", b.properties["axis"]),
                            "minecraft:oak_leaves" => format!(
                                ",distance={},persistent={},waterlogged={}",
                                b.properties["distance"],
                                b.properties["persistent"],
                                b.properties["waterlogged"]
                            ),
                            "minecraft:dirt" => String::new(),
                            _ => continue,
                        };
                        fancy_blocks.push(format!("{x},{y},{z},{}{extra}", b.name));
                    }
                }
            }
            let fancy_ticks = fancy_world
                .saved_block_ticks
                .get(&ChunkPos { x: 0, z: 0 })
                .into_iter()
                .flatten()
                .map(|t| {
                    format!(
                        "{},{},{},{}",
                        t.position[0], t.position[1], t.position[2], t.block
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                fancy_actual,
                format!(
                    "RESULT {} B {} T {}",
                    fancy_success[0],
                    fancy_blocks.join(";"),
                    fancy_ticks.join(";")
                )
            );
        }

        struct BeeTreeData;
        impl ResourceProvider for BeeTreeData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let Some(bytes) = TreeData.read_data(path)? else {
                    return Ok(None);
                };
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let mut v: Value = serde_json::from_slice(&bytes)?;
                    v["features"][0][0]["feature"]["decorators"] =
                        json!([{"type":"minecraft:beehive","probability":1.0}]);
                    return Ok(Some(serde_json::to_vec(&v)?));
                }
                if path.contains("blocks_motion_in_heightmap") {
                    let mut v: Value = serde_json::from_slice(&bytes)?;
                    v["values"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!("minecraft:bee_nest"));
                    return Ok(Some(serde_json::to_vec(&v)?));
                }
                Ok(Some(bytes))
            }
        }
        let bee_output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreeChunkOracle")
            .arg("bees"));
        let bee_actual = bee_output
            .lines()
            .find(|line| line.starts_with("RESULT "))
            .unwrap();
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let bee_schedule =
            FeatureSchedule::load(&mut BeeTreeData, &[plains], &BTreeSet::new()).unwrap();
        let bee_plan = fixture_plan(&bee_schedule, &neighborhood);
        let mut bee_registry = registry.clone();
        bee_registry.insert("minecraft:bee_nest".into());
        let bee_prepared = PreparedDecoration::load(
            &mut BeeTreeData,
            &bee_plan,
            &bee_registry,
            &mut resolve_tree_test,
        )
        .unwrap();
        let (mut bee_terrain, _, _, _) = fixture();
        let bee_success = bee_prepared.execute(&mut bee_terrain, false).unwrap();
        let bee_world = bee_terrain.snapshot();
        let mut bee_blocks = Vec::new();
        for x in 0..16 {
            for y in 7..16 {
                for z in 0..16 {
                    let b = bee_world.block(x, y, z).unwrap();
                    let extra = match b.name.as_str() {
                        "minecraft:oak_log" => format!(",axis={}", b.properties["axis"]),
                        "minecraft:oak_leaves" => format!(
                            ",distance={},persistent={},waterlogged={}",
                            b.properties["distance"],
                            b.properties["persistent"],
                            b.properties["waterlogged"]
                        ),
                        "minecraft:dirt" => String::new(),
                        "minecraft:bee_nest" => {
                            assert_eq!(
                                b.properties.get("facing").map(String::as_str),
                                Some("south")
                            );
                            assert_eq!(
                                b.properties.get("honey_level").map(String::as_str),
                                Some("0")
                            );
                            String::new()
                        }
                        _ => continue,
                    };
                    bee_blocks.push(format!("{x},{y},{z},{}{extra}", b.name));
                }
            }
        }
        let bee_ticks = bee_world
            .saved_block_ticks
            .get(&ChunkPos { x: 0, z: 0 })
            .into_iter()
            .flatten()
            .map(|t| {
                format!(
                    "{},{},{},{}",
                    t.position[0], t.position[1], t.position[2], t.block
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            bee_actual,
            format!(
                "RESULT {} B {} T {}",
                bee_success[0],
                bee_blocks.join(";"),
                bee_ticks.join(";")
            )
        );
        let nests = bee_terrain.generated_bee_nests().collect::<Vec<_>>();
        assert_eq!(nests.len(), 1);
        let nest = nests[0];
        let entity = nest.block_entity();
        let Some(native_persistence::nbt::Tag::List { values, .. }) = entity.get("bees") else {
            panic!("missing bees")
        };
        let ticks = values
            .iter()
            .map(|b| match b.get("ticks_in_hive") {
                Some(native_persistence::nbt::Tag::Int(v)) => v.to_string(),
                _ => panic!("missing bee ticks"),
            })
            .collect::<Vec<_>>()
            .join(",");
        let [x, y, z] = nest.position;
        assert!(
            bee_output
                .lines()
                .any(|line| line == format!("NEST {x},{y},{z} {ticks}"))
        );

        // Load the named oak and its below-trunk provider from the verified
        // installation, then apply the same feature seed to an equal chunk.
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let generated = resources.vanilla_generated_block_registry().unwrap();
        let block_registry = resources.vanilla_block_ids().unwrap();
        let placement =
            PlacedFeature::decode(&json!({"feature":"minecraft:oak","placement":[]})).unwrap();
        let placed =
            PlacedDecorationFeature::from_placement(&mut resources, placement, &mut |value| {
                super::super::resolve_generated_terrain_state(value, &generated)
            })
            .unwrap();
        let tags =
            OreBlockTags::load(&mut resources, &block_registry, &placed.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut resources, &block_registry).unwrap();
        let (mut installed, _, _, _) = fixture();
        let writable = installed.snapshot().chunks.keys().copied().collect();
        let mut region = installed
            .decoration_region(&tags, &motion, writable)
            .unwrap();
        let mut random = plan.feature_random(&plan.features[0]);
        let mut environment = PlacementTestEnv {
            allow: true,
            fail_after: None,
            calls: 0,
        };
        assert!(
            region
                .place_placed(&placed, &mut random, [8, 8, 8], 0, 16, &mut environment)
                .unwrap()
        );
        region.finish().unwrap();
        let installed = installed.snapshot();
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    assert_eq!(
                        installed.block(x, y, z),
                        world.block(x, y, z),
                        "installed oak at {x},{y},{z}"
                    );
                }
            }
        }
        assert_eq!(
            installed.saved_block_ticks.get(&ChunkPos { x: 0, z: 0 }),
            world.saved_block_ticks.get(&ChunkPos { x: 0, z: 0 })
        );

        let grass_output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreeChunkOracle")
            .arg("grass"));
        let grass_actual = grass_output
            .lines()
            .find(|line| line.starts_with("RESULT "))
            .unwrap();
        let (mut grassy, _, _, _) = fixture();
        let mutable = Arc::make_mut(&mut grassy.world);
        let mut grass = state("grass_block");
        grass.properties.insert("snowy".into(), "true".into());
        let id = mutable.intern_state(grass);
        let chunk = Arc::make_mut(mutable.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[8 + 9 * 16 + 8 * 256] = id;
        assert_eq!(prepared.execute(&mut grassy, false).unwrap(), [true]);
        let grassy = grassy.snapshot();
        let mut grass_blocks = Vec::new();
        for x in 0..16 {
            for y in 7..16 {
                for z in 0..16 {
                    let block = grassy.block(x, y, z).unwrap();
                    let extra = match block.name.as_str() {
                        "minecraft:dirt" => String::new(),
                        "minecraft:grass_block" => format!(",snowy={}", block.properties["snowy"]),
                        "minecraft:oak_log" => format!(",axis={}", block.properties["axis"]),
                        "minecraft:oak_leaves" => format!(
                            ",distance={},persistent={},waterlogged={}",
                            block.properties["distance"],
                            block.properties["persistent"],
                            block.properties["waterlogged"]
                        ),
                        _ => continue,
                    };
                    grass_blocks.push(format!("{x},{y},{z},{}{extra}", block.name));
                }
            }
        }
        let grass_ticks = grassy
            .saved_block_ticks
            .get(&ChunkPos { x: 0, z: 0 })
            .into_iter()
            .flatten()
            .map(|tick| {
                format!(
                    "{},{},{},{}",
                    tick.position[0], tick.position[1], tick.position[2], tick.block
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            grass_actual,
            format!(
                "RESULT true B {} T {}",
                grass_blocks.join(";"),
                grass_ticks.join(";")
            )
        );

        struct SnowData(bool);
        impl ResourceProvider for SnowData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/snow.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"values":if self.0 {vec!["minecraft:stone"]}else{vec![]}}),
                    )?));
                }
                TreeData.read_data(path)
            }
        }
        let mut grass_shapes = Vec::new();
        for snowy_tag in [false, true] {
            for initial in [false, true] {
                for (name, direction) in [("up", [0, 1, 0]), ("east", [1, 0, 0])] {
                    let required = BTreeSet::from([ResourceId::parse("minecraft:snow").unwrap()]);
                    let tags =
                        OreBlockTags::load(&mut SnowData(snowy_tag), &registry, &required).unwrap();
                    let motion = HeightmapTags::load(&mut SnowData(snowy_tag), &registry).unwrap();
                    let (mut terrain, _, _, _) = fixture();
                    let world = Arc::make_mut(&mut terrain.world);
                    let mut grass = state("grass_block");
                    grass.properties.insert("snowy".into(), initial.to_string());
                    let id = world.intern_state(grass.clone());
                    let chunk =
                        Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
                    let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
                    section.blocks.as_mut().unwrap()[1 + 16 + 8 * 256] = id;
                    let writable = terrain.snapshot().chunks.keys().copied().collect();
                    let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
                    let changed = native_resources::tree_leaf_update::ShapeEdgeWorld::update_shape(
                        &mut region,
                        [1, 8, 1],
                        &grass,
                        direction,
                        [1, 9, 1],
                        &state("stone"),
                    )
                    .unwrap();
                    if changed != grass {
                        native_resources::tree_leaf_update::ShapeEdgeWorld::set_block(
                            &mut region,
                            [1, 8, 1],
                            &changed,
                            2,
                        )
                        .unwrap();
                    }
                    region.finish().unwrap();
                    let value =
                        terrain.snapshot().block(1, 8, 1).unwrap().properties["snowy"].clone();
                    grass_shapes.push(format!("GRASS {snowy_tag} {initial} {name} {value}"));
                }
            }
        }
        let java_grass: Vec<_> = grass_output
            .lines()
            .filter(|line| line.starts_with("GRASS "))
            .collect();
        assert_eq!(java_grass, grass_shapes);

        struct TreeSupport(bool);
        impl ResourceProvider for TreeSupport {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/supports_vegetation.json" && !self.0 {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                TreeData.read_data(path)
            }
        }
        let mut scenario_registry = registry.clone();
        scenario_registry.insert("minecraft:oak_sapling".into());
        for plant_name in ["short_grass", "oak_sapling"] {
            for supported in [false, true] {
                let mode = match (plant_name, supported) {
                    ("short_grass", true) => "vegetation-supported",
                    ("short_grass", false) => "vegetation",
                    (_, true) => "sapling-supported",
                    _ => "sapling",
                };
                let java_output = run(Command::new(java.join("java.exe"))
                    .arg("-classpath")
                    .arg(&cp)
                    .arg("TreeChunkOracle")
                    .arg(mode));
                let actual = java_output
                    .lines()
                    .find(|line| line.starts_with("RESULT "))
                    .unwrap();
                let prepared = PreparedDecoration::load(
                    &mut TreeSupport(supported),
                    &plan,
                    &scenario_registry,
                    &mut resolve_tree_test,
                )
                .unwrap();
                let (mut terrain, _, _, _) = fixture();
                let world = Arc::make_mut(&mut terrain.world);
                let mut plant = state(plant_name);
                if plant_name == "oak_sapling" {
                    plant.properties.insert("stage".into(), "1".into());
                }
                let id = world.intern_state(plant.clone());
                let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
                let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
                section.blocks.as_mut().unwrap()[8 + 9 * 16 + 8 * 256] = id;
                assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
                let world = terrain.snapshot();
                assert_eq!(
                    world.block(8, 8, 9).unwrap(),
                    &if supported { plant } else { state("air") }
                );
                let mut blocks = Vec::new();
                for x in 0..16 {
                    for y in 7..16 {
                        for z in 0..16 {
                            let block = world.block(x, y, z).unwrap();
                            let extra = match block.name.as_str() {
                                "minecraft:dirt" | "minecraft:short_grass" => String::new(),
                                "minecraft:oak_sapling" => {
                                    format!(",stage={}", block.properties["stage"])
                                }
                                "minecraft:oak_log" => {
                                    format!(",axis={}", block.properties["axis"])
                                }
                                "minecraft:oak_leaves" => format!(
                                    ",distance={},persistent={},waterlogged={}",
                                    block.properties["distance"],
                                    block.properties["persistent"],
                                    block.properties["waterlogged"]
                                ),
                                _ => continue,
                            };
                            blocks.push(format!("{x},{y},{z},{}{extra}", block.name));
                        }
                    }
                }
                let ticks = world
                    .saved_block_ticks
                    .get(&ChunkPos { x: 0, z: 0 })
                    .into_iter()
                    .flatten()
                    .map(|tick| {
                        format!(
                            "{},{},{},{}",
                            tick.position[0], tick.position[1], tick.position[2], tick.block
                        )
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    actual,
                    format!("RESULT true B {} T {}", blocks.join(";"), ticks.join(";")),
                    "{mode}"
                );
            }
        }
    }
    #[test]
    fn unsupported_tree_edge_discards_generated_writes_and_ticks() {
        let (schedule, neighborhood, mut registry) = prepare_tree_fixture();
        registry.insert("minecraft:oak_planks".into());
        let plan = fixture_plan(&schedule, &neighborhood);
        let prepared =
            PreparedDecoration::load(&mut TreeData, &plan, &registry, &mut resolve_tree_test)
                .unwrap();
        let (mut terrain, _, _, _) = fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let id = world.intern_state(state("oak_planks"));
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[8 + 9 * 16 + 8 * 256] = id;
        let before = terrain.snapshot();
        assert!(prepared.execute(&mut terrain, false).is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
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
        for neighbor in [None, Some("diamond_ore"), Some("oak_planks")] {
            let (mut terrain, tags, motion, _) = fixture();
            let center = ChunkPos { x: 0, z: 0 };
            let mut setup = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([center]))
                .unwrap();
            let mut water = state("water");
            water.properties.insert("level".into(), "0".into());
            setup.set_section_block([8, 8, 8], &water).unwrap();
            if let Some(neighbor) = neighbor {
                setup
                    .set_section_block([9, 7, 8], &state(neighbor))
                    .unwrap();
            }
            setup.finish().unwrap();
            let before = terrain.snapshot();
            if neighbor == Some("oak_planks") {
                assert!(prepared.execute(&mut terrain, false).is_err());
                assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
                assert!(terrain.feature_postprocessing(center).is_none());
            } else {
                assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [true]);
                assert_eq!(
                    terrain.snapshot().block(8, 7, 8).unwrap().name,
                    "minecraft:magma_block"
                );
                if neighbor == Some("diamond_ore") {
                    assert_eq!(
                        terrain.snapshot().block(9, 7, 8).unwrap().name,
                        "minecraft:diamond_ore"
                    );
                }
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
    fn firefly_survival_filter_and_feature_use_active_vegetation_tag() {
        struct VegetationData(bool);
        impl ResourceProvider for VegetationData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[{
                        "feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:firefly_bush"}},
                        "placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8},{"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:would_survive","state":"minecraft:firefly_bush"}}]
                    }]]}))?));
                }
                if path == "data/minecraft/tags/block/supports_vegetation.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"values":if self.0 {vec!["minecraft:stone"]}else{vec![]}}),
                    )?));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut VegetationData(true),
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.insert("minecraft:firefly_bush".into());
        for supports in [false, true] {
            let prepared = PreparedDecoration::load(
                &mut VegetationData(supports),
                &plan,
                &registry,
                &mut resolve_test,
            )
            .unwrap();
            let (mut terrain, _, _, _) = fixture();
            assert_eq!(prepared.execute(&mut terrain, false).unwrap(), [supports]);
            assert_eq!(
                terrain.snapshot().block(8, 8, 8).unwrap().name,
                if supports {
                    "minecraft:firefly_bush"
                } else {
                    "minecraft:air"
                }
            );
        }
    }

    #[test]
    fn mushroom_generation_requires_current_brightness_unless_tag_overrides_it() {
        struct MushroomData {
            override_light: bool,
            predicate: bool,
        }
        impl ResourceProvider for MushroomData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let mut modifiers = vec![json!({"type":"minecraft:offset","x":8,"y":8,"z":8})];
                    if self.predicate {
                        modifiers.push(json!({"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:would_survive","state":"minecraft:brown_mushroom"}}));
                    }
                    return Ok(Some(serde_json::to_vec(
                        &json!({"features":[[{"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:brown_mushroom"}},"placement":modifiers}]]}),
                    )?));
                }
                if path == "data/minecraft/tags/block/overrides_mushroom_light_requirement.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"values":if self.override_light{vec!["minecraft:stone"]}else{vec![]}}),
                    )?));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        for predicate in [false, true] {
            for overrides in [false, true] {
                let schedule = FeatureSchedule::load(
                    &mut MushroomData {
                        override_light: overrides,
                        predicate,
                    },
                    std::slice::from_ref(&plains),
                    &BTreeSet::new(),
                )
                .unwrap();
                let (_, _, neighborhood) = scheduled_fixture(None);
                let plan = fixture_plan(&schedule, &neighborhood);
                let mut registry = batch_registry();
                registry.insert("minecraft:brown_mushroom".into());
                let prepared = PreparedDecoration::load(
                    &mut MushroomData {
                        override_light: overrides,
                        predicate,
                    },
                    &plan,
                    &registry,
                    &mut resolve_test,
                )
                .unwrap();
                let (mut without, _, _, _) = fixture();
                let before = without.snapshot();
                if overrides {
                    assert_eq!(prepared.execute(&mut without, false).unwrap(), [true]);
                } else {
                    assert!(prepared.execute(&mut without, false).is_err());
                    assert!(Arc::ptr_eq(&before, &without.snapshot()));
                }
                for brightness in [0, 12, 13, 15] {
                    let (mut terrain, _, _, _) = fixture();
                    let calls = std::cell::Cell::new(0);
                    let light = |_: &World, _: [i32; 3]| {
                        calls.set(calls.get() + 1);
                        Ok(brightness)
                    };
                    let result = prepared
                        .execute_with_raw_brightness(&mut terrain, false, &light)
                        .unwrap();
                    let expected = overrides || brightness < 13;
                    assert_eq!(result, [expected]);
                    assert_eq!(
                        terrain.snapshot().block(8, 8, 8).unwrap().name,
                        if expected {
                            "minecraft:brown_mushroom"
                        } else {
                            "minecraft:air"
                        }
                    );
                    assert_eq!(
                        calls.get(),
                        if overrides {
                            0
                        } else if predicate && expected {
                            2
                        } else {
                            1
                        }
                    );
                }
                if !overrides {
                    let (mut terrain, _, _, _) = fixture();
                    let before = terrain.snapshot();
                    assert!(
                        prepared
                            .execute_with_raw_brightness(&mut terrain, false, &|_, _| Ok(16))
                            .is_err()
                    );
                    assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
                    use native_world::generation_light::{
                        VisibleLayers, VisibleLightColumn, VisibleLightSnapshot,
                    };
                    let missing = VisibleLightSnapshot::new(true, true);
                    let (mut terrain, _, _, _) = fixture();
                    let before = terrain.snapshot();
                    assert!(
                        prepared
                            .execute_with_visible_light(&mut terrain, false, &missing)
                            .is_err()
                    );
                    assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
                    for (registered, expected) in [(false, false), (true, true)] {
                        let center = ChunkPos { x: 0, z: 0 };
                        let statuses = registered.then_some((center, 0));
                        let snapshot = VisibleLightSnapshot::fresh_after_section_updates(
                            true,
                            true,
                            statuses,
                            [center],
                        )
                        .unwrap();
                        let (mut terrain, _, _, _) = fixture();
                        assert_eq!(
                            prepared
                                .execute_with_visible_light(&mut terrain, false, &snapshot)
                                .unwrap(),
                            [expected]
                        );
                    }
                    for (sky_enabled, level, expected) in
                        [(true, 0, false), (false, 0, true), (false, 13, false)]
                    {
                        let mut block = VisibleLayers::new();
                        if level != 0 {
                            block.insert(
                                0,
                                Arc::new([level | (level << 4); native_world::loaded::LIGHT_BYTES]),
                            );
                        }
                        let mut snapshot = VisibleLightSnapshot::new(true, sky_enabled);
                        snapshot.insert_complete_column(
                            ChunkPos { x: 0, z: 0 },
                            VisibleLightColumn::new(block, VisibleLayers::new()),
                        );
                        let (mut terrain, _, _, _) = fixture();
                        assert_eq!(
                            prepared
                                .execute_with_visible_light(&mut terrain, false, &snapshot)
                                .unwrap(),
                            [expected]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn mushroom_brightness_sampler_observes_prior_unpublished_writes() {
        struct StagedData;
        impl ResourceProvider for StagedData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(&json!({"features":[[
                        {"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:pumpkin"}},"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8}]},
                        {"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:red_mushroom"}},"placement":[{"type":"minecraft:offset","x":9,"y":8,"z":8}]}
                    ]]}))?));
                }
                if path == "data/minecraft/tags/block/overrides_mushroom_light_requirement.json" {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut StagedData,
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.extend(["minecraft:pumpkin".into(), "minecraft:red_mushroom".into()]);
        let prepared =
            PreparedDecoration::load(&mut StagedData, &plan, &registry, &mut resolve_test).unwrap();
        let (mut terrain, _, _, _) = fixture();
        let raw = |world: &World, _: [i32; 3]| -> Result<u8> {
            Ok(
                if world
                    .block(8, 8, 8)
                    .is_some_and(|s| s.name == "minecraft:pumpkin")
                {
                    12
                } else {
                    15
                },
            )
        };
        assert_eq!(
            prepared
                .execute_with_raw_brightness(&mut terrain, false, &raw)
                .unwrap(),
            [true, true]
        );
        assert_eq!(
            terrain.snapshot().block(9, 8, 8).unwrap().name,
            "minecraft:red_mushroom"
        );
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
    fn planned_tree_filters_use_water_depth_and_sapling_tags() {
        struct TreeData {
            supports: bool,
        }
        impl ResourceProvider for TreeData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"features":[[{"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:melon"}},"placement":[
                            {"type":"minecraft:offset","x":8,"y":8,"z":8},{"type":"minecraft:surface_water_depth_filter","max_water_depth":0},{"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:would_survive","state":"minecraft:oak_sapling"}}
                        ]}]]}),
                    )?));
                }
                if path == "data/minecraft/tags/block/supports_vegetation.json" {
                    return Ok(Some(if self.supports {
                        br#"{"values":["minecraft:stone"]}"#.to_vec()
                    } else {
                        br#"{"values":[]}"#.to_vec()
                    }));
                }
                Data.read_data(path)
            }
        }
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let schedule = FeatureSchedule::load(
            &mut TreeData { supports: true },
            std::slice::from_ref(&plains),
            &BTreeSet::new(),
        )
        .unwrap();
        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let mut registry = batch_registry();
        registry.extend([
            "minecraft:oak_sapling".into(),
            "minecraft:melon".into(),
            "minecraft:water".into(),
        ]);
        for (wet, supports, expected) in [
            (false, true, true),
            (true, true, false),
            (false, false, false),
        ] {
            let prepared = PreparedDecoration::load(
                &mut TreeData { supports },
                &plan,
                &registry,
                &mut resolve_test,
            )
            .unwrap();
            let (mut terrain, _, _, _) = fixture();
            if wet {
                let mut water = state("water");
                water.properties.insert("level".into(), "0".into());
                let world = Arc::make_mut(&mut terrain.world);
                let water_id = world.intern_state(water);
                let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
                let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
                section.blocks.as_mut().unwrap()[8 + 8 * 16 + 8 * 256] = water_id;
            }
            assert_eq!(
                prepared.execute(&mut terrain, false).unwrap(),
                [expected],
                "wet {wet}, support {supports}"
            );
            if expected {
                assert_eq!(
                    terrain.snapshot().block(8, 8, 8).unwrap().name,
                    "minecraft:melon"
                );
            }
        }
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
            "patch_firefly_bush_near_water",
            "flower_plains",
            "brown_mushroom_normal",
            "red_mushroom_normal",
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
                            status: Some("minecraft:terrain".into()),
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
                sea_level: 8,
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
                sea_level: 8,
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
