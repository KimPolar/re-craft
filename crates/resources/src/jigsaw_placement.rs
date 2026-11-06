//! Placement geometry and free space used by Java's jigsaw assembler.
//!
//! Template lookup, height sampling and block placement are supplied by callers.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;

use crate::{
    jigsaw::{Connector, shuffle_by_selection_priority},
    packs::ResourceId,
    structure_pool::{PoolElement, PoolFallback, Projection, TemplatePool},
    structure_transform::Rotation,
};

/// An integer-aligned box minus full piece boxes. Java tests a candidate after
/// deflating its box by 0.25 blocks before subtracting the full accepted box.
/// For integer-aligned boxes this is equivalent to inclusive block intersection.
#[derive(Clone, Debug)]
pub struct FreeSpace {
    bounds: BoundingBox,
    occupied: Vec<BoundingBox>,
}

impl FreeSpace {
    pub fn new(bounds: BoundingBox) -> Result<Self> {
        validate_box(bounds)?;
        Ok(Self {
            bounds,
            occupied: Vec::new(),
        })
    }

    /// Subtract a box even when it is only partly inside the outer bounds.
    pub fn subtract(&mut self, occupied: BoundingBox) -> Result<()> {
        validate_box(occupied)?;
        self.occupied.push(occupied);
        Ok(())
    }

    pub fn can_place(&self, candidate: BoundingBox) -> Result<bool> {
        validate_box(candidate)?;
        Ok((0..3).all(|i| {
            candidate.min[i] >= self.bounds.min[i] && candidate.max[i] <= self.bounds.max[i]
        }) && self.occupied.iter().all(|other| {
            (0..3).any(|i| candidate.max[i] < other.min[i] || candidate.min[i] > other.max[i])
        }))
    }

    /// Reserve only successful candidates; failed candidates leave the space intact.
    pub fn place(&mut self, candidate: BoundingBox) -> Result<bool> {
        if !self.can_place(candidate)? {
            return Ok(false);
        }
        self.occupied.push(candidate);
        Ok(true)
    }
}

fn validate_box(bounds: BoundingBox) -> Result<()> {
    ensure!(
        (0..3).all(|i| bounds.min[i] <= bounds.max[i]),
        "invalid jigsaw bounds"
    );
    // AABB.of performs max + 1 in Java i32 before converting to double. At
    // i32::MAX that wraps and is not an integer-aligned inclusive box anymore.
    ensure!(
        bounds.max.iter().all(|&value| value != i32::MAX),
        "jigsaw bounds exceed supported AABB coordinates"
    );
    Ok(())
}

/// Java's highest-priority-first queue retains insertion order within a level.
/// Its exhausted-queue scan excludes i32::MIN, matching 26.3's iterator.
pub struct PlacementQueue<T> {
    levels: BTreeMap<i32, VecDeque<T>>,
    highest: i32,
    current: Option<i32>,
    finished: bool,
}

impl<T> Default for PlacementQueue<T> {
    fn default() -> Self {
        Self {
            levels: BTreeMap::new(),
            highest: i32::MIN,
            current: None,
            finished: false,
        }
    }
}

impl<T> PlacementQueue<T> {
    pub fn push(&mut self, item: T, priority: i32) {
        self.levels.entry(priority).or_default().push_back(item);
        if self.current.is_none() || priority >= self.highest {
            self.highest = priority;
            self.current = Some(priority);
        }
    }
}

impl<T> Iterator for PlacementQueue<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.finished {
            return None;
        }
        let Some(priority) = self.current else {
            self.finished = true;
            return None;
        };
        let queue = self.levels.get_mut(&priority).expect("active queue exists");
        let result = queue.pop_front();
        if queue.is_empty() {
            self.current = self.levels.iter().rev().find_map(|(&priority, queue)| {
                (priority > i32::MIN && !queue.is_empty()).then_some(priority)
            });
            self.highest = self.current.unwrap_or(i32::MIN);
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Junction {
    pub source_x: i32,
    pub source_ground_y: i32,
    pub source_z: i32,
    pub delta_y: i32,
    pub destination_projection: Projection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachedGeometry {
    pub origin: [i32; 3],
    pub bounds: BoundingBox,
    pub ground_level_delta: i32,
    pub source_junction: Junction,
    pub target_junction: Junction,
}

/// Inputs are already rotated: the target connector and target bounds are at
/// origin zero, while the source connector and source bounds are world-space.
pub struct Attachment<'a> {
    pub source: &'a Connector,
    pub target: &'a Connector,
    pub source_bounds: BoundingBox,
    pub target_bounds_at_origin: BoundingBox,
    pub source_projection: Projection,
    pub target_projection: Projection,
    pub source_ground_level_delta: i32,
    pub target_ground_level_delta: i32,
    /// WORLD_SURFACE_WG first-free height at the source connector. Required
    /// when either piece uses terrain matching.
    pub surface_height: Option<i32>,
    /// Maximum child pool height selected by Java's optional expansion hack.
    pub expansion: i32,
}

impl Attachment<'_> {
    pub fn geometry(&self) -> Result<Option<AttachedGeometry>> {
        if !self.source.can_attach(self.target) {
            return Ok(None);
        }
        validate_box(self.source_bounds)?;
        validate_box(self.target_bounds_at_origin)?;
        ensure!(self.expansion >= 0, "negative jigsaw expansion");
        let target_pos = self.source.adjacent_position();
        let mut origin =
            std::array::from_fn(|i| target_pos[i].wrapping_sub(self.target.position[i]));
        let source_local_y = self.source.position[1].wrapping_sub(self.source_bounds.min[1]);
        let target_local_y = self.target.position[1];
        let delta_y = source_local_y
            .wrapping_sub(target_local_y)
            .wrapping_add(self.source.orientation.front.step()[1]);
        let source_rigid = self.source_projection == Projection::Rigid;
        let target_rigid = self.target_projection == Projection::Rigid;
        let target_y = if source_rigid && target_rigid {
            self.source_bounds.min[1].wrapping_add(delta_y)
        } else {
            self.surface_height
                .context("jigsaw surface height is missing")?
                .wrapping_sub(target_local_y)
        };
        let raw_min_y = self.target_bounds_at_origin.min[1].wrapping_add(origin[1]);
        let offset_y = target_y.wrapping_sub(raw_min_y);
        origin[1] = origin[1].wrapping_add(offset_y);
        let min =
            std::array::from_fn(|i| self.target_bounds_at_origin.min[i].wrapping_add(origin[i]));
        let mut max =
            std::array::from_fn(|i| self.target_bounds_at_origin.max[i].wrapping_add(origin[i]));
        if self.expansion > 0 {
            let new_size = self
                .expansion
                .wrapping_add(1)
                .max(max[1].wrapping_sub(min[1]));
            max[1] = max[1].max(min[1].wrapping_add(new_size));
        }
        let bounds = BoundingBox::new(min, max).map_err(anyhow::Error::msg)?;
        let ground_level_delta = if target_rigid {
            self.source_ground_level_delta.wrapping_sub(delta_y)
        } else {
            self.target_ground_level_delta
        };
        let junction_y = if source_rigid {
            self.source_bounds.min[1].wrapping_add(source_local_y)
        } else if target_rigid {
            target_y.wrapping_add(target_local_y)
        } else {
            self.surface_height
                .context("jigsaw surface height is missing")?
                .wrapping_add(delta_y / 2)
        };
        let source_junction = Junction {
            source_x: target_pos[0],
            source_ground_y: junction_y
                .wrapping_sub(source_local_y)
                .wrapping_add(self.source_ground_level_delta),
            source_z: target_pos[2],
            delta_y,
            destination_projection: self.target_projection,
        };
        let target_junction = Junction {
            source_x: self.source.position[0],
            source_ground_y: junction_y
                .wrapping_sub(target_local_y)
                .wrapping_add(ground_level_delta),
            source_z: self.source.position[2],
            delta_y: delta_y.wrapping_neg(),
            destination_projection: self.source_projection,
        };
        Ok(Some(AttachedGeometry {
            origin,
            bounds,
            ground_level_delta,
            source_junction,
            target_junction,
        }))
    }
}

/// A generated structure piece before block placement or serialization.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub element: PoolElement,
    pub origin: [i32; 3],
    pub rotation: Rotation,
    pub bounds: BoundingBox,
    pub ground_level_delta: i32,
    pub junctions: Vec<Junction>,
}

/// World-specific inputs for jigsaw assembly. Connector lists returned here
/// must be in Java template order; the assembler performs the shuffle itself.
/// Feature elements require Java's unrotated synthetic connector.
pub trait AssemblyContext {
    fn pool(&mut self, id: &ResourceId) -> Result<Option<Arc<TemplatePool>>>;
    fn connectors(
        &mut self,
        element: &PoolElement,
        origin: [i32; 3],
        rotation: Rotation,
    ) -> Result<Vec<Connector>>;
    fn bounds(
        &mut self,
        element: &PoolElement,
        origin: [i32; 3],
        rotation: Rotation,
    ) -> Result<BoundingBox>;
    fn surface_height(&mut self, position: [i32; 3]) -> Result<i32>;
    fn resolve_alias(&mut self, id: &ResourceId) -> Result<ResourceId> {
        Ok(id.clone())
    }
}

/// A missing padding field uses Java's shared ZERO instance. An explicitly
/// decoded zero is a distinct instance and still checks the dimension limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DimensionPadding {
    DefaultZero,
    Explicit { bottom: i32, top: i32 },
}

impl DimensionPadding {
    fn values(self) -> (i32, i32) {
        match self {
            Self::DefaultZero => (0, 0),
            Self::Explicit { bottom, top } => (bottom, top),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartHeightmap {
    WorldSurfaceWg,
    WorldSurface,
    OceanFloorWg,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

/// The biome precheck and terrain height are kept separate so a rejected
/// column never causes a height sample.
pub trait StartEnvironment {
    fn valid_column(&mut self, x: i32, z: i32, min_y: i32, max_y: i32) -> Result<bool>;
    fn first_free_height(&mut self, x: i32, z: i32, heightmap: StartHeightmap) -> Result<i32>;
}

pub struct StartOptions<'a> {
    pub position: [i32; 3],
    /// Only registry-backed holders participate in start-pool alias lookup.
    pub pool_key: Option<&'a ResourceId>,
    pub start_name: Option<&'a ResourceId>,
    pub project_to_heightmap: Option<StartHeightmap>,
    pub min_y: i32,
    /// Inclusive maximum Y, as returned by Java's LevelHeightAccessor.
    pub max_y: i32,
    pub padding: DimensionPadding,
    pub max_depth: i32,
    pub horizontal_distance: i32,
    pub vertical_distance: i32,
    pub do_expansion_hack: bool,
}

/// Corresponds to Java's GenerationStub before its lazy piece builder runs.
pub struct PreparedStart {
    pub position: [i32; 3],
    pub root: Piece,
    max_depth: i32,
    horizontal_distance: i32,
    vertical_distance: i32,
    min_y: i32,
    max_y: i32,
    padding: DimensionPadding,
    do_expansion_hack: bool,
}

/// Select and align the center piece, consuming randomness in Java order.
/// A missing alias falls back to the original holder, as it does in 26.3.
pub fn prepare_start(
    context: &mut impl AssemblyContext,
    environment: &mut impl StartEnvironment,
    original_pool: &TemplatePool,
    options: StartOptions<'_>,
    random: &mut impl FnMut(i32) -> Option<i32>,
) -> Result<Option<PreparedStart>> {
    ensure!(options.max_depth >= 0, "negative jigsaw depth");
    ensure!(options.min_y <= options.max_y, "invalid dimension height");
    ensure!(
        options.horizontal_distance >= 0 && options.vertical_distance >= 0,
        "negative jigsaw distance"
    );
    let (bottom, top) = options.padding.values();
    ensure!(bottom >= 0 && top >= 0, "negative dimension padding");
    let rotation = match random(4).context("invalid rotation random bound")? {
        0 => Rotation::None,
        1 => Rotation::Clockwise90,
        2 => Rotation::Clockwise180,
        3 => Rotation::Counterclockwise90,
        _ => anyhow::bail!("rotation random result out of bounds"),
    };
    let aliased_pool = if let Some(key) = options.pool_key {
        let alias = context.resolve_alias(key)?;
        context.pool(&alias)?
    } else {
        None
    };
    let pool = aliased_pool.as_deref().unwrap_or(original_pool);
    let Some(element) = pool.random_element(random)? else {
        return Ok(None);
    };
    if matches!(element, PoolElement::Empty) {
        return Ok(None);
    }
    let anchor = if let Some(name) = options.start_name {
        let mut connectors = context.connectors(element, options.position, rotation)?;
        shuffle_by_selection_priority(&mut connectors, random)?;
        let Some(connector) = connectors
            .iter()
            .find(|connector| connector.name.as_ref() == Some(name))
        else {
            return Ok(None);
        };
        connector.position
    } else {
        options.position
    };
    let local_anchor: [i32; 3] =
        std::array::from_fn(|i| anchor[i].wrapping_sub(options.position[i]));
    let mut origin: [i32; 3] =
        std::array::from_fn(|i| options.position[i].wrapping_sub(local_anchor[i]));
    let mut bounds = context.bounds(element, origin, rotation)?;
    let center_x = bounds.max[0].wrapping_add(bounds.min[0]) / 2;
    let center_z = bounds.max[2].wrapping_add(bounds.min[2]) / 2;
    let ground_y = if let Some(heightmap) = options.project_to_heightmap {
        if !environment.valid_column(center_x, center_z, options.min_y, options.max_y)? {
            return Ok(None);
        }
        options.position[1]
            .wrapping_add(environment.first_free_height(center_x, center_z, heightmap)?)
    } else {
        origin[1]
    };
    let move_y = ground_y.wrapping_sub(bounds.min[1].wrapping_add(1));
    origin[1] = origin[1].wrapping_add(move_y);
    bounds.min[1] = bounds.min[1].wrapping_add(move_y);
    bounds.max[1] = bounds.max[1].wrapping_add(move_y);
    if matches!(options.padding, DimensionPadding::Explicit { .. })
        && (bounds.min[1] < options.min_y.wrapping_add(bottom)
            || bounds.max[1] > options.max_y.wrapping_sub(top))
    {
        return Ok(None);
    }
    Ok(Some(PreparedStart {
        position: [center_x, ground_y.wrapping_add(local_anchor[1]), center_z],
        root: Piece {
            element: element.clone(),
            origin,
            rotation,
            bounds,
            ground_level_delta: 1,
            junctions: vec![],
        },
        max_depth: options.max_depth,
        horizontal_distance: options.horizontal_distance,
        vertical_distance: options.vertical_distance,
        min_y: options.min_y,
        max_y: options.max_y,
        padding: options.padding,
        do_expansion_hack: options.do_expansion_hack,
    }))
}

impl PreparedStart {
    /// Execute Java's lazy piece builder after the caller validates the biome.
    pub fn assemble(
        self,
        context: &mut impl AssemblyContext,
        random: &mut impl FnMut(i32) -> Option<i32>,
    ) -> Result<Vec<Piece>> {
        if self.max_depth == 0 {
            // 26.3 returns before copying even the root into the builder.
            return Ok(Vec::new());
        }
        let [x, y, z] = self.position;
        let (bottom, top) = self.padding.values();
        let low_y = y
            .wrapping_sub(self.vertical_distance)
            .max(self.min_y.wrapping_add(bottom));
        let high_y_exclusive = y
            .wrapping_add(self.vertical_distance)
            .wrapping_add(1)
            .min(self.max_y.wrapping_add(1).wrapping_sub(top));
        // AABB normalizes its endpoints, including an inverted vertical range.
        let low = [
            x.wrapping_sub(self.horizontal_distance),
            low_y,
            z.wrapping_sub(self.horizontal_distance),
        ];
        let high = [
            x.wrapping_add(self.horizontal_distance).wrapping_add(1),
            high_y_exclusive,
            z.wrapping_add(self.horizontal_distance).wrapping_add(1),
        ];
        let min = std::array::from_fn(|i| low[i].min(high[i]));
        let max = std::array::from_fn(|i| low[i].max(high[i]).wrapping_sub(1));
        let bounds = BoundingBox::new(min, max).map_err(anyhow::Error::msg)?;
        assemble(
            context,
            self.root,
            bounds,
            self.max_depth,
            self.do_expansion_hack,
            random,
        )
    }
}

fn element_projection(element: &PoolElement) -> Result<Projection> {
    match element {
        PoolElement::Single { projection, .. }
        | PoolElement::Feature { projection, .. }
        | PoolElement::List { projection, .. } => Ok(*projection),
        PoolElement::Empty => anyhow::bail!("empty pool element has no piece"),
    }
}

fn fallback(
    context: &mut impl AssemblyContext,
    pool: &TemplatePool,
) -> Result<(Arc<TemplatePool>, bool)> {
    match &pool.fallback {
        PoolFallback::Reference(id) => {
            let loaded = context
                .pool(id)?
                .with_context(|| format!("missing fallback pool {}:{}", id.namespace, id.path))?;
            Ok((loaded, id.namespace == "minecraft" && id.path == "empty"))
        }
        PoolFallback::Inline(pool) => Ok((Arc::new((**pool).clone()), false)),
    }
}

fn pool_max_height(context: &mut impl AssemblyContext, pool: &TemplatePool) -> Result<i32> {
    let mut height = 0;
    for (element, _) in pool.entries() {
        if matches!(element, PoolElement::Empty) {
            continue;
        }
        let bounds = context.bounds(element, [0; 3], Rotation::None)?;
        validate_box(bounds)?;
        height = height.max(bounds.max[1].wrapping_sub(bounds.min[1]).wrapping_add(1));
    }
    Ok(height)
}

fn expansion_height(
    context: &mut impl AssemblyContext,
    target_bounds: BoundingBox,
    connectors: &[Connector],
) -> Result<i32> {
    let mut height = 0;
    for connector in connectors {
        let adjacent = connector.adjacent_position();
        if !(0..3)
            .all(|i| adjacent[i] >= target_bounds.min[i] && adjacent[i] <= target_bounds.max[i])
        {
            continue;
        }
        let id = context.resolve_alias(&connector.pool)?;
        if let Some(pool) = context.pool(&id)? {
            let (other, _) = fallback(context, &pool)?;
            height = height
                .max(pool_max_height(context, &pool)?)
                .max(pool_max_height(context, &other)?);
        }
    }
    Ok(height)
}

fn shuffle_rotations(random: &mut impl FnMut(i32) -> Option<i32>) -> Result<[Rotation; 4]> {
    let mut rotations = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::Counterclockwise90,
    ];
    for end in (2..=4).rev() {
        let index = random(end).context("invalid rotation random bound")?;
        ensure!(
            (0..end).contains(&index),
            "rotation random result out of bounds"
        );
        rotations.swap((end - 1) as usize, index as usize);
    }
    Ok(rotations)
}

/// Assemble children of an already prepared center piece. The random source
/// and context are consumed in Java order; callers that need rollback must
/// stage their mutable context and random source before calling this function.
pub fn assemble(
    context: &mut impl AssemblyContext,
    root: Piece,
    outer_bounds: BoundingBox,
    max_depth: i32,
    do_expansion_hack: bool,
    random: &mut impl FnMut(i32) -> Option<i32>,
) -> Result<Vec<Piece>> {
    ensure!(max_depth >= 0, "negative jigsaw depth");
    validate_box(root.bounds)?;
    element_projection(&root.element)?;
    let mut pieces = vec![root];
    if max_depth == 0 {
        return Ok(pieces);
    }
    let mut outer = FreeSpace::new(outer_bounds)?;
    outer.subtract(pieces[0].bounds)?;
    let mut spaces = vec![outer];
    let mut queue = PlacementQueue::default();
    // Java processes the root directly, then drains its priority queue.
    let mut next = Some((0usize, 0usize, 0i32));
    while let Some((source_index, context_space, depth)) = next {
        let source_piece = pieces[source_index].clone();
        let source_projection = element_projection(&source_piece.element)?;
        let mut source_jigsaws = context.connectors(
            &source_piece.element,
            source_piece.origin,
            source_piece.rotation,
        )?;
        shuffle_by_selection_priority(&mut source_jigsaws, random)?;
        let mut inside_space = None;
        for source in source_jigsaws {
            let pool_id = context.resolve_alias(&source.pool)?;
            let Some(pool) = context.pool(&pool_id)? else {
                continue;
            };
            if pool.weighted_len() == 0
                && !(pool_id.namespace == "minecraft" && pool_id.path == "empty")
            {
                continue;
            }
            let (other, other_is_empty_id) = fallback(context, &pool)?;
            if other.weighted_len() == 0 && !other_is_empty_id {
                continue;
            }
            let adjacent = source.adjacent_position();
            let inside = (0..3).all(|i| {
                adjacent[i] >= source_piece.bounds.min[i]
                    && adjacent[i] <= source_piece.bounds.max[i]
            });
            let free_index = if inside {
                *inside_space.get_or_insert_with(|| {
                    spaces.push(
                        FreeSpace::new(source_piece.bounds).expect("source bounds were validated"),
                    );
                    spaces.len() - 1
                })
            } else {
                context_space
            };
            let mut candidates = Vec::new();
            if depth != max_depth {
                candidates.extend(pool.shuffled_elements(random)?);
            }
            candidates.extend(other.shuffled_elements(random)?);
            let mut cached_height = None;
            'candidates: for element in candidates {
                if matches!(element, PoolElement::Empty) {
                    break;
                }
                for rotation in shuffle_rotations(random)? {
                    let mut targets = context.connectors(element, [0; 3], rotation)?;
                    shuffle_by_selection_priority(&mut targets, random)?;
                    let target_bounds = context.bounds(element, [0; 3], rotation)?;
                    validate_box(target_bounds)?;
                    let y_span = target_bounds.max[1]
                        .wrapping_sub(target_bounds.min[1])
                        .wrapping_add(1);
                    let expansion = if do_expansion_hack && y_span <= 16 {
                        expansion_height(context, target_bounds, &targets)?
                    } else {
                        0
                    };
                    for target in &targets {
                        if !source.can_attach(target) {
                            continue;
                        }
                        let target_projection = element_projection(element)?;
                        let surface_height = if source_projection == Projection::Rigid
                            && target_projection == Projection::Rigid
                        {
                            None
                        } else {
                            let height = match cached_height {
                                Some(value) => value,
                                None => context.surface_height(source.position)?,
                            };
                            if height != i32::MIN {
                                cached_height = Some(height);
                            }
                            Some(height)
                        };
                        let geometry = Attachment {
                            source: &source,
                            target,
                            source_bounds: source_piece.bounds,
                            target_bounds_at_origin: target_bounds,
                            source_projection,
                            target_projection,
                            source_ground_level_delta: source_piece.ground_level_delta,
                            target_ground_level_delta: 1,
                            surface_height,
                            expansion,
                        }
                        .geometry()?
                        .context("matching jigsaw connectors changed")?;
                        if !spaces[free_index].place(geometry.bounds)? {
                            continue;
                        }
                        pieces[source_index]
                            .junctions
                            .push(geometry.source_junction);
                        let child_index = pieces.len();
                        pieces.push(Piece {
                            element: element.clone(),
                            origin: geometry.origin,
                            rotation,
                            bounds: geometry.bounds,
                            ground_level_delta: geometry.ground_level_delta,
                            junctions: vec![geometry.target_junction],
                        });
                        if depth + 1 <= max_depth {
                            queue.push(
                                (child_index, free_index, depth + 1),
                                source.placement_priority,
                            );
                        }
                        break 'candidates;
                    }
                }
            }
        }
        next = queue.next();
    }
    Ok(pieces)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jigsaw::{Joint, Orientation},
        packs::ResourceId,
        structure_transform::{self, Mirror, Rotation},
    };

    fn bounds(min: [i32; 3], max: [i32; 3]) -> BoundingBox {
        BoundingBox::new(min, max).unwrap()
    }
    fn connector(position: [i32; 3], orientation: &str, name: &str) -> Connector {
        Connector {
            position,
            orientation: Orientation::parse(orientation).unwrap(),
            joint: Joint::Aligned,
            name: Some(ResourceId::parse(name).unwrap()),
            target: ResourceId::parse("example:door").unwrap(),
            pool: ResourceId::parse("minecraft:empty").unwrap(),
            placement_priority: 0,
            selection_priority: 0,
        }
    }

    struct GraphFixture {
        pools: BTreeMap<ResourceId, Arc<TemplatePool>>,
    }

    impl GraphFixture {
        fn new() -> Result<Self> {
            use serde_json::json;
            let single = |name: &str| json!({"element_type":"minecraft:single_pool_element","location":format!("example:{name}"),"processors":"minecraft:empty","projection":"rigid"});
            let pools = [
                (
                    "minecraft:empty",
                    json!({"fallback":"minecraft:empty","elements":[]}),
                ),
                (
                    "example:terminators",
                    json!({"fallback":"minecraft:empty","elements":[{"weight":1,"element":single("end")}]}),
                ),
                (
                    "example:children",
                    json!({"fallback":"example:terminators","elements":[{"weight":2,"element":single("a")},{"weight":1,"element":single("b")},{"weight":1,"element":{"element_type":"minecraft:empty_pool_element"}}]}),
                ),
            ];
            Ok(Self {
                pools: pools
                    .into_iter()
                    .map(|(id, value)| {
                        Ok((
                            ResourceId::parse(id)?,
                            Arc::new(TemplatePool::decode(&value)?),
                        ))
                    })
                    .collect::<Result<_>>()?,
            })
        }
        fn element(&self, name: &str) -> PoolElement {
            self.pools[&ResourceId::parse(if name == "end" {
                "example:terminators"
            } else {
                "example:children"
            })
            .unwrap()]
                .entries()
                .iter()
                .find_map(|(element, _)| match element {
                    PoolElement::Single { location, .. } if location.path == name => {
                        Some(element.clone())
                    }
                    _ => None,
                })
                .unwrap()
        }
    }
    impl AssemblyContext for GraphFixture {
        fn pool(&mut self, id: &ResourceId) -> Result<Option<Arc<TemplatePool>>> {
            Ok(self.pools.get(id).cloned())
        }
        fn connectors(
            &mut self,
            element: &PoolElement,
            origin: [i32; 3],
            rotation: Rotation,
        ) -> Result<Vec<Connector>> {
            let PoolElement::Single { location, .. } = element else {
                anyhow::bail!("unexpected fixture element")
            };
            let size_x = match location.path.as_str() {
                "a" => 3,
                "b" => 2,
                "end" => 1,
                _ => anyhow::bail!("unknown fixture element"),
            };
            let positions = [[size_x - 1, 1, 1], [0, 1, 1], [1, 1, 0], [1, 1, 2]];
            let directions = ["east_up", "west_up", "north_up", "south_up"];
            let placement = [3, 0, 2, 1];
            let selection = [0, 2, 0, 1];
            let count = if location.path == "end" { 1 } else { 4 };
            Ok((0..count)
                .map(|i| {
                    let mut value = connector(positions[i], directions[i], "example:door");
                    value.pool = ResourceId::parse(if location.path == "end" {
                        "minecraft:empty"
                    } else {
                        "example:children"
                    })
                    .unwrap();
                    value.placement_priority = placement[i];
                    value.selection_priority = selection[i];
                    value.transformed(origin, rotation)
                })
                .collect())
        }
        fn bounds(
            &mut self,
            element: &PoolElement,
            origin: [i32; 3],
            rotation: Rotation,
        ) -> Result<BoundingBox> {
            let PoolElement::Single { location, .. } = element else {
                anyhow::bail!("unexpected fixture element")
            };
            let size_x = match location.path.as_str() {
                "a" => 3,
                "b" => 2,
                "end" => 1,
                _ => anyhow::bail!("unknown fixture element"),
            };
            structure_transform::bounding_box(
                [size_x, 3, 3],
                origin,
                Mirror::None,
                rotation,
                [0; 3],
            )
        }
        fn surface_height(&mut self, _: [i32; 3]) -> Result<i32> {
            anyhow::bail!("rigid fixture unexpectedly sampled height")
        }
    }

    struct StartFixture {
        graph: GraphFixture,
        mode: i32,
    }

    impl AssemblyContext for StartFixture {
        fn pool(&mut self, id: &ResourceId) -> Result<Option<Arc<TemplatePool>>> {
            self.graph.pool(id)
        }
        fn connectors(
            &mut self,
            element: &PoolElement,
            origin: [i32; 3],
            rotation: Rotation,
        ) -> Result<Vec<Connector>> {
            self.graph.connectors(element, origin, rotation)
        }
        fn bounds(
            &mut self,
            element: &PoolElement,
            origin: [i32; 3],
            rotation: Rotation,
        ) -> Result<BoundingBox> {
            self.graph.bounds(element, origin, rotation)
        }
        fn surface_height(&mut self, position: [i32; 3]) -> Result<i32> {
            self.graph.surface_height(position)
        }
        fn resolve_alias(&mut self, id: &ResourceId) -> Result<ResourceId> {
            match self.mode {
                8 | 10 => ResourceId::parse("example:terminators"),
                9 => ResourceId::parse("example:missing"),
                _ => Ok(id.clone()),
            }
        }
    }

    struct StartTerrain {
        mode: i32,
        height_calls: usize,
        column_calls: usize,
    }
    impl StartEnvironment for StartTerrain {
        fn valid_column(&mut self, _: i32, _: i32, min_y: i32, max_y: i32) -> Result<bool> {
            assert_eq!((min_y, max_y), (-64, 319));
            self.column_calls += 1;
            Ok(self.mode != 4)
        }
        fn first_free_height(&mut self, _: i32, _: i32, heightmap: StartHeightmap) -> Result<i32> {
            assert_eq!(heightmap, StartHeightmap::WorldSurfaceWg);
            self.height_calls += 1;
            Ok(70)
        }
    }

    fn start_line(mode: i32, seed: i64) -> Result<String> {
        use native_core::random::LegacyRandom;
        let mut fixture = StartFixture {
            graph: GraphFixture::new()?,
            mode,
        };
        let mut terrain = StartTerrain {
            mode,
            height_calls: 0,
            column_calls: 0,
        };
        let children = ResourceId::parse("example:children")?;
        let empty = ResourceId::parse("minecraft:empty")?;
        let name = ResourceId::parse(if mode == 2 {
            "example:missing"
        } else {
            "example:door"
        })?;
        let pool = fixture.graph.pools[if mode == 11 { &empty } else { &children }].clone();
        let mut random = LegacyRandom::new(seed);
        let result = prepare_start(
            &mut fixture,
            &mut terrain,
            &pool,
            StartOptions {
                position: [-5, if mode == 5 || mode == 6 { -100 } else { 30 }, -8],
                pool_key: if mode == 10 {
                    None
                } else if mode == 11 {
                    Some(&empty)
                } else {
                    Some(&children)
                },
                start_name: if mode == 1 || mode == 2 {
                    Some(&name)
                } else {
                    None
                },
                project_to_heightmap: if mode == 3 || mode == 4 {
                    Some(StartHeightmap::WorldSurfaceWg)
                } else {
                    None
                },
                min_y: -64,
                max_y: 319,
                padding: match mode {
                    6 => DimensionPadding::Explicit { bottom: 0, top: 0 },
                    7 => DimensionPadding::Explicit { bottom: 40, top: 1 },
                    _ => DimensionPadding::DefaultZero,
                },
                max_depth: if mode >= 12 { mode - 11 } else { 0 },
                horizontal_distance: 16,
                vertical_distance: 12,
                do_expansion_hack: false,
            },
            &mut |n| random.next_int_bound(n),
        )?;
        if result.is_some() && mode == 3 {
            assert_eq!((terrain.column_calls, terrain.height_calls), (1, 1));
        }
        if mode == 4 {
            assert_eq!(terrain.height_calls, 0);
        }
        let Some(result) = result else {
            return Ok(format!("C\t{mode}\t{seed}\t-\t{}", random.next_long()));
        };
        let piece = result.root.clone();
        let position = result.position;
        let pieces = result.assemble(&mut fixture, &mut |n| random.next_int_bound(n))?;
        if mode < 12 {
            assert!(pieces.is_empty());
        }
        let mut graph = String::new();
        for entry in pieces {
            let PoolElement::Single { location, .. } = &entry.element else {
                anyhow::bail!("unexpected graph element");
            };
            let rotation = match entry.rotation {
                Rotation::None => 0,
                Rotation::Clockwise90 => 1,
                Rotation::Clockwise180 => 2,
                Rotation::Counterclockwise90 => 3,
            };
            graph.push_str(&format!(
                "|{}:{},{},{},{},{},{},{},{},{},{},{},{}",
                location.namespace,
                location.path,
                rotation,
                entry.origin[0],
                entry.origin[1],
                entry.origin[2],
                entry.bounds.min[0],
                entry.bounds.min[1],
                entry.bounds.min[2],
                entry.bounds.max[0],
                entry.bounds.max[1],
                entry.bounds.max[2],
                entry.ground_level_delta
            ));
            for j in entry.junctions {
                let projection = match j.destination_projection {
                    Projection::Rigid => "rigid",
                    Projection::TerrainMatching => "terrain_matching",
                };
                graph.push_str(&format!(
                    ";{},{},{},{},{}",
                    j.source_x, j.source_ground_y, j.source_z, j.delta_y, projection
                ));
            }
        }
        let PoolElement::Single { location, .. } = &piece.element else {
            anyhow::bail!("unexpected start element");
        };
        let rotation = match piece.rotation {
            Rotation::None => 0,
            Rotation::Clockwise90 => 1,
            Rotation::Clockwise180 => 2,
            Rotation::Counterclockwise90 => 3,
        };
        Ok(format!(
            "C\t{mode}\t{seed}\t{}:{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}{graph}",
            location.namespace,
            location.path,
            rotation,
            piece.origin[0],
            piece.origin[1],
            piece.origin[2],
            piece.bounds.min[0],
            piece.bounds.min[1],
            piece.bounds.min[2],
            piece.bounds.max[0],
            piece.bounds.max[1],
            piece.bounds.max[2],
            position[0],
            position[1],
            position[2],
            piece.ground_level_delta,
            random.next_long()
        ))
    }

    #[test]
    fn start_padding_and_missing_anchor() -> Result<()> {
        for seed in 0..16 {
            assert!(start_line(2, seed)?.contains("\t-\t"));
            assert!(start_line(6, seed)?.contains("\t-\t"));
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn start_selection_and_random_state_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let _verified = crate::packs::ResourceStack::open(&game)?;
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
        for (name, source) in [
            (
                "JigsawGraphOracle.java",
                include_str!("../tests/java/JigsawGraphOracle.java"),
            ),
            (
                "JigsawPlacementOracle.java",
                include_str!("../tests/java/JigsawPlacementOracle.java"),
            ),
            (
                "JigsawStartOracle.java",
                include_str!("../tests/java/JigsawStartOracle.java"),
            ),
        ] {
            std::fs::write(temp.path().join(name), source)?;
        }
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .args([
                temp.path().join("JigsawGraphOracle.java"),
                temp.path().join("JigsawPlacementOracle.java"),
                temp.path().join("JigsawStartOracle.java"),
            ])
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawStartOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let mut count = 0;
        for line in String::from_utf8(output.stdout)?
            .lines()
            .filter(|line| line.starts_with("C\t"))
        {
            let mut fields = line.split('\t').skip(1);
            let mode = fields.next().context("missing mode")?.parse()?;
            let seed = fields.next().context("missing seed")?.parse()?;
            assert_eq!(start_line(mode, seed)?, line);
            count += 1;
        }
        assert_eq!(count, 112);
        Ok(())
    }

    fn graph_lines(depth: i32, seed: i64) -> Result<VecDeque<String>> {
        use native_core::random::LegacyRandom;
        let mut fixture = GraphFixture::new()?;
        let element = fixture.element("a");
        let origin = [0, 30, 0];
        let root = Piece {
            bounds: fixture.bounds(&element, origin, Rotation::None)?,
            element,
            origin,
            rotation: Rotation::None,
            ground_level_delta: 1,
            junctions: vec![],
        };
        let mut random = LegacyRandom::new(seed);
        let pieces = assemble(
            &mut fixture,
            root,
            bounds([-9, 20, -9], [9, 40, 9]),
            depth,
            false,
            &mut |n| random.next_int_bound(n),
        )?;
        let mut result = VecDeque::new();
        result.push_back(format!(
            "C\t{depth}\t{seed}\t{}\t{}",
            pieces.len(),
            random.next_long()
        ));
        for piece in pieces {
            let PoolElement::Single { location, .. } = &piece.element else {
                anyhow::bail!("unexpected output piece")
            };
            let rotation = match piece.rotation {
                Rotation::None => 0,
                Rotation::Clockwise90 => 1,
                Rotation::Clockwise180 => 2,
                Rotation::Counterclockwise90 => 3,
            };
            result.push_back(format!(
                "P\t{}:{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                location.namespace,
                location.path,
                rotation,
                piece.origin[0],
                piece.origin[1],
                piece.origin[2],
                piece.bounds.min[0],
                piece.bounds.min[1],
                piece.bounds.min[2],
                piece.bounds.max[0],
                piece.bounds.max[1],
                piece.bounds.max[2],
                piece.ground_level_delta,
                piece.junctions.len()
            ));
            for j in piece.junctions {
                let projection = match j.destination_projection {
                    Projection::Rigid => "rigid",
                    Projection::TerrainMatching => "terrain_matching",
                };
                result.push_back(format!(
                    "J\t{}\t{}\t{}\t{}\t{projection}",
                    j.source_x, j.source_ground_y, j.source_z, j.delta_y
                ));
            }
        }
        Ok(result)
    }

    #[test]
    fn free_space_and_priority_are_transactional() -> Result<()> {
        let mut space = FreeSpace::new(bounds([-5; 3], [5; 3]))?;
        space.subtract(bounds([0; 3], [1; 3]))?;
        assert!(!space.place(bounds([1; 3], [2; 3]))?);
        assert!(space.place(bounds([2; 3], [3; 3]))?);
        assert!(!space.place(bounds([3; 3], [4; 3]))?);
        assert!(!space.place(bounds([5; 3], [6; 3]))?);
        assert!(FreeSpace::new(bounds([0; 3], [i32::MAX; 3])).is_err());
        let mut queue = PlacementQueue::default();
        queue.push('a', 2);
        queue.push('b', 2);
        queue.push('c', 5);
        assert_eq!(queue.next(), Some('c'));
        queue.push('d', 3);
        assert_eq!(queue.collect::<String>(), "dab");
        Ok(())
    }

    #[test]
    fn terrain_matching_requires_height_and_preserves_junctions() -> Result<()> {
        let source = connector([4, 12, 2], "east_up", "example:parent");
        let target = connector([0, 1, 2], "west_up", "example:door");
        let mut input = Attachment {
            source: &source,
            target: &target,
            source_bounds: bounds([0, 10, 0], [4, 14, 4]),
            target_bounds_at_origin: bounds([0, 0, 0], [3, 2, 4]),
            source_projection: Projection::TerrainMatching,
            target_projection: Projection::TerrainMatching,
            source_ground_level_delta: 1,
            target_ground_level_delta: 2,
            surface_height: None,
            expansion: 0,
        };
        assert!(input.geometry().is_err());
        input.surface_height = Some(70);
        let result = input.geometry()?.unwrap();
        assert_eq!(result.origin, [5, 69, 0]);
        assert_eq!(result.ground_level_delta, 2);
        assert_eq!(result.source_junction.source_ground_y, 69);
        assert_eq!(result.target_junction.source_ground_y, 71);
        input.expansion = 5;
        assert_eq!(input.geometry()?.unwrap().bounds.max[1], 75);
        Ok(())
    }

    #[test]
    fn assembly_is_deterministic_and_depth_zero_keeps_only_root() -> Result<()> {
        assert_eq!(graph_lines(2, 42)?, graph_lines(2, 42)?);
        let only_root = graph_lines(0, 42)?;
        assert!(only_root.front().unwrap().starts_with("C\t0\t42\t1\t"));
        assert_eq!(only_root.len(), 2);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn synthetic_piece_graphs_and_random_state_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let _verified = crate::packs::ResourceStack::open(&game)?;
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
        let source = temp.path().join("JigsawGraphOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawGraphOracle.java"),
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
            .arg("JigsawGraphOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut expected = VecDeque::new();
        let mut cases = 0;
        for line in String::from_utf8(output.stdout)?.lines() {
            if let Some(rest) = line.strip_prefix("C\t") {
                ensure!(expected.is_empty(), "previous graph output was incomplete");
                let fields: Vec<_> = rest.split('\t').collect();
                ensure!(fields.len() == 4, "invalid Java graph header");
                expected = graph_lines(fields[0].parse()?, fields[1].parse()?)?;
                cases += 1;
            }
            if line.starts_with("C\t") || line.starts_with("P\t") || line.starts_with("J\t") {
                let rust = expected.pop_front().context("extra Java graph output")?;
                assert_eq!(rust, line, "piece graph mismatch in case {cases}");
            }
        }
        assert_eq!(cases, 24);
        assert!(expected.is_empty());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn placement_space_queue_and_geometry_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let _verified = crate::packs::ResourceStack::open(&game)?;
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
        let source = temp.path().join("JigsawPlacementOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawPlacementOracle.java"),
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
            .arg("JigsawPlacementOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut space = FreeSpace::new(bounds([-5; 3], [5; 3]))?;
        space.subtract(bounds([-1; 3], [1; 3]))?;
        let mut seen = [0usize; 3];
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["B", x, y, z, sx, sy, sz, accepted] => {
                    let min = [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?];
                    let spans = [sx.parse::<i32>()?, sy.parse::<i32>()?, sz.parse::<i32>()?];
                    let max = std::array::from_fn(|i| min[i] + spans[i]);
                    assert_eq!(space.place(bounds(min, max))?, accepted.parse::<bool>()?);
                    seen[0] += 1;
                }
                ["Q", expected] => {
                    let mut queue = PlacementQueue::default();
                    queue.push('a', i32::MIN);
                    queue.push('b', 2);
                    queue.push('c', 2);
                    let mut order = String::new();
                    order.push(queue.next().context("missing first queue item")?);
                    queue.push('d', 3);
                    queue.push('e', 2);
                    queue.push('f', -1);
                    order.extend(queue);
                    assert_eq!(order, *expected);
                    seen[1] += 1;
                }
                [
                    "G",
                    mode,
                    projection,
                    height_case,
                    _seed,
                    rotation,
                    px,
                    py,
                    pz,
                    x0,
                    y0,
                    z0,
                    x1,
                    y1,
                    z1,
                    ground,
                    jx,
                    jy,
                    jz,
                    jd,
                    kx,
                    ky,
                    kz,
                    kd,
                    _next,
                    source_count,
                    target_count,
                ] => {
                    let mode = mode.parse::<usize>()?;
                    let projection = projection.parse::<usize>()?;
                    let height_case = height_case.parse::<usize>()?;
                    let rotations = [
                        Rotation::None,
                        Rotation::Clockwise90,
                        Rotation::Clockwise180,
                        Rotation::Counterclockwise90,
                    ];
                    let rotation = rotations[rotation.parse::<usize>()?];
                    let positions = [
                        [0, 1, 2],
                        [1, 1, 0],
                        [3, 1, 2],
                        [1, 1, 4],
                        [1, 0, 2],
                        [1, 2, 2],
                    ];
                    let orientations = [
                        "west_up",
                        "north_up",
                        "east_up",
                        "south_up",
                        "down_north",
                        "up_north",
                    ];
                    let (source_pos, source_orientation, height) = match mode {
                        0..=3 => ([14, 40 + [2, 0, 4][height_case], 22], "east_up", 42),
                        4 => ([12, 44, 22], "up_north", 45),
                        _ => ([12, 40, 22], "down_north", 39),
                    };
                    let source = connector(source_pos, source_orientation, "example:parent");
                    let target = connector(positions[mode], orientations[mode], "example:door")
                        .transformed([0; 3], rotation);
                    let target_bounds = structure_transform::bounding_box(
                        [4, 3, 5],
                        [0; 3],
                        Mirror::None,
                        rotation,
                        [0; 3],
                    )?;
                    let attachment = Attachment {
                        source: &source,
                        target: &target,
                        source_bounds: bounds([10, 40, 20], [14, 44, 24]),
                        target_bounds_at_origin: target_bounds,
                        source_projection: if projection & 1 == 0 {
                            Projection::Rigid
                        } else {
                            Projection::TerrainMatching
                        },
                        target_projection: if projection & 2 == 0 {
                            Projection::Rigid
                        } else {
                            Projection::TerrainMatching
                        },
                        source_ground_level_delta: 1,
                        target_ground_level_delta: 1,
                        surface_height: Some(height),
                        expansion: 0,
                    };
                    let result = attachment
                        .geometry()?
                        .context("Java placed a mismatched connector")?;
                    assert_eq!(
                        result.origin,
                        [px.parse::<i32>()?, py.parse::<i32>()?, pz.parse::<i32>()?]
                    );
                    assert_eq!(
                        result.bounds,
                        bounds(
                            [x0.parse()?, y0.parse()?, z0.parse()?],
                            [x1.parse()?, y1.parse()?, z1.parse()?]
                        )
                    );
                    assert_eq!(result.ground_level_delta, ground.parse::<i32>()?);
                    assert_eq!(
                        [
                            result.source_junction.source_x,
                            result.source_junction.source_ground_y,
                            result.source_junction.source_z,
                            result.source_junction.delta_y
                        ],
                        [
                            jx.parse::<i32>()?,
                            jy.parse::<i32>()?,
                            jz.parse::<i32>()?,
                            jd.parse::<i32>()?
                        ]
                    );
                    assert_eq!(
                        [
                            result.target_junction.source_x,
                            result.target_junction.source_ground_y,
                            result.target_junction.source_z,
                            result.target_junction.delta_y
                        ],
                        [
                            kx.parse::<i32>()?,
                            ky.parse::<i32>()?,
                            kz.parse::<i32>()?,
                            kd.parse::<i32>()?
                        ]
                    );
                    assert_eq!((*source_count, *target_count), ("1", "1"));
                    seen[2] += 1;
                }
                _ => {}
            }
        }
        assert_eq!(seen, [240, 1, 432]);
        Ok(())
    }
}
