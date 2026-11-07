//! Read verified stateless block registrations and optional local block reports.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use crate::{Manifest, TARGET_VERSION, open_verified_client, packs::ResourceId};

pub type DefaultProperties = BTreeMap<String, BTreeMap<String, String>>;

/// Resolved built-in block classes whose state definitions are implemented.
#[derive(Clone)]
pub struct GeneratedBlockRegistry {
    states: BTreeMap<String, StateSchema>,
    block_entities: BTreeMap<String, Option<bool>>,
}

#[derive(Clone, Copy, Debug)]
enum StateSchema {
    Stateless,
    Snowy,
    SnowLayers,
    Dusted,
    Lit,
    Liquid,
    Axis,
    DoublePlant,
    BerryBush,
    Age15,
    Sapling,
    Leaves,
    AmethystCluster,
    Multiface,
    HugeMushroom,
    LeafLitter,
    CaveVinesBody,
    CaveVinesHead,
    SmallDripleaf,
    BigDripleaf,
    BigDripleafStem,
    HangingRoots,
    Vine,
    Beehive,
    Chest,
    Fence,
    Slab,
    Stairs,
    HorizontalFacing,
}

impl StateSchema {
    fn default_properties(self) -> BTreeMap<String, String> {
        let value = match self {
            Self::Stateless => return BTreeMap::new(),
            Self::Beehive => {
                return BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("honey_level".into(), "0".into()),
                ]);
            }
            Self::Chest => {
                return BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("type".into(), "single".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            Self::Fence => {
                return ["north", "south", "west", "east", "waterlogged"]
                    .into_iter()
                    .map(|name| (name.into(), "false".into()))
                    .collect();
            }
            Self::Stairs => {
                return BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("half".into(), "bottom".into()),
                    ("shape".into(), "straight".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            Self::Slab => {
                return BTreeMap::from([
                    ("type".into(), "bottom".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            Self::HorizontalFacing => return BTreeMap::from([("facing".into(), "north".into())]),
            Self::Leaves => {
                return BTreeMap::from([
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "false".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            Self::AmethystCluster => {
                return BTreeMap::from([
                    ("facing".into(), "up".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            Self::Multiface => {
                return [
                    "down",
                    "up",
                    "north",
                    "south",
                    "west",
                    "east",
                    "waterlogged",
                ]
                .into_iter()
                .map(|name| (name.into(), "false".into()))
                .collect();
            }
            Self::Vine => {
                return ["up", "north", "south", "west", "east"]
                    .into_iter()
                    .map(|name| (name.into(), "false".into()))
                    .collect();
            }
            Self::HugeMushroom => {
                return ["down", "up", "north", "south", "west", "east"]
                    .into_iter()
                    .map(|name| (name.into(), "true".into()))
                    .collect();
            }
            Self::LeafLitter => {
                return BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("segment_amount".into(), "1".into()),
                ]);
            }
            Self::CaveVinesBody => return BTreeMap::from([("berries".into(), "false".into())]),
            Self::CaveVinesHead => {
                return BTreeMap::from([
                    ("age".into(), "0".into()),
                    ("berries".into(), "false".into()),
                ]);
            }
            Self::SmallDripleaf => {
                return BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("half".into(), "lower".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
            }
            Self::BigDripleaf | Self::BigDripleafStem => {
                let mut properties = BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("waterlogged".into(), "false".into()),
                ]);
                if matches!(self, Self::BigDripleaf) {
                    properties.insert("tilt".into(), "none".into());
                }
                return properties;
            }
            Self::Snowy => ("snowy", "false"),
            Self::SnowLayers => ("layers", "1"),
            Self::Dusted => ("dusted", "0"),
            Self::Lit => ("lit", "false"),
            Self::Liquid => ("level", "0"),
            Self::Axis => ("axis", "y"),
            Self::HangingRoots => ("waterlogged", "false"),
            Self::DoublePlant => ("half", "lower"),
            Self::BerryBush => ("age", "0"),
            Self::Age15 => ("age", "0"),
            Self::Sapling => ("stage", "0"),
        };
        BTreeMap::from([(value.0.to_owned(), value.1.to_owned())])
    }

    fn parse_property(self, name: &str, value: &str) -> Option<String> {
        match self {
            Self::Slab
                if (name == "type" && matches!(value, "top" | "bottom" | "double"))
                    || (name == "waterlogged" && matches!(value, "true" | "false")) =>
            {
                Some(value.to_owned())
            }
            Self::HorizontalFacing
                if name == "facing" && matches!(value, "north" | "south" | "east" | "west") =>
            {
                Some(value.to_owned())
            }
            Self::Fence
                if matches!(name, "north" | "south" | "west" | "east" | "waterlogged")
                    && matches!(value, "true" | "false") =>
            {
                Some(value.to_owned())
            }
            Self::Stairs
                if (name == "facing" && matches!(value, "north" | "south" | "west" | "east"))
                    || (name == "half" && matches!(value, "top" | "bottom"))
                    || (name == "shape"
                        && matches!(
                            value,
                            "straight"
                                | "inner_left"
                                | "inner_right"
                                | "outer_left"
                                | "outer_right"
                        ))
                    || (name == "waterlogged" && matches!(value, "true" | "false")) =>
            {
                Some(value.to_owned())
            }
            Self::Beehive | Self::Chest
                if name == "facing" && matches!(value, "north" | "south" | "west" | "east") =>
            {
                Some(value.to_owned())
            }
            Self::Beehive if name == "honey_level" => java_decimal(value)
                .filter(|n| (0..=5).contains(n))
                .map(|n| n.to_string()),
            Self::Chest if name == "type" && matches!(value, "single" | "left" | "right") => {
                Some(value.to_owned())
            }
            Self::Chest if name == "waterlogged" && matches!(value, "true" | "false") => {
                Some(value.to_owned())
            }
            Self::Vine
                if matches!(name, "up" | "north" | "south" | "west" | "east")
                    && matches!(value, "true" | "false") =>
            {
                Some(value.to_owned())
            }
            Self::HangingRoots if name == "waterlogged" && matches!(value, "true" | "false") => {
                Some(value.to_owned())
            }
            Self::SmallDripleaf | Self::BigDripleaf | Self::BigDripleafStem
                if (name == "facing" && matches!(value, "north" | "south" | "west" | "east"))
                    || (name == "waterlogged" && matches!(value, "true" | "false")) =>
            {
                Some(value.to_owned())
            }
            Self::SmallDripleaf if name == "half" && matches!(value, "lower" | "upper") => {
                Some(value.to_owned())
            }
            Self::BigDripleaf
                if name == "tilt" && matches!(value, "none" | "unstable" | "partial" | "full") =>
            {
                Some(value.to_owned())
            }
            Self::Multiface
                if matches!(
                    name,
                    "down" | "up" | "north" | "south" | "west" | "east" | "waterlogged"
                ) && matches!(value, "true" | "false") =>
            {
                Some(value.to_owned())
            }
            Self::HugeMushroom
                if matches!(name, "down" | "up" | "north" | "south" | "west" | "east")
                    && matches!(value, "true" | "false") =>
            {
                Some(value.to_owned())
            }
            Self::LeafLitter
                if name == "facing" && matches!(value, "north" | "south" | "west" | "east") =>
            {
                Some(value.to_owned())
            }
            Self::LeafLitter if name == "segment_amount" => java_decimal(value)
                .filter(|n| (1..=4).contains(n))
                .map(|n| n.to_string()),
            Self::CaveVinesBody | Self::CaveVinesHead
                if name == "berries" && matches!(value, "true" | "false") =>
            {
                Some(value.to_owned())
            }
            Self::CaveVinesHead if name == "age" => java_decimal(value)
                .filter(|n| (0..=25).contains(n))
                .map(|n| n.to_string()),
            Self::AmethystCluster
                if name == "facing"
                    && matches!(value, "down" | "up" | "north" | "south" | "west" | "east") =>
            {
                Some(value.to_owned())
            }
            Self::AmethystCluster if name == "waterlogged" && matches!(value, "true" | "false") => {
                Some(value.to_owned())
            }
            Self::Snowy if name == "snowy" && matches!(value, "true" | "false") => {
                Some(value.to_owned())
            }
            Self::SnowLayers if name == "layers" => java_decimal(value)
                .filter(|n| (1..=8).contains(n))
                .map(|n| n.to_string()),
            Self::Dusted if name == "dusted" => java_decimal(value)
                .filter(|n| (0..=3).contains(n))
                .map(|n| n.to_string()),
            Self::Lit if name == "lit" && matches!(value, "true" | "false") => {
                Some(value.to_owned())
            }
            Self::Axis if name == "axis" && matches!(value, "x" | "y" | "z") => {
                Some(value.to_owned())
            }
            Self::Liquid if name == "level" => java_decimal(value)
                .filter(|n| (0..=15).contains(n))
                .map(|n| n.to_string()),
            Self::DoublePlant if name == "half" && matches!(value, "lower" | "upper") => {
                Some(value.to_owned())
            }
            Self::BerryBush if name == "age" => java_decimal(value)
                .filter(|n| (0..=3).contains(n))
                .map(|n| n.to_string()),
            Self::Age15 if name == "age" => java_decimal(value)
                .filter(|n| (0..=15).contains(n))
                .map(|n| n.to_string()),
            Self::Sapling if name == "stage" => java_decimal(value)
                .filter(|n| (0..=1).contains(n))
                .map(|n| n.to_string()),
            Self::Leaves if name == "distance" => java_decimal(value)
                .filter(|n| (1..=7).contains(n))
                .map(|n| n.to_string()),
            Self::Leaves
                if matches!(name, "persistent" | "waterlogged")
                    && matches!(value, "true" | "false") =>
            {
                Some(value.to_owned())
            }
            _ => None,
        }
    }
}

/// Integer.parseInt reads UTF-16 chars and Character.digit(char, 10).
fn java_decimal(value: &str) -> Option<i32> {
    let mut chars = value.chars();
    let first = chars.next()?;
    let negative = first == '-';
    let signed = negative || first == '+';
    let mut number = 0_i64;
    let mut count = 0usize;
    for ch in (!signed).then_some(first).into_iter().chain(chars) {
        if ch as u32 > 0xffff || ch.general_category() != GeneralCategory::DecimalNumber {
            return None;
        }
        let mut start = ch as u32;
        while let Some(previous) = start.checked_sub(1).and_then(char::from_u32) {
            if previous.general_category() != GeneralCategory::DecimalNumber {
                break;
            }
            start -= 1;
        }
        let digit = (ch as u32 - start) % 10;
        number = number.checked_mul(10)?.checked_add(i64::from(digit))?;
        if number > i64::from(i32::MAX) + i64::from(negative) {
            return None;
        }
        count += 1;
    }
    if count == 0 {
        return None;
    }
    i32::try_from(if negative { -number } else { number }).ok()
}

impl GeneratedBlockRegistry {
    /// Read class files from a hash-verified 26.3 client JAR.
    pub fn load(mut read_class: impl FnMut(&str) -> Result<Option<Vec<u8>>>) -> Result<Self> {
        const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
        const IDS: &str = "net/minecraft/references/BlockItemIds";
        const KEYS: &str = "net/minecraft/references/BlockIds";
        const DIRECT: &str = "(Lnet/minecraft/references/BlockItemId;Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)Lnet/minecraft/world/level/block/Block;";
        const FACTORY: &str = "(Lnet/minecraft/references/BlockItemId;Ljava/util/function/Function;Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)Lnet/minecraft/world/level/block/Block;";
        const KEY_FACTORY: &str = "(Lnet/minecraft/resources/ResourceKey;Ljava/util/function/Function;Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)Lnet/minecraft/world/level/block/Block;";
        const STAIR_FACTORY: &str = "(Lnet/minecraft/references/BlockItemId;Lnet/minecraft/world/level/block/Block;)Lnet/minecraft/world/level/block/Block;";
        const SLAB_FACTORY_F: &str = "(Lnet/minecraft/references/BlockItemId;Lnet/minecraft/world/level/block/Block;F)Lnet/minecraft/world/level/block/Block;";
        const SLAB_FACTORY_FF: &str = "(Lnet/minecraft/references/BlockItemId;Lnet/minecraft/world/level/block/Block;FF)Lnet/minecraft/world/level/block/Block;";
        const COLOR_COLLECTION: &str = "net/minecraft/world/level/block/ColorCollection";
        const COLOR_FACTORY: &str = "(Lnet/minecraft/world/level/block/ColorCollection;Lorg/apache/commons/lang3/function/TriFunction;Ljava/util/function/BiFunction;Ljava/util/function/Function;)Lnet/minecraft/world/level/block/ColorCollection;";
        const COLOR_ZIP_FACTORY: &str = "(Lnet/minecraft/world/level/block/ColorCollection;Lnet/minecraft/world/level/block/ColorCollection;Ljava/util/function/BiFunction;)Lnet/minecraft/world/level/block/ColorCollection;";
        const COPPER_COLLECTION: &str =
            "net/minecraft/world/level/block/WeatheringCopperCollection";
        const COPPER_FACTORY: &str = "(Lnet/minecraft/world/level/block/WeatheringCopperCollection;Lorg/apache/commons/lang3/function/TriFunction;Ljava/util/function/BiFunction;Ljava/util/function/BiFunction;Ljava/util/function/Function;)Lnet/minecraft/world/level/block/WeatheringCopperCollection;";
        const BASE: &str = "net/minecraft/world/level/block/Block";
        let mut load = |name: &str| -> Result<crate::item_registry::Class> {
            let bytes = read_class(&format!("{name}.class"))?
                .with_context(|| format!("missing class {name}"))?;
            let class = crate::item_registry::Class::read(&bytes)?;
            ensure!(class.name == name, "class name does not match {name}");
            Ok(class)
        };
        let names = load(IDS)?.block_reference_names()?;
        let key_names = load(KEYS)?.block_key_reference_names()?;
        let blocks = load(BLOCKS)?;
        let mut candidates = BTreeSet::new();
        let mut registration = None;
        let mut constructors = BTreeSet::new();
        let mut color_factory = false;
        let mut copper_factory = false;
        let mut registered_classes = BTreeMap::new();
        for ins in blocks.instructions()? {
            if ins.op == 0xba {
                if !candidates.is_empty()
                    && let Some(class) = blocks.block_factory_constructor(
                        ins.index.context("missing invokedynamic reference")?,
                    )?
                {
                    constructors.insert(class);
                }
                continue;
            }
            if !matches!(ins.op, 0xb2 | 0xb3 | 0xb8) {
                continue;
            }
            let (owner, member, descriptor) =
                blocks.member(ins.index.context("missing class reference")?)?;
            if ins.op == 0xb2 && (owner == IDS || owner == KEYS) {
                candidates.insert((owner.to_owned(), member.to_owned()));
            } else if ins.op == 0xb8
                && owner == BLOCKS
                && member == "register"
                && descriptor == DIRECT
            {
                ensure!(registration.is_none(), "ambiguous block registration");
                registration = Some(true);
            } else if ins.op == 0xb8
                && owner == BLOCKS
                && member == "register"
                && (descriptor == FACTORY || descriptor == KEY_FACTORY)
            {
                ensure!(registration.is_none(), "ambiguous block registration");
                registration = Some(false);
            } else if ins.op == 0xb8
                && owner == BLOCKS
                && matches!(member, "registerLegacyStair" | "registerStair")
                && descriptor == STAIR_FACTORY
            {
                // Both helpers construct StairBlock in the verified 26.3 client.
                ensure!(registration.is_none(), "ambiguous block registration");
                registration = Some(false);
                constructors.insert("net/minecraft/world/level/block/StairBlock".to_owned());
            } else if ins.op == 0xb8
                && owner == BLOCKS
                && member == "registerSlab"
                && matches!(descriptor, STAIR_FACTORY | SLAB_FACTORY_F | SLAB_FACTORY_FF)
            {
                // All three verified helpers construct SlabBlock in 26.3.
                ensure!(registration.is_none(), "ambiguous block registration");
                registration = Some(false);
                constructors.insert("net/minecraft/world/level/block/SlabBlock".to_owned());
            } else if ins.op == 0xb8
                && owner == COLOR_COLLECTION
                && ((member == "registerBlocks" && descriptor == COLOR_FACTORY)
                    || (member == "zipMap" && descriptor == COLOR_ZIP_FACTORY))
            {
                color_factory = true;
            } else if ins.op == 0xb8
                && owner == COPPER_COLLECTION
                && member == "registerBlocks"
                && descriptor == COPPER_FACTORY
            {
                copper_factory = true;
            } else if ins.op == 0xb3 && owner == BLOCKS {
                if member == "CUT_COPPER_SLAB"
                    && copper_factory
                    && registration.is_none()
                    && candidates.len() == 1
                    && candidates
                        .first()
                        .is_some_and(|(owner, field)| owner == IDS && field == member)
                {
                    for (stage, prefix) in [
                        ("", ""),
                        ("exposed_", ""),
                        ("weathered_", ""),
                        ("oxidized_", ""),
                        ("", "waxed_"),
                        ("exposed_", "waxed_"),
                        ("weathered_", "waxed_"),
                        ("oxidized_", "waxed_"),
                    ] {
                        let class = if prefix.is_empty() {
                            "net/minecraft/world/level/block/WeatheringCopperSlabBlock"
                        } else {
                            "net/minecraft/world/level/block/SlabBlock"
                        };
                        ensure!(
                            registered_classes
                                .insert(
                                    format!("minecraft:{prefix}{stage}cut_copper_slab"),
                                    class.to_owned()
                                )
                                .is_none(),
                            "duplicate copper slab registration"
                        );
                    }
                    candidates.clear();
                    constructors.clear();
                    registration = None;
                    color_factory = false;
                    copper_factory = false;
                    continue;
                }
                let collection = match member {
                    "GLAZED_TERRACOTTA" => Some((
                        "glazed_terracotta",
                        "net/minecraft/world/level/block/GlazedTerracottaBlock",
                    )),
                    "DYED_TERRACOTTA" => Some(("terracotta", BASE)),
                    "WOOL" => Some(("wool", BASE)),
                    "WOOL_SLAB" => Some(("wool_slab", "net/minecraft/world/level/block/SlabBlock")),
                    "CONCRETE_SLAB" => {
                        Some(("concrete_slab", "net/minecraft/world/level/block/SlabBlock"))
                    }
                    "WOOL_STAIRS" => {
                        Some(("wool_stairs", "net/minecraft/world/level/block/StairBlock"))
                    }
                    "CONCRETE_STAIRS" => Some((
                        "concrete_stairs",
                        "net/minecraft/world/level/block/StairBlock",
                    )),
                    _ => None,
                };
                if let Some((suffix, class)) = collection
                    && color_factory
                    && registration.is_none()
                    && candidates.len() == 1
                    && candidates
                        .first()
                        .is_some_and(|(owner, field)| owner == IDS && field == member)
                {
                    // The verified 26.3 color collections register these 16
                    // Minecraft dye-color IDs using the indicated block class.
                    for color in [
                        "white",
                        "orange",
                        "magenta",
                        "light_blue",
                        "yellow",
                        "lime",
                        "pink",
                        "gray",
                        "light_gray",
                        "cyan",
                        "purple",
                        "blue",
                        "brown",
                        "green",
                        "red",
                        "black",
                    ] {
                        ensure!(
                            registered_classes
                                .insert(format!("minecraft:{color}_{suffix}"), class.to_owned())
                                .is_none(),
                            "duplicate block registration"
                        );
                    }
                    candidates.clear();
                    constructors.clear();
                    registration = None;
                    color_factory = false;
                    copper_factory = false;
                    continue;
                }
                let class = match registration {
                    Some(true) => Some(BASE.to_owned()),
                    Some(false) if constructors.len() == 1 => constructors.first().cloned(),
                    _ => None,
                };
                if let Some(class) = class
                    && candidates.len() == 1
                {
                    let (owner, field) = candidates.first().expect("checked single candidate");
                    let name = if owner == IDS { &names } else { &key_names }
                        .get(field)
                        .with_context(|| format!("missing block reference {field}"))?;
                    // Some references name the paired item (for example
                    // SWEET_BERRY_CROP), while Blocks uses the block ID. Only
                    // accept this alias if the literal ID matches that field.
                    if field != member && name != &member.to_ascii_lowercase() {
                        candidates.clear();
                        constructors.clear();
                        registration = None;
                        color_factory = false;
                        copper_factory = false;
                        continue;
                    }
                    let id = ResourceId::parse(name)?;
                    ensure!(
                        id.namespace == "minecraft",
                        "unexpected block reference namespace"
                    );
                    ensure!(
                        registered_classes
                            .insert(format!("{}:{}", id.namespace, id.path), class)
                            .is_none(),
                        "duplicate block registration"
                    );
                }
                candidates.clear();
                constructors.clear();
                registration = None;
                color_factory = false;
                copper_factory = false;
            }
        }
        drop(load);
        let mut cache = BTreeMap::new();
        let mut entity_cache = BTreeMap::new();
        let mut states = BTreeMap::new();
        let mut block_entities = BTreeMap::new();
        for (id, class) in registered_classes {
            let schema = match class.as_str() {
                "net/minecraft/world/level/block/SnowyBlock"
                | "net/minecraft/world/level/block/GrassBlock"
                | "net/minecraft/world/level/block/MyceliumBlock" => Some(StateSchema::Snowy),
                "net/minecraft/world/level/block/LiquidBlock" => Some(StateSchema::Liquid),
                "net/minecraft/world/level/block/RedStoneOreBlock" => Some(StateSchema::Lit),
                "net/minecraft/world/level/block/BeehiveBlock" => Some(StateSchema::Beehive),
                "net/minecraft/world/level/block/ChestBlock"
                | "net/minecraft/world/level/block/TrappedChestBlock" => Some(StateSchema::Chest),
                "net/minecraft/world/level/block/FenceBlock" => Some(StateSchema::Fence),
                "net/minecraft/world/level/block/SlabBlock"
                | "net/minecraft/world/level/block/WeatheringCopperSlabBlock" => {
                    Some(StateSchema::Slab)
                }
                "net/minecraft/world/level/block/StairBlock" => Some(StateSchema::Stairs),
                "net/minecraft/world/level/block/GlazedTerracottaBlock"
                | "net/minecraft/world/level/block/CarvedPumpkinBlock" => {
                    Some(StateSchema::HorizontalFacing)
                }
                "net/minecraft/world/level/block/SnowLayerBlock" => Some(StateSchema::SnowLayers),
                "net/minecraft/world/level/block/SpawnerBlock" => Some(StateSchema::Stateless),
                "net/minecraft/world/level/block/BrushableBlock" => Some(StateSchema::Dusted),
                "net/minecraft/world/level/block/AmethystClusterBlock" => {
                    Some(StateSchema::AmethystCluster)
                }
                "net/minecraft/world/level/block/GlowLichenBlock" => Some(StateSchema::Multiface),
                "net/minecraft/world/level/block/VineBlock" => Some(StateSchema::Vine),
                "net/minecraft/world/level/block/HugeMushroomBlock" => {
                    Some(StateSchema::HugeMushroom)
                }
                "net/minecraft/world/level/block/LeafLitterBlock" => Some(StateSchema::LeafLitter),
                "net/minecraft/world/level/block/CaveVinesPlantBlock" => {
                    Some(StateSchema::CaveVinesBody)
                }
                "net/minecraft/world/level/block/CaveVinesBlock" => {
                    Some(StateSchema::CaveVinesHead)
                }
                "net/minecraft/world/level/block/SmallDripleafBlock" => {
                    Some(StateSchema::SmallDripleaf)
                }
                "net/minecraft/world/level/block/BigDripleafBlock" => {
                    Some(StateSchema::BigDripleaf)
                }
                "net/minecraft/world/level/block/BigDripleafStemBlock" => {
                    Some(StateSchema::BigDripleafStem)
                }
                "net/minecraft/world/level/block/HangingRootsBlock" => {
                    Some(StateSchema::HangingRoots)
                }
                "net/minecraft/world/level/block/RotatedPillarBlock"
                | "net/minecraft/world/level/block/HayBlock" => Some(StateSchema::Axis),
                "net/minecraft/world/level/block/DoublePlantBlock"
                | "net/minecraft/world/level/block/TallSeagrassBlock"
                | "net/minecraft/world/level/block/TallFlowerBlock" => {
                    Some(StateSchema::DoublePlant)
                }
                "net/minecraft/world/level/block/SweetBerryBushBlock" => {
                    Some(StateSchema::BerryBush)
                }
                "net/minecraft/world/level/block/SugarCaneBlock"
                | "net/minecraft/world/level/block/CactusBlock" => Some(StateSchema::Age15),
                "net/minecraft/world/level/block/SaplingBlock" => Some(StateSchema::Sapling),
                "net/minecraft/world/level/block/LeavesBlock"
                | "net/minecraft/world/level/block/TintedParticleLeavesBlock"
                | "net/minecraft/world/level/block/UntintedParticleLeavesBlock" => {
                    Some(StateSchema::Leaves)
                }
                _ if stateless_class(
                    &class,
                    &mut read_class,
                    &mut cache,
                    &mut BTreeSet::new(),
                )? =>
                {
                    Some(StateSchema::Stateless)
                }
                _ => None,
            };
            if let Some(schema) = schema {
                let has_entity = entity_block_class(
                    &class,
                    &mut read_class,
                    &mut entity_cache,
                    &mut BTreeSet::new(),
                )?;
                block_entities.insert(id.clone(), has_entity);
                states.insert(id, schema);
            }
        }
        ensure!(
            states.contains_key("minecraft:stone") && states.contains_key("minecraft:dirt"),
            "block registration format is unsupported"
        );
        Ok(Self {
            states,
            block_entities,
        })
    }

    pub fn contains(&self, id: &str) -> bool {
        self.states.contains_key(id)
    }
    /// Whether the verified block class implements Java's EntityBlock.
    /// None means the block or part of its inheritance graph is unavailable.
    pub fn has_block_entity(&self, id: &str) -> Option<bool> {
        self.block_entities.get(id).copied().flatten()
    }
    /// Check a fully materialized state without applying codec defaults or
    /// silently dropping unknown/invalid property values.
    pub fn is_canonical_state(&self, id: &str, properties: &BTreeMap<String, String>) -> bool {
        let Some(schema) = self.states.get(id).copied() else {
            return false;
        };
        let defaults = schema.default_properties();
        properties.len() == defaults.len()
            && properties.iter().all(|(key, value)| {
                defaults.contains_key(key)
                    && schema.parse_property(key, value).as_deref() == Some(value.as_str())
            })
    }

    /// Apply StructureTemplate's mirror-then-rotate order to verified states.
    /// State classes whose transformation has not been checked are rejected.
    pub fn transform_template_state(
        &self,
        id: &str,
        properties: &BTreeMap<String, String>,
        mirror: crate::structure_transform::Mirror,
        rotation: crate::structure_transform::Rotation,
    ) -> Result<BTreeMap<String, String>> {
        use crate::structure_transform::{Mirror, Rotation};
        ensure!(
            self.is_canonical_state(id, properties),
            "unknown or noncanonical template state {id}"
        );
        let schema = self.states[id];
        if mirror == Mirror::None && rotation == Rotation::None {
            return Ok(properties.clone());
        }
        if matches!(
            schema,
            StateSchema::Stateless | StateSchema::Dusted | StateSchema::Slab
        ) {
            return Ok(properties.clone());
        }
        if matches!(schema, StateSchema::Axis) {
            let mut result = properties.clone();
            if matches!(
                rotation,
                Rotation::Clockwise90 | Rotation::Counterclockwise90
            ) {
                let axis = match properties["axis"].as_str() {
                    "x" => "z",
                    "z" => "x",
                    "y" => "y",
                    _ => unreachable!("canonical axis is valid"),
                };
                result.insert("axis".into(), axis.into());
            }
            return Ok(result);
        }
        if matches!(schema, StateSchema::Fence) {
            let directions = ["north", "east", "south", "west"];
            let mut result = properties.clone();
            for (index, name) in directions.iter().enumerate() {
                let mut direction = index;
                if (mirror == Mirror::LeftRight && index % 2 == 0)
                    || (mirror == Mirror::FrontBack && index % 2 == 1)
                {
                    direction = (direction + 2) % 4;
                }
                direction = (direction
                    + match rotation {
                        Rotation::None => 0,
                        Rotation::Clockwise90 => 1,
                        Rotation::Clockwise180 => 2,
                        Rotation::Counterclockwise90 => 3,
                    })
                    % 4;
                result.insert(directions[direction].into(), properties[*name].clone());
            }
            return Ok(result);
        }
        ensure!(
            matches!(
                schema,
                StateSchema::HorizontalFacing | StateSchema::Stairs | StateSchema::Chest
            ),
            "template state transform is unsupported for {id}"
        );
        let mut result = properties.clone();
        let facing = properties["facing"].as_str();
        let affected = match mirror {
            Mirror::None => false,
            Mirror::LeftRight => matches!(facing, "north" | "south"),
            Mirror::FrontBack => matches!(facing, "east" | "west"),
        };
        let mut index = match facing {
            "north" => 0,
            "east" => 1,
            "south" => 2,
            "west" => 3,
            _ => unreachable!("canonical facing is horizontal"),
        };
        if affected {
            index += 2;
            if matches!(schema, StateSchema::Stairs) {
                let shape = properties["shape"].as_str();
                let mirrored = match (mirror, shape) {
                    (_, "outer_left") => "outer_right",
                    (_, "outer_right") => "outer_left",
                    (Mirror::LeftRight, "inner_left") => "inner_right",
                    (Mirror::LeftRight, "inner_right") => "inner_left",
                    _ => shape,
                };
                result.insert("shape".into(), mirrored.into());
            }
        }
        index += match rotation {
            Rotation::None => 0,
            Rotation::Clockwise90 => 1,
            Rotation::Clockwise180 => 2,
            Rotation::Counterclockwise90 => 3,
        };
        result.insert(
            "facing".into(),
            ["north", "east", "south", "west"][index % 4].into(),
        );
        Ok(result)
    }
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.states.keys().map(String::as_str)
    }
    pub fn len(&self) -> usize {
        self.states.len()
    }
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn default_properties(&self, id: &str) -> Option<BTreeMap<String, String>> {
        self.states
            .get(id)
            .map(|schema| schema.default_properties())
    }

    /// Strict property lookup for command block states. Unlike the NBT codec,
    /// command parsing rejects unknown names and invalid values.
    pub fn command_property(&self, id: &str, name: &str, value: &str) -> Option<String> {
        self.states.get(id)?.parse_property(name, value)
    }

    /// Java's block-state codec ignores unknown or invalid property entries.
    pub fn resolve(&self, value: &serde_json::Value) -> Result<(String, BTreeMap<String, String>)> {
        let raw = value
            .as_str()
            .or_else(|| value.get("id").and_then(serde_json::Value::as_str))
            .context("block state has no identifier")?;
        let id = ResourceId::parse(raw)?;
        let name = format!("{}:{}", id.namespace, id.path);
        let schema = *self
            .states
            .get(&name)
            .with_context(|| format!("block state definition is unavailable: {name}"))?;
        let mut properties = schema.default_properties();
        if let Some(values) = value
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (name, value) in values {
                if let Some(value) = value
                    .as_str()
                    .and_then(|value| schema.parse_property(name, value))
                {
                    properties.insert(name.clone(), value);
                }
            }
        }
        Ok((name, properties))
    }
}

fn entity_block_class(
    name: &str,
    read_class: &mut impl FnMut(&str) -> Result<Option<Vec<u8>>>,
    cache: &mut BTreeMap<String, Option<bool>>,
    visiting: &mut BTreeSet<String>,
) -> Result<Option<bool>> {
    const ENTITY_BLOCK: &str = "net/minecraft/world/level/block/EntityBlock";
    if name == ENTITY_BLOCK {
        return Ok(Some(true));
    }
    // Bootstrap classes cannot depend on Minecraft's application classes.
    if name.starts_with("java/") || name.starts_with("javax/") {
        return Ok(Some(false));
    }
    if let Some(result) = cache.get(name) {
        return Ok(*result);
    }
    ensure!(
        name.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_$".contains(&c))
            && visiting.insert(name.to_owned())
            && visiting.len() <= 128,
        "invalid or cyclic class inheritance"
    );
    let result = (|| {
        let Some(bytes) = read_class(&format!("{name}.class"))? else {
            return Ok(None);
        };
        let class = crate::item_registry::Class::read(&bytes)?;
        ensure!(class.name == name, "class name does not match its path");
        let mut unknown = false;
        for parent in class.superclass.iter().chain(&class.interfaces) {
            match entity_block_class(parent, read_class, cache, visiting)? {
                Some(true) => return Ok(Some(true)),
                Some(false) => {}
                None => unknown = true,
            }
        }
        Ok((!unknown).then_some(false))
    })();
    visiting.remove(name);
    if let Ok(value) = result {
        cache.insert(name.to_owned(), value);
    }
    result
}

fn stateless_class(
    name: &str,
    read_class: &mut impl FnMut(&str) -> Result<Option<Vec<u8>>>,
    cache: &mut BTreeMap<String, bool>,
    visiting: &mut BTreeSet<String>,
) -> Result<bool> {
    const BASE: &str = "net/minecraft/world/level/block/Block";
    const DESCRIPTOR: &str = "(Lnet/minecraft/world/level/block/state/StateDefinition$Builder;)V";
    if let Some(result) = cache.get(name) {
        return Ok(*result);
    }
    ensure!(
        name.starts_with("net/minecraft/world/level/block/")
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/_$".contains(&c)),
        "invalid block class name"
    );
    ensure!(
        visiting.insert(name.to_owned()) && visiting.len() <= 64,
        "cyclic block inheritance"
    );
    let result = (|| {
        let bytes = read_class(&format!("{name}.class"))?
            .with_context(|| format!("missing block class {name}"))?;
        let class = crate::item_registry::Class::read(&bytes)?;
        ensure!(
            class.name == name,
            "block class name does not match its path"
        );
        if name == BASE {
            ensure!(
                class.declares_method("createBlockStateDefinition", DESCRIPTOR),
                "base block state method is missing"
            );
            Ok(true)
        } else if class.declares_method("createBlockStateDefinition", DESCRIPTOR) {
            Ok(false)
        } else {
            stateless_class(
                class
                    .superclass
                    .as_deref()
                    .context("missing block superclass")?,
                read_class,
                cache,
                visiting,
            )
        }
    })();
    visiting.remove(name);
    if let Ok(value) = result {
        cache.insert(name.to_owned(), value);
    }
    result
}

#[derive(Deserialize)]
struct Metadata {
    report_format: u32,
    game_version: String,
    data_version: u32,
    client_sha1: String,
    report_sha1: String,
}

#[derive(Deserialize)]
struct ReportBlock {
    #[serde(default)]
    properties: BTreeMap<String, Vec<String>>,
    states: Vec<ReportState>,
}

#[derive(Deserialize)]
struct ReportState {
    #[serde(default)]
    default: bool,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "{} is too large",
        path.display()
    );
    Ok(bytes)
}

fn parse_report(bytes: &[u8]) -> Result<DefaultProperties> {
    let blocks: BTreeMap<String, ReportBlock> =
        serde_json::from_slice(bytes).context("invalid block report JSON")?;
    ensure!(!blocks.is_empty(), "block report is empty");
    let mut defaults = BTreeMap::new();
    for (id, block) in blocks {
        ResourceId::parse(&id).with_context(|| format!("invalid block id {id}"))?;
        let mut default = block.states.into_iter().filter(|state| state.default);
        let state = default
            .next()
            .with_context(|| format!("block {id} has no default state"))?;
        ensure!(
            default.next().is_none(),
            "block {id} has multiple default states"
        );
        ensure!(
            state.properties.len() == block.properties.len(),
            "block {id} has incomplete default properties"
        );
        for (name, value) in &state.properties {
            let allowed = block
                .properties
                .get(name)
                .with_context(|| format!("unknown default property {name} for {id}"))?;
            ensure!(
                allowed.contains(value),
                "invalid default value {name}={value} for {id}"
            );
        }
        defaults.insert(id, state.properties);
    }
    Ok(defaults)
}

fn load_for_hash(directory: &Path, client_sha1: &str) -> Result<DefaultProperties> {
    let metadata: Metadata =
        serde_json::from_slice(&read_limited(&directory.join("metadata.json"), 16 * 1024)?)
            .context("invalid block report metadata")?;
    ensure!(
        metadata.report_format == 1
            && metadata.game_version == TARGET_VERSION
            && metadata.data_version == 5023,
        "block report targets a different Minecraft version or report format"
    );
    ensure!(
        metadata.client_sha1.eq_ignore_ascii_case(client_sha1),
        "block report was generated for a different client JAR"
    );
    let report = read_limited(&directory.join("blocks.json"), 32 * 1024 * 1024)?;
    let report_hash = format!("{:x}", Sha1::digest(&report));
    if !report_hash.eq_ignore_ascii_case(&metadata.report_sha1) {
        bail!("block report checksum differs from its metadata");
    }
    parse_report(&report)
}

/// Read a cache generated from this verified local 26.3 client.
pub fn load(minecraft_dir: &Path, directory: &Path) -> Result<DefaultProperties> {
    let _client = open_verified_client(minecraft_dir)?;
    let manifest_path = minecraft_dir.join("versions/26.3/26.3.json");
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    load_for_hash(directory, &manifest.downloads.client.sha1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_state_transform_preserves_java_stair_mirror_asymmetry() {
        use crate::structure_transform::{Mirror, Rotation};
        let registry = GeneratedBlockRegistry {
            states: BTreeMap::from([("minecraft:stairs".into(), StateSchema::Stairs)]),
            block_entities: BTreeMap::new(),
        };
        let mut state = StateSchema::Stairs.default_properties();
        state.insert("facing".into(), "east".into());
        state.insert("shape".into(), "inner_left".into());
        let transformed = registry
            .transform_template_state(
                "minecraft:stairs",
                &state,
                Mirror::FrontBack,
                Rotation::None,
            )
            .unwrap();
        assert_eq!(transformed["facing"], "west");
        assert_eq!(transformed["shape"], "inner_left");
        state.insert("shape".into(), "outer_left".into());
        let transformed = registry
            .transform_template_state(
                "minecraft:stairs",
                &state,
                Mirror::FrontBack,
                Rotation::None,
            )
            .unwrap();
        assert_eq!(transformed["shape"], "outer_right");
        state.insert("shape".into(), "invalid".into());
        assert!(
            registry
                .transform_template_state("minecraft:stairs", &state, Mirror::None, Rotation::None)
                .is_err()
        );
    }

    #[test]
    fn slab_properties_and_transform_preserve_shape_and_water() {
        use crate::structure_transform::{Mirror, Rotation};
        let registry = GeneratedBlockRegistry {
            states: BTreeMap::from([("minecraft:deepslate_tile_slab".into(), StateSchema::Slab)]),
            block_entities: BTreeMap::new(),
        };
        let id = "minecraft:deepslate_tile_slab";
        assert_eq!(
            registry.default_properties(id).unwrap(),
            BTreeMap::from([
                ("type".into(), "bottom".into()),
                ("waterlogged".into(), "false".into())
            ])
        );
        for kind in ["top", "bottom", "double"] {
            for wet in ["false", "true"] {
                let state = BTreeMap::from([
                    ("type".into(), kind.into()),
                    ("waterlogged".into(), wet.into()),
                ]);
                assert_eq!(
                    registry
                        .transform_template_state(
                            id,
                            &state,
                            Mirror::LeftRight,
                            Rotation::Clockwise90
                        )
                        .unwrap(),
                    state
                );
            }
        }
        assert_eq!(registry.command_property(id, "type", "invalid"), None);
    }

    #[test]
    fn template_axis_rotation_preserves_vertical_and_swaps_horizontal_axes() {
        use crate::structure_transform::{Mirror, Rotation};
        let registry = GeneratedBlockRegistry {
            states: BTreeMap::from([("minecraft:deepslate".into(), StateSchema::Axis)]),
            block_entities: BTreeMap::new(),
        };
        for (axis, rotation, expected) in [
            ("x", Rotation::Clockwise90, "z"),
            ("z", Rotation::Counterclockwise90, "x"),
            ("x", Rotation::Clockwise180, "x"),
            ("y", Rotation::Clockwise90, "y"),
        ] {
            let state = BTreeMap::from([("axis".into(), axis.into())]);
            let transformed = registry
                .transform_template_state(
                    "minecraft:deepslate",
                    &state,
                    Mirror::FrontBack,
                    rotation,
                )
                .unwrap();
            assert_eq!(transformed["axis"], expected);
        }
        let invalid = BTreeMap::from([("axis".into(), "invalid".into())]);
        assert!(
            registry
                .transform_template_state(
                    "minecraft:deepslate",
                    &invalid,
                    Mirror::None,
                    Rotation::Clockwise90
                )
                .is_err()
        );
    }

    #[test]
    fn identity_template_transform_preserves_verified_non_directional_states() {
        use crate::structure_transform::{Mirror, Rotation};
        let registry = GeneratedBlockRegistry {
            states: BTreeMap::from([("minecraft:water".into(), StateSchema::Liquid)]),
            block_entities: BTreeMap::new(),
        };
        let water = BTreeMap::from([("level".into(), "7".into())]);
        assert_eq!(
            registry
                .transform_template_state("minecraft:water", &water, Mirror::None, Rotation::None)
                .unwrap(),
            water
        );
        let invalid = BTreeMap::from([("level".into(), "99".into())]);
        assert!(
            registry
                .transform_template_state("minecraft:water", &invalid, Mirror::None, Rotation::None)
                .is_err()
        );
        assert!(
            registry
                .transform_template_state(
                    "minecraft:water",
                    &water,
                    Mirror::None,
                    Rotation::Clockwise90
                )
                .is_err()
        );
    }

    const REPORT: &str = r#"{
      "minecraft:stone":{"states":[{"id":1,"default":true}]},
      "minecraft:grass_block":{"properties":{"snowy":["true","false"]},
        "states":[{"id":2,"properties":{"snowy":"true"}},{"id":3,"default":true,"properties":{"snowy":"false"}}]}
    }"#;

    #[test]
    fn stateless_registry_rejects_missing_and_malformed_classes() {
        assert!(GeneratedBlockRegistry::load(|_| Ok(None)).is_err());
        assert!(GeneratedBlockRegistry::load(|_| Ok(Some(vec![0xca, 0xfe, 0xba, 0xbe]))).is_err());
    }

    #[test]
    fn canonical_state_requires_every_valid_property() {
        let registry = GeneratedBlockRegistry {
            states: BTreeMap::from([
                ("minecraft:stone".into(), StateSchema::Stateless),
                ("minecraft:water".into(), StateSchema::Liquid),
                ("minecraft:oak_fence".into(), StateSchema::Fence),
                ("minecraft:spruce_stairs".into(), StateSchema::Stairs),
            ]),
            block_entities: BTreeMap::new(),
        };
        assert!(registry.is_canonical_state("minecraft:stone", &BTreeMap::new()));
        assert!(!registry.is_canonical_state("minecraft:water", &BTreeMap::new()));
        assert!(registry.is_canonical_state(
            "minecraft:water",
            &BTreeMap::from([("level".into(), "15".into())])
        ));
        assert!(!registry.is_canonical_state(
            "minecraft:water",
            &BTreeMap::from([("level".into(), "16".into())])
        ));
        assert!(!registry.is_canonical_state(
            "minecraft:water",
            &BTreeMap::from([("level".into(), "01".into())])
        ));
        assert!(!registry.is_canonical_state("example:stone", &BTreeMap::new()));
        let mut fence = StateSchema::Fence.default_properties();
        assert!(registry.is_canonical_state("minecraft:oak_fence", &fence));
        fence.insert("north".into(), "yes".into());
        assert!(!registry.is_canonical_state("minecraft:oak_fence", &fence));
        let mut stairs = StateSchema::Stairs.default_properties();
        stairs.insert("shape".into(), "outer_left".into());
        assert!(registry.is_canonical_state("minecraft:spruce_stairs", &stairs));
        stairs.remove("half");
        assert!(!registry.is_canonical_state("minecraft:spruce_stairs", &stairs));
    }

    #[test]
    fn brushable_states_cover_dusted_range_and_ignore_rotation() {
        use crate::structure_transform::{Mirror, Rotation};
        let registry = GeneratedBlockRegistry {
            states: BTreeMap::from([("minecraft:suspicious_sand".into(), StateSchema::Dusted)]),
            block_entities: BTreeMap::new(),
        };
        for value in 0..=3 {
            let state = BTreeMap::from([("dusted".into(), value.to_string())]);
            assert!(registry.is_canonical_state("minecraft:suspicious_sand", &state));
            assert_eq!(
                registry
                    .transform_template_state(
                        "minecraft:suspicious_sand",
                        &state,
                        Mirror::FrontBack,
                        Rotation::Clockwise90
                    )
                    .unwrap(),
                state
            );
        }
        assert!(!registry.is_canonical_state(
            "minecraft:suspicious_sand",
            &BTreeMap::from([("dusted".into(), "4".into())])
        ));
    }

    #[test]
    fn java_decimal_handles_signs_unicode_and_integer_limits() {
        assert_eq!(java_decimal("+٠٧"), Some(7));
        assert_eq!(java_decimal("-٠"), Some(0));
        assert_eq!(java_decimal("2147483647"), Some(i32::MAX));
        assert_eq!(java_decimal("-2147483648"), Some(i32::MIN));
        for value in ["", "+", "-", "2147483648", "-2147483649", "1 ", "²", "𝟟"] {
            assert_eq!(java_decimal(value), None, "{value}");
        }
    }

    #[test]
    fn report_requires_a_single_complete_valid_default() {
        let defaults = parse_report(REPORT.as_bytes()).unwrap();
        assert!(defaults["minecraft:stone"].is_empty());
        assert_eq!(defaults["minecraft:grass_block"]["snowy"], "false");
        for invalid in [
            r#"{"minecraft:x":{"states":[{"id":1}]}}"#,
            r#"{"minecraft:x":{"states":[{"default":true},{"default":true}]}}"#,
            r#"{"minecraft:x":{"properties":{"p":["a"]},"states":[{"default":true}]}}"#,
            r#"{"minecraft:x":{"properties":{"p":["a"]},"states":[{"default":true,"properties":{"p":"b"}}]}}"#,
        ] {
            assert!(parse_report(invalid.as_bytes()).is_err(), "{invalid}");
        }
    }

    #[test]
    fn cache_checks_version_client_and_report_hash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blocks.json"), REPORT).unwrap();
        let report_sha1 = format!("{:x}", Sha1::digest(REPORT.as_bytes()));
        let metadata = format!(
            r#"{{"report_format":1,"game_version":"26.3","data_version":5023,"client_sha1":"abc","report_sha1":"{report_sha1}"}}"#
        );
        std::fs::write(dir.path().join("metadata.json"), metadata).unwrap();
        assert!(load_for_hash(dir.path(), "abc").is_ok());
        assert!(load_for_hash(dir.path(), "different").is_err());
        std::fs::write(dir.path().join("blocks.json"), b"{}").unwrap();
        assert!(load_for_hash(dir.path(), "abc").is_err());
    }
}
