//! Building blocks for 26.3 tree generation. These do not apply a complete
//! tree: the caller owns collision checks, providers, writes and decoration.
use anyhow::{Result, ensure};
use native_core::random::DecorationRandom;

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
