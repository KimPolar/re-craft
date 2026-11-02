//! Building blocks for 26.3 tree generation. These do not apply a complete
//! tree: the caller owns collision checks, providers, writes and decoration.
use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;
use std::sync::OnceLock;

use crate::{ore_feature::OreWorld, packs::ResourceId};

/// The vanilla feature-size codec bounds both scan radii and tree heights.
pub struct TreeFeatureSize {
    layers: TreeSizeLayers,
    min_clipped_height: Option<i32>,
}

enum TreeSizeLayers {
    Two {
        limit: i32,
        lower: i32,
        upper: i32,
    },
    Three {
        limit: i32,
        upper_limit: i32,
        lower: i32,
        middle: i32,
        upper: i32,
    },
}

impl TreeFeatureSize {
    pub fn decode(value: &Value) -> Result<Self> {
        let integer = |name: &str, default: i32, max: i32| -> Result<i32> {
            let number = match value.get(name) {
                Some(v) => v.as_i64().with_context(|| format!("invalid {name}"))?,
                None => i64::from(default),
            };
            ensure!((0..=i64::from(max)).contains(&number), "invalid {name}");
            Ok(number as i32)
        };
        let min_clipped_height = value
            .get("min_clipped_height")
            .map(|_| integer("min_clipped_height", 0, 80))
            .transpose()?;
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing feature size type")?,
        )?;
        let layers = match (kind.namespace.as_str(), kind.path.as_str()) {
            ("minecraft", "two_layers_feature_size") => TreeSizeLayers::Two {
                limit: integer("limit", 1, 81)?,
                lower: integer("lower_size", 0, 16)?,
                upper: integer("upper_size", 1, 16)?,
            },
            ("minecraft", "three_layers_feature_size") => TreeSizeLayers::Three {
                limit: integer("limit", 1, 80)?,
                upper_limit: integer("upper_limit", 1, 80)?,
                lower: integer("lower_size", 0, 16)?,
                middle: integer("middle_size", 1, 16)?,
                upper: integer("upper_size", 1, 16)?,
            },
            _ => bail!("unsupported tree feature size"),
        };
        Ok(Self {
            layers,
            min_clipped_height,
        })
    }

    pub fn radius_at(&self, height: i32, y: i32) -> i32 {
        match self.layers {
            TreeSizeLayers::Two {
                limit,
                lower,
                upper,
            } => {
                if y < limit {
                    lower
                } else {
                    upper
                }
            }
            TreeSizeLayers::Three {
                limit,
                upper_limit,
                lower,
                middle,
                upper,
            } => {
                if y < limit {
                    lower
                } else if y >= height - upper_limit {
                    upper
                } else {
                    middle
                }
            }
        }
    }
}

/// Tree placers may have different free-space rules; vines are checked only
/// after the placer reports that a position is free.
pub trait TreeClearanceWorld {
    fn is_free(&mut self, pos: [i32; 3]) -> Result<bool>;
    fn is_vine(&mut self, pos: [i32; 3]) -> Result<bool>;
}

/// Clearance for vanilla trunk placers backed by the world's active tags.
pub struct VanillaTreeClearance<'a, W>(pub &'a mut W);

impl<W: OreWorld> TreeClearanceWorld for VanillaTreeClearance<'_, W> {
    fn is_free(&mut self, pos: [i32; 3]) -> Result<bool> {
        static REPLACEABLE: OnceLock<ResourceId> = OnceLock::new();
        static LOGS: OnceLock<ResourceId> = OnceLock::new();
        let state = self.0.block(pos)?;
        Ok(self.0.is_air(&state)?
            || self.0.in_tag(
                &state,
                REPLACEABLE
                    .get_or_init(|| ResourceId::parse("minecraft:replaceable_by_trees").unwrap()),
            )?
            || self.0.in_tag(
                &state,
                LOGS.get_or_init(|| ResourceId::parse("minecraft:logs").unwrap()),
            )?)
    }

    fn is_vine(&mut self, pos: [i32; 3]) -> Result<bool> {
        static VINE: OnceLock<ResourceId> = OnceLock::new();
        let state = self.0.block(pos)?;
        self.0.block_is(
            &state,
            VINE.get_or_init(|| ResourceId::parse("minecraft:vine").unwrap()),
        )
    }
}

pub fn max_free_tree_height(
    world: &mut impl TreeClearanceWorld,
    origin: [i32; 3],
    height: i32,
    size: &TreeFeatureSize,
    ignore_vines: bool,
) -> Result<i32> {
    ensure!((0..=80).contains(&height), "invalid tree height");
    for y in 0..=height + 1 {
        let radius = size.radius_at(height, y);
        for x in -radius..=radius {
            for z in -radius..=radius {
                let pos = [
                    origin[0].wrapping_add(x),
                    origin[1].wrapping_add(y),
                    origin[2].wrapping_add(z),
                ];
                if !world.is_free(pos)? || (!ignore_vines && world.is_vine(pos)?) {
                    return Ok(y - 2);
                }
            }
        }
    }
    Ok(height)
}

/// Return the height used by the tree placer, or None when the tree cannot fit.
/// `max_y` is the world's inclusive upper block coordinate, as in Java 26.3.
pub fn permitted_tree_height(
    world: &mut impl TreeClearanceWorld,
    root: [i32; 3],
    trunk: [i32; 3],
    min_y: i32,
    max_y: i32,
    height: i32,
    size: &TreeFeatureSize,
    ignore_vines: bool,
) -> Result<Option<i32>> {
    ensure!((0..=80).contains(&height), "invalid tree height");
    if root[1].min(trunk[1]) < min_y.wrapping_add(1)
        || root[1].max(trunk[1]).wrapping_add(height).wrapping_add(1) > max_y.wrapping_add(1)
    {
        return Ok(None);
    }
    let free = max_free_tree_height(world, trunk, height, size, ignore_vines)?;
    if free < height && size.min_clipped_height.is_none_or(|min| free < min) {
        Ok(None)
    } else {
        Ok(Some(free))
    }
}

/// The common height calculation used by vanilla trunk placers.
pub struct TrunkHeight {
    base: i32,
    random_a: i32,
    random_b: i32,
}

impl TrunkHeight {
    pub fn new(base: i32, random_a: i32, random_b: i32) -> Result<Self> {
        ensure!((0..=32).contains(&base), "invalid trunk base height");
        ensure!((0..=24).contains(&random_a), "invalid trunk height_rand_a");
        ensure!((0..=24).contains(&random_b), "invalid trunk height_rand_b");
        Ok(Self {
            base,
            random_a,
            random_b,
        })
    }

    pub fn sample(&self, random: &mut DecorationRandom) -> i32 {
        self.base
            + random.next_int_bound(self.random_a + 1).unwrap()
            + random.next_int_bound(self.random_b + 1).unwrap()
    }
}

/// Position and dimensions supplied by a trunk placer to a foliage placer.
#[derive(Clone, Copy)]
pub struct FoliageAttachment {
    pub position: [i32; 3],
    pub radius_offset: i32,
    pub height_offset: i32,
    pub double_trunk: bool,
}

/// The straight trunk placer asks the world to prepare the soil once and then
/// attempts each log, even if an earlier attempt could not place a log.
pub fn place_straight_trunk(
    random: &mut DecorationRandom,
    origin: [i32; 3],
    height: i32,
    mut place: impl FnMut([i32; 3], bool, &mut DecorationRandom) -> Result<()>,
) -> Result<FoliageAttachment> {
    ensure!((0..=80).contains(&height), "invalid straight trunk height");
    place(
        [origin[0], origin[1].wrapping_sub(1), origin[2]],
        true,
        random,
    )?;
    for y in 0..height {
        place(
            [origin[0], origin[1].wrapping_add(y), origin[2]],
            false,
            random,
        )?;
    }
    Ok(FoliageAttachment {
        position: [origin[0], origin[1].wrapping_add(height), origin[2]],
        radius_offset: 0,
        height_offset: 0,
        double_trunk: false,
    })
}

/// Emit a bending trunk and its foliage attachments in Java's placement order.
pub(crate) fn place_bending_trunk(
    random: &mut DecorationRandom,
    origin: [i32; 3],
    height: i32,
    min_height_for_leaves: i32,
    bend_length: &crate::placed_feature::IntProvider,
    mut place: impl FnMut([i32; 3], bool, &mut DecorationRandom) -> Result<()>,
) -> Result<Vec<FoliageAttachment>> {
    ensure!((0..=80).contains(&height), "invalid bending trunk height");
    let direction = [[0, -1], [1, 0], [0, 1], [-1, 0]][random.next_int_bound(4).unwrap() as usize];
    let last = height - 1;
    let mut pos = origin;
    place([pos[0], pos[1].wrapping_sub(1), pos[2]], true, random)?;
    let mut attachments = Vec::new();
    for i in 0..=last {
        if i + 1 >= last + random.next_int_bound(2).unwrap() {
            pos[0] = pos[0].wrapping_add(direction[0]);
            pos[2] = pos[2].wrapping_add(direction[1]);
        }
        place(pos, false, random)?;
        if i >= min_height_for_leaves {
            attachments.push(FoliageAttachment {
                position: pos,
                radius_offset: 0,
                height_offset: 0,
                double_trunk: false,
            });
        }
        pos[1] = pos[1].wrapping_add(1);
    }
    for _ in 0..=bend_length.sample(random) {
        place(pos, false, random)?;
        attachments.push(FoliageAttachment {
            position: pos,
            radius_offset: 0,
            height_offset: 0,
            double_trunk: false,
        });
        pos[0] = pos[0].wrapping_add(direction[0]);
        pos[2] = pos[2].wrapping_add(direction[1]);
    }
    Ok(attachments)
}

/// Random spread ignores the sampled vertical offset but still consumes it in
/// the caller, before drawing the six coordinates for each placement attempt.
pub fn place_random_spread_foliage(
    random: &mut DecorationRandom,
    attachment: FoliageAttachment,
    foliage_height: i32,
    radius: i32,
    attempts: i32,
    mut place: impl FnMut([i32; 3], &mut DecorationRandom) -> Result<()>,
) -> Result<()> {
    ensure!(
        (1..=512).contains(&foliage_height),
        "invalid random spread foliage height"
    );
    ensure!(
        (0..=16).contains(&radius) && (0..=256).contains(&attempts),
        "invalid random spread foliage configuration"
    );
    // Java's nextInt(0) fails if the data pack requests attempts with radius 0.
    ensure!(
        radius > 0 || attempts == 0,
        "random spread foliage radius is zero"
    );
    for _ in 0..attempts {
        let x = random.next_int_bound(radius).unwrap() - random.next_int_bound(radius).unwrap();
        let y = random.next_int_bound(foliage_height).unwrap()
            - random.next_int_bound(foliage_height).unwrap();
        let z = random.next_int_bound(radius).unwrap() - random.next_int_bound(radius).unwrap();
        place(
            [
                attachment.position[0].wrapping_add(x),
                attachment.position[1].wrapping_add(y),
                attachment.position[2].wrapping_add(z),
            ],
            random,
        )?;
    }
    Ok(())
}

/// Enumerate the vanilla blob foliage placer's candidate writes in order.
/// The callback performs the actual validity check and state-provider sampling;
/// it receives the same RNG so those calls interleave with corner selection.
pub fn place_blob_foliage(
    random: &mut DecorationRandom,
    attachment: FoliageAttachment,
    height: i32,
    radius: i32,
    offset: i32,
    mut place: impl FnMut([i32; 3], &mut DecorationRandom) -> Result<()>,
) -> Result<()> {
    ensure!((0..=16).contains(&height), "invalid blob foliage height");
    ensure!((0..=16).contains(&radius), "invalid foliage radius");
    ensure!((0..=16).contains(&offset), "invalid foliage offset");
    // The caller must reject unverified attachment dimensions rather than run
    // an unbounded data-pack-defined loop.
    ensure!(
        (-32..=32).contains(&attachment.radius_offset),
        "unsupported attachment radius"
    );
    ensure!(
        (-16..=32).contains(&attachment.height_offset),
        "unsupported attachment height"
    );
    let foliage_height = height + attachment.height_offset;
    for y in (offset - foliage_height..=offset).rev() {
        let row_radius = (radius + attachment.radius_offset - 1 - y / 2).max(0);
        let extra = i32::from(attachment.double_trunk);
        for x in -row_radius..=row_radius + extra {
            for z in -row_radius..=row_radius + extra {
                let dx = if attachment.double_trunk {
                    x.abs().min((x - 1).abs())
                } else {
                    x.abs()
                };
                let dz = if attachment.double_trunk {
                    z.abs().min((z - 1).abs())
                } else {
                    z.abs()
                };
                if dx == row_radius
                    && dz == row_radius
                    && (random.next_int_bound(2).unwrap() == 0 || y == 0)
                {
                    continue;
                }
                place(
                    [
                        attachment.position[0].wrapping_add(x),
                        attachment.position[1].wrapping_add(y),
                        attachment.position[2].wrapping_add(z),
                    ],
                    random,
                )?;
            }
        }
    }
    Ok(())
}

/// Enumerate the rounded rows of the 26.3 fancy foliage placer.
pub fn place_fancy_foliage(
    random: &mut DecorationRandom,
    attachment: FoliageAttachment,
    height: i32,
    radius: i32,
    offset: i32,
    mut place: impl FnMut([i32; 3], &mut DecorationRandom) -> Result<()>,
) -> Result<()> {
    ensure!((0..=16).contains(&height), "invalid fancy foliage height");
    ensure!((0..=16).contains(&radius), "invalid foliage radius");
    ensure!((0..=16).contains(&offset), "invalid foliage offset");
    for y in (offset - height..=offset).rev() {
        let row_radius = radius + i32::from(y != offset && y != offset - height);
        let extra = i32::from(attachment.double_trunk);
        for x in -row_radius..=row_radius + extra {
            for z in -row_radius..=row_radius + extra {
                let dx = if attachment.double_trunk {
                    x.abs().min((x - 1).abs())
                } else {
                    x.abs()
                };
                let dz = if attachment.double_trunk {
                    z.abs().min((z - 1).abs())
                } else {
                    z.abs()
                };
                let x_squared = (dx as f32 + 0.5) * (dx as f32 + 0.5);
                let z_squared = (dz as f32 + 0.5) * (dz as f32 + 0.5);
                if x_squared + z_squared > (row_radius * row_radius) as f32 {
                    continue;
                }
                place(
                    [
                        attachment.position[0].wrapping_add(x),
                        attachment.position[1].wrapping_add(y),
                        attachment.position[2].wrapping_add(z),
                    ],
                    random,
                )?;
            }
        }
    }
    Ok(())
}
