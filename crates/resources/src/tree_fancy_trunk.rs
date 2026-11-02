//! Branching trunk placement used by 26.3 fancy trees.
use crate::tree_placers::FoliageAttachment;
use anyhow::{Result, ensure};
use native_core::random::DecorationRandom;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogAxis {
    X,
    Y,
    Z,
}

pub trait FancyTrunkWorld {
    /// Trunk clearance accepts existing logs as well as replaceable blocks.
    fn is_free(&mut self, position: [i32; 3]) -> Result<bool>;
    fn place_soil(&mut self, position: [i32; 3], random: &mut DecorationRandom) -> Result<()>;
    /// The world checks replaceability and samples the provider if necessary.
    fn place_log(
        &mut self,
        position: [i32; 3],
        axis: LogAxis,
        random: &mut DecorationRandom,
    ) -> Result<()>;
}

fn tree_shape(height: i32, y: i32) -> f32 {
    if (y as f32) < height as f32 * 0.3 {
        return -1.0;
    }
    let half = height as f32 / 2.0;
    let difference = half - y as f32;
    let mut size = (half * half - difference * difference).sqrt();
    if difference == 0.0 {
        size = half;
    } else if difference.abs() >= half {
        return 0.0;
    }
    size * 0.5
}

fn offset(pos: [i32; 3], delta: [i32; 3]) -> [i32; 3] {
    [
        pos[0].wrapping_add(delta[0]),
        pos[1].wrapping_add(delta[1]),
        pos[2].wrapping_add(delta[2]),
    ]
}

fn limb(
    world: &mut impl FancyTrunkWorld,
    random: &mut DecorationRandom,
    start: [i32; 3],
    end: [i32; 3],
    write: bool,
) -> Result<bool> {
    if !write && start == end {
        return Ok(true);
    }
    let delta = [
        end[0].wrapping_sub(start[0]),
        end[1].wrapping_sub(start[1]),
        end[2].wrapping_sub(start[2]),
    ];
    let steps = delta.iter().map(|n| n.wrapping_abs()).max().unwrap();
    ensure!(steps >= 0 && steps <= 256, "unsupported fancy limb length");
    let direction = delta.map(|n| n as f32 / steps as f32);
    for step in 0..=steps {
        let delta = direction.map(|n| (0.5_f32 + step as f32 * n).floor() as i32);
        let pos = offset(start, delta);
        if write {
            let dx = pos[0].wrapping_sub(start[0]).wrapping_abs();
            let dz = pos[2].wrapping_sub(start[2]).wrapping_abs();
            let axis = if dx == 0 && dz == 0 {
                LogAxis::Y
            } else if dx >= dz {
                LogAxis::X
            } else {
                LogAxis::Z
            };
            world.place_log(pos, axis, random)?;
        } else if !world.is_free(pos)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Run the trunk placer, returning foliage attachments in vanilla order.
pub fn place_fancy_trunk(
    world: &mut impl FancyTrunkWorld,
    random: &mut DecorationRandom,
    origin: [i32; 3],
    height: i32,
) -> Result<Vec<FoliageAttachment>> {
    ensure!((0..=80).contains(&height), "invalid fancy trunk height");
    let height = height + 2;
    let trunk_height = (height as f64 * 0.618).floor() as i32;
    world.place_soil(offset(origin, [0, -1, 0]), random)?;
    let top = origin[1].wrapping_add(trunk_height);
    let mut clusters = vec![(offset(origin, [0, height - 5, 0]), top)];
    for y in (0..=height - 5).rev() {
        let shape = tree_shape(height, y);
        if shape < 0.0 {
            continue;
        }
        // Vanilla limits its density to at most one attempt per level.
        let distance = f64::from(shape) * (f64::from(random.next_float()) + 0.328);
        let angle = f64::from(random.next_float() * 2.0) * std::f64::consts::PI;
        let x = distance * angle.sin() + 0.5;
        let z = distance * angle.cos() + 0.5;
        // Java Math and native libm can differ by ulps. Never silently round
        // a coordinate whose integer boundary is too close to distinguish.
        ensure!(
            (x - x.round()).abs() > 1.0e-11 && (z - z.round()).abs() > 1.0e-11,
            "fancy branch trigonometry is too close to a block boundary"
        );
        let branch = offset(origin, [x.floor() as i32, y - 1, z.floor() as i32]);
        if !limb(world, random, branch, offset(branch, [0, 5, 0]), false)? {
            continue;
        }
        let dx = origin[0].wrapping_sub(branch[0]);
        let dz = origin[2].wrapping_sub(branch[2]);
        let base_y = (f64::from(branch[1])
            - f64::from(dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz))).sqrt() * 0.381)
            .min(f64::from(top)) as i32;
        let base = [origin[0], base_y, origin[2]];
        if limb(world, random, base, branch, false)? {
            clusters.push((branch, base_y));
        }
    }
    limb(
        world,
        random,
        origin,
        offset(origin, [0, trunk_height, 0]),
        true,
    )?;
    let eligible = |base: i32| f64::from(base.wrapping_sub(origin[1])) >= f64::from(height) * 0.2;
    for &(point, base_y) in &clusters {
        let base = [origin[0], base_y, origin[2]];
        if point != base && eligible(base_y) {
            limb(world, random, base, point, true)?;
        }
    }
    Ok(clusters
        .into_iter()
        .filter(|(_, base)| eligible(*base))
        .map(|(position, _)| FoliageAttachment {
            position,
            radius_offset: 0,
            height_offset: 0,
            double_trunk: false,
        })
        .collect())
}
