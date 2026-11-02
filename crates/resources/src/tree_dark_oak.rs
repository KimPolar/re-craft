//! Java 26.3 dark oak trunk and foliage placement.
use anyhow::{Result, ensure};
use native_core::random::DecorationRandom;

use crate::tree_placers::FoliageAttachment;

pub trait DarkOakTrunkWorld {
    fn is_air_or_leaves(&mut self, pos: [i32; 3]) -> Result<bool>;
    fn place_soil(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()>;
    fn place_log(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()>;
}

pub fn place_dark_oak_trunk(
    world: &mut impl DarkOakTrunkWorld,
    random: &mut DecorationRandom,
    origin: [i32; 3],
    height: i32,
) -> Result<Vec<FoliageAttachment>> {
    ensure!((0..=80).contains(&height), "invalid dark oak height");
    for (x, z) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        world.place_soil(offset(origin, x, -1, z), random)?;
    }
    let direction = [[0, -1], [1, 0], [0, 1], [-1, 0]][random.next_int_bound(4).unwrap() as usize];
    let bend_start = height - random.next_int_bound(4).unwrap();
    let mut bend_steps = 2 - random.next_int_bound(3).unwrap();
    let (mut x, mut z) = (origin[0], origin[2]);
    let top_y = origin[1].wrapping_add(height).wrapping_sub(1);
    for y in 0..height {
        if y >= bend_start && bend_steps > 0 {
            x = x.wrapping_add(direction[0]);
            z = z.wrapping_add(direction[1]);
            bend_steps -= 1;
        }
        let pos = [x, origin[1].wrapping_add(y), z];
        if world.is_air_or_leaves(pos)? {
            for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                world.place_log(offset(pos, dx, 0, dz), random)?;
            }
        }
    }
    let mut foliage = vec![FoliageAttachment {
        position: [x, top_y, z],
        radius_offset: 0,
        height_offset: 0,
        double_trunk: true,
    }];
    for dx in -1..=2 {
        for dz in -1..=2 {
            if (0..=1).contains(&dx) && (0..=1).contains(&dz) {
                continue;
            }
            if random.next_int_bound(3).unwrap() > 0 {
                continue;
            }
            let branch_height = random.next_int_bound(3).unwrap() + 2;
            for y in 0..branch_height {
                world.place_log(
                    [
                        origin[0].wrapping_add(dx),
                        top_y.wrapping_sub(y).wrapping_sub(1),
                        origin[2].wrapping_add(dz),
                    ],
                    random,
                )?;
            }
            foliage.push(FoliageAttachment {
                position: [
                    origin[0].wrapping_add(dx),
                    top_y,
                    origin[2].wrapping_add(dz),
                ],
                radius_offset: 0,
                height_offset: 0,
                double_trunk: false,
            });
        }
    }
    Ok(foliage)
}

/// Emit foliage attempts in Java's order. The callback performs validity and
/// provider sampling so its random draws remain interleaved with row placement.
pub fn place_dark_oak_foliage(
    random: &mut DecorationRandom,
    attachment: FoliageAttachment,
    radius: i32,
    offset_y: i32,
    mut place: impl FnMut([i32; 3], &mut DecorationRandom) -> Result<()>,
) -> Result<()> {
    ensure!(
        (0..=16).contains(&radius) && (0..=16).contains(&offset_y),
        "invalid dark oak foliage dimensions"
    );
    let origin = offset(attachment.position, 0, offset_y, 0);
    let extra = i32::from(attachment.double_trunk);
    let mut row = |random: &mut DecorationRandom, row_radius: i32, y: i32| -> Result<()> {
        if row_radius < 0 {
            return Ok(());
        }
        for x in -row_radius..=row_radius + extra {
            for z in -row_radius..=row_radius + extra {
                if y == 0
                    && attachment.double_trunk
                    && (x == -row_radius || x >= row_radius)
                    && (z == -row_radius || z >= row_radius)
                {
                    continue;
                }
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
                if y == -1 && !attachment.double_trunk && dx == row_radius && dz == row_radius {
                    continue;
                }
                if y == 1 && dx + dz > row_radius * 2 - 2 {
                    continue;
                }
                place(offset(origin, x, y, z), random)?;
            }
        }
        Ok(())
    };
    row(random, radius + 2, -1)?;
    if attachment.double_trunk {
        row(random, radius + 3, 0)?;
        row(random, radius + 2, 1)?;
        if random.next_bool() {
            row(random, radius, 2)?;
        }
    } else {
        row(random, radius + 1, 0)?;
    }
    Ok(())
}

fn offset(pos: [i32; 3], x: i32, y: i32, z: i32) -> [i32; 3] {
    [
        pos[0].wrapping_add(x),
        pos[1].wrapping_add(y),
        pos[2].wrapping_add(z),
    ]
}
