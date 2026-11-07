//! Data-pack structure processors used by jigsaw pool elements.
//! Unsupported processors and unverified block definitions stop preparation.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use native_core::random::LegacyRandom;
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    blockstates::positional_seed,
    heightmap_tags::resolve_block_tag,
    packs::{ResourceId, ResourceProvider},
    structure_pool::HolderValue,
};
use native_world::loaded::BlockState;
use serde_json::Value;

use crate::jigsaw_replacement::{self, PlacementBlock};

#[derive(Clone, Debug)]
enum Predicate {
    Always,
    Block(String),
    RandomBlock(String, f32),
    State(BlockState),
    RandomState(BlockState, f32),
    Tag(BTreeSet<String>),
}

impl Predicate {
    fn test(&self, state: &BlockState, random: &mut LegacyRandom) -> bool {
        match self {
            Self::Always => true,
            Self::Block(id) => state.name == *id,
            Self::RandomBlock(id, probability) => {
                state.name == *id && random.next_float() < *probability
            }
            Self::State(expected) => state == expected,
            Self::RandomState(expected, probability) => {
                state == expected && random.next_float() < *probability
            }
            Self::Tag(values) => values.contains(&state.name),
        }
    }
}

#[derive(Clone, Debug)]
struct Rule {
    input: Predicate,
    location: Predicate,
    output: BlockState,
}

#[derive(Clone, Debug)]
enum Processor {
    Rot {
        blocks: Option<BTreeSet<String>>,
        integrity: f32,
    },
    Rule(Vec<Rule>),
    Protected(BTreeSet<String>),
    Age {
        mossiness: f32,
        tags: crate::structure_block_age::AgeTags,
    },
    BlackstoneReplace,
}

/// Decoded, ordered processors with data-pack tags already resolved.
#[derive(Clone, Debug)]
pub struct PreparedProcessors(Vec<Processor>);

fn identifier(value: &Value, label: &str) -> Result<String> {
    let id = ResourceId::parse(value.as_str().with_context(|| format!("missing {label}"))?)?;
    Ok(format!("{}:{}", id.namespace, id.path))
}

fn number(value: &Value, field: &str) -> Result<f32> {
    let value = value
        .get(field)
        .and_then(Value::as_f64)
        .with_context(|| format!("missing {field}"))? as f32;
    ensure!(value.is_finite(), "non-finite {field}");
    Ok(value)
}

fn block_set(
    value: &Value,
    provider: &mut impl ResourceProvider,
    registry: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    if let Some(tag) = value.as_str().and_then(|s| s.strip_prefix('#')) {
        return resolve_block_tag(provider, &ResourceId::parse(tag)?, registry);
    }
    let values = value
        .as_array()
        .context("block holder set must be a tag or list")?;
    let mut blocks = BTreeSet::new();
    for value in values {
        let id = identifier(value, "block holder")?;
        ensure!(
            registry.contains(&id),
            "block {id} is absent from the registry"
        );
        blocks.insert(id);
    }
    Ok(blocks)
}

fn state(value: &Value, registry: &GeneratedBlockRegistry) -> Result<BlockState> {
    let (name, properties) = registry.resolve(value)?;
    Ok(BlockState { name, properties })
}

fn predicate(
    value: &Value,
    provider: &mut impl ResourceProvider,
    blocks: &BTreeSet<String>,
    registry: &GeneratedBlockRegistry,
) -> Result<Predicate> {
    let kind = identifier(
        value
            .get("predicate_type")
            .context("missing predicate type")?,
        "predicate type",
    )?;
    let block = || -> Result<String> {
        let id = identifier(
            value.get("block").context("missing rule block")?,
            "rule block",
        )?;
        ensure!(
            blocks.contains(&id),
            "rule block {id} is absent from the registry"
        );
        Ok(id)
    };
    Ok(match kind.as_str() {
        "minecraft:always_true" => Predicate::Always,
        "minecraft:block_match" => Predicate::Block(block()?),
        "minecraft:random_block_match" => {
            Predicate::RandomBlock(block()?, number(value, "probability")?)
        }
        "minecraft:blockstate_match" => Predicate::State(state(
            value.get("block_state").context("missing rule state")?,
            registry,
        )?),
        "minecraft:random_blockstate_match" => Predicate::RandomState(
            state(
                value.get("block_state").context("missing rule state")?,
                registry,
            )?,
            number(value, "probability")?,
        ),
        "minecraft:tag_match" => {
            let id = ResourceId::parse(
                value
                    .get("tag")
                    .and_then(Value::as_str)
                    .context("missing rule tag")?,
            )?;
            Predicate::Tag(resolve_block_tag(provider, &id, blocks)?)
        }
        _ => bail!("unsupported structure predicate {kind}"),
    })
}

impl PreparedProcessors {
    pub fn load(
        holder: &HolderValue,
        provider: &mut impl ResourceProvider,
        blocks: &BTreeSet<String>,
        registry: &GeneratedBlockRegistry,
    ) -> Result<Self> {
        let data = match holder {
            HolderValue::Inline(value) => value.clone(),
            HolderValue::Reference(id) => {
                let path = format!(
                    "data/{}/worldgen/processor_list/{}.json",
                    id.namespace, id.path
                );
                serde_json::from_slice(
                    &provider
                        .read_data(&path)?
                        .with_context(|| format!("missing processor list {path}"))?,
                )?
            }
        };
        let entries = data
            .get("processors")
            .and_then(Value::as_array)
            .context("invalid processor list")?;
        let mut processors = Vec::with_capacity(entries.len());
        for entry in entries {
            let kind = identifier(
                entry
                    .get("processor_type")
                    .context("missing processor type")?,
                "processor type",
            )?;
            let processor = match kind.as_str() {
                "minecraft:block_rot" => {
                    let integrity = number(entry, "integrity")?;
                    ensure!((0.0..=1.0).contains(&integrity), "invalid block integrity");
                    Processor::Rot {
                        blocks: entry
                            .get("rottable_blocks")
                            .map(|v| block_set(v, provider, blocks))
                            .transpose()?,
                        integrity,
                    }
                }
                "minecraft:protected_blocks" => Processor::Protected(block_set(
                    entry.get("value").context("missing protected block set")?,
                    provider,
                    blocks,
                )?),
                "minecraft:block_age" => Processor::Age {
                    mossiness: number(entry, "mossiness")?,
                    tags: crate::structure_block_age::AgeTags::load(provider, blocks)?,
                },
                "minecraft:blackstone_replace" => Processor::BlackstoneReplace,
                "minecraft:rule" => {
                    let values = entry
                        .get("rules")
                        .and_then(Value::as_array)
                        .context("missing processor rules")?;
                    let mut rules = Vec::with_capacity(values.len());
                    for rule in values {
                        if let Some(position) = rule.get("position_predicate") {
                            ensure!(
                                position.get("predicate_type").and_then(Value::as_str)
                                    == Some("minecraft:always_true"),
                                "unsupported position predicate"
                            );
                        }
                        if let Some(modifier) = rule.get("block_entity_modifier") {
                            ensure!(
                                modifier.get("type").and_then(Value::as_str)
                                    == Some("minecraft:passthrough"),
                                "unsupported block entity modifier"
                            );
                        }
                        rules.push(Rule {
                            input: predicate(
                                rule.get("input_predicate")
                                    .context("missing input predicate")?,
                                provider,
                                blocks,
                                registry,
                            )?,
                            location: predicate(
                                rule.get("location_predicate")
                                    .context("missing location predicate")?,
                                provider,
                                blocks,
                                registry,
                            )?,
                            output: state(
                                rule.get("output_state").context("missing output state")?,
                                registry,
                            )?,
                        });
                    }
                    Processor::Rule(rules)
                }
                _ => bail!("unsupported structure processor {kind}"),
            };
            processors.push(processor);
        }
        Ok(Self(processors))
    }

    /// Apply per-block processors in order; this does not perform piece-wide
    /// finalization or mutate the world. The caller must provide current world
    /// state at each requested position and stage writes separately.
    pub fn process(
        &self,
        mut block: PlacementBlock,
        world: &mut impl FnMut([i32; 3]) -> Result<BlockState>,
    ) -> Result<Option<PlacementBlock>> {
        for processor in &self.0 {
            let [x, y, z] = block.position;
            match processor {
                Processor::Rot { blocks, integrity } => {
                    if blocks
                        .as_ref()
                        .is_none_or(|set| set.contains(&block.state.name))
                        && LegacyRandom::new(positional_seed(x, y, z)).next_float() > *integrity
                    {
                        return Ok(None);
                    }
                }
                Processor::Protected(blocks) => {
                    if blocks.contains(&world(block.position)?.name) {
                        return Ok(None);
                    }
                }
                Processor::Rule(rules) => {
                    let mut random = LegacyRandom::new(positional_seed(x, y, z));
                    for rule in rules {
                        if !rule.input.test(&block.state, &mut random) {
                            continue;
                        }
                        let matched = match &rule.location {
                            Predicate::Always => true,
                            predicate => predicate.test(&world(block.position)?, &mut random),
                        };
                        if matched {
                            block.state = rule.output.clone();
                            break;
                        }
                    }
                }
                Processor::Age { mossiness, tags } => {
                    let mut random = LegacyRandom::new(positional_seed(x, y, z));
                    block.state = tags.process(&block.state, *mossiness, &mut random)?;
                }
                Processor::BlackstoneReplace => {
                    block.state = blackstone_replace(&block.state)?;
                }
            }
        }
        Ok(Some(block))
    }

    /// Run the pool element's default processors and this configured list.
    /// Projection processors still need to be supplied by the caller.
    pub fn process_pool_block(
        &self,
        registry: &GeneratedBlockRegistry,
        original: &PlacementBlock,
        current: PlacementBlock,
        legacy: bool,
        keep_jigsaws: bool,
        world: &mut impl FnMut([i32; 3]) -> Result<BlockState>,
        mut projection: impl FnMut(&PlacementBlock, PlacementBlock) -> Result<Option<PlacementBlock>>,
    ) -> Result<Option<PlacementBlock>> {
        jigsaw_replacement::process_pool_block(
            registry,
            original,
            current,
            legacy,
            keep_jigsaws,
            |original, block| match self.process(block, world)? {
                Some(block) => projection(original, block),
                None => Ok(None),
            },
        )
    }
}

fn blackstone_replace(state: &BlockState) -> Result<BlockState> {
    use std::collections::BTreeMap;

    let (name, kind) = match state.name.as_str() {
        "minecraft:cobblestone" | "minecraft:mossy_cobblestone" => ("blackstone", 0),
        "minecraft:stone" => ("polished_blackstone", 0),
        "minecraft:stone_bricks" | "minecraft:mossy_stone_bricks" => {
            ("polished_blackstone_bricks", 0)
        }
        "minecraft:cobblestone_stairs" | "minecraft:mossy_cobblestone_stairs" => {
            ("blackstone_stairs", 1)
        }
        "minecraft:stone_stairs" => ("polished_blackstone_stairs", 1),
        "minecraft:stone_brick_stairs" | "minecraft:mossy_stone_brick_stairs" => {
            ("polished_blackstone_brick_stairs", 1)
        }
        "minecraft:cobblestone_slab" | "minecraft:mossy_cobblestone_slab" => ("blackstone_slab", 2),
        "minecraft:smooth_stone_slab" | "minecraft:stone_slab" => ("polished_blackstone_slab", 2),
        "minecraft:stone_brick_slab" | "minecraft:mossy_stone_brick_slab" => {
            ("polished_blackstone_brick_slab", 2)
        }
        "minecraft:stone_brick_wall" | "minecraft:mossy_stone_brick_wall" => {
            ("polished_blackstone_brick_wall", 3)
        }
        "minecraft:cobblestone_wall" | "minecraft:mossy_cobblestone_wall" => ("blackstone_wall", 3),
        "minecraft:chiseled_stone_bricks" => ("chiseled_polished_blackstone", 0),
        "minecraft:cracked_stone_bricks" => ("cracked_polished_blackstone_bricks", 0),
        "minecraft:iron_bars" => ("iron_chain", 4),
        _ => return Ok(state.clone()),
    };
    let mut properties = BTreeMap::new();
    let mut insert = |key: &str, value: &str| {
        properties.insert(key.to_owned(), value.to_owned());
    };
    match kind {
        1 => {
            for key in ["facing", "half"] {
                insert(
                    key,
                    state
                        .properties
                        .get(key)
                        .with_context(|| format!("missing {key} on {}", state.name))?,
                );
            }
            insert("shape", "straight");
            insert("waterlogged", "false");
        }
        2 => {
            insert(
                "type",
                state
                    .properties
                    .get("type")
                    .with_context(|| format!("missing type on {}", state.name))?,
            );
            insert("waterlogged", "false");
        }
        3 => {
            for key in ["east", "north", "south", "west"] {
                insert(key, "none");
            }
            insert("up", "true");
            insert("waterlogged", "false");
        }
        4 => {
            insert("axis", "y");
            insert("waterlogged", "false");
        }
        _ => {}
    }
    Ok(BlockState {
        name: format!("minecraft:{name}"),
        properties,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn block(name: &str, position: [i32; 3]) -> PlacementBlock {
        PlacementBlock {
            position,
            state: BlockState {
                name: format!("minecraft:{name}"),
                properties: BTreeMap::new(),
            },
            nbt: Some(crate::nbt::Tag::Compound(Vec::new())),
        }
    }

    #[test]
    fn rules_preserve_nbt_and_skip_unneeded_world_reads() -> Result<()> {
        let processors = PreparedProcessors(vec![Processor::Rule(vec![Rule {
            input: Predicate::Block("minecraft:stone".into()),
            location: Predicate::Always,
            output: block("dirt", [0; 3]).state,
        }])]);
        let result = processors
            .process(block("stone", [1, 2, 3]), &mut |_| {
                panic!("unexpected world read")
            })?
            .unwrap();
        assert_eq!(result.state.name, "minecraft:dirt");
        assert_eq!(result.position, [1, 2, 3]);
        assert!(result.nbt.is_some());
        Ok(())
    }

    #[test]
    fn blackstone_replacement_preserves_only_java_selected_properties() -> Result<()> {
        let mut stair = block("stone_brick_stairs", [1, 2, 3]);
        stair.state.properties = [
            ("facing", "west"),
            ("half", "top"),
            ("shape", "inner_left"),
            ("waterlogged", "true"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        let result = PreparedProcessors(vec![Processor::BlackstoneReplace])
            .process(stair, &mut |_| panic!("unexpected world read"))?
            .unwrap();
        assert_eq!(
            result.state.name,
            "minecraft:polished_blackstone_brick_stairs"
        );
        assert_eq!(result.state.properties["facing"], "west");
        assert_eq!(result.state.properties["half"], "top");
        assert_eq!(result.state.properties["shape"], "straight");
        assert_eq!(result.state.properties["waterlogged"], "false");
        assert!(result.nbt.is_some());
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed verified 26.3 client and Java 25"]
    fn active_city_processors_and_java_results() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = data.vanilla_block_ids()?;
        let registry = data.vanilla_generated_block_registry()?;
        for name in [
            "ancient_city_generic_degradation",
            "ancient_city_start_degradation",
            "ancient_city_walls_degradation",
            "outpost_rot",
        ] {
            PreparedProcessors::load(
                &HolderValue::Reference(ResourceId::parse(name)?),
                &mut data,
                &blocks,
                &registry,
            )?;
        }
        let spec = serde_json::json!({"processors":[
            {"processor_type":"minecraft:block_rot","integrity":0.95,"rottable_blocks":["minecraft:deepslate_bricks","minecraft:deepslate_tiles"]},
            {"processor_type":"minecraft:rule","rules":[
                {"input_predicate":{"predicate_type":"minecraft:random_block_match","block":"minecraft:deepslate_bricks","probability":0.3},"location_predicate":{"predicate_type":"minecraft:always_true"},"output_state":"minecraft:cracked_deepslate_bricks"},
                {"input_predicate":{"predicate_type":"minecraft:random_block_match","block":"minecraft:deepslate_tiles","probability":0.3},"location_predicate":{"predicate_type":"minecraft:always_true"},"output_state":"minecraft:cracked_deepslate_tiles"},
                {"input_predicate":{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.6},"location_predicate":{"predicate_type":"minecraft:random_block_match","block":"minecraft:dirt","probability":0.5},"output_state":"minecraft:air"},
                {"input_predicate":{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.7},"location_predicate":{"predicate_type":"minecraft:always_true"},"output_state":"minecraft:dirt"}
            ]},
            {"processor_type":"minecraft:protected_blocks","value":["minecraft:bedrock"]}
        ]});
        let processors =
            PreparedProcessors::load(&HolderValue::Inline(spec), &mut data, &blocks, &registry)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home is missing")?
            .join("bin");
        let source = temp.path().join("StructureProcessorsOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/StructureProcessorsOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("StructureProcessorsOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let inputs = ["deepslate_bricks", "deepslate_tiles", "stone", "oak_planks"];
        let world_states = ["dirt", "bedrock"];
        let mut count = 0;
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            let ["RESULT", x, y, input, world, expected] = fields.as_slice() else {
                continue;
            };
            let x: i32 = x.parse()?;
            let position = [x, y.parse()?, x * 13 - 7];
            let input: usize = input.parse()?;
            let world: usize = world.parse()?;
            let result = processors.process(block(inputs[input], position), &mut |_| {
                Ok(block(world_states[world], position).state)
            })?;
            let actual =
                result.map_or_else(|| "<skip>".into(), |v| format!("Block{{{}}}", v.state.name));
            assert_eq!(
                &actual, expected,
                "{position:?} input {} world {}",
                inputs[input], world_states[world]
            );
            count += 1;
        }
        assert_eq!(count, 17 * 4 * 4 * 2);
        Ok(())
    }
}
