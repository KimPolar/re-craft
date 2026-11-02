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
    fn write_tree(&mut self, pos: [i32; 3], state: &Self::State, kind: TreeWrite) -> Result<()>;
}

pub struct TreeFeature<S> {
    trunk: Provider<S>,
    foliage: Provider<S>,
    below: Provider<S>,
    trunk_height: TrunkHeight,
    foliage_height: i32,
    fancy_foliage: bool,
    radius: IntProvider,
    offset: IntProvider,
    size: TreeFeatureSize,
    ignore_vines: bool,
    decorators: Vec<BeehiveDecorator>,
}

impl<S: Clone + Eq> TreeFeature<S> {
    /// Decode a straight-trunk tree with blob or fancy foliage, without roots.
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
            let data;
            let value = if let Some(id) = value.as_str() {
                let id = ResourceId::parse(id)?;
                let path = format!(
                    "data/{}/worldgen/block_state_provider/{}.json",
                    id.namespace, id.path
                );
                let bytes = provider
                    .read_data(&path)?
                    .with_context(|| format!("missing tree state provider {path}"))?;
                data = serde_json::from_slice::<Value>(&bytes)
                    .with_context(|| format!("invalid tree state provider {path}"))?;
                &data
            } else {
                value
            };
            Provider::decode(value, resolve, 0)
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
            ensure!(
                name(decorator)? == ResourceId::parse("minecraft:beehive")?,
                "unsupported tree decorator"
            );
            let probability = field(decorator, "probability")?
                .as_f64()
                .context("invalid beehive probability")? as f32;
            decorators.push(BeehiveDecorator::new(probability)?);
        }
        let trunk_placer = field(value, "trunk_placer")?;
        ensure!(
            name(trunk_placer)? == ResourceId::parse("minecraft:straight_trunk_placer")?,
            "unsupported tree trunk placer"
        );
        let foliage_placer = field(value, "foliage_placer")?;
        let fancy_foliage = match name(foliage_placer)? {
            id if id == ResourceId::parse("minecraft:blob_foliage_placer")? => false,
            id if id == ResourceId::parse("minecraft:fancy_foliage_placer")? => true,
            _ => anyhow::bail!("unsupported tree foliage placer"),
        };
        let foliage_height = integer(foliage_placer, "height")?;
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
            foliage_height,
            fancy_foliage,
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
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::new();
        self.trunk.blocks(&mut blocks);
        self.foliage.blocks(&mut blocks);
        self.below.blocks(&mut blocks);
        if !self.decorators.is_empty() {
            blocks.insert(ResourceId::parse("minecraft:bee_nest").unwrap());
        }
        blocks
    }
    /// Apply decorators only after successful trunk and foliage placement.
    pub fn place_decorators(
        &self,
        world: &mut impl BeehiveWorld,
        random: &mut DecorationRandom,
        logs: &[[i32; 3]],
        leaves: &[[i32; 3]],
    ) -> Result<()> {
        for decorator in &self.decorators {
            decorator.place(world, random, logs, leaves)?;
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
        let attachment = place_straight_trunk(random, origin, height, |pos, soil, random| {
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
        })?;
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
        if self.fancy_foliage {
            place_fancy_foliage(
                random,
                attachment,
                self.foliage_height,
                radius,
                offset,
                &mut place,
            )?;
        } else {
            place_blob_foliage(
                random,
                attachment,
                self.foliage_height,
                radius,
                offset,
                &mut place,
            )?;
        }
        Ok(true)
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
