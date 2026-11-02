//! The trunk and foliage phase of a tree feature. Final leaf-distance updates,
//! decorators and shape propagation remain the caller's responsibility.
use std::{collections::BTreeSet, sync::OnceLock};

use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::{
    disk_feature::{DiskWorld, Provider},
    packs::{ResourceId, ResourceProvider},
    placed_feature::IntProvider,
    tree_beehive::{BeehiveDecorator, BeehiveWorld},
    tree_dark_oak::{DarkOakTrunkWorld, place_dark_oak_foliage, place_dark_oak_trunk},
    tree_fancy_trunk::{FancyTrunkWorld, LogAxis, place_fancy_trunk},
    tree_ground_decorator::{GroundDecorationWorld, PlaceOnGroundDecorator},
    tree_placers::{
        TreeFeatureSize, TrunkHeight, VanillaTreeClearance, permitted_tree_height,
        place_blob_foliage, place_fancy_foliage, place_straight_trunk,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeWrite {
    Trunk,
    Foliage,
}

/// Implementations apply actual writes and track trunk and foliage positions.
pub trait TreeWorld: DiskWorld {
    fn tree_min_y(&self) -> i32;
    fn tree_max_y(&self) -> i32;
    fn persistent_foliage(&mut self, pos: [i32; 3]) -> Result<bool>;
    fn foliage_state(&mut self, pos: [i32; 3], state: Self::State) -> Result<Self::State>;
    /// Java's trySetValue changes the axis only when the state has that property.
    fn trunk_axis(&mut self, state: Self::State, axis: LogAxis) -> Result<Self::State>;
    fn write_tree(&mut self, pos: [i32; 3], state: &Self::State, kind: TreeWrite) -> Result<()>;
}

pub struct TreeFeature<S> {
    trunk: Provider<S>,
    foliage: Provider<S>,
    below: Provider<S>,
    trunk_height: TrunkHeight,
    trunk_kind: TrunkKind,
    foliage_height: i32,
    foliage_kind: FoliageKind,
    radius: IntProvider,
    offset: IntProvider,
    size: TreeFeatureSize,
    ignore_vines: bool,
    decorators: Vec<TreeDecorator<S>>,
}

enum TreeDecorator<S> {
    Beehive(BeehiveDecorator),
    PlaceOnGround(PlaceOnGroundDecorator<S>),
}

#[derive(Clone, Copy)]
enum TrunkKind {
    Straight,
    Fancy,
    DarkOak,
}
#[derive(Clone, Copy)]
enum FoliageKind {
    Blob,
    Fancy,
    DarkOak,
}

impl<S: Clone + Eq> TreeFeature<S> {
    /// Decode supported trunk and foliage placers without roots.
    /// Reject unsupported configurations before a world can be modified.
    pub fn decode(
        provider: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        fn field<'a>(v: &'a Value, name: &str) -> Result<&'a Value> {
            v.get(name).with_context(|| format!("missing tree {name}"))
        }
        fn integer(v: &Value, name: &str) -> Result<i32> {
            Ok(i32::try_from(
                field(v, name)?
                    .as_i64()
                    .with_context(|| format!("invalid tree {name}"))?,
            )?)
        }
        fn name(v: &Value) -> Result<ResourceId> {
            ResourceId::parse(field(v, "type")?.as_str().context("invalid tree type")?)
        }
        fn state_provider<S: Clone>(
            provider: &mut impl ResourceProvider,
            value: &Value,
            resolve: &mut impl FnMut(&Value) -> Result<S>,
        ) -> Result<Provider<S>> {
            Provider::load(provider, value, resolve)
        }
        ensure!(
            name(value)? == ResourceId::parse("minecraft:tree")?,
            "wrong tree feature type"
        );
        ensure!(
            value.get("root_placer").is_none(),
            "tree root placer is not supported"
        );
        let mut decorators = Vec::new();
        for decorator in field(value, "decorators")?
            .as_array()
            .context("invalid tree decorators")?
        {
            let kind = name(decorator)?;
            match (kind.namespace.as_str(), kind.path.as_str()) {
                ("minecraft", "beehive") => {
                    let probability = field(decorator, "probability")?
                        .as_f64()
                        .context("invalid beehive probability")?
                        as f32;
                    decorators.push(TreeDecorator::Beehive(BeehiveDecorator::new(probability)?));
                }
                ("minecraft", "place_on_ground") => {
                    decorators.push(TreeDecorator::PlaceOnGround(
                        PlaceOnGroundDecorator::decode(provider, decorator, resolve)?,
                    ));
                }
                _ => anyhow::bail!("unsupported tree decorator"),
            }
        }
        let trunk_placer = field(value, "trunk_placer")?;
        let trunk_kind = match name(trunk_placer)? {
            id if id == ResourceId::parse("minecraft:straight_trunk_placer")? => {
                TrunkKind::Straight
            }
            id if id == ResourceId::parse("minecraft:fancy_trunk_placer")? => TrunkKind::Fancy,
            id if id == ResourceId::parse("minecraft:dark_oak_trunk_placer")? => TrunkKind::DarkOak,
            _ => anyhow::bail!("unsupported tree trunk placer"),
        };
        let foliage_placer = field(value, "foliage_placer")?;
        let foliage_kind = match name(foliage_placer)? {
            id if id == ResourceId::parse("minecraft:blob_foliage_placer")? => FoliageKind::Blob,
            id if id == ResourceId::parse("minecraft:fancy_foliage_placer")? => FoliageKind::Fancy,
            id if id == ResourceId::parse("minecraft:dark_oak_foliage_placer")? => {
                FoliageKind::DarkOak
            }
            _ => anyhow::bail!("unsupported tree foliage placer"),
        };
        let foliage_height = match foliage_kind {
            FoliageKind::DarkOak => 4,
            _ => integer(foliage_placer, "height")?,
        };
        ensure!(
            (0..=16).contains(&foliage_height),
            "invalid blob foliage height"
        );
        let radius = IntProvider::parse(field(foliage_placer, "radius")?, 0)?;
        let offset = IntProvider::parse(field(foliage_placer, "offset")?, 0)?;
        for p in [&radius, &offset] {
            let (min, max) = p.bounds();
            ensure!(min >= 0 && max <= 16, "invalid foliage integer provider");
        }
        let ignore_vines = match value.get("ignore_vines") {
            Some(v) => v.as_bool().context("invalid ignore_vines")?,
            None => false,
        };
        Ok(Self {
            trunk: state_provider(provider, field(value, "trunk_provider")?, resolve)?,
            foliage: state_provider(provider, field(value, "foliage_provider")?, resolve)?,
            below: state_provider(provider, field(value, "below_trunk_provider")?, resolve)?,
            trunk_height: TrunkHeight::new(
                integer(trunk_placer, "base_height")?,
                integer(trunk_placer, "height_rand_a")?,
                integer(trunk_placer, "height_rand_b")?,
            )?,
            trunk_kind,
            foliage_height,
            foliage_kind,
            radius,
            offset,
            size: TreeFeatureSize::decode(field(value, "minimum_size")?)?,
            ignore_vines,
            decorators,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = BTreeSet::from([
            ResourceId::parse("minecraft:replaceable_by_trees").unwrap(),
            ResourceId::parse("minecraft:logs").unwrap(),
        ]);
        self.trunk.tags(&mut tags);
        self.foliage.tags(&mut tags);
        self.below.tags(&mut tags);
        if matches!(self.trunk_kind, TrunkKind::DarkOak) {
            tags.insert(ResourceId::parse("minecraft:leaves").unwrap());
        }
        for decorator in &self.decorators {
            if let TreeDecorator::PlaceOnGround(v) = decorator {
                tags.extend(v.required_tags());
            }
        }
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::new();
        self.trunk.blocks(&mut blocks);
        self.foliage.blocks(&mut blocks);
        self.below.blocks(&mut blocks);
        for decorator in &self.decorators {
            match decorator {
                TreeDecorator::Beehive(_) => {
                    blocks.insert(ResourceId::parse("minecraft:bee_nest").unwrap());
                }
                TreeDecorator::PlaceOnGround(v) => blocks.extend(v.required_blocks()),
            }
        }
        blocks
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.trunk.needs_survival(name)
            || self.foliage.needs_survival(name)
            || self.below.needs_survival(name)
            || self
                .decorators
                .iter()
                .any(|d| matches!(d, TreeDecorator::PlaceOnGround(v) if v.needs_survival(name)))
    }
    /// Apply decorators only after successful trunk and foliage placement.
    pub fn place_decorators(
        &self,
        world: &mut (impl BeehiveWorld + GroundDecorationWorld<State = S>),
        random: &mut DecorationRandom,
        logs: &[[i32; 3]],
        leaves: &[[i32; 3]],
    ) -> Result<()> {
        for decorator in &self.decorators {
            match decorator {
                TreeDecorator::Beehive(v) => v.place(world, random, logs, leaves)?,
                TreeDecorator::PlaceOnGround(v) => v.place(world, random, logs, &[])?,
            }
        }
        Ok(())
    }
    /// Run `TreeFeature.doPlace`; caller must subsequently run vanilla's tree
    /// finalization before publishing or saving the resulting world.
    pub fn place_components<W: TreeWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        let height = self.trunk_height.sample(random);
        let radius = self.radius.sample(random);
        let min_y = world.tree_min_y();
        let max_y = world.tree_max_y();
        let Some(height) = permitted_tree_height(
            &mut VanillaTreeClearance(world),
            origin,
            origin,
            min_y,
            max_y,
            height,
            &self.size,
            self.ignore_vines,
        )?
        else {
            return Ok(false);
        };
        let attachments = match self.trunk_kind {
            TrunkKind::Fancy => place_fancy_trunk(
                &mut FancyTreeAdapter { tree: self, world },
                random,
                origin,
                height,
            )?,
            TrunkKind::DarkOak => place_dark_oak_trunk(
                &mut DarkOakAdapter { tree: self, world },
                random,
                origin,
                height,
            )?,
            TrunkKind::Straight => vec![place_straight_trunk(
                random,
                origin,
                height,
                |pos, soil, random| {
                    if soil {
                        if let Some(state) = self.below.state(world, random, pos)? {
                            world.write_tree(pos, &state, TreeWrite::Trunk)?;
                        }
                    } else if valid_tree_pos(world, pos)? {
                        let state = self
                            .trunk
                            .state(world, random, pos)?
                            .context("tree trunk provider returned no state")?;
                        world.write_tree(pos, &state, TreeWrite::Trunk)?;
                    }
                    Ok(())
                },
            )?],
        };
        for attachment in attachments {
            let offset = self.offset.sample(random);
            let mut place = |pos, random: &mut DecorationRandom| {
                if !world.persistent_foliage(pos)? && valid_tree_pos(world, pos)? {
                    let state = self
                        .foliage
                        .state(world, random, pos)?
                        .context("tree foliage provider returned no state")?;
                    let state = world.foliage_state(pos, state)?;
                    world.write_tree(pos, &state, TreeWrite::Foliage)?;
                }
                Ok(())
            };
            match self.foliage_kind {
                FoliageKind::Fancy => place_fancy_foliage(
                    random,
                    attachment,
                    self.foliage_height,
                    radius,
                    offset,
                    &mut place,
                )?,
                FoliageKind::Blob => place_blob_foliage(
                    random,
                    attachment,
                    self.foliage_height,
                    radius,
                    offset,
                    &mut place,
                )?,
                FoliageKind::DarkOak => {
                    place_dark_oak_foliage(random, attachment, radius, offset, &mut place)?
                }
            }
        }
        Ok(true)
    }
}

struct DarkOakAdapter<'a, S, W> {
    tree: &'a TreeFeature<S>,
    world: &'a mut W,
}
impl<S: Clone + Eq, W: TreeWorld<State = S>> DarkOakTrunkWorld for DarkOakAdapter<'_, S, W> {
    fn is_air_or_leaves(&mut self, pos: [i32; 3]) -> Result<bool> {
        static LEAVES: OnceLock<ResourceId> = OnceLock::new();
        let state = self.world.block(pos)?;
        Ok(self.world.is_air(&state)?
            || self.world.in_tag(
                &state,
                LEAVES.get_or_init(|| ResourceId::parse("minecraft:leaves").unwrap()),
            )?)
    }
    fn place_soil(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()> {
        if let Some(state) = self.tree.below.state(self.world, random, pos)? {
            self.world.write_tree(pos, &state, TreeWrite::Trunk)?;
        }
        Ok(())
    }
    fn place_log(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()> {
        if valid_tree_pos(self.world, pos)? {
            let state = self
                .tree
                .trunk
                .state(self.world, random, pos)?
                .context("tree trunk provider returned no state")?;
            self.world.write_tree(pos, &state, TreeWrite::Trunk)?;
        }
        Ok(())
    }
}

struct FancyTreeAdapter<'a, S, W> {
    tree: &'a TreeFeature<S>,
    world: &'a mut W,
}
impl<S: Clone + Eq, W: TreeWorld<State = S>> FancyTrunkWorld for FancyTreeAdapter<'_, S, W> {
    fn is_free(&mut self, pos: [i32; 3]) -> Result<bool> {
        if valid_tree_pos(self.world, pos)? {
            return Ok(true);
        }
        let state = self.world.block(pos)?;
        static LOGS: OnceLock<ResourceId> = OnceLock::new();
        self.world.in_tag(
            &state,
            LOGS.get_or_init(|| ResourceId::parse("minecraft:logs").unwrap()),
        )
    }
    fn place_soil(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()> {
        if let Some(state) = self.tree.below.state(self.world, random, pos)? {
            self.world.write_tree(pos, &state, TreeWrite::Trunk)?;
        }
        Ok(())
    }
    fn place_log(
        &mut self,
        pos: [i32; 3],
        axis: LogAxis,
        random: &mut DecorationRandom,
    ) -> Result<()> {
        if valid_tree_pos(self.world, pos)? {
            let state = self
                .tree
                .trunk
                .state(self.world, random, pos)?
                .context("tree trunk provider returned no state")?;
            let state = self.world.trunk_axis(state, axis)?;
            self.world.write_tree(pos, &state, TreeWrite::Trunk)?;
        }
        Ok(())
    }
}

fn valid_tree_pos<W: DiskWorld>(world: &mut W, pos: [i32; 3]) -> Result<bool> {
    static REPLACEABLE: OnceLock<ResourceId> = OnceLock::new();
    let state = world.block(pos)?;
    if world.is_air(&state)? {
        return Ok(true);
    }
    world.in_tag(
        &state,
        REPLACEABLE.get_or_init(|| ResourceId::parse("minecraft:replaceable_by_trees").unwrap()),
    )
}
