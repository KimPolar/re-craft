//! Layered block columns with Java's height sampling and truncation order.
use crate::{
    disk_feature::{BlockPredicate, DiskWorld, Provider},
    packs::ResourceId,
    placed_feature::IntProvider,
};
use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;
use std::collections::BTreeSet;

pub trait ColumnWorld: DiskWorld {
    /// Perform the real setBlock effects, or reject unsupported effects.
    fn set_column_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
}
struct Layer<S> {
    height: IntProvider,
    provider: Provider<S>,
}
pub struct BlockColumnFeature<S> {
    layers: Vec<Layer<S>>,
    direction: [i32; 3],
    allowed: BlockPredicate,
    prioritize_tip: bool,
}
impl<S: Clone + Eq> BlockColumnFeature<S> {
    pub fn needs_sugar_cane_survival(&self) -> bool {
        self.needs_survival("sugar_cane")
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.allowed.needs_survival(name)
            || self
                .layers
                .iter()
                .any(|layer| layer.provider.needs_survival(name))
    }
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing column feature type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "block_column",
            "wrong column feature type"
        );
        let direction = match value
            .get("direction")
            .and_then(Value::as_str)
            .context("missing column direction")?
        {
            "up" => [0, 1, 0],
            "down" => [0, -1, 0],
            "north" => [0, 0, -1],
            "south" => [0, 0, 1],
            "west" => [-1, 0, 0],
            "east" => [1, 0, 0],
            _ => anyhow::bail!("invalid column direction"),
        };
        let values = value
            .get("layers")
            .and_then(Value::as_array)
            .context("missing column layers")?;
        ensure!(values.len() <= 4096, "too many column layers");
        let mut layers = Vec::with_capacity(values.len());
        for layer in values {
            let height =
                IntProvider::parse(layer.get("height").context("missing column height")?, 0)?;
            ensure!(height.bounds().0 >= 0, "negative column height");
            let provider = Provider::decode(
                layer
                    .get("provider")
                    .context("missing column state provider")?,
                resolve,
                0,
            )?;
            layers.push(Layer { height, provider });
        }
        Ok(Self {
            layers,
            direction,
            allowed: BlockPredicate::decode(
                value
                    .get("allowed_placement")
                    .context("missing column predicate")?,
            )?,
            prioritize_tip: value
                .get("prioritize_tip")
                .and_then(Value::as_bool)
                .context("missing prioritize_tip")?,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = self.allowed.required_tags();
        for layer in &self.layers {
            layer.provider.tags(&mut tags);
        }
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = self.allowed.required_blocks();
        for layer in &self.layers {
            layer.provider.blocks(&mut blocks);
        }
        blocks
    }
    pub fn possible_states(&self) -> Vec<&S> {
        let mut states = Vec::new();
        for layer in &self.layers {
            layer.provider.states(&mut states);
        }
        states
    }
    pub fn place<W: ColumnWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        let mut heights = Vec::with_capacity(self.layers.len());
        let mut total = 0i32;
        for layer in &self.layers {
            let n = layer.height.sample(random);
            total = total.checked_add(n).context("column height overflow")?;
            heights.push(n);
        }
        if total == 0 {
            return Ok(false);
        }
        let advance =
            |pos: [i32; 3]| std::array::from_fn(|i| pos[i].wrapping_add(self.direction[i]));
        let mut test = advance(origin);
        for index in 0..total {
            if !self.allowed.test(world, test)? {
                let mut remove = total - index;
                if self.prioritize_tip {
                    for height in &mut heights {
                        let n = (*height).min(remove);
                        *height -= n;
                        remove -= n;
                        if remove == 0 {
                            break;
                        }
                    }
                } else {
                    for height in heights.iter_mut().rev() {
                        let n = (*height).min(remove);
                        *height -= n;
                        remove -= n;
                        if remove == 0 {
                            break;
                        }
                    }
                }
                break;
            }
            test = advance(test);
        }
        let mut pos = origin;
        for (layer, height) in self.layers.iter().zip(heights) {
            for _ in 0..height {
                let state = layer
                    .provider
                    .state(world, random, pos)?
                    .context("column provider returned no state")?;
                world.set_column_block(pos, &state)?;
                pos = advance(pos);
            }
        }
        Ok(true)
    }
}
