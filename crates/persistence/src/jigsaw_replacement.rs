//! Jigsaw replacement processing for structure template blocks.
//!
//! The result is an intermediate block, not a placed structure. Callers must
//! run the remaining processors and world placement before publishing a chunk.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use native_resources::{block_registry::GeneratedBlockRegistry, packs::ResourceId};
use native_world::loaded::BlockState;

use crate::nbt::Tag;

#[derive(Clone, Debug, PartialEq)]
pub struct PlacementBlock {
    pub position: [i32; 3],
    pub state: BlockState,
    pub nbt: Option<Tag>,
}

/// Block definitions must distinguish unavailable definitions from invalid
/// properties. Unknown blocks must not silently become discarded blocks.
pub trait BlockDefinitions {
    fn defaults(&self, id: &str) -> Option<BTreeMap<String, String>>;
    fn property(&self, id: &str, name: &str, value: &str) -> Option<String>;
}

impl BlockDefinitions for GeneratedBlockRegistry {
    fn defaults(&self, id: &str) -> Option<BTreeMap<String, String>> {
        self.default_properties(id)
    }
    fn property(&self, id: &str, name: &str, value: &str) -> Option<String> {
        self.command_property(id, name, value)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }
    fn take(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.offset += 1;
        Some(value)
    }
    fn whitespace(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.offset += 1;
        }
    }
    fn string(&mut self) -> Option<String> {
        if matches!(self.peek(), Some(b'\'' | b'"')) {
            let quote = self.take()?;
            let mut text = Vec::new();
            loop {
                let value = self.take()?;
                if value == quote {
                    return String::from_utf8(text).ok();
                }
                if value == b'\\' {
                    let escaped = self.take()?;
                    if escaped != quote && escaped != b'\\' {
                        return None;
                    }
                    text.push(escaped);
                } else {
                    text.push(value);
                }
            }
        }
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+'))
        {
            self.offset += 1;
        }
        String::from_utf8(self.bytes[start..self.offset].to_vec()).ok()
    }
}

/// Parse the command-state subset used by jigsaw `final_state`.
/// `Ok(None)` means Java rejects the syntax or property; `Err` means the
/// implementation cannot prove the outcome. SNBT is deliberately rejected
/// until its complete Java parser is available. Java ignores trailing input.
pub fn parse_final_state(
    definitions: &impl BlockDefinitions,
    input: &str,
) -> Result<Option<BlockState>> {
    if !input.is_ascii() {
        bail!("non-ASCII jigsaw final state is not yet supported");
    }
    let mut reader = Reader {
        bytes: input.as_bytes(),
        offset: 0,
    };
    while reader.peek().is_some_and(|c| {
        c.is_ascii_lowercase()
            || c.is_ascii_digit()
            || matches!(c, b'_' | b'-' | b'.' | b'/' | b':')
    }) {
        reader.offset += 1;
    }
    let raw = &input[..reader.offset];
    let Ok(id) = ResourceId::parse(raw) else {
        return Ok(None);
    };
    let name = format!("{}:{}", id.namespace, id.path);
    let mut properties = definitions
        .defaults(&name)
        .with_context(|| format!("jigsaw final-state block definition is unavailable: {name}"))?;
    if reader.peek() == Some(b'[') {
        reader.take();
        reader.whitespace();
        let mut seen = BTreeSet::new();
        loop {
            match reader.peek() {
                Some(b']') => {
                    reader.take();
                    break;
                }
                None => return Ok(None),
                _ => {}
            }
            reader.whitespace();
            let Some(key) = reader.string() else {
                return Ok(None);
            };
            if !properties.contains_key(&key) || !seen.insert(key.clone()) {
                return Ok(None);
            }
            reader.whitespace();
            if reader.take() != Some(b'=') {
                return Ok(None);
            }
            reader.whitespace();
            let Some(value) = reader.string() else {
                return Ok(None);
            };
            let Some(value) = definitions.property(&name, &key, &value) else {
                return Ok(None);
            };
            properties.insert(key, value);
            reader.whitespace();
            match reader.peek() {
                Some(b',') => {
                    reader.take();
                }
                Some(b']') => {
                    reader.take();
                    break;
                }
                _ => return Ok(None),
            }
        }
    }
    if reader.peek() == Some(b'{') {
        bail!("SNBT in a jigsaw final state is not yet supported");
    }
    Ok(Some(BlockState { name, properties }))
}

/// Apply Java 26.3's JigsawReplacementProcessor. Missing NBT preserves the
/// original block; a successful replacement discards the original NBT.
pub fn process(
    definitions: &impl BlockDefinitions,
    block: PlacementBlock,
    keep_jigsaws: bool,
) -> Result<Option<PlacementBlock>> {
    if keep_jigsaws || block.state.name != "minecraft:jigsaw" || block.nbt.is_none() {
        return Ok(Some(block));
    }
    let raw = match block.nbt.as_ref().and_then(|nbt| nbt.get("final_state")) {
        Some(Tag::String(value)) => value.to_string_lossy(),
        _ => "minecraft:air".to_owned(),
    };
    let Some(state) = parse_final_state(definitions, &raw)? else {
        return Ok(None);
    };
    if state.name == "minecraft:structure_void" {
        return Ok(None);
    }
    Ok(Some(PlacementBlock {
        position: block.position,
        state,
        nbt: None,
    }))
}

/// Apply the per-block part of a single pool element's processor chain.
/// Configured and projection processors receive both the original and current
/// records; their separate list-finalization phase remains the caller's duty.
/// Legacy elements append their air/structure filter after those processors.
pub fn process_pool_block(
    definitions: &impl BlockDefinitions,
    original: &PlacementBlock,
    current: PlacementBlock,
    legacy: bool,
    keep_jigsaws: bool,
    mut configured_and_projection: impl FnMut(
        &PlacementBlock,
        PlacementBlock,
    ) -> Result<Option<PlacementBlock>>,
) -> Result<Option<PlacementBlock>> {
    if !legacy && current.state.name == "minecraft:structure_block" {
        return Ok(None);
    }
    let Some(block) = process(definitions, current, keep_jigsaws)? else {
        return Ok(None);
    };
    let Some(block) = configured_and_projection(original, block)? else {
        return Ok(None);
    };
    if legacy
        && matches!(
            block.state.name.as_str(),
            "minecraft:air" | "minecraft:structure_block"
        )
    {
        return Ok(None);
    }
    Ok(Some(block))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Definitions;
    impl BlockDefinitions for Definitions {
        fn defaults(&self, id: &str) -> Option<BTreeMap<String, String>> {
            match id {
                "minecraft:air" | "minecraft:stone" | "minecraft:structure_void" => {
                    Some(BTreeMap::new())
                }
                "minecraft:oak_stairs" => Some(BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("waterlogged".into(), "false".into()),
                ])),
                _ => None,
            }
        }
        fn property(&self, id: &str, key: &str, value: &str) -> Option<String> {
            (id == "minecraft:oak_stairs"
                && match key {
                    "facing" => matches!(value, "north" | "south" | "east" | "west"),
                    "waterlogged" => matches!(value, "true" | "false"),
                    _ => false,
                })
            .then(|| value.into())
        }
    }

    fn jigsaw(final_state: Option<Tag>) -> PlacementBlock {
        PlacementBlock {
            position: [3, -4, 5],
            state: BlockState {
                name: "minecraft:jigsaw".into(),
                properties: BTreeMap::new(),
            },
            nbt: final_state.map(|v| Tag::Compound(vec![("final_state".into(), v)])),
        }
    }

    #[test]
    fn replacement_preserves_missing_nbt_and_discards_replaced_nbt() -> Result<()> {
        let without_nbt = jigsaw(None);
        assert_eq!(
            process(&Definitions, without_nbt.clone(), false)?,
            Some(without_nbt)
        );
        let input = jigsaw(Some(Tag::String("oak_stairs[facing=east]".into())));
        assert_eq!(
            process(&Definitions, input.clone(), true)?,
            Some(input.clone())
        );
        let output = process(&Definitions, input, false)?.unwrap();
        assert_eq!(output.position, [3, -4, 5]);
        assert_eq!(output.state.properties["facing"], "east");
        assert_eq!(output.state.properties["waterlogged"], "false");
        assert_eq!(output.nbt, None);
        let wrong_type = jigsaw(Some(Tag::Int(1)));
        assert_eq!(
            process(&Definitions, wrong_type, false)?
                .unwrap()
                .state
                .name,
            "minecraft:air"
        );
        Ok(())
    }

    #[test]
    fn invalid_states_are_skipped_but_unavailable_definitions_abort() -> Result<()> {
        for value in [
            "oak_stairs[facing=sideways]",
            "oak_stairs[facing=east,facing=west]",
            "oak_stairs[unknown=x]",
            "oak_stairs[facing=east",
            "minecraft:structure_void",
        ] {
            assert!(
                process(&Definitions, jigsaw(Some(Tag::String(value.into()))), false)?.is_none(),
                "{value}"
            );
        }
        for value in ["example:unknown", "stone{foo:1}"] {
            assert!(
                process(&Definitions, jigsaw(Some(Tag::String(value.into()))), false).is_err(),
                "{value}"
            );
        }
        assert_eq!(
            parse_final_state(&Definitions, "stone trailing data")?
                .unwrap()
                .name,
            "minecraft:stone"
        );
        assert_eq!(
            parse_final_state(&Definitions, "oak_stairs[facing='east',]")?
                .unwrap()
                .properties["facing"],
            "east"
        );
        Ok(())
    }

    #[test]
    fn pool_processing_keeps_java_legacy_filter_order() -> Result<()> {
        let input = jigsaw(Some(Tag::String("minecraft:air".into())));
        assert!(
            process_pool_block(
                &Definitions,
                &input,
                input.clone(),
                false,
                false,
                |original, current| {
                    assert_eq!(original.state.name, "minecraft:jigsaw");
                    assert_eq!(current.state.name, "minecraft:air");
                    Ok(Some(current))
                }
            )?
            .is_some()
        );
        assert!(
            process_pool_block(
                &Definitions,
                &input,
                input.clone(),
                true,
                false,
                |_, current| Ok(Some(current))
            )?
            .is_none()
        );
        let marker = PlacementBlock {
            position: [0; 3],
            state: BlockState {
                name: "minecraft:structure_block".into(),
                properties: BTreeMap::new(),
            },
            nbt: None,
        };
        assert!(
            process_pool_block(
                &Definitions,
                &marker,
                marker.clone(),
                false,
                false,
                |_, _| panic!("normal filter runs first")
            )?
            .is_none()
        );
        let legacy = process_pool_block(
            &Definitions,
            &marker,
            marker.clone(),
            true,
            false,
            |_, mut current| {
                current.state.name = "minecraft:stone".into();
                Ok(Some(current))
            },
        )?;
        assert_eq!(legacy.unwrap().state.name, "minecraft:stone");
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed verified 26.3 client and Java 25"]
    fn matches_installed_java_jigsaw_replacement() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let registry = native_resources::packs::ResourceStack::open(&game)?
            .vanilla_generated_block_registry()?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
        let source = temp.path().join("JigsawReplacementOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawReplacementOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        anyhow::ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawReplacementOracle")
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let values = [
            "minecraft:air",
            "stone",
            "minecraft:structure_void",
            "oak_stairs[facing=east]",
            "oak_stairs[waterlogged=true,half=top]",
            "oak_stairs[shape=outer_left,facing=west]",
            "oak_stairs[facing=sideways]",
            "oak_stairs[facing=east,facing=west]",
            "oak_stairs[unknown=x]",
            "oak_stairs[facing=east",
            "stone trailing data",
            "oak_stairs[facing='east',]",
            "oak_stairs[]",
            "stone[]",
            "stone[foo=bar]",
            "water[level=15]",
            "water[level=16]",
            "grass_block[snowy=true]",
            "oak_stairs[ facing = \"south\" , waterlogged = true ]",
            "oak_stairs[facing=east, ]",
            "stone{broken",
        ];
        let mut count = 0;
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            let ["RESULT", index, expected, expected_nbt] = fields.as_slice() else {
                continue;
            };
            let index: usize = index.parse()?;
            let block = jigsaw(Some(Tag::String(values[index].into())));
            if index == values.len() - 1 {
                assert!(process(&registry, block, false).is_err());
                count += 1;
                continue;
            }
            let actual = process(&registry, block, false)?;
            let state = match &actual {
                None => "<skip>".to_owned(),
                Some(block) => {
                    let mut value = format!("Block{{{}}}", block.state.name);
                    if !block.state.properties.is_empty() {
                        let properties = block
                            .state
                            .properties
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join(",");
                        value.push_str(&format!("[{properties}]"));
                    }
                    value
                }
            };
            assert_eq!(&state, expected, "case {}: {}", index, values[index]);
            let nbt = match actual {
                None => "-",
                Some(block) if block.nbt.is_none() => "none",
                Some(_) => "present",
            };
            assert_eq!(&nbt, expected_nbt, "case {index}");
            count += 1;
        }
        assert_eq!(count, values.len());
        Ok(())
    }
}
