//! Java's square horizontal biome-color average.

use glam::IVec3;

/// Blend RGB samples with Java's integer rounding. The input supplies fallback
/// colors for unloaded chunks; valid UI radii are 0 through 7.
pub fn blend(position: IVec3, radius: u8, mut sample: impl FnMut(IVec3) -> u32) -> u32 {
    let radius = i32::from(radius.min(7));
    if radius == 0 {
        return sample(position);
    }
    let side = radius * 2 + 1;
    let mut sum = [0_u32; 3];
    for z in -radius..=radius {
        for x in -radius..=radius {
            let color = sample(IVec3::new(
                position.x.wrapping_add(x),
                position.y,
                position.z.wrapping_add(z),
            ));
            sum[0] += (color >> 16) & 255;
            sum[1] += (color >> 8) & 255;
            sum[2] += color & 255;
        }
    }
    let count = (side * side) as u32;
    0xff00_0000 | ((sum[0] / count) << 16) | ((sum[1] / count) << 8) | (sum[2] / count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_channels_separately_and_clamps_radius() {
        assert_eq!(blend(IVec3::ZERO, 0, |_| 0x123456), 0x123456);
        assert_eq!(
            blend(IVec3::ZERO, 1, |p| if p.x < 0 {
                0xff0000
            } else {
                0x00ff00
            }),
            0xff55aa00
        );
        let mut calls = 0;
        blend(IVec3::ZERO, 255, |_| {
            calls += 1;
            0
        });
        assert_eq!(calls, 225);
    }
}
