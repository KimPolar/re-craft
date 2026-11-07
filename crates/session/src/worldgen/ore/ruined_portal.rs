//! Apply ruined-portal terrain decoration to an unpublished generation region.
use super::super::jigsaw::{JigsawPlacementWorld, StructurePlacementWorld};
use super::*;
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    heightmap_tags::resolve_block_tag,
    ruined_portal_decoration::{PortalBlock, PortalDecorationWorld},
    structure_start::Heightmap,
    tree_leaf_update::ShapeEdgeWorld,
};
use native_world::fluid::FluidState;

// These stateless blocks have no updateShape effects; magma marks above itself.
pub(super) fn portal_full(state: &BlockState) -> bool {
    state.properties.is_empty()
        && matches!(
            state.name.as_str(),
            "minecraft:stone"
                | "minecraft:stone_bricks"
                | "minecraft:mossy_stone_bricks"
                | "minecraft:cracked_stone_bricks"
                | "minecraft:chiseled_stone_bricks"
                | "minecraft:cobblestone"
                | "minecraft:mossy_cobblestone"
                | "minecraft:obsidian"
                | "minecraft:crying_obsidian"
                | "minecraft:gold_block"
                | "minecraft:netherrack"
                | "minecraft:magma_block"
                | "minecraft:blackstone"
                | "minecraft:polished_blackstone"
                | "minecraft:polished_blackstone_bricks"
                | "minecraft:cracked_polished_blackstone_bricks"
                | "minecraft:chiseled_polished_blackstone"
        )
}

pub(super) fn portal_template_full(state: &BlockState) -> Result<bool> {
    if portal_full(state) {
        return Ok(true);
    }
    if native_world::collision::jigsaw_state(state) {
        return Ok(true);
    }
    if native_world::collision::slab_stair_solid(state).is_some() {
        return Ok(state.name.ends_with("_slab") && state.properties["type"] == "double");
    }
    if state.name == "minecraft:lava" {
        ensure!(
            state.properties.len() == 1
                && state.properties.get("level").is_some_and(|raw| raw
                    .parse::<u8>()
                    .ok()
                    .is_some_and(|level| level <= 15 && level.to_string() == *raw)),
            "invalid portal lava state"
        );
        return Ok(false);
    }
    if is_air(state)
        || native_world::wall::Wall::from_state(state).is_some()
        || (state.name == "minecraft:chest"
            && state.properties.get("type").map(String::as_str) == Some("single"))
        || (state.name == "minecraft:iron_bars"
            && native_world::collision::pane_waterlogged(state).is_some())
    {
        return Ok(false);
    }
    bail!("unsupported portal template static shape: {state:?}")
}

/// Active data-pack tags used by ordinary portal neighbor updates.
pub struct PortalUpdateTags {
    walls: BTreeSet<String>,
    post_override: BTreeSet<String>,
    bubble_fluids: BTreeSet<String>,
    bubble_drag: BTreeSet<String>,
    bubble_push: BTreeSet<String>,
    pub(super) fence_tags: Arc<OreBlockTags>,
}

impl PortalUpdateTags {
    pub fn load(
        resources: &mut impl native_resources::packs::ResourceProvider,
        blocks: &BTreeSet<String>,
    ) -> Result<Self> {
        let fence_tags = [
            "minecraft:fences",
            "minecraft:wooden_fences",
            "minecraft:shulker_boxes",
        ]
        .into_iter()
        .map(ResourceId::parse)
        .collect::<Result<BTreeSet<_>>>()?;
        Ok(Self {
            fence_tags: Arc::new(OreBlockTags::load(resources, blocks, &fence_tags)?),
            walls: resolve_block_tag(resources, &ResourceId::parse("minecraft:walls")?, blocks)?,
            post_override: resolve_block_tag(
                resources,
                &ResourceId::parse("minecraft:wall_post_override")?,
                blocks,
            )?,
            bubble_fluids: native_resources::fluid_tags::load_bubble_column_occupiable(resources)?,
            bubble_drag: resolve_block_tag(
                resources,
                &ResourceId::parse("minecraft:enables_bubble_column_drag_down")?,
                blocks,
            )?,
            bubble_push: resolve_block_tag(
                resources,
                &ResourceId::parse("minecraft:enables_bubble_column_push_up")?,
                blocks,
            )?,
        })
    }
}

/// Resources shared by portal starts from one unchanged data-pack stack.
pub struct PortalPlacementResources<'a> {
    registry: &'a GeneratedBlockRegistry,
    blocks: &'a BTreeSet<String>,
    protected: BTreeSet<String>,
    update_tags: PortalUpdateTags,
}

impl<'a> PortalPlacementResources<'a> {
    pub fn load(
        provider: &mut impl ResourceProvider,
        blocks: &'a BTreeSet<String>,
        registry: &'a GeneratedBlockRegistry,
    ) -> Result<Arc<Self>> {
        let protected = resolve_block_tag(
            provider,
            &ResourceId::parse("minecraft:features_cannot_replace")?,
            blocks,
        )?;
        let update_tags = PortalUpdateTags::load(provider, blocks)?;
        Ok(Arc::new(Self {
            registry,
            blocks,
            protected,
            update_tags,
        }))
    }

    /// Prepare a start against the same unchanged data-pack stack.
    pub fn prepare(
        self: &Arc<Self>,
        provider: &mut impl ResourceProvider,
        start: &super::super::ruined_portal::PreparedStart,
    ) -> Result<PreparedPortalPlacement<'a>> {
        ensure!(
            start.template.data_version == Some(5023),
            "unsupported portal template version"
        );
        ensure!(
            start.template.size == start.piece.template_size(),
            "portal template size changed"
        );
        ensure!(
            start.point.position == start.piece.position
                && start.point.bounds == start.piece.bounds
                && start.point.pivot == [start.template.size[0] / 2, 0, start.template.size[2] / 2],
            "portal generation point differs from piece"
        );
        let processors = start.piece.prepare_processors(provider, self.blocks)?;
        Ok(PreparedPortalPlacement {
            source: start.clone(),
            processors,
            resources: Arc::clone(self),
        })
    }
}

/// Immutable processor chain and data-pack tags for a single portal start.
#[derive(Clone)]
pub struct PreparedPortalPlacement<'a> {
    source: super::super::ruined_portal::PreparedStart,
    processors: native_persistence::structure_processors::PreparedProcessors,
    resources: Arc<PortalPlacementResources<'a>>,
}

impl PreparedPortalPlacement<'_> {
    pub fn place(
        &self,
        region: &mut DecorationRegion<'_>,
        start: &super::super::ruined_portal::PreparedStart,
        clip: &mut native_core::beardifier::BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(
            self.source.chunk == start.chunk
                && self.source.point == start.point
                && self.source.terrain_adjustment == start.terrain_adjustment
                && self.source.piece == start.piece
                && Arc::ptr_eq(&self.source.template, &start.template),
            "prepared portal start changed"
        );
        region.place_ruined_portal(
            start,
            self.resources.registry,
            &self.processors,
            &self.resources.protected,
            &self.resources.update_tags,
            clip,
            random,
        )
    }
}

pub(super) struct PortalTemplateRegion<'r, 'a, 'g> {
    pub(super) inner: super::jigsaw::JigsawRegion<'r, 'a, 'g>,
    pub(super) wall_tags: &'g PortalUpdateTags,
    pub(super) temporary_jigsaws: BTreeSet<[i32; 3]>,
}

impl JigsawPlacementWorld for PortalTemplateRegion<'_, '_, '_> {
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        JigsawPlacementWorld::block(&mut self.inner, pos)
    }
    fn fluid_at(&mut self, pos: [i32; 3]) -> Result<Option<FluidState>> {
        self.inner.fluid_at(pos)
    }
    fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>> {
        self.inner.fluid_of_state(state)
    }
    fn place_template_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&native_persistence::nbt::Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        self.inner.place_template_block(pos, state, nbt, random)
    }
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
        self.inner.is_liquid_container(state)
    }
    fn place_liquid(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        fluid: Option<FluidState>,
    ) -> Result<()> {
        self.inner.place_liquid(pos, state, fluid)
    }
    fn mark_block_entity_changed(&mut self, pos: [i32; 3]) -> Result<()> {
        if self.temporary_jigsaws.contains(&pos) {
            ensure!(
                native_world::collision::jigsaw_state(&self.inner.region.read(pos)?),
                "temporary jigsaw changed"
            );
            return Ok(());
        }
        self.inner.mark_block_entity_changed(pos)
    }
    fn place_template_entities(
        &mut self,
        _: &[native_persistence::nbt::Tag],
        _: [i32; 3],
        _: native_resources::structure_transform::Rotation,
        _: native_core::beardifier::BoundingBox,
    ) -> Result<()> {
        bail!("portal template entities are unsupported")
    }
}

impl ShapeEdgeWorld for PortalTemplateRegion<'_, '_, '_> {
    type State = BlockState;
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        JigsawPlacementWorld::block(self, pos)
    }
    fn update_shape(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        direction: [i32; 3],
        _: [i32; 3],
        neighbor: &BlockState,
    ) -> Result<BlockState> {
        ensure!(
            self.inner
                .registry
                .is_canonical_state(&state.name, &state.properties),
            "invalid portal edge state"
        );
        if native_world::collision::jigsaw_state(state) {
            return Ok(state.clone());
        }
        if state.name == "minecraft:lava" {
            ensure!(
                self.inner
                    .registry
                    .is_canonical_state(&neighbor.name, &neighbor.properties),
                "invalid portal lava neighbor"
            );
            portal_template_full(state)?;
            let fluid = FluidState::from_block(state)
                .map_err(anyhow::Error::msg)?
                .context("missing portal lava")?;
            let adjacent = FluidState::from_block(neighbor).map_err(anyhow::Error::msg)?;
            let fluid_id = if fluid.source {
                "minecraft:lava"
            } else {
                "minecraft:flowing_lava"
            };
            if fluid.source || adjacent.is_some_and(|value| value.source) {
                SpringWorld::schedule_spring_tick(
                    self.inner.region,
                    pos,
                    &ResourceId::parse(fluid_id)?,
                )?;
            }
            if direction == [0, -1, 0]
                && fluid.source
                && fluid.amount == 8
                && self.wall_tags.bubble_fluids.contains(fluid_id)
                && (self.wall_tags.bubble_drag.contains(&neighbor.name)
                    || self.wall_tags.bubble_push.contains(&neighbor.name))
            {
                SimpleBlockWorld::schedule_simple_tick(self.inner.region, pos)?;
            }
            return Ok(state.clone());
        }
        if let Some(wall) = native_world::wall::Wall::from_state(state) {
            use native_world::wall::{Above, Side};
            if wall.waterlogged {
                SpringWorld::schedule_spring_tick(
                    self.inner.region,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            if direction == [0, -1, 0] {
                return Ok(state.clone());
            }
            ensure!(
                self.inner
                    .registry
                    .is_canonical_state(&neighbor.name, &neighbor.properties),
                "invalid portal wall neighbor"
            );
            let mut connected = wall.shape.sides.map(|side| side != Side::None);
            let changed = match direction {
                [0, 0, -1] => Some(0),
                [1, 0, 0] => Some(1),
                [0, 0, 1] => Some(2),
                [-1, 0, 0] => Some(3),
                [0, 1, 0] => None,
                _ => bail!("invalid portal wall direction"),
            };
            if let Some(index) = changed {
                connected[index] = if self.wall_tags.walls.contains(&neighbor.name)
                    || native_world::collision::jigsaw_state(neighbor)
                    || (neighbor.name == "minecraft:iron_bars"
                        && native_world::collision::pane_waterlogged(neighbor).is_some())
                    || portal_full(neighbor)
                {
                    true
                } else if native_world::collision::slab_stair_solid(neighbor).is_some() {
                    let opposite = glam::IVec3::from_array(direction.map(|value| -value));
                    native_world::collision::button_support_face(neighbor, opposite)
                        .context("unsupported portal wall support face")?
                } else if is_air(neighbor)
                    || neighbor.name == "minecraft:lava"
                    || neighbor.name == "minecraft:chest"
                    || native_world::wall::Wall::from_state(neighbor).is_some()
                {
                    false
                } else {
                    bail!("unsupported portal wall neighbor: {neighbor:?}")
                };
            }
            let above = if changed.is_none() {
                neighbor.clone()
            } else {
                JigsawPlacementWorld::block(self, [pos[0], pos[1].wrapping_add(1), pos[2]])?
            };
            ensure!(
                self.inner
                    .registry
                    .is_canonical_state(&above.name, &above.properties),
                "invalid block above portal wall"
            );
            let boxes = if portal_full(&above) {
                vec![native_world::collision::Aabb {
                    min: glam::Vec3::ZERO,
                    max: glam::Vec3::ONE,
                }]
            } else {
                native_world::collision::block_shape(&above).with_context(|| {
                    format!("unsupported collision shape above portal wall: {above:?}")
                })?
            };
            let wall_post =
                native_world::wall::Wall::from_state(&above).is_some_and(|above| above.shape.up);
            let top = Above::from_collision(
                &boxes,
                wall_post,
                self.wall_tags.post_override.contains(&above.name),
            );
            let shape = native_world::wall::shape(connected, top);
            let mut next = state.clone();
            for (key, side) in ["north", "east", "south", "west"]
                .into_iter()
                .zip(shape.sides)
            {
                next.properties.insert(
                    key.into(),
                    match side {
                        Side::None => "none",
                        Side::Low => "low",
                        Side::Tall => "tall",
                    }
                    .into(),
                );
            }
            next.properties.insert("up".into(), shape.up.to_string());
            return Ok(next);
        }
        if state.name == "minecraft:chest" {
            ensure!(
                self.inner
                    .registry
                    .is_canonical_state(&neighbor.name, &neighbor.properties),
                "invalid portal chest neighbor"
            );
            ensure!(
                state.properties.get("type").map(String::as_str) == Some("single"),
                "double portal chest updates are not yet supported"
            );
            ensure!(
                direction[1] != 0
                    || !matches!(
                        neighbor.name.as_str(),
                        "minecraft:chest" | "minecraft:trapped_chest"
                    ),
                "adjacent portal chest updates are not yet supported"
            );
            if state.properties.get("waterlogged").map(String::as_str) == Some("true") {
                SpringWorld::schedule_spring_tick(
                    self.inner.region,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            return Ok(state.clone());
        }
        if let Some((_, waterlogged)) = native_world::collision::trapdoor_state(state) {
            if waterlogged {
                SpringWorld::schedule_spring_tick(
                    self.inner.region,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            return Ok(state.clone());
        }
        if native_world::collision::slab_stair_solid(state).is_some() {
            if state.properties.get("waterlogged").map(String::as_str) == Some("true") {
                SpringWorld::schedule_spring_tick(
                    self.inner.region,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            if state.name.ends_with("_slab") || direction[1] != 0 {
                return Ok(state.clone());
            }
            use native_world::stairs::{Neighbor, Stair};
            let stair = Stair::from_state(state).context("invalid portal stair")?;
            let shape = native_world::stairs::shape(stair, |offset| {
                let Some(x) = pos[0].checked_add(offset.x) else {
                    return Neighbor::Unknown;
                };
                let Some(z) = pos[2].checked_add(offset.z) else {
                    return Neighbor::Unknown;
                };
                if self.inner.region.is_outside(pos[1]) {
                    return if self
                        .inner
                        .region
                        .world
                        .chunks
                        .contains_key(&chunk([x, pos[1], z]))
                    {
                        Neighbor::Other
                    } else {
                        Neighbor::Unknown
                    };
                }
                let Ok(other) = self.inner.region.read([x, pos[1], z]) else {
                    return Neighbor::Unknown;
                };
                if !self
                    .inner
                    .registry
                    .is_canonical_state(&other.name, &other.properties)
                {
                    return Neighbor::Unknown;
                }
                if other.name.starts_with("minecraft:") && other.name.ends_with("_stairs") {
                    Stair::from_state(&other).map_or(Neighbor::Unknown, Neighbor::Stair)
                } else {
                    Neighbor::Other
                }
            })
            .context("portal stair needs unavailable neighbors")?;
            let mut next = state.clone();
            next.properties.insert("shape".into(), shape.into());
            return Ok(next);
        }
        if state.name == "minecraft:iron_bars"
            && native_world::collision::pane_waterlogged(state).is_some()
        {
            if state.properties.get("waterlogged").map(String::as_str) == Some("true") {
                SpringWorld::schedule_spring_tick(
                    self.inner.region,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            let face = match direction {
                [0, 0, -1] => Some("north"),
                [1, 0, 0] => Some("east"),
                [0, 0, 1] => Some("south"),
                [-1, 0, 0] => Some("west"),
                [0, -1, 0] | [0, 1, 0] => None,
                _ => bail!("invalid portal edge direction"),
            };
            if let Some(face) = face {
                ensure!(
                    self.inner
                        .registry
                        .is_canonical_state(&neighbor.name, &neighbor.properties),
                    "invalid portal neighbor state"
                );
                let connected = if portal_full(neighbor)
                    || native_world::collision::jigsaw_state(neighbor)
                    || self.wall_tags.walls.contains(&neighbor.name)
                    || (neighbor.name == "minecraft:iron_bars"
                        && native_world::collision::pane_waterlogged(neighbor).is_some())
                {
                    true
                } else if native_world::collision::slab_stair_solid(neighbor).is_some() {
                    let opposite = glam::IVec3::from_array(direction.map(|value| -value));
                    native_world::collision::button_support_face(neighbor, opposite)
                        .context("unsupported portal stair or slab support face")?
                } else if is_air(neighbor)
                    || neighbor.name == "minecraft:lava"
                    || neighbor.name == "minecraft:chest"
                    || native_world::wall::Wall::from_state(neighbor).is_some()
                {
                    false
                } else {
                    bail!("unsupported portal bars neighbor: {neighbor:?}")
                };
                let mut next = state.clone();
                next.properties.insert(face.into(), connected.to_string());
                return Ok(next);
            }
            return Ok(state.clone());
        }
        ensure!(
            portal_full(state)
                || is_air(state)
                || (state.name == "minecraft:dirt" && state.properties.is_empty()),
            "unsupported portal edge update: {state:?}"
        );
        Ok(state.clone())
    }
    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        ensure!(matches!(flags, 2 | 18), "unsupported portal edge flags");
        ensure!(
            flags != 18 || state.name != "minecraft:magma_block",
            "unsupported known-shape magma update"
        );
        let mut unused = DecorationRandom::new(0);
        self.place_structure_block(pos, state, None, &mut unused)?;
        Ok(())
    }
}

impl StructurePlacementWorld for PortalTemplateRegion<'_, '_, '_> {
    fn place_structure_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&native_persistence::nbt::Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        if native_world::collision::jigsaw_state(state) {
            ensure!(
                matches!(nbt, Some(native_persistence::nbt::Tag::Compound(_))),
                "permanent jigsaw blocks are unsupported"
            );
            ensure!(
                self.inner
                    .registry
                    .is_canonical_state(&state.name, &state.properties)
                    && self.inner.registry.has_block_entity(&state.name) == Some(true),
                "invalid temporary jigsaw state"
            );
            if !self.inner.region.writable.contains(&chunk(pos)) {
                return Ok(false);
            }
            ensure!(
                self.inner.region.world.chunks.contains_key(&chunk(pos))
                    && !self.inner.region.is_outside(pos[1]),
                "temporary jigsaw is outside the supported region"
            );
            ensure!(
                !self.temporary_jigsaws.contains(&pos),
                "duplicate temporary jigsaw"
            );
            let old = self.inner.region.read(pos)?;
            ensure!(
                self.inner
                    .registry
                    .is_canonical_state(&old.name, &old.properties)
                    && self.inner.registry.has_block_entity(&old.name) == Some(false)
                    && (disk_safe_solid(&old.name).is_some()
                        || portal_full(&old)
                        || native_world::collision::slab_stair_solid(&old).is_some()
                        || native_world::wall::Wall::from_state(&old).is_some()
                        || native_world::collision::pane_waterlogged(&old).is_some())
                    && !self.inner.region.monster_entities.contains_key(&pos)
                    && !self.inner.region.bee_nests.contains_key(&pos),
                "unsupported temporary jigsaw source: {old:?}"
            );
            ensure!(
                !self
                    .inner
                    .region
                    .light_guard
                    .as_ref()
                    .is_some_and(|guard| guard.registered.contains(&chunk(pos))),
                "temporary jigsaw placement in a registered light chunk is unsupported"
            );
            self.inner.region.set_proto_block(
                pos,
                &BlockState {
                    name: "minecraft:barrier".into(),
                    properties: BTreeMap::from([("waterlogged".into(), "false".into())]),
                },
            )?;
            self.inner.region.set_proto_block(pos, state)?;
            // WorldGenRegion records a DUMMY entity before the template asks
            // for it, then ProtoChunk materializes it into the live map.
            let pending = self
                .inner
                .region
                .pending_order
                .entry(chunk(pos))
                .or_default();
            Arc::make_mut(pending).insert(pos);
            Arc::make_mut(pending).remove(pos);
            Arc::make_mut(
                self.inner
                    .region
                    .entity_order
                    .entry(chunk(pos))
                    .or_default(),
            )
            .insert(pos);
            self.temporary_jigsaws.insert(pos);
            return Ok(true);
        }
        if state.name == "minecraft:chest" && nbt.is_some() {
            return self.inner.place_template_block(pos, state, nbt, random);
        }
        ensure!(nbt.is_none(), "portal block entities are not yet supported");
        if state.name == "minecraft:iron_bars"
            || native_world::collision::slab_stair_solid(state).is_some()
            || native_world::wall::Wall::from_state(state).is_some()
            || native_world::collision::trapdoor_state(state).is_some()
        {
            return self.inner.place_template_block(pos, state, None, random);
        }
        ensure!(
            portal_full(state) || is_air(state) || state.name == "minecraft:lava",
            "unsupported portal template block: {state:?}"
        );
        if !self.inner.region.writable.contains(&chunk(pos)) {
            return Ok(false);
        }
        ensure!(
            self.inner.region.world.chunks.contains_key(&chunk(pos)),
            "portal template chunk missing"
        );
        if self.inner.region.is_outside(pos[1]) {
            if state.name == "minecraft:magma_block" {
                mark_magma(self.inner.region, pos)?;
            }
            return Ok(true);
        }
        let old = self.inner.region.read(pos)?;
        ensure!(
            self.inner
                .registry
                .is_canonical_state(&old.name, &old.properties)
                && self.inner.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some()
                    || portal_full(&old)
                    || (old.name == "minecraft:iron_bars"
                        && native_world::collision::pane_waterlogged(&old).is_some()))
                && !self.inner.region.monster_entities.contains_key(&pos)
                && !self.inner.region.bee_nests.contains_key(&pos),
            "unsupported portal template source: {old:?}"
        );
        ensure!(
            self.inner
                .registry
                .is_canonical_state(&state.name, &state.properties)
                && self.inner.registry.has_block_entity(&state.name) == Some(false),
            "invalid portal template output"
        );
        self.inner.region.magma_block_write = state.name == "minecraft:magma_block";
        let result = self.inner.region.set_proto_block(pos, state);
        self.inner.region.magma_block_write = false;
        result?;
        if state.name == "minecraft:magma_block" {
            mark_magma(self.inner.region, pos)?;
        }
        Ok(true)
    }
    fn update_from_neighbor_shapes(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
    ) -> Result<BlockState> {
        let mut result = state.clone();
        for direction in [
            [-1, 0, 0],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let neighbor = std::array::from_fn(|i| pos[i].wrapping_add(direction[i]));
            let other = ShapeEdgeWorld::block(self, neighbor)?;
            result = self.update_shape(pos, &result, direction, neighbor, &other)?;
        }
        Ok(result)
    }
    fn update_neighbors_at(&mut self, _: [i32; 3], _: &BlockState) -> Result<()> {
        Ok(())
    }

    fn finalize_jigsaw_marker(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        if !self.inner.region.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.inner.region.world.chunks.contains_key(&chunk(pos))
                && !self.inner.region.is_outside(pos[1]),
            "jigsaw final state is outside the supported region"
        );
        ensure!(
            self.inner
                .registry
                .is_canonical_state(&state.name, &state.properties)
                && self.inner.registry.has_block_entity(&state.name) == Some(false)
                && (portal_full(state)
                    || super::jigsaw::jigsaw_wood(state)
                    || is_air(state)
                    || state.name == "minecraft:lava"),
            "unsupported jigsaw final state: {state:?}"
        );
        if self.temporary_jigsaws.contains(&pos) {
            let old = self.inner.region.read(pos)?;
            ensure!(
                native_world::collision::jigsaw_state(&old),
                "temporary jigsaw was replaced"
            );
            self.inner.region.magma_block_write = state.name == "minecraft:magma_block";
            let result = self.inner.region.set_proto_block(pos, state);
            self.inner.region.magma_block_write = false;
            result?;
            if let Some(order) = self.inner.region.entity_order.get_mut(&chunk(pos)) {
                Arc::make_mut(order).remove(pos);
            }
            if let Some(pending) = self.inner.region.pending_order.get_mut(&chunk(pos)) {
                Arc::make_mut(pending).remove(pos);
            }
            if state.name == "minecraft:magma_block" {
                mark_magma(self.inner.region, pos)?;
            }
            self.temporary_jigsaws.remove(&pos);
        } else {
            let mut unused = DecorationRandom::new(0);
            if super::jigsaw::jigsaw_wood(state) {
                self.inner
                    .place_template_block(pos, state, None, &mut unused)?;
            } else {
                self.place_structure_block(pos, state, None, &mut unused)?;
            }
        }
        Ok(())
    }

    fn finish_jigsaw_markers(&mut self) -> Result<()> {
        ensure!(
            self.temporary_jigsaws.is_empty(),
            "temporary jigsaw survived marker finalization"
        );
        Ok(())
    }
}

struct PortalRegion<'r, 'a, 'g> {
    region: &'r mut DecorationRegion<'a>,
    registry: &'g GeneratedBlockRegistry,
    protected: &'g BTreeSet<String>,
}

fn mark_magma(region: &mut DecorationRegion<'_>, pos: [i32; 3]) -> Result<()> {
    DiskWorld::mark_postprocessing(region, [pos[0], pos[1].wrapping_add(1), pos[2]])
}

impl PortalDecorationWorld for PortalRegion<'_, '_, '_> {
    type State = BlockState;

    fn block(&mut self, position: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self.region, position)
    }
    fn id<'a>(&self, state: &'a BlockState) -> &'a str {
        &state.name
    }
    fn is_air(&self, state: &BlockState) -> bool {
        super::is_air(state)
    }
    fn is_protected(&self, state: &BlockState) -> Result<bool> {
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties),
            "unverified portal block state: {state:?}"
        );
        Ok(self.protected.contains(&state.name))
    }
    fn height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
        match map {
            Heightmap::WorldSurface => self.region.world_surface_height(x, z),
            Heightmap::OceanFloor => OreVeinWorld::ocean_floor_height(self.region, x, z),
        }
    }
    fn collision_face_full(&mut self, pos: [i32; 3], direction: [i32; 3]) -> Result<bool> {
        let state = self.block(pos)?;
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties),
            "unverified portal collision state"
        );
        if portal_full(&state) {
            return Ok(true);
        }
        let shape = native_world::collision::block_shape(&state)
            .with_context(|| format!("unsupported portal collision shape for {}", state.name))?;
        Ok(native_world::collision::covers_full_face(
            &shape,
            glam::IVec3::from_array(direction),
        ))
    }
    fn set_block(&mut self, pos: [i32; 3], block: PortalBlock) -> Result<()> {
        if !self.region.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.region.world.chunks.contains_key(&chunk(pos)),
            "portal block chunk missing"
        );
        if self.region.is_outside(pos[1]) {
            if block == PortalBlock::Magma {
                mark_magma(self.region, pos)?;
            }
            return Ok(());
        }
        let old = self.region.read(pos)?;
        let portal_state = matches!(
            old.name.as_str(),
            "minecraft:gold_block"
                | "minecraft:obsidian"
                | "minecraft:crying_obsidian"
                | "minecraft:stone_bricks"
                | "minecraft:cracked_stone_bricks"
                | "minecraft:mossy_stone_bricks"
                | "minecraft:stone_brick_stairs"
                | "minecraft:mossy_stone_brick_stairs"
                | "minecraft:stone_slab"
                | "minecraft:stone_brick_slab"
                | "minecraft:mossy_stone_brick_slab"
                | "minecraft:stone_brick_wall"
                | "minecraft:mossy_stone_brick_wall"
                | "minecraft:iron_bars"
                | "minecraft:vine"
                | "minecraft:jungle_leaves"
        );
        ensure!(
            self.registry.is_canonical_state(&old.name, &old.properties)
                && self.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some()
                    || portal_state
                    || native_world::collision::slab_stair_solid(&old).is_some()
                    || native_world::wall::Wall::from_state(&old).is_some())
                && !self.region.monster_entities.contains_key(&pos)
                && !self.region.bee_nests.contains_key(&pos),
            "unsupported portal replacement source at {pos:?}: {old:?}"
        );
        let (name, properties) = match block {
            PortalBlock::Netherrack => ("minecraft:netherrack", BTreeMap::new()),
            PortalBlock::Magma => ("minecraft:magma_block", BTreeMap::new()),
            PortalBlock::PersistentJungleLeaves => (
                "minecraft:jungle_leaves",
                BTreeMap::from([
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "true".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            ),
            PortalBlock::Vine(direction) => {
                let face = match direction {
                    [0, 0, -1] => "north",
                    [1, 0, 0] => "east",
                    [0, 0, 1] => "south",
                    [-1, 0, 0] => "west",
                    _ => bail!("invalid portal vine face"),
                };
                let mut props: BTreeMap<String, String> = ["up", "north", "south", "east", "west"]
                    .into_iter()
                    .map(|name| (name.into(), "false".into()))
                    .collect();
                props.insert(face.into(), "true".into());
                ("minecraft:vine", props)
            }
        };
        let state = BlockState {
            name: name.into(),
            properties,
        };
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties)
                && self.registry.has_block_entity(&state.name) == Some(false),
            "unverified portal output state"
        );
        self.region.magma_block_write = block == PortalBlock::Magma;
        let result = self.region.set_proto_block(pos, &state);
        self.region.magma_block_write = false;
        result?;
        if block == PortalBlock::Magma {
            mark_magma(self.region, pos)?;
        }
        Ok(())
    }
}

impl DecorationRegion<'_> {
    /// Place and decorate a portal whose center lies in this decoration clip.
    /// Failures invalidate the entire unpublished region and preserve inputs.
    pub fn place_ruined_portal(
        &mut self,
        start: &super::super::ruined_portal::PreparedStart,
        registry: &GeneratedBlockRegistry,
        processors: &native_persistence::structure_processors::PreparedProcessors,
        protected: &BTreeSet<String>,
        wall_tags: &PortalUpdateTags,
        clip: &mut native_core::beardifier::BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        ensure!(
            (0..3).all(|i| clip.min[i] <= clip.max[i]),
            "invalid portal clip"
        );
        let bounds = start.piece.bounds;
        let mut center = [0; 3];
        for axis in 0..3 {
            let span = bounds.max[axis]
                .checked_sub(bounds.min[axis])
                .and_then(|v| v.checked_add(1))
                .context("portal bounds overflow")?;
            ensure!(span > 0, "invalid portal bounds");
            center[axis] = bounds.min[axis]
                .checked_add(span / 2)
                .context("portal center overflow")?;
        }
        if !(0..3).all(|i| (clip.min[i]..=clip.max[i]).contains(&center[i])) {
            return Ok(false);
        }
        let mut next_clip = *clip;
        for axis in 0..3 {
            next_clip.min[axis] = next_clip.min[axis].min(bounds.min[axis]);
            next_clip.max[axis] = next_clip.max[axis].max(bounds.max[axis]);
        }
        let mut next_random = random.clone();
        self.failed = true;
        start.place_template(
            &mut PortalTemplateRegion {
                inner: super::jigsaw::JigsawRegion {
                    region: self,
                    registry,
                },
                wall_tags,
                temporary_jigsaws: BTreeSet::new(),
            },
            registry,
            processors,
            next_clip,
            portal_template_full,
            &mut next_random,
        )?;
        start.piece.decorate(
            &mut PortalRegion {
                region: self,
                registry,
                protected,
            },
            &mut next_random,
        )?;
        *clip = next_clip;
        *random = next_random;
        self.failed = false;
        Ok(true)
    }

    /// Run the piece's post-template decoration on this discardable region.
    /// `protected` must be resolved from the same active block registry and packs.
    pub fn decorate_ruined_portal(
        &mut self,
        piece: &super::super::ruined_portal::SavedRuinedPortalPiece,
        registry: &GeneratedBlockRegistry,
        protected: &BTreeSet<String>,
        random: &mut DecorationRandom,
    ) -> Result<()> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        piece.decorate(
            &mut PortalRegion {
                region: self,
                registry,
                protected,
            },
            random,
        )?;
        self.failed = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::{
        heightmap_tags::{HeightmapTags, resolve_block_tag},
        packs::ResourceStack,
        ruined_portal::{GenerationPoint, InitialSelection, Setup, VerticalPlacement},
        structure_transform::{Mirror, Rotation, bounding_box},
    };

    fn piece() -> Result<super::super::super::ruined_portal::SavedRuinedPortalPiece> {
        let size = [5, 4, 5];
        let pos = [6, 7, 6];
        let pivot = [2, 0, 2];
        let selection = InitialSelection {
            setup_index: 0,
            air_pocket: false,
            template: ResourceId::parse("minecraft:ruined_portal/portal_1")?,
            rotation: Rotation::None,
            mirror: Mirror::None,
        };
        let setup = Setup {
            placement: VerticalPlacement::OnLandSurface,
            air_pocket_probability: 0.0,
            mossiness: 0.0,
            overgrown: false,
            vines: false,
            can_be_cold: false,
            replace_with_blackstone: false,
            weight: 1.0,
        };
        let bounds = bounding_box(size, pos, selection.mirror, selection.rotation, pivot)?;
        let point = GenerationPoint {
            position: pos,
            pivot,
            bounds,
        };
        super::super::super::ruined_portal::SavedRuinedPortalPiece::capture(
            &selection, &setup, &point, size, false,
        )
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn temporary_jigsaw_preserves_block_entity_map_history() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let wall_tags = PortalUpdateTags::load(&mut resources, &ids)?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let pos = [3, 7, 3];
        let chunk = chunk(pos);
        for i in 0..24 {
            Arc::make_mut(region.entity_order.entry(chunk).or_default()).insert([i, 0, 0]);
        }
        let mut expected_live = region.entity_order[&chunk].as_ref().clone();
        let mut expected_pending = super::super::super::entity_order::PendingPositions::default();
        expected_pending.insert(pos);
        expected_pending.remove(pos);
        expected_live.insert(pos);
        let (name, properties) = registry.resolve(&serde_json::json!("minecraft:jigsaw"))?;
        let state = BlockState { name, properties };
        let nbt = native_persistence::nbt::Tag::Compound(Vec::new());
        let mut world = PortalTemplateRegion {
            inner: super::super::jigsaw::JigsawRegion {
                region: &mut region,
                registry: &registry,
            },
            wall_tags: &wall_tags,
            temporary_jigsaws: BTreeSet::new(),
        };
        assert!(world.place_structure_block(
            pos,
            &state,
            Some(&nbt),
            &mut DecorationRandom::new(0)
        )?);
        let actual = &world.inner.region;
        assert_eq!(
            actual.entity_order[&chunk]
                .saving_order_with_pending(actual.pending_order.get(&chunk).map(AsRef::as_ref))?,
            expected_live.saving_order_with_pending(Some(&expected_pending))?
        );
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        };
        world.finalize_jigsaw_marker(pos, &stone)?;
        world.finish_jigsaw_markers()?;
        expected_live.remove(pos);
        let actual = &world.inner.region;
        assert_eq!(
            actual.entity_order[&chunk]
                .saving_order_with_pending(actual.pending_order.get(&chunk).map(AsRef::as_ref))?,
            expected_live.saving_order_with_pending(Some(&expected_pending))?
        );
        let (name, properties) = registry.resolve(&serde_json::json!("minecraft:oak_planks"))?;
        let wood = BlockState { name, properties };
        world.finalize_jigsaw_marker(pos, &wood)?;
        assert_eq!(world.inner.region.read(pos)?, wood);
        world.finish_jigsaw_markers()?;
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_portal_templates_place_and_finalize_jigsaw_markers() -> Result<()> {
        use super::super::super::ruined_portal::{PreparedStart, SavedRuinedPortalPiece};
        use super::super::{MixedOceanRuinPlacement, MixedPortalPlacementContext};
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_persistence::structure_template::StructureTemplate;
        use native_resources::structure_decoration::StructureDecorationEntry;
        use native_world::loaded::{BLOCK_COUNT, BlockStateId, Section};

        #[derive(Clone)]
        struct NoOther;
        impl MixedOceanRuinPlacement for NoOther {
            fn place(
                &mut self,
                _: &mut DecorationRegion<'_>,
                _: &mut super::super::super::ocean_ruin::PreparedStart,
                _: BoundingBox,
                _: &mut DecorationRandom,
            ) -> Result<()> {
                anyhow::bail!("unexpected ocean ruin")
            }
        }

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let portal_resources = PortalPlacementResources::load(&mut resources, &ids, &registry)?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let mut placed_count = 0;
        let mut marker_count = 0;
        for name in (1..=10)
            .map(|n| format!("portal_{n}"))
            .chain((1..=3).map(|n| format!("giant_portal_{n}")))
        {
            let id = ResourceId::parse(&format!("minecraft:ruined_portal/{name}"))?;
            let template =
                StructureTemplate::load(&mut resources, &id)?.context("missing portal template")?;
            let selection = InitialSelection {
                setup_index: 0,
                air_pocket: false,
                template: id,
                rotation: Rotation::None,
                mirror: Mirror::None,
            };
            let setup = Setup {
                placement: VerticalPlacement::OnLandSurface,
                air_pocket_probability: 0.0,
                mossiness: 0.8,
                overgrown: false,
                vines: false,
                can_be_cold: false,
                replace_with_blackstone: false,
                weight: 1.0,
            };
            let position = [0, 8, 0];
            let pivot = [template.size[0] / 2, 0, template.size[2] / 2];
            let bounds = bounding_box(
                template.size,
                position,
                selection.mirror,
                selection.rotation,
                pivot,
            )?;
            let point = GenerationPoint {
                position,
                pivot,
                bounds,
            };
            let piece =
                SavedRuinedPortalPiece::capture(&selection, &setup, &point, template.size, false)?;
            let start = PreparedStart {
                chunk: [0, 0],
                point,
                terrain_adjustment: TerrainAdjustment::None,
                template: Arc::new(template),
                piece,
            };
            let prepared = portal_resources.prepare(&mut resources, &start)?;
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            terrain.height = 48;
            let world = Arc::make_mut(&mut terrain.world);
            let air = world.intern_state(BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            });
            let base = world.chunks[&ChunkPos { x: 0, z: 0 }].as_ref().clone();
            for z in -3..=3 {
                for x in -3..=3 {
                    let position = ChunkPos { x, z };
                    let mut chunk = base.clone();
                    chunk.position = position;
                    for y in 1..3 {
                        chunk.sections.insert(
                            y,
                            Arc::new(Section {
                                blocks: Some(Box::new([BlockStateId(air.0); BLOCK_COUNT])),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        );
                    }
                    world.chunks.insert(position, Arc::new(chunk));
                }
            }
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut clip =
                BoundingBox::new([-32, 0, -32], [31, 47, 31]).map_err(anyhow::Error::msg)?;
            let mut random = DecorationRandom::new(42);
            let markers = start.jigsaw_markers(&registry, clip)?;
            let mut stale = start.clone();
            stale.chunk[0] += 1;
            let old_clip = clip;
            let mut expected_random = random.clone();
            assert!(
                prepared
                    .place(&mut region, &stale, &mut clip, &mut random)
                    .is_err()
            );
            assert_eq!(clip, old_clip);
            assert_eq!(random.next_long(), expected_random.next_long());
            let mut random = DecorationRandom::new(42);
            if name == "portal_1" {
                let entry = StructureDecorationEntry {
                    id: ResourceId::parse("minecraft:ruined_portal")?,
                    step: 0,
                    index: 0,
                };
                let plan = [super::super::super::structure_batch::PlannedMixedStructureDecoration {
                    entry: &entry, starts: vec![super::super::super::structure_batch::PlannedStructureStart::RuinedPortal(&start)] }];
                let mut context = MixedPortalPlacementContext::load(
                    NoOther,
                    &plan,
                    &mut resources,
                    &ids,
                    &registry,
                )?;
                let mut placed_start = start.clone();
                context.place_ruined_portal(
                    &mut region,
                    &(entry.id.clone(), ChunkPos { x: 0, z: 0 }),
                    &mut placed_start,
                    clip,
                    &mut random,
                )?;
            } else {
                let placed = prepared.place(&mut region, &start, &mut clip, &mut random);
                ensure!(placed.with_context(|| format!("placing {name}"))?);
            }
            region
                .finish()
                .with_context(|| format!("finishing {name}"))?;
            for marker in &markers {
                let [x, y, z] = marker.position;
                ensure!(
                    terrain
                        .snapshot()
                        .block(x, y, z)
                        .context("marker block missing")?
                        .name
                        != "minecraft:jigsaw",
                    "{name} retained a temporary jigsaw"
                );
            }
            marker_count += markers.len();
            placed_count += 1;
        }
        assert_eq!(placed_count, 13);
        assert!(marker_count >= 5);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn portal_template_and_decoration_share_one_transaction() -> Result<()> {
        use super::super::super::ruined_portal::PreparedStart;
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_persistence::{
            nbt::{NamedTag, Tag},
            structure_template::{StructureTemplate, TemplateBlock},
        };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let wall_tags = PortalUpdateTags::load(&mut resources, &ids)?;
        let protected = resolve_block_tag(
            &mut resources,
            &ResourceId::parse("minecraft:features_cannot_replace")?,
            &ids,
        )?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let piece = piece()?;
        let processors = native_persistence::structure_processors::PreparedProcessors::load(
            &native_resources::structure_pool::HolderValue::Inline(
                serde_json::json!({"processors":[]}),
            ),
            &mut resources,
            &ids,
            &registry,
        )?;
        let template = StructureTemplate {
            document: NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![]),
            },
            data_version: Some(5023),
            size: [5, 4, 5],
            palettes: vec![vec![
                Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:obsidian".into()),
                )]),
                Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:iron_bars".into()),
                )]),
                Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:stone_brick_stairs".into()),
                )]),
                Tag::Compound(vec![
                    (
                        "id".into(),
                        Tag::String("minecraft:stone_brick_stairs".into()),
                    ),
                    (
                        "properties".into(),
                        Tag::Compound(vec![("facing".into(), Tag::String("east".into()))]),
                    ),
                ]),
                Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:stone_slab".into()),
                )]),
                Tag::Compound(vec![("id".into(), Tag::String("minecraft:chest".into()))]),
                Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:stone_brick_wall".into()),
                )]),
                Tag::Compound(vec![("id".into(), Tag::String("minecraft:stone".into()))]),
            ]],
            blocks: vec![
                TemplateBlock {
                    position: [0, 1, 0],
                    state: 0,
                    nbt: None,
                },
                TemplateBlock {
                    position: [1, 1, 0],
                    state: 1,
                    nbt: None,
                },
                TemplateBlock {
                    position: [2, 1, 1],
                    state: 2,
                    nbt: None,
                },
                TemplateBlock {
                    position: [2, 1, 0],
                    state: 3,
                    nbt: None,
                },
                TemplateBlock {
                    position: [0, 1, 1],
                    state: 4,
                    nbt: None,
                },
                TemplateBlock {
                    position: [0, 1, 2],
                    state: 6,
                    nbt: None,
                },
                TemplateBlock {
                    position: [1, 1, 2],
                    state: 6,
                    nbt: None,
                },
                TemplateBlock {
                    position: [1, 2, 2],
                    state: 7,
                    nbt: None,
                },
                TemplateBlock {
                    position: [1, 1, 3],
                    state: 1,
                    nbt: None,
                },
                TemplateBlock {
                    position: [3, 1, 0],
                    state: 5,
                    nbt: Some(Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:chest".into())),
                        (
                            "LootTable".into(),
                            Tag::String("minecraft:chests/ruined_portal".into()),
                        ),
                    ])),
                },
            ],
            entities: vec![],
        };
        let point = GenerationPoint {
            position: piece.position,
            pivot: [2, 0, 2],
            bounds: piece.bounds,
        };
        let start = PreparedStart {
            chunk: [0, 0],
            point,
            terrain_adjustment: TerrainAdjustment::None,
            template: Arc::new(template),
            piece,
        };
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut clip = BoundingBox::new([8, 9, 8], [8, 9, 8]).unwrap();
        let mut random = DecorationRandom::new(42);
        let expected_loot_seed = random.clone().next_long();
        assert!(region.place_ruined_portal(
            &start,
            &registry,
            &processors,
            &protected,
            &wall_tags,
            &mut clip,
            &mut random
        )?);
        assert_eq!(clip, start.piece.bounds);
        region.finish()?;
        let world = terrain.snapshot();
        assert!(matches!(
            world.block(6, 8, 6).unwrap().name.as_str(),
            "minecraft:obsidian" | "minecraft:crying_obsidian"
        ));
        let bars = world.block(7, 8, 6).unwrap();
        assert_eq!(bars.name, "minecraft:iron_bars");
        assert_eq!(bars.properties["west"], "true");
        let stair = world.block(8, 8, 7).unwrap();
        assert_eq!(stair.name, "minecraft:stone_brick_stairs");
        assert_eq!(stair.properties["shape"], "outer_right");
        assert_eq!(world.block(6, 8, 7).unwrap().name, "minecraft:stone_slab");
        let left_wall = world.block(6, 8, 8).unwrap();
        assert_eq!(left_wall.name, "minecraft:stone_brick_wall");
        assert_eq!(left_wall.properties["east"], "low");
        let right_wall = world.block(7, 8, 8).unwrap();
        assert_eq!(right_wall.properties["west"], "tall");
        assert_eq!(right_wall.properties["south"], "tall");
        assert_eq!(right_wall.properties["up"], "true");
        assert_eq!(world.block(7, 8, 9).unwrap().properties["north"], "true");
        assert_eq!(world.block(9, 8, 6).unwrap().name, "minecraft:chest");
        let Tag::List { values, .. } =
            terrain.generated_block_entity_data(ChunkPos { x: 0, z: 0 }, &registry)?
        else {
            bail!("missing portal block entities")
        };
        assert_eq!(values.len(), 1);
        assert!(
            matches!(values[0].get("LootTable"), Some(Tag::String(id)) if id.matches("minecraft:chests/ruined_portal"))
        );
        assert_eq!(
            values[0].get("LootTableSeed"),
            Some(&Tag::Long(expected_loot_seed))
        );
        assert!(
            (-6..=22).any(
                |x| (-6..=22).any(|z| world.block(x, 7, z).is_some_and(|s| matches!(
                    s.name.as_str(),
                    "minecraft:netherrack" | "minecraft:magma_block"
                )))
            )
        );

        // Entity anchors outside Java's expanded clip must not prevent the
        // block pass; an intersecting entity is still rejected transactionally.
        for (anchor, accepted) in [([50, 1, 0], true), ([0, 1, 0], false)] {
            let mut with_entity = start.clone();
            Arc::make_mut(&mut with_entity.template)
                .entities
                .push(Tag::Compound(vec![
                    (
                        "blockPos".into(),
                        Tag::List {
                            element_type: native_persistence::nbt::TagType::Int,
                            values: anchor.into_iter().map(Tag::Int).collect(),
                        },
                    ),
                    (
                        "pos".into(),
                        Tag::List {
                            element_type: native_persistence::nbt::TagType::Double,
                            values: anchor
                                .into_iter()
                                .map(|v| Tag::Double(f64::from(v) + 0.5))
                                .collect(),
                        },
                    ),
                    (
                        "nbt".into(),
                        Tag::Compound(vec![("id".into(), Tag::String("minecraft:pig".into()))]),
                    ),
                ]));
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let before = terrain.snapshot();
            let writable = before.chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut clip = BoundingBox::new([8, 9, 8], [8, 9, 8]).unwrap();
            let mut random = DecorationRandom::new(42);
            let mut expected = random.clone();
            let result = region.place_ruined_portal(
                &with_entity,
                &registry,
                &processors,
                &protected,
                &wall_tags,
                &mut clip,
                &mut random,
            );
            if accepted {
                assert!(result?);
                region.finish()?;
                assert!(matches!(
                    terrain.snapshot().block(6, 8, 6).unwrap().name.as_str(),
                    "minecraft:obsidian" | "minecraft:crying_obsidian"
                ));
            } else {
                assert!(
                    format!("{:#}", result.unwrap_err())
                        .contains("entity placement is unsupported")
                );
                assert_eq!(random.next_long(), expected.next_long());
                assert!(region.finish().is_err());
                assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
            }
        }

        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut outside = BoundingBox::new([0, 0, 0], [1, 1, 1]).unwrap();
        let old_clip = outside;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        assert!(!region.place_ruined_portal(
            &start,
            &registry,
            &processors,
            &protected,
            &wall_tags,
            &mut outside,
            &mut random
        )?);
        assert_eq!(outside, old_clip);
        assert_eq!(random.next_long(), expected.next_long());
        region.finish()?;
        assert_eq!(
            terrain.snapshot().block(6, 8, 6).unwrap().name,
            "minecraft:air"
        );
        let mut unsupported = start.clone();
        let template = Arc::make_mut(&mut unsupported.template);
        let Some(Tag::Compound(fields)) = &mut template.blocks.last_mut().unwrap().nbt else {
            bail!("missing chest data")
        };
        fields.push(("unsupported".into(), Tag::Int(1)));
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut clip = BoundingBox::new([8, 9, 8], [8, 9, 8]).unwrap();
        let old_clip = clip;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        assert!(
            region
                .place_ruined_portal(
                    &unsupported,
                    &registry,
                    &processors,
                    &protected,
                    &wall_tags,
                    &mut clip,
                    &mut random
                )
                .is_err()
        );
        assert_eq!(clip, old_clip);
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn magma_marks_and_waterlogged_template_ticks_are_recorded() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let wall_tags = PortalUpdateTags::load(&mut resources, &ids)?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let protected = BTreeSet::new();
        {
            let mut world = PortalRegion {
                region: &mut region,
                registry: &registry,
                protected: &protected,
            };
            PortalDecorationWorld::set_block(&mut world, [4, 7, 4], PortalBlock::Magma)?;
            PortalDecorationWorld::set_block(&mut world, [4, 7, 4], PortalBlock::Magma)?;
        }
        {
            let mut world = PortalTemplateRegion {
                inner: super::super::jigsaw::JigsawRegion {
                    region: &mut region,
                    registry: &registry,
                },
                wall_tags: &wall_tags,
                temporary_jigsaws: BTreeSet::new(),
            };
            let state = BlockState {
                name: "minecraft:magma_block".into(),
                properties: BTreeMap::new(),
            };
            world.place_structure_block([5, 7, 4], &state, None, &mut DecorationRandom::new(0))?;
        }
        let marks = &region.feature_marks[&ChunkPos { x: 0, z: 0 }];
        assert_eq!(marks.sections()[0], [0x484, 0x484, 0x485]);
        let (name, mut properties) = registry.resolve(&serde_json::json!("minecraft:iron_bars"))?;
        properties.insert("waterlogged".into(), "true".into());
        let bars = BlockState { name, properties };
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        };
        {
            let mut world = PortalTemplateRegion {
                inner: super::super::jigsaw::JigsawRegion {
                    region: &mut region,
                    registry: &registry,
                },
                wall_tags: &wall_tags,
                temporary_jigsaws: BTreeSet::new(),
            };
            for _ in 0..2 {
                assert_eq!(
                    world.update_shape([6, 8, 4], &bars, [0, 1, 0], [6, 9, 4], &air)?,
                    bars
                );
            }
            for (name, pos) in [
                ("minecraft:stone_brick_stairs", [7, 8, 4]),
                ("minecraft:stone_slab", [8, 8, 4]),
                ("minecraft:chest", [9, 8, 4]),
                ("minecraft:stone_brick_wall", [10, 8, 4]),
                ("minecraft:dark_oak_trapdoor", [11, 8, 4]),
            ] {
                let (name, mut properties) = registry.resolve(&serde_json::json!(name))?;
                properties.insert("waterlogged".into(), "true".into());
                let state = BlockState { name, properties };
                for _ in 0..2 {
                    assert_eq!(
                        world.update_shape(pos, &state, [0, 1, 0], [pos[0], 9, pos[2]], &air)?,
                        state
                    );
                }
                if state.name == "minecraft:chest" {
                    assert!(
                        world
                            .update_shape(pos, &state, [1, 0, 0], [10, 8, 4], &state)
                            .is_err()
                    );
                }
            }
        }
        let ticks = &region.world.saved_fluid_ticks[&ChunkPos { x: 0, z: 0 }];
        for pos in [
            [6, 8, 4],
            [7, 8, 4],
            [8, 8, 4],
            [9, 8, 4],
            [10, 8, 4],
            [11, 8, 4],
        ] {
            assert_eq!(
                ticks
                    .iter()
                    .filter(|tick| tick.position == pos && tick.fluid == "minecraft:water")
                    .count(),
                1
            );
        }
        let mut custom_tags = PortalUpdateTags {
            walls: wall_tags.walls.clone(),
            post_override: wall_tags.post_override.clone(),
            bubble_fluids: wall_tags.bubble_fluids.clone(),
            bubble_drag: wall_tags.bubble_drag.clone(),
            bubble_push: wall_tags.bubble_push.clone(),
            fence_tags: Arc::clone(&wall_tags.fence_tags),
        };
        custom_tags.walls.insert("minecraft:air".into());
        custom_tags.post_override.insert("minecraft:air".into());
        custom_tags.bubble_fluids.insert("minecraft:lava".into());
        custom_tags.bubble_drag.insert("minecraft:stone".into());
        let (name, properties) =
            registry.resolve(&serde_json::json!("minecraft:stone_brick_wall"))?;
        let wall = BlockState { name, properties };
        let mut world = PortalTemplateRegion {
            inner: super::super::jigsaw::JigsawRegion {
                region: &mut region,
                registry: &registry,
            },
            wall_tags: &custom_tags,
            temporary_jigsaws: BTreeSet::new(),
        };
        let connected = world.update_shape([11, 8, 4], &wall, [1, 0, 0], [12, 8, 4], &air)?;
        assert_eq!(connected.properties["east"], "low");
        let bars_connected = world.update_shape([11, 8, 4], &bars, [1, 0, 0], [12, 8, 4], &air)?;
        assert_eq!(bars_connected.properties["east"], "true");
        let mut straight = wall;
        straight.properties.insert("north".into(), "low".into());
        straight.properties.insert("south".into(), "low".into());
        straight.properties.insert("up".into(), "false".into());
        let post = world.update_shape([11, 8, 4], &straight, [0, 1, 0], [11, 9, 4], &air)?;
        assert_eq!(post.properties["up"], "true");
        let (name, properties) = registry.resolve(&serde_json::json!("minecraft:lava"))?;
        let lava = BlockState { name, properties };
        world.place_structure_block([11, 8, 3], &lava, None, &mut DecorationRandom::new(0))?;
        world.update_shape([11, 8, 3], &lava, [1, 0, 0], [12, 8, 3], &air)?;
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        };
        world.update_shape([11, 8, 3], &lava, [0, -1, 0], [11, 7, 3], &stone)?;
        let mut flowing = lava.clone();
        flowing.properties.insert("level".into(), "1".into());
        let (name, properties) = registry.resolve(&serde_json::json!("minecraft:water"))?;
        let water = BlockState { name, properties };
        world.update_shape([12, 8, 3], &flowing, [1, 0, 0], [13, 8, 3], &water)?;
        let ticks = &region.world.saved_fluid_ticks[&ChunkPos { x: 0, z: 0 }];
        assert!(
            ticks
                .iter()
                .any(|tick| tick.position == [11, 8, 3] && tick.fluid == "minecraft:lava")
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.position == [12, 8, 3] && tick.fluid == "minecraft:flowing_lava")
        );
        let block_ticks = &region.world.saved_block_ticks[&ChunkPos { x: 0, z: 0 }];
        assert!(
            block_ticks
                .iter()
                .any(|tick| tick.position == [11, 8, 3] && tick.block == "minecraft:lava")
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn portal_decoration_updates_draft_and_rolls_back_unsupported_entity() -> Result<()> {
        use native_world::loaded::BlockStateId;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let protected = resolve_block_tag(
            &mut resources,
            &ResourceId::parse("minecraft:features_cannot_replace")?,
            &ids,
        )?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        region.decorate_ruined_portal(&piece()?, &registry, &protected, &mut random)?;
        assert_eq!(region.world_surface_height(8, 8)?, 8);
        region.finish()?;
        assert!((-6..=22).any(|x| (-6..=22).any(|z| {
            terrain.snapshot().block(x, 7, z).is_some_and(|state| {
                matches!(
                    state.name.as_str(),
                    "minecraft:netherrack" | "minecraft:magma_block"
                )
            })
        })));

        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let chest = BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        };
        let id: BlockStateId = world.intern_state(chest);
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[8 + 8 * 16 + 7 * 256] = id;
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        // A data pack may remove chests from the protected tag. The world
        // adapter must then reject an unsupported block-entity replacement.
        let mut unprotected = protected.clone();
        assert!(unprotected.remove("minecraft:chest"));
        assert!(
            region
                .decorate_ruined_portal(&piece()?, &registry, &unprotected, &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }
}
