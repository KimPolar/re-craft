//! 26.3 simple-block decoration, with world-owned survival and write effects.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::{
    disk_feature::{DiskWorld, Provider},
    packs::{ResourceId, ResourceProvider},
};

/// The world must implement the selected block's real survival, multi-block
/// placement, setBlock effects and scheduled ticks, or return an error.
pub trait SimpleBlockWorld: DiskWorld {
    fn place_simple_state(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
    fn schedule_simple_tick(&mut self, pos: [i32; 3]) -> Result<()>;
}

pub struct SimpleBlockFeature<S> {
    provider: Provider<S>,
    schedule_tick: bool,
}
impl<S: Clone + Eq> SimpleBlockFeature<S> {
    pub fn needs_sugar_cane_survival(&self) -> bool {
        self.provider.needs_sugar_cane_survival()
    }
    pub fn load(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let path = format!("data/{}/worldgen/feature/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing feature {path}"))?;
        Self::decode(
            &serde_json::from_slice::<Value>(&bytes)
                .with_context(|| format!("invalid feature {path}"))?,
            resolve,
        )
    }
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing feature type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "simple_block",
            "wrong simple-block feature type"
        );
        let schedule_tick = match value.get("schedule_tick") {
            None => false,
            Some(value) => value.as_bool().context("invalid schedule_tick")?,
        };
        Ok(Self {
            provider: Provider::decode(
                value
                    .get("to_place")
                    .context("missing simple-block state provider")?,
                resolve,
                0,
            )?,
            schedule_tick,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = BTreeSet::new();
        self.provider.tags(&mut tags);
        tags
    }
    pub fn possible_states(&self) -> Vec<&S> {
        let mut states = Vec::new();
        self.provider.states(&mut states);
        states
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::new();
        self.provider.blocks(&mut blocks);
        blocks
    }
    pub fn place<W: SimpleBlockWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        pos: [i32; 3],
    ) -> Result<bool> {
        let Some(state) = self.provider.state(world, random, pos)? else {
            return Ok(false);
        };
        if !world.place_simple_state(pos, &state)? {
            return Ok(false);
        }
        if self.schedule_tick {
            world.schedule_simple_tick(pos)?;
        }
        Ok(true)
    }
}
