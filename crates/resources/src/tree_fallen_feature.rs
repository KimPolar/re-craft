//! Decode and execute fallen-tree features from active worldgen data.
use crate::{
    disk_feature::{DiskWorld, Provider},
    packs::{ResourceId, ResourceProvider},
    tree_fallen::{FallenTreeLayout, FallenTreeWorld},
    tree_fallen_decorators::{
        Direction, FallenDecorationWorld, place_attached_to_logs, place_trunk_vines,
    },
    tree_fancy_trunk::LogAxis,
};
use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;
use std::{collections::BTreeSet, sync::OnceLock};

pub trait FallenFeatureWorld: DiskWorld {
    fn fallen_sturdy_below(&mut self, pos: [i32; 3]) -> Result<bool>;
    fn fallen_write_log(
        &mut self,
        pos: [i32; 3],
        state: Self::State,
        axis: Option<LogAxis>,
    ) -> Result<()>;
    fn fallen_postprocess_above(&mut self, pos: [i32; 3]) -> Result<()>;
    fn fallen_write_vine(&mut self, pos: [i32; 3], attachment: Direction) -> Result<()>;
    fn fallen_write_decoration(&mut self, pos: [i32; 3], state: Self::State) -> Result<()>;
}
enum Decorator<S> {
    TrunkVine,
    Attached {
        probability: f32,
        directions: Vec<Direction>,
        provider: Provider<S>,
    },
}
pub struct FallenTreeFeature<S> {
    layout: FallenTreeLayout,
    trunk: Provider<S>,
    stump: Vec<Decorator<S>>,
    logs: Vec<Decorator<S>>,
}
fn provider<S: Clone>(
    resources: &mut impl ResourceProvider,
    value: &Value,
    resolve: &mut impl FnMut(&Value) -> Result<S>,
) -> Result<Provider<S>> {
    Provider::load(resources, value, resolve)
}
fn decorators<S: Clone>(
    resources: &mut impl ResourceProvider,
    value: &Value,
    resolve: &mut impl FnMut(&Value) -> Result<S>,
) -> Result<Vec<Decorator<S>>> {
    let mut result = Vec::new();
    for value in value.as_array().context("invalid fallen tree decorators")? {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing decorator type")?,
        )?;
        match (id.namespace.as_str(), id.path.as_str()) {
            ("minecraft", "trunk_vine") => result.push(Decorator::TrunkVine),
            ("minecraft", "attached_to_logs") => {
                let probability = value
                    .get("probability")
                    .and_then(Value::as_f64)
                    .context("invalid attachment probability")?
                    as f32;
                ensure!(
                    probability.is_finite() && (0.0..=1.0).contains(&probability),
                    "invalid attachment probability"
                );
                let entries = value
                    .get("directions")
                    .and_then(Value::as_array)
                    .context("invalid attachment directions")?;
                ensure!(
                    !entries.is_empty() && entries.len() <= 4096,
                    "invalid attachment directions"
                );
                let directions = entries
                    .iter()
                    .map(|v| {
                        Ok(match v.as_str().context("invalid attachment direction")? {
                            "down" => Direction::Down,
                            "up" => Direction::Up,
                            "north" => Direction::North,
                            "south" => Direction::South,
                            "west" => Direction::West,
                            "east" => Direction::East,
                            _ => bail!("invalid attachment direction"),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let provider = provider(
                    resources,
                    value
                        .get("block_provider")
                        .context("missing attachment provider")?,
                    resolve,
                )?;
                result.push(Decorator::Attached {
                    probability,
                    directions,
                    provider,
                });
            }
            _ => bail!("unsupported fallen tree decorator"),
        }
    }
    Ok(result)
}
impl<S: Clone + Eq> FallenTreeFeature<S> {
    pub fn decode(
        resources: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        ensure!(
            ResourceId::parse(
                value
                    .get("type")
                    .and_then(Value::as_str)
                    .context("missing fallen tree type")?
            )? == ResourceId::parse("minecraft:fallen_tree")?,
            "wrong fallen tree type"
        );
        Ok(Self {
            layout: FallenTreeLayout::decode(value)?,
            trunk: provider(
                resources,
                value
                    .get("trunk_provider")
                    .context("missing fallen tree trunk")?,
                resolve,
            )?,
            stump: decorators(
                resources,
                value
                    .get("stump_decorators")
                    .context("missing stump decorators")?,
                resolve,
            )?,
            logs: decorators(
                resources,
                value
                    .get("log_decorators")
                    .context("missing log decorators")?,
                resolve,
            )?,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags =
            BTreeSet::from([ResourceId::parse("minecraft:replaceable_by_trees").unwrap()]);
        self.trunk.tags(&mut tags);
        for d in self.stump.iter().chain(&self.logs) {
            if let Decorator::Attached { provider, .. } = d {
                provider.tags(&mut tags);
            }
        }
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::new();
        self.trunk.blocks(&mut blocks);
        for d in self.stump.iter().chain(&self.logs) {
            match d {
                Decorator::TrunkVine => {
                    blocks.insert(ResourceId::parse("minecraft:vine").unwrap());
                }
                Decorator::Attached { provider, .. } => provider.blocks(&mut blocks),
            }
        }
        blocks
    }
    pub fn place(
        &self,
        world: &mut impl FallenFeatureWorld<State = S>,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        self.layout.place(
            &mut Adapter {
                feature: self,
                world,
            },
            random,
            origin,
        )
    }
}
struct Adapter<'a, S, W> {
    feature: &'a FallenTreeFeature<S>,
    world: &'a mut W,
}
impl<S: Clone + Eq, W: FallenFeatureWorld<State = S>> Adapter<'_, S, W> {
    fn decorate(
        &mut self,
        decorators: &[Decorator<S>],
        logs: &[[i32; 3]],
        random: &mut DecorationRandom,
    ) -> Result<()> {
        for decorator in decorators {
            match decorator {
                Decorator::TrunkVine => place_trunk_vines(
                    &mut DecorationAdapter {
                        world: self.world,
                        provider: None,
                    },
                    random,
                    logs,
                )?,
                Decorator::Attached {
                    probability,
                    directions,
                    provider,
                } => place_attached_to_logs(
                    &mut DecorationAdapter {
                        world: self.world,
                        provider: Some(provider),
                    },
                    random,
                    logs,
                    *probability,
                    directions,
                )?,
            }
        }
        Ok(())
    }
}
impl<S: Clone + Eq, W: FallenFeatureWorld<State = S>> FallenTreeWorld for Adapter<'_, S, W> {
    fn replaceable(&mut self, pos: [i32; 3]) -> Result<bool> {
        static REPLACEABLE: OnceLock<ResourceId> = OnceLock::new();
        let state = self.world.block(pos)?;
        Ok(self.world.is_air(&state)?
            || self.world.in_tag(
                &state,
                REPLACEABLE
                    .get_or_init(|| ResourceId::parse("minecraft:replaceable_by_trees").unwrap()),
            )?)
    }
    fn sturdy_below(&mut self, pos: [i32; 3]) -> Result<bool> {
        self.world.fallen_sturdy_below(pos)
    }
    fn place_log(
        &mut self,
        pos: [i32; 3],
        axis: Option<LogAxis>,
        random: &mut DecorationRandom,
    ) -> Result<()> {
        let state = self
            .feature
            .trunk
            .state(self.world, random, pos)?
            .context("fallen trunk provider returned no state")?;
        self.world.fallen_write_log(pos, state, axis)
    }
    fn postprocess_above(&mut self, pos: [i32; 3]) -> Result<()> {
        self.world.fallen_postprocess_above(pos)
    }
    fn decorate_stump(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()> {
        self.decorate(&self.feature.stump, &[pos], random)
    }
    fn decorate_logs(&mut self, logs: &[[i32; 3]], random: &mut DecorationRandom) -> Result<()> {
        self.decorate(&self.feature.logs, logs, random)
    }
}
struct DecorationAdapter<'a, S, W> {
    world: &'a mut W,
    provider: Option<&'a Provider<S>>,
}
impl<S: Clone + Eq, W: FallenFeatureWorld<State = S>> FallenDecorationWorld
    for DecorationAdapter<'_, S, W>
{
    fn is_air(&mut self, pos: [i32; 3]) -> Result<bool> {
        let state = self.world.block(pos)?;
        self.world.is_air(&state)
    }
    fn place_vine(&mut self, pos: [i32; 3], attachment: Direction) -> Result<()> {
        self.world.fallen_write_vine(pos, attachment)
    }
    fn place_attached(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()> {
        let state = self
            .provider
            .context("missing attachment provider")?
            .state(self.world, random, pos)?
            .context("attachment provider returned no state")?;
        self.world.fallen_write_decoration(pos, state)
    }
}
