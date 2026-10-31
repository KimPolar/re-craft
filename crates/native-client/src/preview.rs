//! Read-only terrain preparation for loaded 26.3 worlds.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use glam::{IVec3, Vec3};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use native_resources::{
    biomes::{BiomeColors, ColorMaps, GrassModifier},
    blockstates::{self, BlockstateDefinition, ModelChoice},
    geometry::{self, BakedBlockstate, CuboidModel, Direction, Quad, WeightedModel},
    models::ResolvedModel,
    packs::{ResourceAsset, ResourceId, ResourceProvider},
    sprites::{SpriteAtlas, Transparency},
};
use native_world::{
    fluid::{Cell, FluidKind, FluidState},
    loaded::{BiomeId, BlockState, BlockStateId, Chunk, ChunkPos, World},
};

use crate::{
    fluid_mesh::{self, Texture as FluidTexture},
    mesh::Vertex,
    preview_lighting::PreviewLighting,
};

const MISSING_TEXTURE: &str = "assets/native/textures/block/missing.png";
const MAX_VERTICES_PER_CHUNK: usize = 6_000_000;

#[derive(Clone)]
pub struct TransparentQuad {
    pub center: Vec3,
    pub vertices: [Vertex; 6],
}

pub struct PreviewScene {
    pub atlas: Arc<SpriteAtlas>,
    pub chunks: BTreeMap<ChunkPos, Arc<ChunkMesh>>,
    pub origin: IVec3,
    pub warnings: Vec<String>,
    /// Identifies atlas and model resources within the lifetime of a mesh worker.
    pub material_generation: Option<u64>,
    pub lightmap: Arc<[[[f32; 3]; 16]; 16]>,
    pub base_lightmap: Arc<native_world::lighting::LightmapSettings>,
    pub biome_colors: Arc<BTreeMap<String, BiomeColors>>,
}

struct BlockModel {
    baked: BakedBlockstate,
    opaque_cube: bool,
    solid_faces: [bool; 6],
}

struct FluidTextures {
    still: ResourceId,
    flow: ResourceId,
    overlay: Option<ResourceId>,
    translucent: bool,
}

impl FluidTextures {
    fn new(kind: FluidKind, atlas: &SpriteAtlas) -> Result<Self> {
        let (still, flow, overlay) = match kind {
            FluidKind::Water => (
                "block/water_still",
                "block/water_flow",
                Some("block/water_overlay"),
            ),
            FluidKind::Lava => ("block/lava_still", "block/lava_flow", None),
        };
        let still = ResourceId::parse(still)?;
        let flow = ResourceId::parse(flow)?;
        let overlay = overlay.map(ResourceId::parse).transpose()?;
        let translucent = [Some(&still), Some(&flow), overlay.as_ref()]
            .into_iter()
            .flatten()
            .any(|id| atlas.transparency(id) == Some(Transparency::Translucent));
        Ok(Self {
            still,
            flow,
            overlay,
            translucent,
        })
    }

    fn sprite(&self, texture: FluidTexture) -> &ResourceId {
        match texture {
            FluidTexture::Still => &self.still,
            FluidTexture::Flow => &self.flow,
            FluidTexture::Overlay => self.overlay.as_ref().unwrap_or(&self.flow),
        }
    }
}

struct PreviewSource<'a, T> {
    inner: &'a mut T,
    missing_png: Vec<u8>,
    missing: BTreeSet<String>,
}

impl<T: ResourceProvider> ResourceProvider for PreviewSource<'_, T> {
    fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path == MISSING_TEXTURE {
            return Ok(Some(self.missing_png.clone()));
        }
        let value = self.inner.read_asset(path)?;
        if value.is_none()
            && path.starts_with("assets/")
            && path.contains("/textures/")
            && path.ends_with(".png")
        {
            self.missing.insert(path.to_owned());
            Ok(Some(self.missing_png.clone()))
        } else {
            Ok(value)
        }
    }

    fn read_asset_with_metadata(&mut self, path: &str) -> Result<Option<ResourceAsset>> {
        if path == MISSING_TEXTURE {
            return Ok(Some(ResourceAsset {
                bytes: self.missing_png.clone(),
                metadata: None,
            }));
        }
        let result = self.inner.read_asset_with_metadata(path)?;
        if result.is_none()
            && path.starts_with("assets/")
            && path.contains("/textures/")
            && path.ends_with(".png")
        {
            self.missing.insert(path.to_owned());
            Ok(Some(ResourceAsset {
                bytes: self.missing_png.clone(),
                metadata: None,
            }))
        } else {
            Ok(result)
        }
    }
}

struct PreviewMaterials {
    atlas: Arc<SpriteAtlas>,
    models: BTreeMap<BlockStateId, BlockModel>,
    fluids: BTreeMap<BlockStateId, FluidState>,
    water_textures: Option<FluidTextures>,
    lava_textures: Option<FluidTextures>,
    biome_colors: Arc<BTreeMap<String, BiomeColors>>,
    lighting: PreviewLighting,
    lightmap: Arc<[[[f32; 3]; 16]; 16]>,
    base_lightmap: Arc<native_world::lighting::LightmapSettings>,
    warnings: Vec<String>,
}

#[derive(Default)]
pub struct ChunkMesh {
    pub opaque: Arc<[Vertex]>,
    pub translucent: Arc<[TransparentQuad]>,
    pub chest_lids: Arc<[ChestLidMesh]>,
}

#[derive(Clone)]
pub struct ChestLidMesh {
    pub position: IVec3,
    pub local_origin: Vec3,
    pub facing: String,
    pub start: usize,
    pub end: usize,
}

impl ChestLidMesh {
    pub fn animated_vertices(&self, base: &[Vertex], angle: f32) -> Vec<Vertex> {
        let mut vertices = base[self.start..self.end].to_vec();
        if angle == 0.0 {
            return vertices;
        }
        let (sin, cos) = angle.sin_cos();
        for vertex in &mut vertices {
            let point = Vec3::from_array(vertex.position) - self.local_origin;
            let (x, z) = match self.facing.as_str() {
                "north" => (1.0 - point.x, 1.0 - point.z),
                "west" => (point.z, 1.0 - point.x),
                "east" => (1.0 - point.z, point.x),
                _ => (point.x, point.z),
            };
            let y = point.y - 9.0 / 16.0;
            let z = z - 1.0 / 16.0;
            let next_y = y * cos - z * sin + 9.0 / 16.0;
            let next_z = y * sin + z * cos + 1.0 / 16.0;
            let (x, z) = match self.facing.as_str() {
                "north" => (1.0 - x, 1.0 - next_z),
                "west" => (1.0 - next_z, x),
                "east" => (next_z, 1.0 - x),
                _ => (x, next_z),
            };
            vertex.position = (self.local_origin + Vec3::new(x, next_y, z)).to_array();
        }
        vertices
    }
}

impl ChunkMesh {
    fn reuse_unchanged_layers(&mut self, previous: &Self) {
        if bytemuck::cast_slice::<Vertex, u8>(&self.opaque)
            == bytemuck::cast_slice::<Vertex, u8>(&previous.opaque)
        {
            self.opaque = Arc::clone(&previous.opaque);
        }
        if self.translucent.len() == previous.translucent.len()
            && self
                .translucent
                .iter()
                .zip(previous.translucent.iter())
                .all(|(a, b)| {
                    a.center.to_array().map(f32::to_bits) == b.center.to_array().map(f32::to_bits)
                        && bytemuck::cast_slice::<Vertex, u8>(&a.vertices)
                            == bytemuck::cast_slice::<Vertex, u8>(&b.vertices)
                })
        {
            self.translucent = Arc::clone(&previous.translucent);
        }
    }
}

fn chunk_usage(chunk: &Chunk) -> (BTreeSet<BlockStateId>, BTreeSet<BiomeId>) {
    let mut states = BTreeSet::new();
    let mut biomes = BTreeSet::new();
    for section in chunk.sections.values() {
        if let Some(blocks) = &section.blocks {
            states.extend(blocks.iter().copied());
        }
        if let Some(values) = &section.biomes {
            biomes.extend(values.iter().copied());
        }
    }
    (states, biomes)
}

impl PreviewMaterials {
    fn prepare(
        world: &World,
        source: &mut impl ResourceProvider,
        max_atlas_side: u32,
        state_ids: BTreeSet<BlockStateId>,
        biome_ids: BTreeSet<BiomeId>,
    ) -> Result<Self> {
        let mut provider = PreviewSource {
            inner: source,
            missing_png: missing_png()?,
            missing: BTreeSet::new(),
        };
        let fallback = missing_model()?;
        let mut models: BTreeMap<BlockStateId, BlockModel> = BTreeMap::new();
        let mut fluids: BTreeMap<BlockStateId, FluidState> = BTreeMap::new();
        let mut warnings = Vec::new();
        let lighting_result = match &world.dimension_type {
            Some(id) => PreviewLighting::load_type(provider.inner, id),
            None => PreviewLighting::load(provider.inner, &world.dimension),
        };
        let lighting = match lighting_result {
            Ok(lighting) => lighting,
            Err(error) => {
                warnings.push(format!("dimension lighting uses a fallback: {error:#}"));
                PreviewLighting::fallback(&world.dimension)
            }
        };
        let lightmap = Arc::new(lighting.lightmap.table());
        let base_lightmap = Arc::new(lighting.lightmap.clone());
        let mut biome_colors = BTreeMap::new();
        if !biome_ids.is_empty() {
            let maps = ColorMaps::load(provider.inner, &mut warnings);
            let mut swamp_warned = false;
            for id in biome_ids {
                let name = world.biome_by_id(id).context("invalid biome ID")?;
                match BiomeColors::load_world(provider.inner, name, &maps) {
                    Ok(Some(biome)) => {
                        if biome.grass_modifier == GrassModifier::Swamp && !swamp_warned {
                            warnings.push("swamp grass color uses an approximation; spatial noise is not implemented".into());
                            swamp_warned = true;
                        }
                        biome_colors.insert(name.to_owned(), biome);
                    }
                    Ok(None) => warnings.push(format!(
                        "biome {name} has no resolved definition; using fallback colors"
                    )),
                    Err(error) => warnings.push(format!(
                        "cannot read biome {name}: {error:#}; using fallback colors"
                    )),
                }
            }
        }
        for id in state_ids {
            let state = world.state(id).context("invalid block state ID")?;
            match FluidState::from_block(state) {
                Ok(Some(fluid)) => {
                    fluids.insert(id, fluid);
                }
                Ok(None) => {}
                Err(error) => warnings.push(error),
            }
            if invisible(&state.name) {
                continue;
            }
            if fluids.contains_key(&id)
                && matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
            {
                continue;
            }
            let baked = match crate::chest_model::bake(state) {
                Ok(Some(mut entity)) => {
                    // Chest blockstates may also supply ordinary geometry in a resource pack.
                    match geometry::bake_blockstate(&mut provider, &state.name, &state.properties) {
                        Ok(mut block) => {
                            block
                                .definition
                                .groups
                                .append(&mut entity.definition.groups);
                            block.groups.append(&mut entity.groups);
                            block
                        }
                        Err(error) => {
                            warnings
                                .push(format!("{} {:?}: {error:#}", state.name, state.properties));
                            entity
                        }
                    }
                }
                Ok(None) => {
                    match geometry::bake_blockstate(&mut provider, &state.name, &state.properties) {
                        Ok(model) => model,
                        Err(error) => {
                            warnings
                                .push(format!("{} {:?}: {error:#}", state.name, state.properties));
                            fallback.clone()
                        }
                    }
                }
                Err(error) => {
                    warnings.push(format!("{} {:?}: {error:#}", state.name, state.properties));
                    fallback.clone()
                }
            };
            models.insert(
                id,
                BlockModel {
                    baked,
                    opaque_cube: false,
                    solid_faces: [false; 6],
                },
            );
        }
        let mut textures: Vec<_> = models
            .values()
            .flat_map(|block| block.baked.groups.iter().flatten())
            .flat_map(|model| &model.model.quads)
            .map(|quad| quad.texture.sprite.clone())
            .chain(std::iter::once(ResourceId::parse("native:block/missing")?))
            .collect();
        let has_water = fluids.values().any(|fluid| fluid.kind == FluidKind::Water);
        let has_lava = fluids.values().any(|fluid| fluid.kind == FluidKind::Lava);
        if has_water {
            for name in [
                "block/water_still",
                "block/water_flow",
                "block/water_overlay",
            ] {
                textures.push(ResourceId::parse(name)?);
            }
        }
        if has_lava {
            for name in ["block/lava_still", "block/lava_flow"] {
                textures.push(ResourceId::parse(name)?);
            }
        }
        let atlas = SpriteAtlas::build(&mut provider, textures, max_atlas_side)?;
        let water_textures = has_water
            .then(|| FluidTextures::new(FluidKind::Water, &atlas))
            .transpose()?;
        let lava_textures = has_lava
            .then(|| FluidTextures::new(FluidKind::Lava, &atlas))
            .transpose()?;
        warnings.extend(
            provider
                .missing
                .iter()
                .map(|path| format!("missing texture: {path}")),
        );
        for model in models.values_mut() {
            for (index, face) in Direction::ALL.into_iter().enumerate() {
                model.solid_faces[index] = model.baked.groups.iter().any(|group| {
                    !group.is_empty()
                        && group
                            .iter()
                            .all(|choice| full_face(&choice.model, face, &atlas))
                });
            }
            model.opaque_cube = model.solid_faces.iter().all(|solid| *solid);
        }

        Ok(Self {
            atlas: Arc::new(atlas),
            models,
            fluids,
            water_textures,
            lava_textures,
            biome_colors: Arc::new(biome_colors),
            lighting,
            lightmap,
            base_lightmap,
            warnings,
        })
    }

    fn mesh_chunk(&self, world: &World, chunk: &Chunk, origin: IVec3) -> Result<ChunkMesh> {
        let Self {
            atlas,
            models,
            fluids,
            water_textures,
            lava_textures,
            biome_colors,
            lighting,
            ..
        } = self;
        let mut opaque = Vec::new();
        let mut translucent = Vec::new();
        let mut chest_lids = Vec::new();
        let base_x = chunk
            .position
            .x
            .checked_mul(16)
            .context("chunk X is outside the supported block range")?;
        let base_z = chunk
            .position
            .z
            .checked_mul(16)
            .context("chunk Z is outside the supported block range")?;
        for (&section_y, section) in &chunk.sections {
            let Some(blocks) = section.blocks.as_ref() else {
                continue;
            };
            let base_y = i32::from(section_y) * 16;
            for (index, id) in blocks.iter().enumerate() {
                let block = models.get(id);
                let liquid = fluids.get(id).copied();
                if block.is_none() && liquid.is_none() {
                    continue;
                }
                let local_x = (index % 16) as i32;
                let local_z = ((index / 16) % 16) as i32;
                let local_y = (index / 256) as i32;
                let position = IVec3::new(
                    base_x.checked_add(local_x).context("block X overflow")?,
                    base_y + local_y,
                    base_z.checked_add(local_z).context("block Z overflow")?,
                );
                let state = world.state(*id).context("invalid block state ID")?;
                if let Some(liquid) = liquid {
                    let cell = |pos: IVec3| {
                        let Some(id) = world.block_id(pos.x, pos.y, pos.z) else {
                            return Cell::Missing;
                        };
                        if let Some(fluid) = fluids.get(&id) {
                            Cell::Fluid(*fluid)
                        } else if models.get(&id).is_some_and(|model| model.opaque_cube) {
                            Cell::Solid
                        } else {
                            Cell::Empty
                        }
                    };
                    let overlay = |pos: IVec3| {
                        world
                            .block(pos.x, pos.y, pos.z)
                            .is_some_and(|state| fluid_overlay_neighbor(&state.name))
                    };
                    let faces = block.map(|model| model.solid_faces).unwrap_or([false; 6]);
                    let materials = match liquid.kind {
                        FluidKind::Water => water_textures.as_ref(),
                        FluidKind::Lava => lava_textures.as_ref(),
                    }
                    .context("missing fluid materials")?;
                    let tint = if liquid.kind == FluidKind::Water {
                        biome_tint(world, biome_colors, position, BiomeTint::Water)
                    } else {
                        [1.0; 3]
                    };
                    let block_position = relative_position(position, origin);
                    for quad in fluid_mesh::build(&cell, &overlay, position, liquid, faces) {
                        let sprite = materials.sprite(quad.texture);
                        let rect = atlas
                            .rectangle(sprite)
                            .context("fluid sprite was not put in atlas")?;
                        let shade = directional_shade(quad.direction, lighting.nether_cardinal);
                        let levels = fluid_light(
                            world,
                            position,
                            quad.direction,
                            liquid.kind,
                            lighting.has_skylight,
                        );
                        let vertices = std::array::from_fn(|i| {
                            let corner = [0, 1, 2, 0, 2, 3][i];
                            Vertex {
                                position: (block_position
                                    + Vec3::from_array(quad.positions[corner]))
                                .to_array(),
                                uv: [
                                    (rect.x as f32 + quad.uv[corner][0] * rect.width as f32)
                                        / atlas.texture.width as f32,
                                    (rect.y as f32 + quad.uv[corner][1] * rect.height as f32)
                                        / atlas.texture.height as f32,
                                ],
                                shade,
                                tint,
                                light: u32::from(levels.0) + 16 * u32::from(levels.1),
                            }
                        });
                        if materials.translucent {
                            let center = vertices.iter().fold(Vec3::ZERO, |sum, vertex| {
                                sum + Vec3::from_array(vertex.position)
                            }) / 6.0;
                            translucent.push(TransparentQuad { center, vertices });
                        } else {
                            opaque.extend_from_slice(&vertices);
                        }
                        ensure!(
                            opaque.len() + translucent.len() * 6 <= MAX_VERTICES_PER_CHUNK,
                            "chunk exceeds the {MAX_VERTICES_PER_CHUNK} vertex rendering limit"
                        );
                    }
                }
                let Some(block) = block else {
                    continue;
                };
                let selections =
                    block
                        .baked
                        .definition
                        .select_indices(blockstates::positional_seed(
                            position.x, position.y, position.z,
                        ))?;
                let mut cached_tints: [Option<(i32, [f32; 3])>; 4] = [None; 4];
                for (group, selected) in block.baked.groups.iter().zip(selections) {
                    let weighted = group.get(selected).context("invalid selected model")?;
                    let model = &weighted.model;
                    let chest_model = weighted.choice.model.namespace == "native"
                        && weighted.choice.model.path == "entity/chest";
                    let body_faces = if state.properties.get("type").is_some_and(|v| v == "single")
                    {
                        6
                    } else {
                        5
                    };
                    let mut lid_start = None;
                    let mut lid_translucent = false;
                    for (quad_index, quad) in model.quads.iter().enumerate() {
                        if chest_model && quad_index == body_faces {
                            lid_start = Some(opaque.len());
                        }
                        if let Some(face) = quad.cullface {
                            let offset = direction_offset(face);
                            if let Some(neighbor) = checked_add(position, offset)
                                && world
                                    .block_id(neighbor.x, neighbor.y, neighbor.z)
                                    .is_some_and(|neighbor| {
                                        models
                                            .get(&neighbor)
                                            .is_some_and(|neighbor| neighbor.opaque_cube)
                                            || (self_culls(&state.name)
                                                && world
                                                    .state(neighbor)
                                                    .is_some_and(|other| other.name == state.name))
                                    })
                            {
                                continue;
                            }
                        }
                        let rect = atlas
                            .rectangle(&quad.texture.sprite)
                            .context("model sprite was not put in atlas")?;
                        let shade = directional_shade(
                            quad.shade_direction_override.unwrap_or(quad.direction),
                            lighting.nether_cardinal,
                        );
                        let levels = model_light(world, position, quad, lighting.has_skylight);
                        let tint = if quad.tint_index < 0 {
                            [1.0; 3]
                        } else if let Some((_, value)) = cached_tints
                            .iter()
                            .flatten()
                            .find(|(index, _)| *index == quad.tint_index)
                        {
                            *value
                        } else {
                            let value = tint(state, quad.tint_index, world, biome_colors, position);
                            if let Some(slot) = cached_tints.iter_mut().find(|slot| slot.is_none())
                            {
                                *slot = Some((quad.tint_index, value));
                            }
                            value
                        };
                        let block_position = relative_position(position, origin);
                        let vertices = std::array::from_fn(|i| {
                            let vertex = quad.vertices[[0, 1, 2, 0, 2, 3][i]];
                            Vertex {
                                position: (block_position + Vec3::from_array(vertex.position))
                                    .to_array(),
                                uv: [
                                    (rect.x as f32 + vertex.uv[0] * rect.width as f32)
                                        / atlas.texture.width as f32,
                                    (rect.y as f32 + vertex.uv[1] * rect.height as f32)
                                        / atlas.texture.height as f32,
                                ],
                                shade,
                                tint,
                                light: u32::from(levels.0) + 16 * u32::from(levels.1),
                            }
                        });
                        let transparent = quad.texture.force_translucent
                            || atlas.transparency(&quad.texture.sprite)
                                == Some(Transparency::Translucent);
                        if chest_model && quad_index >= body_faces && transparent {
                            lid_translucent = true;
                        }
                        if transparent {
                            let center = vertices.iter().fold(Vec3::ZERO, |sum, vertex| {
                                sum + Vec3::from_array(vertex.position)
                            }) / 6.0;
                            translucent.push(TransparentQuad { center, vertices });
                        } else {
                            opaque.extend_from_slice(&vertices);
                        }
                        ensure!(
                            opaque.len() + translucent.len() * 6 <= MAX_VERTICES_PER_CHUNK,
                            "chunk exceeds the {MAX_VERTICES_PER_CHUNK} vertex rendering limit"
                        );
                    }
                    if chest_model
                        && !lid_translucent
                        && let (Some(start), Some(facing)) =
                            (lid_start, state.properties.get("facing"))
                    {
                        chest_lids.push(ChestLidMesh {
                            position,
                            local_origin: relative_position(position, origin),
                            facing: facing.clone(),
                            start,
                            end: opaque.len(),
                        });
                    }
                }
            }
        }
        Ok(ChunkMesh {
            opaque: opaque.into(),
            translucent: translucent.into(),
            chest_lids: chest_lids.into(),
        })
    }
}

impl PreviewScene {
    pub fn prepare(
        world: &World,
        source: &mut impl ResourceProvider,
        origin: IVec3,
        max_atlas_side: u32,
    ) -> Result<Self> {
        let mut states = BTreeSet::new();
        let mut biomes = BTreeSet::new();
        for chunk in world.chunks.values() {
            let (chunk_states, chunk_biomes) = chunk_usage(chunk);
            states.extend(chunk_states);
            biomes.extend(chunk_biomes);
        }
        let materials = PreviewMaterials::prepare(world, source, max_atlas_side, states, biomes)?;
        let mut chunks = BTreeMap::new();
        for (&position, chunk) in &world.chunks {
            let mesh = materials.mesh_chunk(world, chunk, origin)?;
            chunks.insert(position, Arc::new(mesh));
        }
        Ok(Self {
            atlas: materials.atlas,
            chunks,
            origin,
            warnings: materials.warnings,
            material_generation: None,
            lightmap: Arc::clone(&materials.lightmap),
            base_lightmap: Arc::clone(&materials.base_lightmap),
            biome_colors: Arc::clone(&materials.biome_colors),
        })
    }

    #[cfg(any(test, debug_assertions))]
    pub fn opaque_len(&self) -> usize {
        self.chunks.values().map(|chunk| chunk.opaque.len()).sum()
    }

    #[cfg(any(test, debug_assertions))]
    pub fn translucent_len(&self) -> usize {
        self.chunks
            .values()
            .map(|chunk| chunk.translucent.len())
            .sum()
    }

    #[cfg(test)]
    fn opaque_vertices(&self) -> impl Iterator<Item = &Vertex> {
        self.chunks.values().flat_map(|chunk| chunk.opaque.iter())
    }

    pub fn translucent_quads(&self) -> impl Iterator<Item = &TransparentQuad> {
        self.chunks
            .values()
            .flat_map(|chunk| chunk.translucent.iter())
    }

    pub fn sorted_translucent_quads(&self, eye: Vec3) -> Vec<&TransparentQuad> {
        let mut quads: Vec<_> = self.translucent_quads().collect();
        quads.sort_unstable_by(|a, b| {
            b.center
                .distance_squared(eye)
                .total_cmp(&a.center.distance_squared(eye))
        });
        quads
    }

    #[cfg(test)]
    pub fn transparent_vertices(&self, eye: Vec3) -> Vec<Vertex> {
        self.sorted_translucent_quads(eye)
            .iter()
            .flat_map(|quad| quad.vertices)
            .collect()
    }
}

#[derive(PartialEq, Eq)]
struct MaterialsKey {
    dimension: String,
    dimension_type: Option<String>,
    max_atlas_side: u32,
    states: Vec<(BlockStateId, BlockState)>,
    biomes: Vec<(BiomeId, String)>,
}

struct CachedUsage {
    chunk: Arc<Chunk>,
    states: BTreeSet<BlockStateId>,
    biomes: BTreeSet<BiomeId>,
}

struct CachedChunkMesh {
    neighbors: [Option<Arc<Chunk>>; 9],
    mesh: Arc<ChunkMesh>,
}

/// Reuses unchanged chunk meshes within a worker. A neighboring chunk is part of
/// the key because culling, lighting and fluid corners sample across borders.
#[derive(Default)]
pub struct IncrementalPreview {
    usage: BTreeMap<ChunkPos, CachedUsage>,
    chunks: BTreeMap<ChunkPos, CachedChunkMesh>,
    materials: Option<(MaterialsKey, PreviewMaterials)>,
    origin: Option<IVec3>,
    biome_zoom_seed: Option<Option<i64>>,
    generation: u64,
    #[cfg(test)]
    last_rebuilt: usize,
}

fn neighborhood(world: &World, pos: ChunkPos) -> [Option<Arc<Chunk>>; 9] {
    std::array::from_fn(|index| {
        let dx = (index % 3) as i32 - 1;
        let dz = (index / 3) as i32 - 1;
        let neighbor = ChunkPos {
            x: pos.x.checked_add(dx)?,
            z: pos.z.checked_add(dz)?,
        };
        world.chunks.get(&neighbor).cloned()
    })
}

fn same_neighborhood(a: &[Option<Arc<Chunk>>; 9], b: &[Option<Arc<Chunk>>; 9]) -> bool {
    a.iter()
        .zip(b)
        .enumerate()
        .all(|(index, (a, b))| match (a, b) {
            (Some(a), Some(b)) if Arc::ptr_eq(a, b) => true,
            (Some(a), Some(b)) if index != 4 => {
                let dx = (index % 3) as i32 - 1;
                let dz = (index / 3) as i32 - 1;
                same_neighbor_boundary(a, b, dx, dz)
            }
            (None, None) => true,
            _ => false,
        })
}

fn same_neighbor_boundary(a: &Chunk, b: &Chunk, dx: i32, dz: i32) -> bool {
    if a.block_light_correct != b.block_light_correct || a.sections.len() != b.sections.len() {
        return false;
    }
    for (y, a) in &a.sections {
        let Some(b) = b.sections.get(y) else {
            return false;
        };
        if Arc::ptr_eq(a, b) {
            continue;
        }
        if a.biomes != b.biomes
            || a.blocks.is_some() != b.blocks.is_some()
            || a.block_light.is_some() != b.block_light.is_some()
            || a.sky_light.is_some() != b.sky_light.is_some()
        {
            return false;
        }
        // A neighbor at +X contributes its X=0 edge, and one at -X its X=15 edge.
        let xs = match dx {
            -1 => 15..16,
            1 => 0..1,
            _ => 0..16,
        };
        for x in xs {
            let zs = match dz {
                -1 => 15..16,
                1 => 0..1,
                _ => 0..16,
            };
            for z in zs {
                for local_y in 0..16 {
                    let index = x + z * 16 + local_y * 256;
                    if a.blocks.as_ref().map(|v| v[index]) != b.blocks.as_ref().map(|v| v[index]) {
                        return false;
                    }
                    let light = |data: &Option<Box<[u8; native_world::loaded::LIGHT_BYTES]>>| {
                        data.as_ref().map(|data| {
                            let byte = data[index / 2];
                            if index % 2 == 0 { byte & 15 } else { byte >> 4 }
                        })
                    };
                    if light(&a.block_light) != light(&b.block_light)
                        || light(&a.sky_light) != light(&b.sky_light)
                    {
                        return false;
                    }
                }
            }
        }
    }
    true
}

impl IncrementalPreview {
    pub fn prepare(
        &mut self,
        world: &World,
        source: &mut impl ResourceProvider,
        requested_origin: IVec3,
        max_atlas_side: u32,
    ) -> Result<PreviewScene> {
        self.usage.retain(|pos, _| world.chunks.contains_key(pos));
        self.chunks.retain(|pos, _| world.chunks.contains_key(pos));
        for (&pos, chunk) in &world.chunks {
            ensure!(
                chunk.position == pos,
                "chunk position does not match its key"
            );
            if self
                .usage
                .get(&pos)
                .is_none_or(|old| !Arc::ptr_eq(&old.chunk, chunk))
            {
                let (states, biomes) = chunk_usage(chunk);
                self.usage.insert(
                    pos,
                    CachedUsage {
                        chunk: Arc::clone(chunk),
                        states,
                        biomes,
                    },
                );
            }
        }
        let current_states: BTreeSet<_> = self
            .usage
            .values()
            .flat_map(|usage| usage.states.iter().copied())
            .collect();
        let current_biomes: BTreeSet<_> = self
            .usage
            .values()
            .flat_map(|usage| usage.biomes.iter().copied())
            .collect();
        let mut states = current_states.clone();
        let mut biomes = current_biomes.clone();
        if let Some((old, _)) = &self.materials
            && old.dimension == world.dimension
            && old.dimension_type == world.dimension_type
            && old.max_atlas_side == max_atlas_side
            && old
                .states
                .iter()
                .all(|(id, state)| world.state(*id) == Some(state))
            && old
                .biomes
                .iter()
                .all(|(id, biome)| world.biome_by_id(*id) == Some(biome.as_str()))
        {
            states.extend(old.states.iter().map(|(id, _)| *id));
            biomes.extend(old.biomes.iter().map(|(id, _)| *id));
            // Bound historical registry retention; the current world can still
            // contain more states or biomes than these cache thresholds.
            if states.len() > 8192 || biomes.len() > 1024 {
                states = current_states.clone();
                biomes = current_biomes.clone();
            }
        }
        let make_key =
            |states: &BTreeSet<BlockStateId>, biomes: &BTreeSet<BiomeId>| -> Result<MaterialsKey> {
                Ok(MaterialsKey {
                    dimension: world.dimension.clone(),
                    dimension_type: world.dimension_type.clone(),
                    max_atlas_side,
                    states: states
                        .iter()
                        .map(|id| {
                            Ok((
                                *id,
                                world.state(*id).context("invalid block state ID")?.clone(),
                            ))
                        })
                        .collect::<Result<_>>()?,
                    biomes: biomes
                        .iter()
                        .map(|id| {
                            Ok((
                                *id,
                                world
                                    .biome_by_id(*id)
                                    .context("invalid biome ID")?
                                    .to_owned(),
                            ))
                        })
                        .collect::<Result<_>>()?,
                })
            };
        let mut key = make_key(&states, &biomes)?;
        if self.materials.as_ref().is_none_or(|(old, _)| *old != key) {
            let materials =
                match PreviewMaterials::prepare(world, source, max_atlas_side, states, biomes) {
                    Ok(materials) => materials,
                    Err(error)
                        if key.states.len() != current_states.len()
                            || key.biomes.len() != current_biomes.len() =>
                    {
                        // Retained textures may no longer fit. Retry with only
                        // resources used by the currently loaded chunks.
                        let materials = PreviewMaterials::prepare(
                            world,
                            source,
                            max_atlas_side,
                            current_states.clone(),
                            current_biomes.clone(),
                        )
                        .with_context(|| format!("material cache fallback after: {error:#}"))?;
                        key = make_key(&current_states, &current_biomes)?;
                        materials
                    }
                    Err(error) => return Err(error),
                };
            self.materials = Some((key, materials));
            self.chunks.clear();
            self.generation = self.generation.wrapping_add(1);
        }
        // The camera is translated into this frame by eye_for_scene. Keeping a
        // nearby anchor avoids rebuilding every chunk at each chunk boundary.
        let rebase = self.origin.is_none_or(|origin| {
            [origin.x, origin.y, origin.z]
                .into_iter()
                .zip([requested_origin.x, requested_origin.y, requested_origin.z])
                .any(|(a, b)| (i64::from(a) - i64::from(b)).abs() > 256)
        });
        if rebase {
            self.origin = Some(requested_origin);
            self.chunks.clear();
        }
        if self.biome_zoom_seed != Some(world.biome_zoom_seed()) {
            self.biome_zoom_seed = Some(world.biome_zoom_seed());
            self.chunks.clear();
        }
        let origin = self.origin.expect("mesh origin is initialized");
        let materials = &self
            .materials
            .as_ref()
            .expect("mesh materials are initialized")
            .1;
        #[cfg(test)]
        {
            self.last_rebuilt = 0;
        }
        for (&pos, chunk) in &world.chunks {
            let neighbors = neighborhood(world, pos);
            if self
                .chunks
                .get(&pos)
                .is_none_or(|old| !same_neighborhood(&old.neighbors, &neighbors))
            {
                let mut mesh = materials.mesh_chunk(world, chunk, origin)?;
                if let Some(previous) = self.chunks.get(&pos) {
                    mesh.reuse_unchanged_layers(&previous.mesh);
                }
                self.chunks.insert(
                    pos,
                    CachedChunkMesh {
                        neighbors,
                        mesh: Arc::new(mesh),
                    },
                );
                #[cfg(test)]
                {
                    self.last_rebuilt += 1;
                }
            }
        }
        Ok(PreviewScene {
            atlas: Arc::clone(&materials.atlas),
            chunks: self
                .chunks
                .iter()
                .map(|(pos, chunk)| (*pos, Arc::clone(&chunk.mesh)))
                .collect(),
            origin,
            warnings: materials.warnings.clone(),
            material_generation: Some(self.generation),
            lightmap: Arc::clone(&materials.lightmap),
            base_lightmap: Arc::clone(&materials.base_lightmap),
            biome_colors: Arc::clone(&materials.biome_colors),
        })
    }
}

fn invisible(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:barrier"
            | "minecraft:structure_void"
            | "minecraft:light"
    )
}

fn relative_position(position: IVec3, origin: IVec3) -> Vec3 {
    Vec3::new(
        (i64::from(position.x) - i64::from(origin.x)) as f32,
        (i64::from(position.y) - i64::from(origin.y)) as f32,
        (i64::from(position.z) - i64::from(origin.z)) as f32,
    )
}

fn fluid_overlay_neighbor(name: &str) -> bool {
    name.starts_with("minecraft:")
        && (name.ends_with("_leaves")
            || name.ends_with("_stained_glass")
            || matches!(
                name,
                "minecraft:glass"
                    | "minecraft:tinted_glass"
                    | "minecraft:ice"
                    | "minecraft:frosted_ice"
                    | "minecraft:slime_block"
                    | "minecraft:honey_block"
            ))
}

// Vanilla's transparent glass blocks suppress the shared face against the same block type.
fn self_culls(name: &str) -> bool {
    name == "minecraft:glass"
        || name == "minecraft:tinted_glass"
        || (name.starts_with("minecraft:") && name.ends_with("_stained_glass"))
}

fn checked_add(a: IVec3, b: IVec3) -> Option<IVec3> {
    Some(IVec3::new(
        a.x.checked_add(b.x)?,
        a.y.checked_add(b.y)?,
        a.z.checked_add(b.z)?,
    ))
}

fn direction_offset(direction: Direction) -> IVec3 {
    match direction {
        Direction::Down => IVec3::NEG_Y,
        Direction::Up => IVec3::Y,
        Direction::North => IVec3::NEG_Z,
        Direction::South => IVec3::Z,
        Direction::West => IVec3::NEG_X,
        Direction::East => IVec3::X,
    }
}

fn directional_shade(direction: Direction, nether: bool) -> f32 {
    match direction {
        Direction::Up if nether => 0.9,
        Direction::Down if nether => 0.9,
        Direction::Up => 1.0,
        Direction::Down => 0.5,
        Direction::North | Direction::South => 0.8,
        Direction::West | Direction::East => 0.6,
    }
}

fn stored_light(world: &World, position: IVec3, sky: bool) -> Option<u8> {
    world.light(position.x, position.y, position.z, sky)
}

fn model_light(world: &World, position: IVec3, quad: &Quad, has_skylight: bool) -> (u8, u8) {
    let offset = direction_offset(quad.direction);
    let axis = if offset.x != 0 {
        0
    } else if offset.y != 0 {
        1
    } else {
        2
    };
    let boundary = if offset[axis] > 0 { 1.0 } else { 0.0 };
    let touches_boundary = quad
        .vertices
        .iter()
        .all(|vertex| (vertex.position[axis] - boundary).abs() < 0.0001);
    let sample_offset = quad
        .cullface
        .map(direction_offset)
        .or_else(|| touches_boundary.then_some(offset));
    let sample = sample_offset
        .and_then(|offset| checked_add(position, offset))
        .unwrap_or(position);
    let get = |sky| {
        (if sky && !has_skylight {
            0
        } else {
            stored_light(world, sample, sky)
                .or_else(|| stored_light(world, position, sky))
                .unwrap_or(if sky { 15 } else { 0 })
        })
        .max(quad.light_emission)
    };
    (get(false), get(true))
}

fn fluid_light(
    world: &World,
    position: IVec3,
    direction: Direction,
    kind: FluidKind,
    has_skylight: bool,
) -> (u8, u8) {
    let sample = if direction == Direction::Down {
        checked_add(position, IVec3::NEG_Y).unwrap_or(position)
    } else {
        position
    };
    let above = checked_add(sample, IVec3::Y);
    let get = |sky| {
        if sky && !has_skylight {
            return 0;
        }
        let first = stored_light(world, sample, sky);
        let second = above.and_then(|pos| stored_light(world, pos, sky));
        first
            .into_iter()
            .chain(second)
            .max()
            .unwrap_or(if sky { 15 } else { 0 })
    };
    let emitted = if kind == FluidKind::Lava { 15 } else { 0 };
    (get(false).max(emitted), get(true))
}

#[derive(Clone, Copy)]
enum BiomeTint {
    Grass,
    Foliage,
    DryFoliage,
    Water,
}

fn rgb(color: u32) -> [f32; 3] {
    [
        ((color >> 16) & 255) as f32 / 255.0,
        ((color >> 8) & 255) as f32 / 255.0,
        (color & 255) as f32 / 255.0,
    ]
}

fn biome_tint(
    world: &World,
    biomes: &BTreeMap<String, BiomeColors>,
    position: IVec3,
    kind: BiomeTint,
) -> [f32; 3] {
    let biome = world
        .biome_at_block(position.x, position.y, position.z)
        .and_then(|name| biomes.get(name));
    rgb(match kind {
        BiomeTint::Grass => biome.map_or(0x91bd5a, BiomeColors::grass),
        BiomeTint::Foliage => biome.map_or(0x7ab847, |b| b.foliage),
        BiomeTint::DryFoliage => biome.map_or(0xae9d77, |b| b.dry_foliage),
        BiomeTint::Water => biome.map_or(0x3f76e4, |b| b.water),
    })
}

pub(crate) fn grass_particle_tint(
    world: &World,
    biomes: &BTreeMap<String, BiomeColors>,
    position: IVec3,
) -> [f32; 3] {
    biome_tint(world, biomes, position, BiomeTint::Grass).map(|component| component * 0.6)
}

fn tint(
    state: &BlockState,
    index: i32,
    world: &World,
    biomes: &BTreeMap<String, BiomeColors>,
    mut position: IVec3,
) -> [f32; 3] {
    if index < 0 {
        return [1.0; 3];
    }
    if state.name == "minecraft:redstone_wire" && index == 0 {
        let power = state
            .properties
            .get("power")
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(0)
            .min(15) as f32
            / 15.0;
        return [
            0.3 + 0.7 * power,
            (0.7 * power * power - 0.5).max(0.0),
            (0.6 * power * power - 0.7).max(0.0),
        ];
    }
    if matches!(
        state.name.as_str(),
        "minecraft:large_fern" | "minecraft:tall_grass"
    ) && state
        .properties
        .get("half")
        .is_some_and(|value| value == "upper")
    {
        position.y = position.y.saturating_sub(1);
    }
    match (state.name.as_str(), index) {
        ("minecraft:spruce_leaves", 0) => rgb(0x619961),
        ("minecraft:birch_leaves", 0) => rgb(0x80a755),
        (
            "minecraft:oak_leaves"
            | "minecraft:jungle_leaves"
            | "minecraft:acacia_leaves"
            | "minecraft:dark_oak_leaves"
            | "minecraft:mangrove_leaves"
            | "minecraft:vine",
            0,
        ) => biome_tint(world, biomes, position, BiomeTint::Foliage),
        ("minecraft:leaf_litter", 0) => biome_tint(world, biomes, position, BiomeTint::DryFoliage),
        ("minecraft:water_cauldron", 0) => biome_tint(world, biomes, position, BiomeTint::Water),
        (
            "minecraft:grass_block"
            | "minecraft:fern"
            | "minecraft:short_grass"
            | "minecraft:potted_fern"
            | "minecraft:bush"
            | "minecraft:large_fern"
            | "minecraft:tall_grass"
            | "minecraft:sugar_cane",
            0,
        )
        | ("minecraft:pink_petals" | "minecraft:wildflowers", 1) => {
            biome_tint(world, biomes, position, BiomeTint::Grass)
        }
        ("minecraft:attached_melon_stem" | "minecraft:attached_pumpkin_stem", 0) => rgb(0xe0c71c),
        ("minecraft:melon_stem" | "minecraft:pumpkin_stem", 0) => {
            let age = state
                .properties
                .get("age")
                .and_then(|age| age.parse::<u32>().ok())
                .unwrap_or(0)
                .min(7);
            rgb(((age * 32) << 16) | ((255 - age * 8) << 8) | (age * 4))
        }
        ("minecraft:lily_pad", 0) => rgb(0x208030),
        _ => [1.0; 3],
    }
}

fn full_face(model: &CuboidModel, direction: Direction, atlas: &SpriteAtlas) -> bool {
    model.quads.iter().any(|quad| {
        quad.cullface == Some(direction)
            && quad.direction == direction
            && !quad.texture.force_translucent
            && atlas.transparency(&quad.texture.sprite) == Some(Transparency::Opaque)
            && covers_full_face(quad, direction)
    })
}

fn covers_full_face(quad: &Quad, direction: Direction) -> bool {
    let (axis, side) = match direction {
        Direction::Down => (1, 0.0),
        Direction::Up => (1, 1.0),
        Direction::North => (2, 0.0),
        Direction::South => (2, 1.0),
        Direction::West => (0, 0.0),
        Direction::East => (0, 1.0),
    };
    let axes: Vec<_> = (0..3).filter(|&index| index != axis).collect();
    let mut corners = 0u8;
    for vertex in quad.vertices {
        if (vertex.position[axis] - side).abs() > 1e-5 {
            return false;
        }
        let coordinate = |axis: usize| {
            let value = vertex.position[axis];
            if value.abs() < 1e-5 {
                Some(0)
            } else if (value - 1.0).abs() < 1e-5 {
                Some(1)
            } else {
                None
            }
        };
        let Some(a) = coordinate(axes[0]) else {
            return false;
        };
        let Some(b) = coordinate(axes[1]) else {
            return false;
        };
        corners |= 1 << (a + 2 * b);
    }
    corners == 0b1111
}

fn missing_png() -> Result<Vec<u8>> {
    let mut image = RgbaImage::new(16, 16);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = if (x / 8 + y / 8) % 2 == 0 {
            Rgba([255, 0, 255, 255])
        } else {
            Rgba([0, 0, 0, 255])
        };
    }
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image).write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

fn missing_model() -> Result<BakedBlockstate> {
    let model: serde_json::Value = serde_json::from_str(
        r##"{"textures":{"all":"native:block/missing"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"down":{"texture":"#all","cullface":"down"},"up":{"texture":"#all","cullface":"up"},"north":{"texture":"#all","cullface":"north"},"south":{"texture":"#all","cullface":"south"},"west":{"texture":"#all","cullface":"west"},"east":{"texture":"#all","cullface":"east"}}}]}"##,
    )?;
    let model = ResolvedModel {
        fields: model
            .as_object()
            .context("missing model must be an object")?
            .clone(),
        generated: false,
    };
    let choice = ModelChoice {
        model: ResourceId::parse("native:block/missing")?,
        x_rotation: 0,
        y_rotation: 0,
        uv_lock: false,
        weight: 1,
    };
    let baked = geometry::bake(&model, &choice)?;
    Ok(BakedBlockstate {
        definition: BlockstateDefinition {
            multipart: false,
            groups: vec![vec![choice.clone()]],
        },
        groups: vec![vec![WeightedModel {
            choice,
            model: baked,
        }]],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_world::loaded::{BLOCK_COUNT, Chunk, ChunkPos, Section};
    use std::sync::Arc;

    fn fixture_dimension(path: &str) -> Option<Vec<u8>> {
        (path == "data/minecraft/dimension_type/overworld.json")
            .then(|| br##"{"has_skylight":true,"attributes":{"minecraft:visual/ambient_light_color":"#0a0a0a"}}"##.to_vec())
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn chest_mesh_uses_vanilla_entity_textures() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        for (index, name) in ["minecraft:chest", "minecraft:trapped_chest"]
            .into_iter()
            .enumerate()
        {
            let id = world.intern_state(BlockState {
                name: name.into(),
                properties: BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("type".into(), "single".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            });
            blocks[3 + index * 3 + 2 * 16 + 2 * 256] = id;
        }
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let scene = PreviewScene::prepare(&world, &mut resources, IVec3::ZERO, 1024).unwrap();
        assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
        assert_eq!(scene.opaque_len(), 2 * 18 * 6);
        assert_eq!(scene.translucent_len(), 0);
        let mesh = &scene.chunks[&pos];
        assert_eq!(mesh.chest_lids.len(), 2);
        for lid in mesh.chest_lids.iter() {
            assert_eq!(lid.end - lid.start, 12 * 6);
            let closed = lid.animated_vertices(&mesh.opaque, 0.0);
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&closed),
                bytemuck::cast_slice::<Vertex, u8>(&mesh.opaque[lid.start..lid.end])
            );
            let opened = lid.animated_vertices(&mesh.opaque, -std::f32::consts::FRAC_PI_2);
            assert!(
                opened
                    .iter()
                    .zip(&closed)
                    .any(|(a, b)| a.position != b.position)
            );
            assert!(opened.iter().zip(&closed).all(|(a, b)| a.uv == b.uv));
        }
        for name in ["normal", "trapped"] {
            let id = ResourceId::parse(&format!("minecraft:entity/chest/{name}")).unwrap();
            let rect = scene.atlas.rectangle(&id).unwrap();
            assert_eq!((rect.width, rect.height), (64, 64));
            let original = image::load_from_memory(
                &resources
                    .read_asset(&id.asset_path("textures", "png"))
                    .unwrap()
                    .unwrap(),
            )
            .unwrap()
            .to_rgba8();
            let offset = ((rect.y * scene.atlas.texture.width + rect.x) * 4) as usize;
            assert_eq!(
                &scene.atlas.texture.rgba[offset..offset + 4],
                &original.get_pixel(0, 0).0
            );
        }
        struct Override<'a>(&'a mut native_resources::packs::ResourceStack);
        impl ResourceProvider for Override<'_> {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "assets/minecraft/models/block/chest.json" {
                    return Ok(Some(br##"{"textures":{"all":"minecraft:block/stone"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"down":{"texture":"#all"},"up":{"texture":"#all"},"north":{"texture":"#all"},"south":{"texture":"#all"},"west":{"texture":"#all"},"east":{"texture":"#all"}}}]}"##.to_vec()));
                }
                self.0.read_asset(path)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.0.read_data(path)
            }
        }
        let pack_scene =
            PreviewScene::prepare(&world, &mut Override(&mut resources), IVec3::ZERO, 1024)
                .unwrap();
        assert!(pack_scene.warnings.is_empty(), "{:?}", pack_scene.warnings);
        assert_eq!(pack_scene.opaque_len(), 2 * 18 * 6 + 36);
        assert!(
            pack_scene
                .atlas
                .rectangle(&ResourceId::parse("minecraft:block/stone").unwrap())
                .is_some()
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn placed_planks_and_crafting_table_resolve_to_real_models() {
        use native_core::wood::Planks;
        use native_resources::{default_minecraft_dir, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        for (index, name) in Planks::ALL
            .into_iter()
            .map(|p| p.id())
            .chain(["minecraft:crafting_table"])
            .enumerate()
        {
            let id = world.intern_state(BlockState {
                name: name.into(),
                properties: BTreeMap::new(),
            });
            let x = 1 + (index % 7) * 2;
            let z = 1 + (index / 7) * 3;
            blocks[x + z * 16 + 2 * 256] = id;
        }
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let scene = PreviewScene::prepare(&world, &mut resources, IVec3::ZERO, 1024).unwrap();
        assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
        assert_eq!(scene.opaque_len(), (Planks::ALL.len() + 1) * 36);
        assert!(scene.translucent_len() == 0);
        for name in Planks::ALL
            .into_iter()
            .map(|p| p.id().strip_prefix("minecraft:").unwrap())
            .chain([
                "crafting_table_top",
                "crafting_table_front",
                "crafting_table_side",
            ])
        {
            assert!(
                scene
                    .atlas
                    .rectangle(&ResourceId::parse(&format!("minecraft:block/{name}")).unwrap())
                    .is_some(),
                "missing {name} sprite"
            );
        }
    }

    struct Empty;
    impl ResourceProvider for Empty {
        fn read_asset(&mut self, _path: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(fixture_dimension(path))
        }
    }

    #[test]
    fn incremental_mesh_reloads_materials_when_dimension_type_changes() {
        let mut world = World::new("minecraft:overworld");
        let mut cache = IncrementalPreview::default();
        cache.prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap();
        let initial = cache.generation;
        world.dimension_type = Some("minecraft:overworld".into());
        cache.prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap();
        assert_eq!(cache.generation, initial + 1);
        cache.prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap();
        assert_eq!(cache.generation, initial + 1);
    }

    #[test]
    fn unloaded_unique_block_reuses_materials_and_reloads_after_registry_changes() {
        struct Counting(usize);
        impl ResourceProvider for Counting {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.0 += 1;
                Empty.read_asset(path)
            }
            fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.0 += 1;
                Empty.read_builtin_data(path)
            }
        }
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let a = world.intern_state(BlockState {
            name: "example:a".into(),
            properties: BTreeMap::new(),
        });
        let b = world.intern_state(BlockState {
            name: "example:b".into(),
            properties: BTreeMap::new(),
        });
        let home = ChunkPos { x: 0, z: 0 };
        let far = ChunkPos { x: 3, z: 0 };
        for (position, state) in [(home, a), (far, b)] {
            let mut blocks = Box::new([air; BLOCK_COUNT]);
            blocks[0] = state;
            world.chunks.insert(
                position,
                Arc::new(Chunk {
                    block_light_correct: false,
                    position,
                    status: Some("minecraft:full".into()),
                    sections: BTreeMap::from([(
                        0,
                        Arc::new(Section {
                            blocks: Some(blocks),
                            biomes: None,
                            block_light: None,
                            sky_light: None,
                        }),
                    )]),
                }),
            );
        }
        let mut source = Counting(0);
        let mut cache = IncrementalPreview::default();
        let first = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        let reads = source.0;
        world.set_biome_world_seed(Some(12345));
        let seeded = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 2);
        assert!(Arc::ptr_eq(&first.atlas, &seeded.atlas));
        cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 0);
        world.set_biome_world_seed(None);
        cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 2);
        assert_eq!(source.0, reads);
        let removed = world.chunks.remove(&far).unwrap();
        let scene = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 0);
        assert_eq!(scene.material_generation, first.material_generation);
        assert!(Arc::ptr_eq(&scene.atlas, &first.atlas));
        assert_eq!(source.0, reads);
        assert_same_mesh(
            &scene,
            &PreviewScene::prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap(),
        );
        world.chunks.insert(far, removed);
        let scene = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 1);
        assert_eq!(source.0, reads);
        assert_same_mesh(
            &scene,
            &PreviewScene::prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap(),
        );

        let mut replaced = World::new("minecraft:overworld");
        let changed = replaced.intern_state(BlockState {
            name: "example:changed".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([changed; BLOCK_COUNT]);
        blocks[0] = changed;
        replaced.chunks.insert(
            home,
            Arc::new(Chunk {
                block_light_correct: false,
                position: home,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let generation = scene.material_generation;
        let changed_scene = cache
            .prepare(&replaced, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_ne!(changed_scene.material_generation, generation);
        assert!(source.0 > reads);
        assert_same_mesh(
            &changed_scene,
            &PreviewScene::prepare(&replaced, &mut Empty, IVec3::ZERO, 128).unwrap(),
        );
    }

    #[test]
    fn retained_texture_failure_falls_back_to_current_resources() {
        struct Source {
            fail_old: bool,
            png: Vec<u8>,
        }
        impl ResourceProvider for Source {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let asset = match path {
                    "assets/example/blockstates/old.json" => {
                        Some(br#"{"variants":{"":{"model":"example:block/old"}}}"#.to_vec())
                    }
                    "assets/example/blockstates/new.json" => {
                        Some(br#"{"variants":{"":{"model":"example:block/new"}}}"#.to_vec())
                    }
                    "assets/example/models/block/old.json"
                    | "assets/example/models/block/new.json" => {
                        let name = if path.contains("old.json") {
                            "old"
                        } else {
                            "new"
                        };
                        Some(format!(r##"{{"textures":{{"all":"example:block/{name}"}},"elements":[{{"from":[0,0,0],"to":[16,16,16],"faces":{{"up":{{"texture":"#all"}}}}}}]}}"##).into_bytes())
                    }
                    "assets/example/textures/block/old.png" if self.fail_old => {
                        anyhow::bail!("old texture is unavailable")
                    }
                    "assets/example/textures/block/old.png"
                    | "assets/example/textures/block/new.png" => Some(self.png.clone()),
                    _ => None,
                };
                Ok(asset)
            }
            fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(fixture_dimension(path))
            }
        }
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(16, 16, Rgba([10, 20, 30, 255])))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let mut source = Source {
            fail_old: false,
            png: png.into_inner(),
        };
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let old = world.intern_state(BlockState {
            name: "example:old".into(),
            properties: BTreeMap::new(),
        });
        let new = world.intern_state(BlockState {
            name: "example:new".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[0] = old;
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let mut cache = IncrementalPreview::default();
        let initial = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        world.set_block_id(0, 0, 0, new).unwrap();
        source.fail_old = true;
        let result = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_ne!(result.material_generation, initial.material_generation);
        assert!(
            !cache
                .materials
                .as_ref()
                .unwrap()
                .0
                .states
                .iter()
                .any(|(id, _)| *id == old)
        );
        assert_same_mesh(
            &result,
            &PreviewScene::prepare(&world, &mut source, IVec3::ZERO, 128).unwrap(),
        );
    }

    #[test]
    fn incremental_scene_accepts_more_than_six_million_vertices_across_chunks() {
        let mut world = World::new("minecraft:overworld");
        for x in -12..=12 {
            for z in -12..=12 {
                let position = ChunkPos { x, z };
                world.chunks.insert(
                    position,
                    Arc::new(Chunk {
                        block_light_correct: false,
                        position,
                        status: Some("minecraft:full".into()),
                        sections: BTreeMap::new(),
                    }),
                );
            }
        }
        let mut cache = IncrementalPreview::default();
        cache.prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap();
        let vertex = Vertex {
            position: [0.0; 3],
            uv: [0.0; 2],
            shade: 1.0,
            tint: [1.0; 3],
            light: u32::MAX,
        };
        let mesh = Arc::new(ChunkMesh {
            opaque: vec![vertex; 10_000].into(),
            translucent: Arc::from([]),
            chest_lids: Arc::from([]),
        });
        for cached in cache.chunks.values_mut() {
            cached.mesh = Arc::clone(&mesh);
        }
        let scene = cache.prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap();
        assert_eq!(scene.chunks.len(), 625);
        assert_eq!(scene.opaque_len(), 6_250_000);
        assert_eq!(cache.last_rebuilt, 0);
    }

    fn assert_same_mesh(incremental: &PreviewScene, full: &PreviewScene) {
        assert_eq!(incremental.origin, full.origin);
        assert_eq!(incremental.atlas.texture.width, full.atlas.texture.width);
        assert_eq!(incremental.atlas.texture.height, full.atlas.texture.height);
        assert_eq!(incremental.atlas.texture.rgba, full.atlas.texture.rgba);
        assert_eq!(incremental.opaque_len(), full.opaque_len());
        for (a, b) in incremental.opaque_vertices().zip(full.opaque_vertices()) {
            assert_eq!(bytemuck::bytes_of(a), bytemuck::bytes_of(b));
        }
        assert_eq!(incremental.translucent_len(), full.translucent_len());
        for (a, b) in incremental
            .translucent_quads()
            .zip(full.translucent_quads())
        {
            assert_eq!(a.center, b.center);
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&a.vertices),
                bytemuck::cast_slice::<Vertex, u8>(&b.vertices)
            );
        }
        for eye in [Vec3::ZERO, Vec3::new(23.0, 7.0, -11.0)] {
            let a = incremental.transparent_vertices(eye);
            let b = full.transparent_vertices(eye);
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&a),
                bytemuck::cast_slice::<Vertex, u8>(&b)
            );
        }
    }

    #[test]
    fn incremental_mesh_matches_full_rebuild_for_edits_streaming_and_rebase() {
        struct Counting(usize);
        impl ResourceProvider for Counting {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.0 += 1;
                Empty.read_asset(path)
            }
            fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.0 += 1;
                Empty.read_builtin_data(path)
            }
        }
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let solid = world.intern_state(BlockState {
            name: "example:unknown".into(),
            properties: BTreeMap::new(),
        });
        for x in -2..=2 {
            for z in -2..=2 {
                let pos = ChunkPos { x, z };
                let mut blocks = Box::new([air; BLOCK_COUNT]);
                blocks[8 + 8 * 16] = solid;
                world.chunks.insert(
                    pos,
                    Arc::new(Chunk {
                        block_light_correct: false,
                        position: pos,
                        status: Some("minecraft:full".into()),
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: Some(Box::new([0; native_world::loaded::LIGHT_BYTES])),
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let mut source = Counting(0);
        let mut cache = IncrementalPreview::default();
        let first = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 25);
        let reads = source.0;
        assert_same_mesh(
            &first,
            &PreviewScene::prepare(&world, &mut Empty, first.origin, 128).unwrap(),
        );

        for (position, expected) in [
            (IVec3::new(2, 1, 2), 1),
            (IVec3::new(15, 1, 6), 2),
            (IVec3::new(15, 1, 15), 4),
        ] {
            world
                .set_block_id(position.x, position.y, position.z, solid)
                .unwrap();
            let scene = cache
                .prepare(&world, &mut source, IVec3::ZERO, 128)
                .unwrap();
            assert_eq!(cache.last_rebuilt, expected, "edit at {position:?}");
            assert_eq!(
                source.0, reads,
                "unchanged materials should not reload resources"
            );
            assert_eq!(scene.material_generation, first.material_generation);
            assert!(Arc::ptr_eq(&scene.atlas, &first.atlas));
            assert_same_mesh(
                &scene,
                &PreviewScene::prepare(&world, &mut Empty, scene.origin, 128).unwrap(),
            );
        }

        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        let index = 15 + 6 * 16 + 2 * 256;
        section.block_light.as_mut().unwrap()[index / 2] |= 0xf0;
        let scene = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 2);
        assert_same_mesh(
            &scene,
            &PreviewScene::prepare(&world, &mut Empty, scene.origin, 128).unwrap(),
        );

        let removed = world.chunks.remove(&ChunkPos { x: 2, z: 2 }).unwrap();
        let scene = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 3);
        assert_same_mesh(
            &scene,
            &PreviewScene::prepare(&world, &mut Empty, scene.origin, 128).unwrap(),
        );
        world.chunks.insert(removed.position, removed);
        let scene = cache
            .prepare(&world, &mut source, IVec3::X * 16, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 4);
        assert_eq!(scene.origin, IVec3::ZERO);
        assert_same_mesh(
            &scene,
            &PreviewScene::prepare(&world, &mut Empty, scene.origin, 128).unwrap(),
        );
        let scene = cache
            .prepare(&world, &mut source, IVec3::X * 512, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 25);
        assert_eq!(scene.origin, IVec3::X * 512);
        assert_same_mesh(
            &scene,
            &PreviewScene::prepare(&world, &mut Empty, scene.origin, 128).unwrap(),
        );

        let other = world.intern_state(BlockState {
            name: "example:new_block".into(),
            properties: BTreeMap::new(),
        });
        world.set_block_id(3, 1, 3, other).unwrap();
        let scene = cache
            .prepare(&world, &mut source, IVec3::X * 512, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 25);
        assert_ne!(scene.material_generation, first.material_generation);
        assert!(source.0 > reads);
        assert_same_mesh(
            &scene,
            &PreviewScene::prepare(&world, &mut Empty, scene.origin, 128).unwrap(),
        );
    }

    #[test]
    #[ignore = "performance measurement using locally installed Minecraft 26.3 assets"]
    fn measure_incremental_world_mesh() {
        use native_resources::{default_minecraft_dir, packs::ResourceStack};
        use std::time::Instant;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let mut source = ResourceStack::open(&game).unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        for x in -4..=4 {
            for z in -4..=4 {
                let pos = ChunkPos { x, z };
                let mut blocks = Box::new([air; BLOCK_COUNT]);
                blocks[..4 * 256].fill(stone);
                world.chunks.insert(
                    pos,
                    Arc::new(Chunk {
                        block_light_correct: false,
                        position: pos,
                        status: Some("minecraft:full".into()),
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let mut cache = IncrementalPreview::default();
        cache
            .prepare(&world, &mut source, IVec3::ZERO, 1024)
            .unwrap();
        let mut full_times = Vec::new();
        let mut cached_times = Vec::new();
        for i in 0..12 {
            world
                .set_block_id(8, 4, 8, if i % 2 == 0 { stone } else { air })
                .unwrap();
            let start = Instant::now();
            let cached = cache
                .prepare(&world, &mut source, IVec3::ZERO, 1024)
                .unwrap();
            cached_times.push(start.elapsed());
            assert_eq!(cache.last_rebuilt, 1);
            let start = Instant::now();
            let full = PreviewScene::prepare(&world, &mut source, cached.origin, 1024).unwrap();
            full_times.push(start.elapsed());
            assert_same_mesh(&cached, &full);
        }
        full_times.sort_unstable();
        cached_times.sort_unstable();
        println!(
            "81 chunks, 12 interior block changes: full median {:?}, max {:?}; incremental median {:?}, max {:?}",
            full_times[6], full_times[11], cached_times[6], cached_times[11]
        );
    }

    struct Glass {
        png: Vec<u8>,
    }

    struct FluidImages {
        water: Vec<u8>,
        opaque: Vec<u8>,
    }

    struct BiomeImages {
        fluids: FluidImages,
        colormap: Vec<u8>,
    }
    impl ResourceProvider for BiomeImages {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            if path.starts_with("assets/minecraft/textures/colormap/") {
                Ok(Some(self.colormap.clone()))
            } else {
                self.fluids.read_asset(path)
            }
        }

        fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/worldgen/biome/plains.json" => Some(
                    br##"{"temperature":0.8,"downfall":0.4,"effects":{"water_color":"#123456"}}"##
                        .to_vec(),
                ),
                "data/minecraft/worldgen/biome/cherry_grove.json" => Some(
                    br##"{"temperature":0.5,"downfall":0.8,"effects":{"water_color":"#abcdef"}}"##
                        .to_vec(),
                ),
                _ => fixture_dimension(path),
            })
        }
    }
    impl ResourceProvider for FluidImages {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "assets/minecraft/blockstates/oak_slab.json" => Some(br#"{"variants":{"type=bottom":{"model":"block/slab"}}}"#.to_vec()),
                "assets/minecraft/models/block/slab.json" => Some(br##"{"textures":{"all":"block/oak_planks"},"elements":[{"from":[0,0,0],"to":[16,8,16],"faces":{"down":{"texture":"#all","cullface":"down"},"up":{"texture":"#all"},"north":{"texture":"#all","cullface":"north"},"south":{"texture":"#all","cullface":"south"},"west":{"texture":"#all","cullface":"west"},"east":{"texture":"#all","cullface":"east"}}}]}"##.to_vec()),
                "assets/minecraft/textures/block/water_still.png" | "assets/minecraft/textures/block/water_flow.png" | "assets/minecraft/textures/block/water_overlay.png" => Some(self.water.clone()),
                "assets/minecraft/textures/block/lava_still.png" | "assets/minecraft/textures/block/lava_flow.png" | "assets/minecraft/textures/block/oak_planks.png" => Some(self.opaque.clone()),
                _ => None,
            })
        }
        fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(fixture_dimension(path))
        }
    }

    #[test]
    fn incremental_mesh_updates_diagonal_fluid_corners() {
        let png = |color| {
            let image = RgbaImage::from_pixel(16, 16, Rgba(color));
            let mut output = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(image)
                .write_to(&mut output, ImageFormat::Png)
                .unwrap();
            output.into_inner()
        };
        let mut source = FluidImages {
            water: png([80, 120, 255, 180]),
            opaque: png([200, 100, 0, 255]),
        };
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        for x in 0..=1 {
            for z in 0..=1 {
                let pos = ChunkPos { x, z };
                let mut blocks = Box::new([air; BLOCK_COUNT]);
                if pos == (ChunkPos { x: 1, z: 1 }) {
                    blocks[256] = water;
                }
                world.chunks.insert(
                    pos,
                    Arc::new(Chunk {
                        block_light_correct: false,
                        position: pos,
                        status: Some("minecraft:full".into()),
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let mut cache = IncrementalPreview::default();
        let before = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert!(before.translucent_len() != 0);
        world.set_block_id(15, 1, 15, water).unwrap();
        let after = cache
            .prepare(&world, &mut source, IVec3::ZERO, 128)
            .unwrap();
        assert_eq!(cache.last_rebuilt, 4);
        let full = PreviewScene::prepare(&world, &mut source, after.origin, 128).unwrap();
        assert_same_mesh(&after, &full);
        assert_ne!(before.translucent_len(), after.translucent_len());
    }
    impl ResourceProvider for Glass {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "assets/minecraft/blockstates/glass.json" => Some(br#"{"variants":{"":{"model":"block/glass"}}}"#.to_vec()),
                "assets/minecraft/models/block/glass.json" => Some(br##"{"textures":{"all":"block/glass"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"down":{"texture":"#all","cullface":"down"},"up":{"texture":"#all","cullface":"up"},"north":{"texture":"#all","cullface":"north"},"south":{"texture":"#all","cullface":"south"},"west":{"texture":"#all","cullface":"west"},"east":{"texture":"#all","cullface":"east"}}}]}"##.to_vec()),
                "assets/minecraft/textures/block/glass.png" => Some(self.png.clone()),
                _ => None,
            })
        }
        fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(fixture_dimension(path))
        }
    }

    #[test]
    fn meshes_loaded_chunks_and_culls_between_neighboring_opaque_models() {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let unknown = world.intern_state(BlockState {
            name: "example:unknown".into(),
            properties: BTreeMap::new(),
        });
        for x in 0..=1 {
            let mut blocks = Box::new([air; BLOCK_COUNT]);
            blocks[if x == 0 { 15 } else { 0 }] = unknown;
            let pos = ChunkPos { x, z: 0 };
            world.chunks.insert(
                pos,
                Arc::new(Chunk {
                    block_light_correct: false,
                    position: pos,
                    status: Some("minecraft:full".into()),
                    sections: BTreeMap::from([(
                        0,
                        Arc::new(Section {
                            blocks: Some(blocks),
                            biomes: None,
                            block_light: None,
                            sky_light: None,
                        }),
                    )]),
                }),
            );
        }
        let scene = PreviewScene::prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap();
        assert_eq!(scene.opaque_len(), 60);
        assert!(scene.translucent_len() == 0);
        assert_eq!(scene.warnings.len(), 1);
        assert!(scene.warnings[0].contains("example:unknown"));
    }

    #[test]
    fn unloaded_registry_entries_do_not_require_models_or_biome_data() {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        world.intern_state(BlockState {
            name: "example:unloaded_block".into(),
            properties: BTreeMap::new(),
        });
        world.intern_biome("example:unloaded_biome".into());
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([air; BLOCK_COUNT])),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let scene = PreviewScene::prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap();
        assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
        assert!(scene.opaque_len() == 0);
    }

    #[test]
    fn light_samples_keep_block_and_sky_separate_for_faces_and_fluids() {
        use native_world::loaded::LIGHT_BYTES;
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let mut block_light = Box::new([0u8; LIGHT_BYTES]);
        let mut sky_light = Box::new([0u8; LIGHT_BYTES]);
        let put = |data: &mut [u8], pos: IVec3, value: u8| {
            let index = pos.x as usize + pos.z as usize * 16 + pos.y as usize * 256;
            if index.is_multiple_of(2) {
                data[index / 2] = (data[index / 2] & 0xf0) | value;
            } else {
                data[index / 2] = (data[index / 2] & 0x0f) | (value << 4);
            }
        };
        let position = IVec3::new(8, 8, 8);
        for (pos, block, sky) in [
            (position, 2, 1),
            (position + IVec3::Y, 8, 12),
            (position - IVec3::Y, 4, 5),
        ] {
            put(block_light.as_mut(), pos, block);
            put(sky_light.as_mut(), pos, sky);
        }
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            chunk_pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: chunk_pos,
                status: None,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([air; BLOCK_COUNT])),
                        biomes: None,
                        block_light: Some(block_light),
                        sky_light: Some(sky_light),
                    }),
                )]),
            }),
        );
        let fallback = missing_model().unwrap();
        let mut quad = fallback.groups[0][0]
            .model
            .quads
            .iter()
            .find(|q| q.direction == Direction::Up)
            .unwrap()
            .clone();
        assert_eq!(model_light(&world, position, &quad, true), (8, 12));
        quad.cullface = None;
        for vertex in &mut quad.vertices {
            vertex.position[1] = 0.5;
        }
        assert_eq!(model_light(&world, position, &quad, true), (2, 1));
        quad.cullface = Some(Direction::Down);
        assert_eq!(model_light(&world, position, &quad, true), (4, 5));
        quad.cullface = None;
        quad.light_emission = 6;
        assert_eq!(model_light(&world, position, &quad, true), (6, 6));
        assert_eq!(
            fluid_light(&world, position, Direction::Up, FluidKind::Water, true),
            (8, 12)
        );
        assert_eq!(
            fluid_light(&world, position, Direction::Down, FluidKind::Water, true),
            (4, 5)
        );
        assert_eq!(
            fluid_light(&world, position, Direction::Up, FluidKind::Lava, true),
            (15, 12)
        );
        quad.light_emission = 0;
        quad.cullface = Some(Direction::Up);
        assert_eq!(model_light(&world, position, &quad, false), (8, 0));
        quad.light_emission = 6;
        assert_eq!(model_light(&world, position, &quad, false), (8, 6));
        assert_eq!(
            fluid_light(&world, position, Direction::Up, FluidKind::Water, false),
            (8, 0)
        );
        assert_eq!(
            fluid_light(&world, position, Direction::Up, FluidKind::Lava, false),
            (15, 0)
        );
        assert_eq!(directional_shade(Direction::Up, true), 0.9);
        assert_eq!(directional_shade(Direction::Down, false), 0.5);
    }

    #[test]
    fn mesh_uses_colored_stored_block_light() {
        use native_world::loaded::LIGHT_BYTES;
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let block = world.intern_state(BlockState {
            name: "example:unknown".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[8 + 8 * 16 + 8 * 256] = block;
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: None,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: Some(Box::new([0x88; LIGHT_BYTES])),
                        sky_light: Some(Box::new([0; LIGHT_BYTES])),
                    }),
                )]),
            }),
        );
        let scene = PreviewScene::prepare(&world, &mut Empty, IVec3::ZERO, 128).unwrap();
        assert_eq!(scene.opaque_len(), 36);
        assert!(scene.opaque_vertices().all(|vertex| {
            let id = vertex.light as usize;
            let light = scene.lightmap[id / 16][id % 16];
            let color = std::array::from_fn::<_, 3, _>(|i| vertex.tint[i] * light[i]);
            color[0] > color[1] && color[1] > color[2]
        }));
    }

    #[test]
    fn same_glass_blocks_hide_their_shared_faces() {
        let mut image = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(16, 16, Rgba([100, 200, 255, 128])))
            .write_to(&mut image, ImageFormat::Png)
            .unwrap();
        let mut source = Glass {
            png: image.into_inner(),
        };
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let glass = world.intern_state(BlockState {
            name: "minecraft:glass".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[0] = glass;
        blocks[1] = glass;
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let scene = PreviewScene::prepare(&world, &mut source, IVec3::ZERO, 128).unwrap();
        assert!(scene.opaque_len() == 0);
        assert_eq!(scene.translucent_len(), 10);
        assert!(scene.warnings.is_empty());
    }

    #[test]
    fn water_lava_and_waterlogged_models_share_the_scene_without_placeholder_models() {
        let png = |pixel| {
            let mut output = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(16, 16, Rgba(pixel)))
                .write_to(&mut output, ImageFormat::Png)
                .unwrap();
            output.into_inner()
        };
        let mut source = FluidImages {
            water: png([80, 120, 255, 180]),
            opaque: png([255, 120, 20, 255]),
        };
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let flowing = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "4".into())]),
        });
        let lava = world.intern_state(BlockState {
            name: "minecraft:lava".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let slab = world.intern_state(BlockState {
            name: "minecraft:oak_slab".into(),
            properties: BTreeMap::from([
                ("type".into(), "bottom".into()),
                ("waterlogged".into(), "true".into()),
            ]),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[0] = water;
        blocks[1] = flowing;
        blocks[2] = lava;
        blocks[3] = slab;
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let scene = PreviewScene::prepare(&world, &mut source, IVec3::ZERO, 128).unwrap();
        assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
        assert!(scene.opaque_len() != 0);
        assert!(scene.translucent_len() != 0);
        assert!(
            scene
                .translucent_quads()
                .flat_map(|quad| quad.vertices)
                .any(|vertex| vertex.position[0] >= 3.0)
        );
    }

    #[test]
    fn fluid_vertices_use_the_biome_of_each_block() {
        let png = |size, pixel| {
            let mut output = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(size, size, Rgba(pixel)))
                .write_to(&mut output, ImageFormat::Png)
                .unwrap();
            output.into_inner()
        };
        let mut source = BiomeImages {
            fluids: FluidImages {
                water: png(16, [80, 120, 255, 180]),
                opaque: png(16, [255, 120, 20, 255]),
            },
            colormap: png(256, [100, 200, 50, 255]),
        };
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let plains = world.intern_biome("minecraft:plains".into());
        let cherry = world.intern_biome("minecraft:cherry_grove".into());
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[0] = water;
        blocks[4] = water;
        let mut biomes = Box::new([plains; native_world::loaded::BIOME_COUNT]);
        biomes[1] = cherry;
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: None,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: Some(biomes),
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let scene = PreviewScene::prepare(&world, &mut source, IVec3::ZERO, 128).unwrap();
        assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
        assert!(
            scene
                .translucent_quads()
                .any(|quad| quad.center.x < 2.0 && quad.vertices[0].tint == rgb(0x123456))
        );
        assert!(
            scene
                .translucent_quads()
                .any(|quad| quad.center.x > 3.0 && quad.vertices[0].tint == rgb(0xabcdef))
        );
        assert_eq!(
            tint(
                &BlockState {
                    name: "minecraft:birch_leaves".into(),
                    properties: BTreeMap::new()
                },
                0,
                &world,
                &BTreeMap::new(),
                IVec3::ZERO
            ),
            rgb(0x80a755)
        );
    }

    #[test]
    #[ignore = "requires the user's Minecraft Java Edition 26.3 installation"]
    fn world_data_pack_colors_fluid_vertices() {
        let temp = tempfile::tempdir().unwrap();
        let pack = temp.path().join("datapacks/custom");
        let biome_dir = pack.join("data/example/worldgen/biome");
        std::fs::create_dir_all(&biome_dir).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        std::fs::write(
            biome_dir.join("custom.json"),
            br##"{"has_precipitation":true,"temperature":0.8,"downfall":0.4,"effects":{"water_color":"#123456"},"carvers":[],"features":[]}"##,
        )
        .unwrap();
        let minecraft = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = native_resources::packs::ResourceStack::open(&minecraft).unwrap();
        let warnings = source
            .configure_world_data(
                temp.path(),
                &native_resources::datapacks::DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/custom".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let biome = world.intern_biome("example:custom".into());
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[0] = water;
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: None,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: Some(Box::new([biome; native_world::loaded::BIOME_COUNT])),
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let scene = PreviewScene::prepare(&world, &mut source, IVec3::ZERO, 1024).unwrap();
        assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
        assert!(scene.translucent_len() != 0);
        assert!(
            scene
                .translucent_quads()
                .all(|quad| quad.vertices[0].tint == rgb(0x123456))
        );
    }
}
