//! Jigsaw structure settings and the random state preceding center placement.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use native_core::{beardifier::TerrainAdjustment, random::LegacyRandom};
use serde_json::Value;

use crate::{
    height_provider::{HeightContext, HeightProvider},
    jigsaw_alias::PoolAliases,
    jigsaw_placement::{
        self, AssemblyContext, DimensionPadding, PreparedStart, StartEnvironment, StartHeightmap,
        StartOptions,
    },
    packs::ResourceId,
    structure_placement::large_feature,
    structure_pool::{LiquidSettings, TemplatePool},
    structure_start::decode_terrain_adjustment,
};

#[derive(Clone, Debug)]
pub enum StartPool {
    Reference(ResourceId),
    Inline(Arc<TemplatePool>),
}

#[derive(Clone, Debug)]
pub struct JigsawStructure {
    pub start_pool: StartPool,
    pub start_name: Option<ResourceId>,
    pub max_depth: i32,
    pub start_height: HeightProvider,
    pub use_expansion_hack: bool,
    pub heightmap: Option<StartHeightmap>,
    pub horizontal_distance: i32,
    pub vertical_distance: i32,
    pub aliases: PoolAliases,
    pub padding: DimensionPadding,
    pub liquid_settings: LiquidSettings,
    pub terrain_adjustment: TerrainAdjustment,
}

fn integer(value: &Value) -> Result<i32> {
    value
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .context("invalid jigsaw integer")
}

impl JigsawStructure {
    /// Decode generation-specific fields; biome and spawn settings are
    /// resolved independently by the structure set and biome registries.
    pub fn decode(value: &Value) -> Result<Self> {
        let field = |name| {
            value
                .get(name)
                .with_context(|| format!("missing jigsaw {name}"))
        };
        let kind = ResourceId::parse(field("type")?.as_str().context("invalid structure type")?)?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "jigsaw",
            "not a jigsaw structure"
        );
        let start_pool = match field("start_pool")? {
            Value::String(id) => StartPool::Reference(ResourceId::parse(id)?),
            value @ Value::Object(_) => StartPool::Inline(Arc::new(TemplatePool::decode(value)?)),
            _ => bail!("invalid jigsaw start pool"),
        };
        let start_name = value
            .get("start_jigsaw_name")
            .map(|v| ResourceId::parse(v.as_str().context("invalid start jigsaw name")?))
            .transpose()?;
        let max_depth = integer(field("size")?)?;
        ensure!((0..=20).contains(&max_depth), "jigsaw depth out of range");
        let start_height = HeightProvider::decode(field("start_height")?)?;
        let use_expansion_hack = field("use_expansion_hack")?
            .as_bool()
            .context("invalid jigsaw expansion setting")?;
        let heightmap = value
            .get("project_start_to_heightmap")
            .map(|v| -> Result<_> {
                Ok(match v.as_str().context("invalid jigsaw heightmap")? {
                    "WORLD_SURFACE_WG" => StartHeightmap::WorldSurfaceWg,
                    "WORLD_SURFACE" => StartHeightmap::WorldSurface,
                    "OCEAN_FLOOR_WG" => StartHeightmap::OceanFloorWg,
                    "OCEAN_FLOOR" => StartHeightmap::OceanFloor,
                    "MOTION_BLOCKING" => StartHeightmap::MotionBlocking,
                    "MOTION_BLOCKING_NO_LEAVES" => StartHeightmap::MotionBlockingNoLeaves,
                    _ => bail!("unsupported jigsaw heightmap"),
                })
            })
            .transpose()?;
        let distance = field("max_distance_from_center")?;
        let (horizontal_distance, vertical_distance) = if distance.is_object() {
            (
                integer(
                    distance
                        .get("horizontal")
                        .context("missing horizontal distance")?,
                )?,
                distance
                    .get("vertical")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(4064),
            )
        } else {
            let n = integer(distance)?;
            (n, n)
        };
        ensure!(
            (1..=128).contains(&horizontal_distance) && (1..=4064).contains(&vertical_distance),
            "jigsaw distance out of range"
        );
        let terrain_adjustment = decode_terrain_adjustment(value)?;
        let terrain_margin = if terrain_adjustment == TerrainAdjustment::None {
            0
        } else {
            12
        };
        ensure!(
            horizontal_distance + terrain_margin <= 128,
            "jigsaw range exceeds terrain adaptation limit"
        );
        let aliases =
            PoolAliases::decode(value.get("pool_aliases").unwrap_or(&Value::Array(vec![])))?;
        let padding = match value.get("dimension_padding") {
            None => DimensionPadding::DefaultZero,
            Some(Value::Object(obj)) => {
                // DimensionPadding uses lenient optional fields in 26.3.
                let get = |key| {
                    obj.get(key)
                        .and_then(|v| integer(v).ok())
                        .filter(|&v| v >= 0)
                        .unwrap_or(0)
                };
                DimensionPadding::Explicit {
                    bottom: get("bottom"),
                    top: get("top"),
                }
            }
            Some(value) => {
                let n = integer(value)?;
                ensure!(n >= 0, "negative dimension padding");
                DimensionPadding::Explicit { bottom: n, top: n }
            }
        };
        let liquid_settings = match value.get("liquid_settings").map(Value::as_str) {
            None | Some(Some("apply_waterlogging")) => LiquidSettings::ApplyWaterlogging,
            Some(Some("ignore_waterlogging")) => LiquidSettings::IgnoreWaterlogging,
            _ => bail!("invalid jigsaw liquid settings"),
        };
        Ok(Self {
            start_pool,
            start_name,
            max_depth,
            start_height,
            use_expansion_hack,
            heightmap,
            horizontal_distance,
            vertical_distance,
            aliases,
            padding,
            liquid_settings,
            terrain_adjustment,
        })
    }

    /// Sample the configured height before selecting the root rotation. Alias
    /// choices use a separate positional RNG based on the sampled position.
    pub fn begin(
        &self,
        world_seed: i64,
        chunk: [i32; 2],
        height: HeightContext,
    ) -> Result<SeededStart> {
        let mut random = large_feature(world_seed, chunk[0], chunk[1]);
        let y = self
            .start_height
            .sample(height, &mut |bound| random.next_int_bound(bound))?;
        let position = [chunk[0].wrapping_mul(16), y, chunk[1].wrapping_mul(16)];
        let aliases = self.aliases.resolve(world_seed, position)?;
        Ok(SeededStart {
            position,
            aliases,
            random,
        })
    }
}

/// Caller creates an assembly context with these aliases, then validates the
/// resulting generation point's biome before executing its lazy assembler.
pub struct SeededStart {
    pub position: [i32; 3],
    pub aliases: BTreeMap<ResourceId, ResourceId>,
    pub random: LegacyRandom,
}

impl SeededStart {
    pub fn prepare(
        &mut self,
        settings: &JigsawStructure,
        context: &mut impl AssemblyContext,
        env: &mut impl StartEnvironment,
        min_y: i32,
        max_y: i32,
    ) -> Result<Option<PreparedStart>> {
        let (pool, key) = match &settings.start_pool {
            StartPool::Reference(id) => (
                context
                    .pool(id)?
                    .with_context(|| format!("missing start pool {}:{}", id.namespace, id.path))?,
                Some(id),
            ),
            StartPool::Inline(pool) => (Arc::clone(pool), None),
        };
        jigsaw_placement::prepare_start(
            context,
            env,
            &pool,
            StartOptions {
                position: self.position,
                pool_key: key,
                start_name: settings.start_name.as_ref(),
                project_to_heightmap: settings.heightmap,
                min_y,
                max_y,
                padding: settings.padding,
                max_depth: settings.max_depth,
                horizontal_distance: settings.horizontal_distance,
                vertical_distance: settings.vertical_distance,
                do_expansion_hack: settings.use_expansion_hack,
            },
            &mut |bound| self.random.next_int_bound(bound),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal() -> Value {
        json!({"type":"minecraft:jigsaw", "start_pool":"example:pool", "size":1, "start_height":{"absolute":0}, "use_expansion_hack":false, "max_distance_from_center":80})
    }

    #[test]
    fn settings_check_ranges_and_default_padding_identity() -> Result<()> {
        let value = minimal();
        let decoded = JigsawStructure::decode(&value)?;
        assert_eq!(decoded.padding, DimensionPadding::DefaultZero);
        let mut explicit = value.clone();
        explicit["dimension_padding"] = json!(0);
        assert_eq!(
            JigsawStructure::decode(&explicit)?.padding,
            DimensionPadding::Explicit { bottom: 0, top: 0 }
        );
        explicit["max_distance_from_center"] = json!({"horizontal":1});
        assert_eq!(JigsawStructure::decode(&explicit)?.vertical_distance, 4064);
        explicit["size"] = json!(21);
        assert!(JigsawStructure::decode(&explicit).is_err());
        Ok(())
    }

    fn settings_line(id: &str, settings: &JigsawStructure) -> Result<String> {
        let StartPool::Reference(pool) = &settings.start_pool else {
            bail!("expected named test pool");
        };
        let name = settings
            .start_name
            .as_ref()
            .map(|id| format!("{}:{}", id.namespace, id.path))
            .unwrap_or("-".into());
        let (padding, bottom, top) = match settings.padding {
            DimensionPadding::DefaultZero => ("default", 0, 0),
            DimensionPadding::Explicit { bottom, top } => ("explicit", bottom, top),
        };
        let liquid = match settings.liquid_settings {
            LiquidSettings::ApplyWaterlogging => "apply_waterlogging",
            LiquidSettings::IgnoreWaterlogging => "ignore_waterlogging",
        };
        let map = match settings.heightmap {
            None => "-",
            Some(StartHeightmap::WorldSurfaceWg) => "WORLD_SURFACE_WG",
            Some(StartHeightmap::WorldSurface) => "WORLD_SURFACE",
            Some(StartHeightmap::OceanFloorWg) => "OCEAN_FLOOR_WG",
            Some(StartHeightmap::OceanFloor) => "OCEAN_FLOOR",
            Some(StartHeightmap::MotionBlocking) => "MOTION_BLOCKING",
            Some(StartHeightmap::MotionBlockingNoLeaves) => "MOTION_BLOCKING_NO_LEAVES",
        };
        Ok(format!(
            "C\t{id}\t{}:{}\t{name}\t{}\t{}\t{}\t{}\t{padding}\t{bottom}\t{top}\t{liquid}\t{map}",
            pool.namespace,
            pool.path,
            settings.max_depth,
            settings.use_expansion_hack,
            settings.horizontal_distance,
            settings.vertical_distance
        ))
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn vanilla_settings_and_start_rng_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let _verified = crate::packs::ResourceStack::open(&game)?;
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
        let source = temp.path().join("JigsawSettingsOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawSettingsOracle.java"),
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
            .arg("JigsawSettingsOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let mut cases = BTreeMap::new();
        let mut seen = [0, 0];
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["D", id, json] => {
                    cases.insert(
                        id.parse::<usize>()?,
                        JigsawStructure::decode(&serde_json::from_str::<Value>(json)?),
                    );
                }
                ["C", id, "-"] => {
                    assert!(cases.get(&id.parse()?).context("missing case")?.is_err());
                    seen[0] += 1;
                }
                ["C", id, ..] => {
                    let settings = cases
                        .get(&id.parse()?)
                        .context("missing case")?
                        .as_ref()
                        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                    assert_eq!(settings_line(id, settings)?, line);
                    seen[0] += 1;
                }
                ["S", id, seed, x, z, expected_y, remainder] => {
                    let settings = cases
                        .get(&id.parse()?)
                        .context("missing case")?
                        .as_ref()
                        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                    let mut start = settings.begin(
                        seed.parse()?,
                        [x.parse()?, z.parse()?],
                        HeightContext {
                            min_y: -64,
                            height: 384,
                            sea_level: 63,
                        },
                    )?;
                    assert_eq!(
                        start.position[1],
                        expected_y.parse::<i32>()?,
                        "height case {id}"
                    );
                    let mut rest = start.random.next_long().to_string();
                    for (alias, target) in start.aliases {
                        rest.push_str(&format!(
                            "|{}:{}={}:{}",
                            alias.namespace, alias.path, target.namespace, target.path
                        ));
                    }
                    assert_eq!(rest, *remainder, "RNG/alias case {id}, seed {seed}");
                    seen[1] += 1;
                }
                _ => {}
            }
        }
        assert_eq!(seen, [6, 60]);
        Ok(())
    }
}
