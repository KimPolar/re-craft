//! Prepare an existing world's noise generator from captured settings.
use super::*;
use anyhow::{Context, bail};
use native_persistence::{
    nbt::{NamedTag, Tag},
    world::{DimensionId, GenerationContextSnapshot, GenerationInputSnapshot, WorldReader},
};
use native_session::worldgen::schedule::GenerationSchedule;

/// A generator bound to the exact captured inputs from which it was prepared.
/// Bind the captured inputs with `add_save_dependencies` when saving.
/// Live state and generation completeness still need independent checks.
pub(crate) struct PreparedExistingGenerator {
    pub generator: TerrainGenerator,
    pub lighting: GeneratedDimensionLighting,
    pub dimension_definition: Value,
    pub generate_structures: bool,
    pub resources: ResourceStack,
    context: GenerationContextSnapshot,
    identity: Arc<()>,
    terrain_assets: Option<SavedTerrainAssets>,
}

struct SavedTerrainAssets {
    tags: CarverBlockTags,
    blocks: native_resources::block_registry::GeneratedBlockRegistry,
    carvers: native_resources::carver::schedule::CarverCache,
}

/// Read-only restoration of captured disk inputs. It does not include unsaved
/// session changes or prove that the chunks can resume every generation stage.
pub(crate) struct RestoredSavedTerrain {
    identity: Arc<()>,
    world: native_world::loaded::World,
    biomes:
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_session::worldgen::QuartBiomeChunk>>,
    heightmaps: BTreeMap<
        native_world::loaded::ChunkPos,
        native_persistence::heightmap::SavedGenerationHeightmaps,
    >,
    postprocessing: BTreeMap<
        native_world::loaded::ChunkPos,
        Option<native_core::postprocessing::PostProcessing>,
    >,
}

impl RestoredSavedTerrain {
    pub fn world(&self) -> &native_world::loaded::World {
        &self.world
    }
    pub fn biomes(
        &self,
    ) -> &BTreeMap<native_world::loaded::ChunkPos, Arc<native_session::worldgen::QuartBiomeChunk>>
    {
        &self.biomes
    }
    pub fn heightmaps(
        &self,
    ) -> &BTreeMap<
        native_world::loaded::ChunkPos,
        native_persistence::heightmap::SavedGenerationHeightmaps,
    > {
        &self.heightmaps
    }
    pub fn postprocessing(
        &self,
    ) -> &BTreeMap<
        native_world::loaded::ChunkPos,
        Option<native_core::postprocessing::PostProcessing>,
    > {
        &self.postprocessing
    }
}

impl PreparedExistingGenerator {
    pub fn inputs(&self) -> &GenerationInputSnapshot {
        self.context.inputs()
    }

    /// Restore captured biome volumes against this generator's pack registry.
    /// The caller must still restore other stage data before advancing a plan.
    pub fn restore_saved_biomes(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
    ) -> Result<
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_session::worldgen::QuartBiomeChunk>>,
    > {
        let registry: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        ensure!(!registry.is_empty(), "active biome registry is empty");
        let mut restored = BTreeMap::new();
        let sources = self.context.inputs().sources().chunks();
        for position in positions {
            let source = sources
                .get(&position)
                .context("biome chunk was not captured")?;
            let source = source
                .terrain
                .as_ref()
                .context("saved biome chunk is missing")?;
            let chunk = native_session::worldgen::QuartBiomeChunk::from_saved_chunk(
                source,
                self.context.inputs().settings().seed(),
                self.lighting.min_y,
                self.lighting.height,
                |biome| {
                    ensure!(
                        registry.contains(biome),
                        "saved biome is not in the active registry: {}:{}",
                        biome.namespace,
                        biome.path
                    );
                    Ok(())
                },
            )?;
            let position = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            ensure!(
                restored.insert(position, Arc::new(chunk)).is_none(),
                "duplicate saved biome chunk"
            );
        }
        ensure!(!restored.is_empty(), "no saved biome chunks requested");
        Ok(restored)
    }

    /// Restore the effective reference sets from one captured saved chunk.
    /// The returned report identifies data Java would discard on loading.
    pub fn restore_saved_structure_references(
        &mut self,
        position: native_persistence::world::ChunkPos,
    ) -> Result<native_session::worldgen::structure_references::SavedStructureReferences> {
        use native_session::worldgen::{
            schedule::GenerationStage, structure_references::restore_saved_structure_references,
        };
        let registry: BTreeSet<_> = self
            .resources
            .list_data("worldgen/structure")?
            .into_iter()
            .collect();
        let saved = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&position)
            .context("structure chunk was not captured")?
            .terrain
            .as_ref()
            .context("saved structure chunk is missing")?;
        let status = saved
            .status
            .as_ref()
            .context("saved chunk status is missing")?;
        let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
        ensure!(
            GenerationStage::from_id(&status)
                .is_some_and(|stage| stage >= GenerationStage::StructureReferences),
            "saved chunk has not generated structure references"
        );
        let Tag::Compound(root) = &saved.document.value else {
            bail!("saved chunk is not a compound")
        };
        let mut structures = root.iter().filter(|(name, _)| name.matches("structures"));
        let structure = structures.next().map(|(_, tag)| tag);
        ensure!(structures.next().is_none(), "duplicate structure data");
        let references = if let Some(Tag::Compound(fields)) = structure {
            let mut entries = fields.iter().filter(|(name, _)| name.matches("References"));
            let references = entries.next().map(|(_, tag)| tag);
            ensure!(entries.next().is_none(), "duplicate structure references");
            references
        } else {
            structure
        };
        restore_saved_structure_references(
            native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            },
            references,
            |id| Ok(registry.contains(id)),
        )
    }

    /// Restore supported starts using the captured world's active structure
    /// definitions. Unknown or incomplete pieces reject the whole result.
    pub fn restore_saved_structure_starts(
        &mut self,
        position: native_persistence::world::ChunkPos,
    ) -> Result<
        BTreeMap<
            ResourceId,
            native_session::worldgen::saved_structure_starts::RestoredStructureStart,
        >,
    > {
        use native_session::worldgen::{
            saved_structure_starts::restore_saved_structure_starts_with_templates,
            schedule::GenerationStage,
        };
        let registry: BTreeSet<_> = self
            .resources
            .list_data("worldgen/structure")?
            .into_iter()
            .collect();
        let saved = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&position)
            .context("structure chunk was not captured")?
            .terrain
            .as_ref()
            .context("saved structure chunk is missing")?;
        let status = saved
            .status
            .as_ref()
            .context("saved chunk status is missing")?;
        let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
        ensure!(
            GenerationStage::from_id(&status)
                .is_some_and(|stage| stage >= GenerationStage::StructureStarts),
            "saved chunk has not generated structure starts"
        );
        let document = Arc::clone(&saved.document);
        let Tag::Compound(root) = &document.value else {
            bail!("saved chunk is not a compound")
        };
        let mut entries = root.iter().filter(|(name, _)| name.matches("structures"));
        let structures = entries.next().map(|(_, tag)| tag);
        ensure!(entries.next().is_none(), "duplicate structure data");
        let starts = match structures {
            None => None,
            Some(Tag::Compound(fields)) => {
                let mut entries = fields.iter().filter(|(name, _)| name.matches("starts"));
                let value = entries.next().map(|(_, tag)| tag);
                ensure!(entries.next().is_none(), "duplicate structure starts");
                value
            }
            Some(_) => bail!("structure data is not a compound"),
        };
        let resources = std::cell::RefCell::new(&mut self.resources);
        restore_saved_structure_starts_with_templates(
            native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            },
            starts,
            |id| {
                if !registry.contains(id) {
                    return Ok(None);
                }
                let path = format!("data/{}/worldgen/structure/{}.json", id.namespace, id.path);
                let bytes = resources
                    .borrow_mut()
                    .read_data(&path)?
                    .with_context(|| format!("missing structure definition {path}"))?;
                Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                    format!("invalid structure definition {path}")
                })?))
            },
            |id| {
                let mut resources = resources.borrow_mut();
                let template = native_persistence::structure_template::StructureTemplate::load(
                    &mut **resources,
                    id,
                )?;
                template
                    .map(|template| {
                        ensure!(
                            template.data_version == Some(5023),
                            "unsupported portal template data version"
                        );
                        Ok(template.size)
                    })
                    .transpose()
            },
        )
    }

    /// Prepare terrain adaptation from captured starts and references. The
    /// caller must provide the corresponding Java structure-key order.
    pub fn restore_saved_beardifier(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
    ) -> Result<native_core::beardifier::Beardifier> {
        use native_session::worldgen::saved_structure_starts::saved_beardifier_for_chunk;
        let references = self.restore_saved_structure_references(target)?;
        ensure!(
            references.discarded.is_empty(),
            "saved structure references contain discarded data"
        );
        let sources: BTreeSet<_> = references
            .references
            .membership()
            .values()
            .flatten()
            .map(|packed| native_persistence::world::ChunkPos {
                x: *packed as u32 as i32,
                z: ((*packed as u64) >> 32) as u32 as i32,
            })
            .collect();
        let mut starts = BTreeMap::new();
        for source in sources {
            let loaded = self.restore_saved_structure_starts(source)?;
            starts.insert(
                native_world::loaded::ChunkPos {
                    x: source.x,
                    z: source.z,
                },
                loaded,
            );
        }
        saved_beardifier_for_chunk(
            native_world::loaded::ChunkPos {
                x: target.x,
                z: target.z,
            },
            &references,
            structure_order,
            &starts,
        )
    }

    /// Resume noise generation from a captured BIOMES-stage chunk using its
    /// saved structure references. This intermediate result is not a completed
    /// chunk: callers must still verify blending, live state, all generation
    /// dependencies, subsequent stages and save dependencies before use.
    pub fn generate_saved_structure_noise(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
        evaluator: &mut native_resources::density::DensityEvaluator,
    ) -> Result<native_resources::noise_stage::NoiseStageChunk> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            self.generate_structures,
            "generator has no structure context"
        );
        let saved = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&target)
            .context("noise chunk was not captured")?
            .terrain
            .as_ref()
            .context("saved noise chunk is missing")?;
        let status = saved
            .status
            .as_ref()
            .context("saved chunk status is missing")?;
        let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
        ensure!(
            GenerationStage::from_id(&status) == Some(GenerationStage::Biomes),
            "saved chunk must be at the biomes stage"
        );
        let beardifier = Arc::new(self.restore_saved_beardifier(target, structure_order)?);
        self.generator.generate_noise_with_structure_context(
            evaluator,
            [target.x, target.z],
            beardifier,
        )
    }

    /// Resume a captured BIOMES-stage chunk through terrain, surface and
    /// carvers using its saved neighbors. This is an intermediate result: live
    /// state, remaining stages and save dependencies still require validation.
    pub fn generate_saved_terrain(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
        evaluator: &mut native_resources::density::DensityEvaluator,
    ) -> Result<native_resources::surface_pass::SurfaceStageChunk> {
        let source = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&target)
            .context("terrain target was not captured")?
            .terrain
            .as_ref()
            .context("saved terrain target is missing")?;
        validate_empty_biomes_target(source)?;

        let mut positions = Vec::with_capacity(9);
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let pos = native_persistence::world::ChunkPos {
                    x: target
                        .x
                        .checked_add(dx)
                        .context("neighbor chunk X overflow")?,
                    z: target
                        .z
                        .checked_add(dz)
                        .context("neighbor chunk Z overflow")?,
                };
                let saved = self
                    .context
                    .inputs()
                    .sources()
                    .chunks()
                    .get(&pos)
                    .context("biome neighbor was not captured")?
                    .terrain
                    .as_ref()
                    .context("saved biome neighbor is missing")?;
                reject_saved_blending(&saved.document)?;
                positions.push(pos);
            }
        }
        let biomes = self.restore_saved_biomes(positions)?;
        let mut neighborhood = self.generator.biome_neighborhood()?;
        for biome in biomes.into_values() {
            neighborhood.insert(biome)?;
        }
        let structure = if self.generate_structures {
            Some(Arc::new(
                self.restore_saved_beardifier(target, structure_order)?,
            ))
        } else {
            ensure!(structure_order.is_empty(), "structures are disabled");
            None
        };
        if self.terrain_assets.is_none() {
            let blocks = self.resources.vanilla_block_ids()?;
            let tags = CarverBlockTags::load(&mut self.resources, &blocks)?;
            let blocks = self.resources.vanilla_generated_block_registry()?;
            self.terrain_assets = Some(SavedTerrainAssets {
                tags,
                blocks,
                carvers: native_resources::carver::schedule::CarverCache::default(),
            });
        }
        let assets = self
            .terrain_assets
            .as_mut()
            .expect("terrain assets initialized");
        let classify = |value: &Value| -> Result<CarverBlockState> {
            let state = resolve_generated_terrain_state(value, &assets.blocks)?;
            Ok(CarverBlockState {
                is_air: matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ),
                has_fluid: FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some(),
            })
        };
        if let Some(structure) = structure {
            self.generator
                .generate_carved_surface_with_structure_context(
                    &mut self.resources,
                    evaluator,
                    &mut assets.carvers,
                    [target.x, target.z],
                    &neighborhood,
                    &assets.tags,
                    structure,
                    classify,
                )
        } else {
            let previous = evaluator.replace_structure_terrain_context(None);
            let result = self.generator.generate_carved_surface(
                &mut self.resources,
                evaluator,
                &mut assets.carvers,
                [target.x, target.z],
                &neighborhood,
                &assets.tags,
                classify,
            );
            evaluator.replace_structure_terrain_context(previous);
            result
        }
    }

    /// Restore captured terrain and recorded generation metadata without
    /// replacing unknown blocks, priming heightmaps or consuming marks.
    pub fn restore_saved_terrain(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
    ) -> Result<RestoredSavedTerrain> {
        use native_persistence::{
            heightmap::{HeightBounds, SavedGenerationHeightmaps},
            world::ChunkArea,
        };
        use native_session::worldgen::schedule::GenerationStage;

        let mut requested = BTreeSet::new();
        for position in positions {
            ensure!(requested.insert(position), "duplicate saved terrain chunk");
        }
        ensure!(!requested.is_empty(), "no saved terrain chunks requested");
        let biomes = self.restore_saved_biomes(requested.iter().copied())?;
        let block_registry = self.resources.vanilla_generated_block_registry()?;
        let bounds = HeightBounds {
            min_y: self.lighting.min_y,
            max_y: self
                .lighting
                .min_y
                .checked_add(self.lighting.height)
                .context("dimension height overflow")?,
        };
        let mut chunks = BTreeMap::new();
        let mut heightmaps = BTreeMap::new();
        let mut postprocessing = BTreeMap::new();
        for position in requested {
            let source = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&position)
                .context("terrain chunk was not captured")?
                .terrain
                .as_ref()
                .context("saved terrain chunk is missing")?;
            let status = source
                .status
                .as_ref()
                .context("saved chunk status is missing")?;
            let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
            ensure!(
                GenerationStage::from_id(&status)
                    .is_some_and(|stage| stage >= GenerationStage::Terrain),
                "saved chunk has not generated terrain"
            );
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            heightmaps.insert(
                key,
                SavedGenerationHeightmaps::read(&source.document, bounds)?,
            );
            let Tag::Compound(fields) = &source.document.value else {
                bail!("saved chunk is not a compound")
            };
            let mut marks = fields
                .iter()
                .filter(|(name, _)| name.matches("PostProcessing"));
            let decoded = marks
                .next()
                .map(|(_, tag)| {
                    native_persistence::postprocessing::decode(
                        tag,
                        bounds.min_y,
                        self.lighting.height,
                    )
                })
                .transpose()?;
            ensure!(
                marks.next().is_none(),
                "duplicate saved postprocessing field"
            );
            postprocessing.insert(key, decoded);
            chunks.insert(position, Arc::new(source.clone()));
        }
        let area = ChunkArea {
            center: *chunks.keys().next().expect("nonempty requested set"),
            radius: 0,
            chunks,
            missing: vec![],
        };
        let world = area.materialize_with_defaults(
            self.context.inputs().sources().dimension(),
            &block_registry.default_states(),
        )?;
        for (_, state) in world.states() {
            ensure!(
                block_registry.is_canonical_state(&state.name, &state.properties),
                "unsupported saved block state: {}",
                state.name
            );
        }
        Ok(RestoredSavedTerrain {
            identity: Arc::clone(&self.identity),
            world,
            biomes,
            heightmaps,
            postprocessing,
        })
    }

    /// Bind every captured terrain position to the current live session.
    /// The returned guard must be checked again on the simulation thread; it
    /// does not protect live block-entity internals, entities, POIs, generation
    /// metadata, game clocks, packs, or disk inputs.
    pub fn capture_live_terrain(
        &self,
        session: &native_session::loaded::LoadedSession,
        restored: &RestoredSavedTerrain,
    ) -> Result<native_session::loaded::GenerationTerrainGuard> {
        ensure!(
            Arc::ptr_eq(&self.identity, &restored.identity),
            "restored terrain belongs to another capture"
        );
        let sources = self.context.inputs().sources().chunks();
        for (pos, source) in sources {
            let key = native_world::loaded::ChunkPos { x: pos.x, z: pos.z };
            ensure!(
                source.terrain.is_some() == restored.world.chunks.contains_key(&key),
                "captured terrain was not fully restored at {key:?}"
            );
        }
        session.capture_generation_terrain(
            &restored.world,
            sources
                .keys()
                .map(|pos| native_world::loaded::ChunkPos { x: pos.x, z: pos.z }),
        )
    }

    /// Bind captured disk and pack inputs; live session state and generation
    /// completeness still require independent checks before saving.
    pub fn add_save_dependencies(
        &self,
        plan: &mut native_persistence::save::WorldSavePlan,
    ) -> Result<()> {
        plan.add_generation_context(&self.context)
    }
}

fn reject_saved_blending(document: &NamedTag) -> Result<()> {
    let Tag::Compound(fields) = &document.value else {
        bail!("saved chunk is not a compound");
    };
    ensure!(
        !fields
            .iter()
            .any(|(name, _)| name.matches("blending_data") || name.matches("below_zero_retrogen")),
        "saved chunk requires unsupported blending or retrogen"
    );
    Ok(())
}

fn validate_empty_biomes_target(saved: &native_persistence::world::LoadedChunk) -> Result<()> {
    use native_session::worldgen::schedule::GenerationStage;
    ensure!(
        saved.data_version == native_persistence::world::TARGET_DATA_VERSION,
        "unsupported saved biome data version"
    );
    let status = saved
        .status
        .as_ref()
        .context("saved chunk status is missing")?;
    let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
    ensure!(
        GenerationStage::from_id(&status) == Some(GenerationStage::Biomes),
        "saved chunk must be at the biomes stage"
    );
    reject_saved_blending(&saved.document)?;
    for section in native_persistence::palette::decode_sections(&saved.document.value)? {
        if let Some(blocks) = section.blocks {
            ensure!(
                blocks.indices.iter().all(|&index| {
                    blocks.states.get(index as usize).is_some_and(|state| {
                        state.name.matches("minecraft:air") && state.properties.is_empty()
                    })
                }),
                "saved biome-stage chunk contains blocks"
            );
        }
    }
    let Tag::Compound(fields) = &saved.document.value else {
        unreachable!()
    };
    ensure!(
        fields
            .iter()
            .filter(|(key, _)| key.matches("sections"))
            .count()
            <= 1,
        "duplicate saved sections"
    );
    for name in [
        "block_entities",
        "block_ticks",
        "fluid_ticks",
        "PostProcessing",
    ] {
        let values: Vec<_> = fields.iter().filter(|(key, _)| key.matches(name)).collect();
        ensure!(values.len() <= 1, "duplicate saved {name}");
        if let Some((_, value)) = values.first() {
            let empty = match value {
                Tag::List { values, .. } if values.is_empty() => true,
                Tag::List {
                    element_type: native_persistence::nbt::TagType::List,
                    values,
                } if name == "PostProcessing" => values
                    .iter()
                    .all(|value| matches!(value, Tag::List { values, .. } if values.is_empty())),
                _ => false,
            };
            ensure!(empty, "saved biome-stage chunk contains {name}");
        }
    }
    Ok(())
}

fn numeric_bool(tag: &Tag) -> Option<bool> {
    Some(match tag {
        Tag::Byte(v) => *v != 0,
        Tag::Short(v) => *v != 0,
        Tag::Int(v) => *v != 0,
        Tag::Long(v) => *v != 0,
        Tag::Float(v) if v.is_finite() => *v != 0.0,
        Tag::Double(v) if v.is_finite() => *v != 0.0,
        _ => return None,
    })
}

// NBT encodes booleans as numbers, so only fields known to have boolean
// codec types can be converted without changing ordinary numeric values.
fn boolean_key(key: &str) -> bool {
    matches!(
        key,
        "has_skylight"
            | "has_ceiling"
            | "has_fixed_time"
            | "has_ender_dragon_fight"
            | "destroy_on_use"
            | "destroy_on_leave"
            | "gameplay/fast_lava"
            | "minecraft:gameplay/fast_lava"
            | "gameplay/water_evaporates"
            | "minecraft:gameplay/water_evaporates"
            | "gameplay/respawn_anchor_works"
            | "minecraft:gameplay/respawn_anchor_works"
            | "legacy_random_source"
            | "disable_mob_generation"
            | "aquifers_enabled"
            | "ore_veins_enabled"
            | "is_3d"
            | "add_stone_depth"
            | "add_surface_depth"
    )
}

fn saved_value(value: &Tag, boolean: bool, depth: usize, nodes: &mut usize) -> Result<Value> {
    ensure!(depth <= 128, "saved dimension is too deeply nested");
    *nodes = nodes
        .checked_add(1)
        .context("saved dimension node count overflow")?;
    ensure!(*nodes <= 1_000_000, "saved dimension has too many values");
    if boolean && let Some(value) = numeric_bool(value) {
        return Ok(Value::Bool(value));
    }
    Ok(match value {
        Tag::Byte(v) => Value::from(*v),
        Tag::Short(v) => Value::from(*v),
        Tag::Int(v) => Value::from(*v),
        Tag::Long(v) => Value::from(*v),
        Tag::Float(v) => Value::Number(
            serde_json::Number::from_f64(f64::from(*v)).context("non-finite saved value")?,
        ),
        Tag::Double(v) => {
            Value::Number(serde_json::Number::from_f64(*v).context("non-finite saved value")?)
        }
        Tag::String(v) => Value::String(String::from_utf16(&v.0).context("invalid saved string")?),
        Tag::List { values, .. } => Value::Array(
            values
                .iter()
                .map(|v| saved_value(v, false, depth + 1, nodes))
                .collect::<Result<_>>()?,
        ),
        Tag::Compound(fields) => {
            let mut output = serde_json::Map::new();
            for (key, value) in fields {
                let key = String::from_utf16(&key.0).context("invalid saved key")?;
                let is_boolean = boolean_key(&key) || (boolean && key == "argument");
                let value = saved_value(value, is_boolean, depth + 1, nodes)?;
                ensure!(
                    output.insert(key, value).is_none(),
                    "duplicate saved dimension field"
                );
            }
            Value::Object(output)
        }
        Tag::ByteArray(values) => {
            Value::Array(values.iter().map(|v| Value::from(*v as i8)).collect())
        }
        Tag::IntArray(values) => Value::Array(values.iter().map(|v| Value::from(*v)).collect()),
        Tag::LongArray(values) => Value::Array(values.iter().map(|v| Value::from(*v)).collect()),
    })
}

fn resolve_stem(
    provider: &mut impl ResourceProvider,
    document: &NamedTag,
    dimension: &DimensionId,
) -> Result<(Value, bool)> {
    let Tag::Compound(data) = document
        .value
        .get("data")
        .context("missing world generation data")?
    else {
        bail!("world generation data is not a compound");
    };
    let one = |name: &str| -> Result<Option<&Tag>> {
        let mut found = data.iter().filter(|(key, _)| key.matches(name));
        let value = found.next().map(|(_, value)| value);
        ensure!(
            found.next().is_none(),
            "duplicate world generation field {name}"
        );
        Ok(value)
    };
    let generate_structures = one("generate_structures")?
        .map(|v| numeric_bool(v).context("invalid generate_structures"))
        .transpose()?
        .unwrap_or(true);
    let Tag::Compound(dimensions) = one("dimensions")?.context("missing saved dimensions")? else {
        bail!("saved dimensions are not a compound");
    };
    let mut saved = None;
    let mut names = BTreeSet::new();
    for (name, stem) in dimensions {
        let name = String::from_utf16(&name.0).context("invalid saved dimension name")?;
        let id = DimensionId::parse(&name)?;
        ensure!(names.insert(id.clone()), "duplicate saved dimension {name}");
        if &id == dimension {
            saved = Some(stem);
        }
    }
    let path = format!(
        "data/{}/dimension/{}.json",
        dimension.namespace, dimension.path
    );
    if let Some(bytes) = provider.read_data(&path)? {
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "dimension definition is too large"
        );
        let stem =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid dimension {path}"))?;
        return Ok((stem, generate_structures));
    }
    let saved = saved.context("dimension is missing from the saved world and active packs")?;
    ensure!(
        matches!(saved, Tag::Compound(_)),
        "saved dimension is not a compound"
    );
    Ok((saved_value(saved, false, 0, &mut 0)?, generate_structures))
}

/// Compile using the saved generator and the pack selection captured from the
/// same world. A later save must bind these inputs as transaction dependencies.
pub(crate) fn prepare_existing_generator(
    context: GenerationContextSnapshot,
    minecraft_dir: &std::path::Path,
) -> Result<PreparedExistingGenerator> {
    let packs = Arc::clone(context.packs());
    packs.verify()?;
    let selected = context.selection();
    let selection = DataPackSettings {
        enabled: selected.enabled.clone(),
        disabled: selected.disabled.clone(),
        enabled_features: selected.enabled_features.clone(),
    };
    let mut resources = ResourceStack::open(minecraft_dir)?;
    let warnings = resources.configure_snapshot_data(Arc::clone(&packs), &selection)?;
    ensure!(
        warnings.is_empty(),
        "data pack selection needs attention: {}",
        warnings.join("; ")
    );
    let inputs = context.inputs();
    let dimension = inputs.sources().dimension();
    let (stem, generate_structures) =
        resolve_stem(&mut resources, inputs.settings().document(), dimension)?;
    let id = ResourceId::parse(&format!("{}:{}", dimension.namespace, dimension.path))?;
    let (lighting, definition, dimension_definition) =
        GeneratedDimensionLighting::from_stem(&mut resources, id, &stem)?;
    let seed = inputs.settings().seed();
    let generator = if generate_structures {
        TerrainGenerator::with_structure_context(
            &mut resources,
            &definition,
            seed,
            lighting.min_y,
            lighting.height,
        )?
    } else {
        TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &definition,
            seed,
            lighting.min_y,
            lighting.height,
        )?
    };
    packs.verify()?;
    Ok(PreparedExistingGenerator {
        generator,
        lighting,
        dimension_definition,
        generate_structures,
        resources,
        context,
        identity: Arc::new(()),
        terrain_assets: None,
    })
}

/// Capture the scheduler's complete current neighborhood before preparing the
/// generator. If the schedule is expanded, callers must capture and prepare
/// again; dynamic inputs outside this schedule and unsaved session changes
/// still need independent tracking.
pub(crate) fn prepare_existing_generator_for_schedule(
    reader: &mut WorldReader,
    schedule: &GenerationSchedule,
    minecraft_dir: &std::path::Path,
) -> Result<PreparedExistingGenerator> {
    let context = capture_scheduled_context(reader, schedule)?;
    prepare_existing_generator(context, minecraft_dir)
}

fn capture_scheduled_context(
    reader: &mut WorldReader,
    schedule: &GenerationSchedule,
) -> Result<GenerationContextSnapshot> {
    ensure!(schedule.requested_chunks() != 0, "no generation requests");
    reader.capture_generation_context(
        schedule
            .requirements()
            .map(|(pos, _)| native_persistence::world::ChunkPos { x: pos.x, z: pos.z }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::{JavaString, TagType};
    use serde_json::json;

    struct Packs(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Packs {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }

    fn settings(stem: Tag) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "data".into(),
                    Tag::Compound(vec![
                        ("seed".into(), Tag::Long(42)),
                        ("generate_structures".into(), Tag::Byte(0)),
                        (
                            "dimensions".into(),
                            Tag::Compound(vec![("minecraft:overworld".into(), stem)]),
                        ),
                    ]),
                ),
            ]),
        }
    }

    #[test]
    fn saved_biome_target_refuses_terrain_or_deferred_changes() -> Result<()> {
        use native_persistence::{
            palette::decode_sections,
            world::{ChunkPos, LoadedChunk},
        };
        let mut document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(0)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String("minecraft:air".into())],
                                },
                            )]),
                        ),
                    ])],
                },
            )]),
        };
        let loaded = |document: &NamedTag| -> Result<LoadedChunk> {
            Ok(LoadedChunk {
                position: ChunkPos { x: 0, z: 0 },
                data_version: 5023,
                status: Some("minecraft:biomes".into()),
                document: Arc::new(document.clone()),
                sections: decode_sections(&document.value)?,
            })
        };
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_ok());
        let mut later = loaded(&document)?;
        later.status = Some("minecraft:terrain".into());
        assert!(validate_empty_biomes_target(&later).is_err());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.push((
            "PostProcessing".into(),
            Tag::List {
                element_type: TagType::List,
                values: vec![Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                }],
            },
        ));
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_ok());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields.last_mut().unwrap().1 else {
            unreachable!()
        };
        values[0] = Tag::List {
            element_type: TagType::Short,
            values: vec![Tag::Short(1)],
        };
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_err());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.pop();
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.push(("blending_data".into(), Tag::Compound(vec![])));
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_err());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.pop();
        fields.push((
            "block_ticks".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![])],
            },
        ));
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_err());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.pop();
        let Tag::List { values, .. } = &mut fields[0].1 else {
            unreachable!()
        };
        let Tag::Compound(section) = &mut values[0] else {
            unreachable!()
        };
        let Tag::Compound(states) = &mut section[1].1 else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut states[0].1 else {
            unreachable!()
        };
        values[0] = Tag::String("minecraft:stone".into());
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_err());
        Ok(())
    }

    #[test]
    fn scheduled_capture_tracks_every_required_neighbor_and_expansion() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_session::worldgen::schedule::GenerationStage;
        use native_world::loaded::ChunkPos;
        use std::io::Write;

        let world = tempfile::tempdir()?;
        let root = world.path();
        std::fs::create_dir_all(root.join("data/minecraft"))?;
        std::fs::create_dir_all(root.join("dimensions/minecraft/overworld/region"))?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&settings(Tag::Compound(vec![])))?,
        )?;
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        let mut reader = WorldReader::open_for_session(root, DimensionId::parse("overworld")?)?;
        let mut schedule = GenerationSchedule::new();
        assert!(capture_scheduled_context(&mut reader, &schedule).is_err());
        let target = ChunkPos { x: -3, z: 4 };
        schedule.request_many([
            (target, GenerationStage::Full),
            (ChunkPos { x: -2, z: 4 }, GenerationStage::Terrain),
        ])?;
        let expected: BTreeSet<_> = schedule
            .requirements()
            .map(|(pos, _)| (pos.x, pos.z))
            .collect();
        let captured = capture_scheduled_context(&mut reader, &schedule)?;
        let sources = captured.inputs().sources().chunks();
        assert_eq!(
            sources
                .keys()
                .map(|pos| (pos.x, pos.z))
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert!(sources.values().all(|source| {
            source.terrain.is_none() && source.entities.is_none() && source.poi.is_none()
        }));
        assert!(expected.len() > 1);

        let before = schedule.requested_chunks();
        assert!(
            schedule
                .request(ChunkPos { x: i32::MAX, z: 0 }, GenerationStage::Full)
                .is_err()
        );
        assert_eq!(schedule.requested_chunks(), before);
        schedule.request(ChunkPos { x: 50, z: 50 }, GenerationStage::Full)?;
        let expanded = capture_scheduled_context(&mut reader, &schedule)?;
        assert!(expanded.inputs().sources().chunks().len() > sources.len());
        assert_eq!(
            expanded.inputs().sources().chunks().len(),
            schedule.requested_chunks()
        );
        Ok(())
    }

    #[test]
    fn saved_stems_keep_numeric_values_and_respect_active_overrides() -> Result<()> {
        let stem = Tag::Compound(vec![
            ("type".into(), Tag::String("minecraft:overworld".into())),
            (
                "generator".into(),
                Tag::Compound(vec![
                    ("type".into(), Tag::String("minecraft:noise".into())),
                    (
                        "settings".into(),
                        Tag::Compound(vec![
                            ("legacy_random_source".into(), Tag::Byte(1)),
                            ("sea_level".into(), Tag::Byte(1)),
                            (
                                "material_rule".into(),
                                Tag::Compound(vec![("add_stone_depth".into(), Tag::Byte(0))]),
                            ),
                        ]),
                    ),
                    (
                        "biome_source".into(),
                        Tag::Compound(vec![
                            ("type".into(), Tag::String("minecraft:fixed".into())),
                            ("biome".into(), Tag::String("minecraft:plains".into())),
                        ]),
                    ),
                ]),
            ),
        ]);
        let doc = settings(stem.clone());
        let dimension = DimensionId::parse("overworld")?;
        let mut packs = Packs(BTreeMap::new());
        let (saved, structures) = resolve_stem(&mut packs, &doc, &dimension)?;
        assert!(!structures);
        assert_eq!(saved["generator"]["settings"]["legacy_random_source"], true);
        assert_eq!(saved["generator"]["settings"]["sea_level"], 1);
        assert_eq!(
            saved["generator"]["settings"]["material_rule"]["add_stone_depth"],
            false
        );
        let active = json!({"type":"minecraft:the_nether","generator":{"type":"minecraft:noise","settings":"minecraft:nether","biome_source":{"type":"minecraft:the_end"}}});
        packs.0.insert(
            "data/minecraft/dimension/overworld.json".into(),
            serde_json::to_vec(&active)?,
        );
        assert_eq!(resolve_stem(&mut packs, &doc, &dimension)?.0, active);
        assert!(resolve_stem(&mut packs, &doc, &DimensionId::parse("minecraft:missing")?).is_err());
        let duplicate = settings(Tag::Compound(vec![
            ("type".into(), Tag::String("minecraft:overworld".into())),
            ("type".into(), Tag::String("minecraft:the_nether".into())),
        ]));
        packs.0.clear();
        assert!(resolve_stem(&mut packs, &duplicate, &dimension).is_err());
        let malformed = settings(Tag::Compound(vec![(
            "type".into(),
            Tag::String(JavaString(vec![0xd800])),
        )]));
        assert!(resolve_stem(&mut packs, &malformed, &dimension).is_err());
        assert!(
            saved_value(
                &Tag::List {
                    element_type: TagType::Int,
                    values: vec![Tag::Int(7)]
                },
                false,
                0,
                &mut 0
            )?
            .is_array()
        );
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and a temporary world"]
    fn captured_vanilla_world_prepares_each_noise_dimension() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::{
            save::WorldSavePlan,
            world::{ChunkPos, WorldReader},
            worldgen_settings::{NewWorldOptions, draft_baked_worldgen_settings},
        };
        use std::io::{Seek, SeekFrom, Write};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let mut source = ResourceStack::open(&game)?;
        let worldgen = draft_baked_worldgen_settings(
            &mut source,
            "minecraft:normal",
            NewWorldOptions {
                seed: -42,
                generate_structures: true,
                bonus_chest: false,
            },
        )?;
        std::fs::create_dir_all(root.join("data/minecraft"))?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&worldgen)?,
        )?;
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        for name in ["overworld", "the_nether", "the_end"] {
            use native_persistence::save::{FreshRegionDraft, FreshRegionKind};
            let region_dir = root.join(format!("dimensions/minecraft/{name}/region"));
            std::fs::create_dir_all(&region_dir)?;
            let lighting = GeneratedDimensionLighting::load_preset(
                &mut source,
                "minecraft:normal",
                &format!("minecraft:{name}"),
            )?;
            let sections = (0..lighting.height / 16)
                .map(|index| {
                    let section_y = i8::try_from(lighting.min_y / 16 + index).unwrap();
                    let biome = if index % 2 == 0 {
                        "minecraft:plains"
                    } else {
                        "minecraft:forest"
                    };
                    Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(section_y)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String("minecraft:stone".into())],
                                },
                            )]),
                        ),
                        (
                            "biomes".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String(biome.into())],
                                },
                            )]),
                        ),
                    ])
                })
                .collect();
            let saved_height = [lighting.min_y + 7; 256];
            let maps = native_persistence::heightmap::encode_generated_heightmaps(
                native_persistence::heightmap::HeightBounds {
                    min_y: lighting.min_y,
                    max_y: lighting.min_y + lighting.height,
                },
                &native_persistence::heightmap::GeneratedHeightmaps {
                    world_surface_wg: Some(&saved_height),
                    ..Default::default()
                },
            )?;
            let mut marks =
                native_core::postprocessing::PostProcessing::new(lighting.min_y, lighting.height)
                    .unwrap();
            marks.mark(0, lighting.min_y + 1, 0);
            marks.mark(0, lighting.min_y + 1, 0);
            let template_id = ResourceId::parse("minecraft:ruined_portal/portal_1")?;
            let template = native_persistence::structure_template::StructureTemplate::load(
                &mut source,
                &template_id,
            )?
            .context("portal template is missing")?;
            let selection = native_resources::ruined_portal::InitialSelection {
                setup_index: 0,
                air_pocket: false,
                template: template_id,
                rotation: native_resources::structure_transform::Rotation::None,
                mirror: native_resources::structure_transform::Mirror::None,
            };
            let setup = native_resources::ruined_portal::Setup {
                placement: native_resources::ruined_portal::VerticalPlacement::OnLandSurface,
                air_pocket_probability: 0.0,
                mossiness: 0.2,
                overgrown: false,
                vines: false,
                can_be_cold: true,
                replace_with_blackstone: false,
                weight: 1.0,
            };
            let portal_position = [0, lighting.min_y + 8, 0];
            let pivot = [template.size[0] / 2, 0, template.size[2] / 2];
            let portal_point = native_resources::ruined_portal::GenerationPoint {
                position: portal_position,
                pivot,
                bounds: native_resources::structure_transform::bounding_box(
                    template.size,
                    portal_position,
                    selection.mirror,
                    selection.rotation,
                    pivot,
                )?,
            };
            let portal = native_session::worldgen::ruined_portal::SavedRuinedPortalPiece::capture(
                &selection,
                &setup,
                &portal_point,
                template.size,
                false,
            )?;
            let saved = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("xPos".into(), Tag::Int(0)),
                    ("zPos".into(), Tag::Int(0)),
                    ("Status".into(), Tag::String("minecraft:full".into())),
                    ("Heightmaps".into(), maps),
                    (
                        "PostProcessing".into(),
                        native_persistence::postprocessing::encode(&marks),
                    ),
                    (
                        "structures".into(),
                        Tag::Compound(vec![
                            (
                                "starts".into(),
                                Tag::Compound(vec![
                                    (
                                        "minecraft:buried_treasure".into(),
                                        Tag::Compound(vec![
                                            (
                                                "id".into(),
                                                Tag::String("minecraft:buried_treasure".into()),
                                            ),
                                            ("ChunkX".into(), Tag::Int(0)),
                                            ("ChunkZ".into(), Tag::Int(0)),
                                            ("references".into(), Tag::Int(0)),
                                            (
                                                "Children".into(),
                                                Tag::List {
                                                    element_type: TagType::Compound,
                                                    values: vec![Tag::Compound(vec![
                                                        (
                                                            "id".into(),
                                                            Tag::String("minecraft:btp".into()),
                                                        ),
                                                        (
                                                            "BB".into(),
                                                            Tag::IntArray(vec![
                                                                9,
                                                                lighting.min_y + 8,
                                                                9,
                                                                9,
                                                                lighting.min_y + 8,
                                                                9,
                                                            ]),
                                                        ),
                                                        ("O".into(), Tag::Int(-1)),
                                                        ("GD".into(), Tag::Int(0)),
                                                    ])],
                                                },
                                            ),
                                        ]),
                                    ),
                                    (
                                        "minecraft:swamp_hut".into(),
                                        Tag::Compound(vec![(
                                            "id".into(),
                                            Tag::String("INVALID".into()),
                                        )]),
                                    ),
                                    (
                                        "minecraft:ruined_portal".into(),
                                        Tag::Compound(vec![
                                            (
                                                "id".into(),
                                                Tag::String("minecraft:ruined_portal".into()),
                                            ),
                                            ("ChunkX".into(), Tag::Int(0)),
                                            ("ChunkZ".into(), Tag::Int(0)),
                                            ("references".into(), Tag::Int(0)),
                                            (
                                                "Children".into(),
                                                Tag::List {
                                                    element_type: TagType::Compound,
                                                    values: vec![portal.to_tag()?],
                                                },
                                            ),
                                        ]),
                                    ),
                                ]),
                            ),
                            (
                                "References".into(),
                                Tag::Compound(vec![
                                    (
                                        "minecraft:buried_treasure".into(),
                                        Tag::LongArray(vec![0, 1, 9]),
                                    ),
                                    ("example:unknown".into(), Tag::LongArray(vec![0])),
                                ]),
                            ),
                        ]),
                    ),
                    (
                        "sections".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: sections,
                        },
                    ),
                ]),
            };
            let mut region = FreshRegionDraft::new(FreshRegionKind::Terrain, 0, 0);
            region.add(ChunkPos { x: 0, z: 0 }, &saved, 1)?;
            let mut unknown = saved.clone();
            let Tag::Compound(fields) = &mut unknown.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("xPos"))
                .unwrap()
                .1 = Tag::Int(1);
            let Tag::List {
                values: sections, ..
            } = &mut fields
                .iter_mut()
                .find(|(key, _)| key.matches("sections"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            let Tag::Compound(section) = &mut sections[0] else {
                unreachable!()
            };
            let Tag::Compound(states) = &mut section
                .iter_mut()
                .find(|(key, _)| key.matches("block_states"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            states[0].1 = Tag::List {
                element_type: TagType::String,
                values: vec![Tag::String("example:unknown".into())],
            };
            region.add(ChunkPos { x: 1, z: 0 }, &unknown, 1)?;
            let mut clean = saved.clone();
            let Tag::Compound(fields) = &mut clean.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("xPos"))
                .unwrap()
                .1 = Tag::Int(3);
            let Tag::Compound(structures) = &mut fields
                .iter_mut()
                .find(|(key, _)| key.matches("structures"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            structures
                .iter_mut()
                .find(|(key, _)| key.matches("References"))
                .unwrap()
                .1 = Tag::Compound(vec![(
                "minecraft:buried_treasure".into(),
                Tag::LongArray(vec![0]),
            )]);
            region.add(ChunkPos { x: 3, z: 0 }, &clean, 1)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .read(true)
                .create_new(true)
                .open(region_dir.join("r.0.0.mca"))?;
            region.write_staged(&mut file)?;
            // FreshRegionDraft deliberately writes only full chunks. Replace
            // this test fixture's entry with an uncompressed protochunk so the
            // reader encounters a real BIOMES-stage region record.
            let Tag::Compound(fields) = &mut clean.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("Status"))
                .unwrap()
                .1 = Tag::String("minecraft:biomes".into());
            let bytes = native_persistence::nbt::write(&clean)?;
            let offset = file.seek(SeekFrom::End(0))?;
            assert_eq!(offset % 4096, 0);
            let sectors = (bytes.len() + 5).div_ceil(4096);
            assert!(sectors < 256 && offset / 4096 < (1 << 24));
            file.write_all(&(u32::try_from(bytes.len() + 1)?).to_be_bytes())?;
            file.write_all(&[3])?;
            file.write_all(&bytes)?;
            file.write_all(&vec![0; sectors * 4096 - bytes.len() - 5])?;
            file.seek(SeekFrom::Start(3 * 4))?;
            file.write_all(&(((offset / 4096) as u32) << 8 | sectors as u32).to_be_bytes())?;
            // Nine real BIOMES-stage region entries exercise resuming terrain
            // from captured neighbors, including saved biome identities.
            for z in 2..=4 {
                for x in 2..=4 {
                    let mut biome_chunk = saved.clone();
                    let Tag::Compound(fields) = &mut biome_chunk.value else {
                        unreachable!()
                    };
                    for (key, value) in fields.iter_mut() {
                        if key.matches("xPos") {
                            *value = Tag::Int(x);
                        }
                        if key.matches("zPos") {
                            *value = Tag::Int(z);
                        }
                        if key.matches("Status") {
                            *value = Tag::String("minecraft:biomes".into());
                        }
                        if key.matches("structures") {
                            *value = Tag::Compound(vec![
                                ("starts".into(), Tag::Compound(vec![])),
                                ("References".into(), Tag::Compound(vec![])),
                            ]);
                        }
                        if key.matches("sections") {
                            let Tag::List { values, .. } = value else {
                                unreachable!()
                            };
                            for section in values {
                                let Tag::Compound(section) = section else {
                                    unreachable!()
                                };
                                for (key, value) in section {
                                    if key.matches("block_states") {
                                        *value = Tag::Compound(vec![(
                                            "palette".into(),
                                            Tag::List {
                                                element_type: TagType::String,
                                                values: vec![Tag::String("minecraft:air".into())],
                                            },
                                        )]);
                                    }
                                }
                            }
                        }
                    }
                    fields.retain(|(key, _)| {
                        !key.matches("Heightmaps") && !key.matches("PostProcessing")
                    });
                    let bytes = native_persistence::nbt::write(&biome_chunk)?;
                    let offset = file.seek(SeekFrom::End(0))?;
                    let sectors = (bytes.len() + 5).div_ceil(4096);
                    ensure!(
                        sectors < 256 && offset % 4096 == 0 && offset / 4096 < (1 << 24),
                        "invalid test region offset"
                    );
                    file.write_all(&(u32::try_from(bytes.len() + 1)?).to_be_bytes())?;
                    file.write_all(&[3])?;
                    file.write_all(&bytes)?;
                    file.write_all(&vec![0; sectors * 4096 - bytes.len() - 5])?;
                    file.seek(SeekFrom::Start(((x + z * 32) * 4) as u64))?;
                    file.write_all(
                        &(((offset / 4096) as u32) << 8 | sectors as u32).to_be_bytes(),
                    )?;
                }
            }
            file.flush()?;
            drop(file);
            let dimension = DimensionId::parse(name)?;
            let mut reader = WorldReader::open_for_session(root, dimension)?;
            let mut schedule = GenerationSchedule::new();
            schedule.request(
                native_world::loaded::ChunkPos { x: 0, z: 0 },
                native_session::worldgen::schedule::GenerationStage::Full,
            )?;
            let mut actual =
                prepare_existing_generator_for_schedule(&mut reader, &schedule, &game)?;
            assert_eq!(
                actual.inputs().sources().chunks().len(),
                schedule.requested_chunks()
            );
            assert!(actual.generate_structures);
            assert_eq!(actual.inputs().settings().seed(), -42);
            let position = ChunkPos { x: 0, z: 0 };
            let references = actual.restore_saved_structure_references(position)?;
            let treasure = ResourceId::parse("minecraft:buried_treasure")?;
            let starts = actual.restore_saved_structure_starts(position)?;
            let portal_id = ResourceId::parse("minecraft:ruined_portal")?;
            assert_eq!(
                starts[&portal_id]
                    .reference_bounds(&portal_id)?
                    .unwrap()
                    .bounds,
                portal.bounds
            );
            let bounds = starts[&treasure]
                .reference_bounds(&treasure)?
                .unwrap()
                .bounds;
            assert_eq!(bounds.min, [9, lighting.min_y + 8, 9]);
            assert!(
                starts[&ResourceId::parse("minecraft:swamp_hut")?]
                    .reference_bounds(&treasure)?
                    .is_none()
            );
            assert_eq!(
                references.references.membership()[&treasure],
                BTreeSet::from([0, 1])
            );
            assert_eq!(references.discarded.len(), 2);
            assert!(
                actual
                    .restore_saved_beardifier(position, std::slice::from_ref(&treasure))
                    .is_err()
            );
            let clean_target = ChunkPos { x: 3, z: 0 };
            let beardifier =
                actual.restore_saved_beardifier(clean_target, std::slice::from_ref(&treasure))?;
            assert!(beardifier.affected_bounds().is_none());
            assert!(actual.restore_saved_beardifier(clean_target, &[]).is_err());
            let previous = Arc::new(native_core::beardifier::Beardifier::new(
                vec![],
                vec![],
                None,
            ));
            let mut evaluator = native_resources::density::DensityEvaluator::default();
            evaluator.replace_structure_terrain_context(Some(Arc::clone(&previous)));
            assert!(
                actual
                    .generate_saved_structure_noise(
                        position,
                        std::slice::from_ref(&treasure),
                        &mut evaluator
                    )
                    .is_err()
            );
            assert!(
                actual
                    .generate_saved_structure_noise(clean_target, &[], &mut evaluator)
                    .is_err()
            );
            let result = actual.generate_saved_structure_noise(
                clean_target,
                std::slice::from_ref(&treasure),
                &mut evaluator,
            )?;
            assert!(Arc::ptr_eq(
                &previous,
                evaluator.structure_terrain_context().unwrap()
            ));
            let mut expected_evaluator = native_resources::density::DensityEvaluator::default();
            let expected = actual.generator.generate_noise_with_structure_context(
                &mut expected_evaluator,
                [3, 0],
                Arc::new(beardifier),
            )?;
            assert_eq!(result.chunk_position(), [3, 0]);
            assert_eq!(result.materials(), expected.materials());
            assert_eq!(result.fluid_updates(), expected.fluid_updates());
            let target = ChunkPos { x: 3, z: 3 };
            let mut restored_evaluator = native_resources::density::DensityEvaluator::default();
            restored_evaluator.replace_structure_terrain_context(Some(Arc::clone(&previous)));
            let resumed = actual.generate_saved_terrain(target, &[], &mut restored_evaluator)?;
            assert_eq!(resumed.chunk_position, [3, 3]);
            assert_eq!(resumed.states.len(), lighting.height as usize * 256);
            assert!(resumed.carver_postprocessing.is_some());
            let repeated = actual.generate_saved_terrain(target, &[], &mut restored_evaluator)?;
            assert_eq!(repeated.states, resumed.states, "{name} cached terrain");
            assert_eq!(
                repeated.fluid_updates, resumed.fluid_updates,
                "{name} cached fluid marks"
            );
            assert!(Arc::ptr_eq(
                &previous,
                restored_evaluator.structure_terrain_context().unwrap()
            ));
            assert!(
                actual
                    .generate_saved_terrain(position, &[], &mut restored_evaluator)
                    .is_err()
            );
            assert!(
                actual
                    .generate_saved_terrain(clean_target, &[], &mut restored_evaluator)
                    .is_err()
            );
            let restored = actual.restore_saved_biomes([position])?;
            let chunk = &restored[&native_world::loaded::ChunkPos { x: 0, z: 0 }];
            let mut neighborhood = actual.generator.biome_neighborhood()?;
            neighborhood.insert(Arc::clone(chunk))?;
            assert_eq!(
                neighborhood
                    .noise_biome_at_block(0, lighting.min_y, 0)?
                    .path,
                "plains"
            );
            let quart_height = (lighting.height / 4) as usize;
            for xz in 0..16 {
                for y in 0..quart_height {
                    let expected = if (y / 4) % 2 == 0 { "plains" } else { "forest" };
                    assert_eq!(
                        chunk.values()[xz * quart_height + y].path,
                        expected,
                        "{name}"
                    );
                }
            }
            assert!(actual.restore_saved_biomes([position, position]).is_err());
            assert!(
                actual
                    .restore_saved_biomes([ChunkPos { x: 2, z: 0 }])
                    .is_err()
            );
            assert!(
                actual
                    .restore_saved_biomes([ChunkPos { x: 100, z: 100 }])
                    .is_err()
            );
            let terrain = actual.restore_saved_terrain([position])?;
            let key = native_world::loaded::ChunkPos { x: 0, z: 0 };
            assert_eq!(
                terrain.world.block(0, lighting.min_y, 0).unwrap().name,
                "minecraft:stone"
            );
            assert_eq!(terrain.biomes[&key].values(), chunk.values());
            assert_eq!(terrain.heightmaps[&key].first_available(native_persistence::heightmap::SavedGenerationHeightmapKind::WorldSurfaceWorldgen, 0, 0), Some(lighting.min_y + 7));
            assert_eq!(
                terrain.heightmaps[&key].first_available(
                    native_persistence::heightmap::SavedGenerationHeightmapKind::WorldSurface,
                    0,
                    0
                ),
                None
            );
            assert_eq!(terrain.postprocessing[&key].as_ref().unwrap(), &marks);
            assert!(
                actual
                    .restore_saved_terrain([position, ChunkPos { x: 1, z: 0 }])
                    .is_err()
            );
            let player = native_session::PlayerState {
                position: glam::Vec3::new(8.5, lighting.min_y as f32 + 2.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut live = native_session::loaded::LoadedSession::new(
                terrain.world().clone(),
                glam::IVec3::ZERO,
                native_session::loaded::BuildBounds {
                    min_y: lighting.min_y,
                    max_y: lighting.min_y + lighting.height,
                },
                player,
            )
            .map_err(anyhow::Error::msg)?;
            assert!(actual.capture_live_terrain(&live, &terrain).is_err());
            let context = reader.capture_generation_context([position])?;
            let mut narrow = prepare_existing_generator(context, &game)?;
            let narrow_terrain = narrow.restore_saved_terrain([position])?;
            assert!(narrow.capture_live_terrain(&live, &terrain).is_err());
            let guard = narrow.capture_live_terrain(&live, &narrow_terrain)?;
            live.verify_generation_terrain(&guard)?;
            live.queue_streamed_chunk(key, None, None, None, None, None);
            assert!(live.verify_generation_terrain(&guard).is_err());
            let mut plan = WorldSavePlan::new();
            actual.add_save_dependencies(&mut plan)?;
            assert!(actual.add_save_dependencies(&mut plan).is_err());
            let mut reference_resources = ResourceStack::open(&game)?;
            let (lighting, definition, dimension_definition) =
                GeneratedDimensionLighting::load_preset_with_definition(
                    &mut reference_resources,
                    "minecraft:normal",
                    &format!("minecraft:{name}"),
                )?;
            assert_eq!(actual.lighting, lighting);
            assert_eq!(actual.dimension_definition, dimension_definition);
            let mut reference = TerrainGenerator::with_structure_context(
                &mut reference_resources,
                &definition,
                -42,
                lighting.min_y,
                lighting.height,
            )?;
            let positions = (2..=4).flat_map(|z| (2..=4).map(move |x| ChunkPos { x, z }));
            let saved_neighbors = actual.restore_saved_biomes(positions)?;
            let mut expected_biomes = reference.biome_neighborhood()?;
            for biome in saved_neighbors.into_values() {
                expected_biomes.insert(biome)?;
            }
            let blocks = reference_resources.vanilla_block_ids()?;
            let tags = CarverBlockTags::load(&mut reference_resources, &blocks)?;
            let registry = reference_resources.vanilla_generated_block_registry()?;
            let classify = |value: &Value| -> Result<CarverBlockState> {
                let state = resolve_generated_terrain_state(value, &registry)?;
                Ok(CarverBlockState {
                    is_air: matches!(
                        state.name.as_str(),
                        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                    ),
                    has_fluid: FluidState::from_block(&state)
                        .map_err(anyhow::Error::msg)?
                        .is_some(),
                })
            };
            let structure = Arc::new(actual.restore_saved_beardifier(target, &[])?);
            let expected_terrain = reference.generate_carved_surface_with_structure_context(
                &mut reference_resources,
                &mut native_resources::density::DensityEvaluator::default(),
                &mut native_resources::carver::schedule::CarverCache::default(),
                [target.x, target.z],
                &expected_biomes,
                &tags,
                structure,
                classify,
            )?;
            assert_eq!(
                resumed.states, expected_terrain.states,
                "{name} saved terrain"
            );
            assert_eq!(
                resumed.fluid_updates, expected_terrain.fluid_updates,
                "{name} saved fluid marks"
            );
            assert_eq!(
                resumed.carver_postprocessing, expected_terrain.carver_postprocessing,
                "{name} saved postprocessing"
            );
            assert_eq!(
                actual.generator.sea_level(),
                reference.sea_level(),
                "{name}"
            );
            assert_eq!(
                actual.generator.possible_biomes(),
                reference.possible_biomes(),
                "{name}"
            );
            let mut a = native_resources::density::DensityEvaluator::default();
            let mut b = native_resources::density::DensityEvaluator::default();
            assert_eq!(
                actual.generator.generate_biomes([0, 0], &mut a)?.values(),
                reference.generate_biomes([0, 0], &mut b)?.values(),
                "{name}"
            );
            assert!(actual.resources.data_pack_snapshot().is_some());
        }
        let no_structures = draft_baked_worldgen_settings(
            &mut source,
            "minecraft:normal",
            NewWorldOptions {
                seed: -42,
                generate_structures: false,
                bonus_chest: false,
            },
        )?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&no_structures)?,
        )?;
        let pack = root.join("datapacks/custom");
        let dimensions = pack.join("data/minecraft/dimension");
        std::fs::create_dir_all(&dimensions)?;
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        let dimension_file = dimensions.join("overworld.json");
        let custom_stem = |biome: &str| {
            json!({
                "type":"minecraft:overworld",
                "generator":{"type":"minecraft:noise","settings":"minecraft:overworld",
                    "biome_source":{"type":"minecraft:fixed","biome":biome}}
            })
        };
        std::fs::write(
            &dimension_file,
            serde_json::to_vec(&custom_stem("minecraft:desert"))?,
        )?;
        let mut reader = WorldReader::open_for_session(root, DimensionId::parse("overworld")?)?;
        let mut updated_level = level.clone();
        let Tag::Compound(root_fields) = &mut updated_level.value else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut root_fields
            .iter_mut()
            .find(|(name, _)| name.matches("Data"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        data.push((
            "DataPacks".into(),
            Tag::Compound(vec![
                (
                    "Enabled".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![
                            Tag::String("vanilla".into()),
                            Tag::String("file/custom".into()),
                        ],
                    },
                ),
                (
                    "Disabled".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
            ]),
        ));
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&updated_level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        let positions = (2..=4).flat_map(|z| (2..=4).map(move |x| ChunkPos { x, z }));
        let context = reader.capture_generation_context(positions)?;
        assert_eq!(context.selection().enabled, ["vanilla", "file/custom"]);
        std::fs::write(
            &dimension_file,
            serde_json::to_vec(&custom_stem("minecraft:plains"))?,
        )?;
        let mut actual = prepare_existing_generator(context, &game)?;
        assert!(!actual.generate_structures);
        assert_eq!(
            actual.generator.possible_biomes(),
            &BTreeSet::from([ResourceId::parse("minecraft:desert")?])
        );
        let previous = Arc::new(native_core::beardifier::Beardifier::new(
            vec![],
            vec![],
            None,
        ));
        let mut evaluator = native_resources::density::DensityEvaluator::default();
        evaluator.replace_structure_terrain_context(Some(Arc::clone(&previous)));
        let resumed =
            actual.generate_saved_terrain(ChunkPos { x: 3, z: 3 }, &[], &mut evaluator)?;
        assert_eq!(resumed.chunk_position, [3, 3]);
        assert!(resumed.carver_postprocessing.is_some());
        assert!(Arc::ptr_eq(
            &previous,
            evaluator.structure_terrain_context().unwrap()
        ));
        assert!(
            actual
                .generate_saved_terrain(
                    ChunkPos { x: 3, z: 3 },
                    &[ResourceId::parse("minecraft:buried_treasure")?],
                    &mut evaluator
                )
                .is_err()
        );
        Ok(())
    }
}
