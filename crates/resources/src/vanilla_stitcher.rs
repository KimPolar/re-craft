//! Java 26.3's sprite stitching layout, independent of GPU texture allocation.
//!
//! The renderer's existing terrain atlas has different packing rules. This
//! layout describes the original game's UVs for model and glint comparison.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};

use crate::packs::ResourceId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpriteSize {
    pub id: ResourceId,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpritePlacement {
    /// Position of the padded slot; the sprite starts at `(x + padding, y + padding)`.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub padding: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StitchLayout {
    pub width: u32,
    pub height: u32,
    pub placements: BTreeMap<ResourceId, SpritePlacement>,
}

struct Holder<'a> {
    sprite: &'a SpriteSize,
    width: u32,
    height: u32,
}

struct Region {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    occupied: bool,
    children: Vec<usize>,
}

impl Region {
    fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
            occupied: false,
            children: Vec::new(),
        }
    }
}

/// Select the mip level used by `SpriteLoader` after examining sprite sizes.
pub fn effective_mip_level(sprites: &[SpriteSize], requested: u32) -> Result<u32> {
    ensure!(requested <= 16, "unsupported mip level");
    let mut min_dimension = u32::MAX;
    let mut min_power = 1_u32 << requested;
    for sprite in sprites {
        ensure!(sprite.width > 0 && sprite.height > 0, "empty sprite");
        min_dimension = min_dimension.min(sprite.width.min(sprite.height));
        let width_power = 1_u32 << sprite.width.trailing_zeros();
        let height_power = 1_u32 << sprite.height.trailing_zeros();
        min_power = min_power.min(width_power.min(height_power));
    }
    let minimum = min_dimension.min(min_power);
    Ok(requested.min(31 - minimum.leading_zeros()))
}

/// Pack sprites with the same ordering, alignment, padding and growth decisions
/// as the installed Java client's `Stitcher`.
pub fn stitch(
    sprites: &[SpriteSize],
    max_width: u32,
    max_height: u32,
    mip_level: u32,
    anisotropy_bit: u32,
) -> Result<StitchLayout> {
    ensure!(mip_level <= 16, "unsupported mip level");
    ensure!(max_width > 0 && max_height > 0, "invalid atlas limit");
    ensure!(sprites.len() <= 100_000, "too many atlas sprites");
    let step = 1_u32 << mip_level;
    let padding = step
        .checked_shl(anisotropy_bit.saturating_sub(1).min(4))
        .ok_or_else(|| anyhow::anyhow!("atlas padding overflow"))?;
    let padded = |size: u32| -> Result<u32> {
        let size = size
            .checked_add(
                padding
                    .checked_mul(2)
                    .ok_or_else(|| anyhow::anyhow!("atlas padding overflow"))?,
            )
            .and_then(|value| value.checked_add(step - 1))
            .ok_or_else(|| anyhow::anyhow!("sprite dimensions overflow"))?;
        Ok(size & !(step - 1))
    };
    let mut seen = BTreeSet::new();
    let mut holders = Vec::with_capacity(sprites.len());
    for sprite in sprites {
        ensure!(sprite.width > 0 && sprite.height > 0, "empty sprite");
        ensure!(
            seen.insert(sprite.id.clone()),
            "duplicate sprite identifier"
        );
        holders.push(Holder {
            sprite,
            width: padded(sprite.width)?,
            height: padded(sprite.height)?,
        });
    }
    holders.sort_by(|a, b| {
        b.height
            .cmp(&a.height)
            .then_with(|| b.width.cmp(&a.width))
            // Java Identifier.compareTo compares path before namespace.
            .then_with(|| a.sprite.id.path.cmp(&b.sprite.id.path))
            .then_with(|| a.sprite.id.namespace.cmp(&b.sprite.id.namespace))
    });

    let mut regions = Vec::<Region>::new();
    let mut roots = Vec::<usize>::new();
    let mut todo = Vec::<usize>::new();
    let mut placements = BTreeMap::new();
    let (mut width, mut height) = (0_u32, 0_u32);
    for holder in holders {
        let mut location = None;
        for &root in &roots {
            if let Some(position) = add_to_region(&mut regions, root, &holder, &mut todo) {
                location = Some(position);
                break;
            }
        }
        if location.is_none() {
            let old_width = if width == 0 {
                0
            } else {
                width.next_power_of_two()
            };
            let old_height = if height == 0 {
                0
            } else {
                height.next_power_of_two()
            };
            let next_width = width
                .checked_add(holder.width)
                .and_then(u32::checked_next_power_of_two)
                .ok_or_else(|| anyhow::anyhow!("atlas width overflow"))?;
            let next_height = height
                .checked_add(holder.height)
                .and_then(u32::checked_next_power_of_two)
                .ok_or_else(|| anyhow::anyhow!("atlas height overflow"))?;
            let fits_width = next_width <= max_width;
            let fits_height = next_height <= max_height;
            if !fits_width && !fits_height {
                bail!("sprites do not fit in the atlas");
            }
            let changes_width = fits_width && old_width != next_width;
            let changes_height = fits_height && old_height != next_height;
            let grow_width = if changes_width != changes_height {
                changes_width
            } else {
                fits_width && old_width <= old_height
            };
            let region = if grow_width {
                if height == 0 {
                    height = next_height;
                }
                let region = Region::new(width, 0, next_width - width, height);
                width = next_width;
                region
            } else {
                let region = Region::new(0, height, width, next_height - height);
                height = next_height;
                region
            };
            let root = regions.len();
            regions.push(region);
            roots.push(root);
            location = add_to_region(&mut regions, root, &holder, &mut todo);
        }
        let (x, y) =
            location.ok_or_else(|| anyhow::anyhow!("sprite cannot fit the expanded atlas"))?;
        placements.insert(
            holder.sprite.id.clone(),
            SpritePlacement {
                x,
                y,
                width: holder.sprite.width,
                height: holder.sprite.height,
                padding,
            },
        );
    }
    Ok(StitchLayout {
        width,
        height,
        placements,
    })
}

fn add_to_region(
    regions: &mut Vec<Region>,
    root: usize,
    holder: &Holder<'_>,
    todo: &mut Vec<usize>,
) -> Option<(u32, u32)> {
    todo.clear();
    todo.push(root);
    while let Some(index) = todo.pop() {
        let region = &regions[index];
        if region.occupied || holder.width > region.width || holder.height > region.height {
            continue;
        }
        if holder.width == region.width && holder.height == region.height {
            let position = (region.x, region.y);
            regions[index].occupied = true;
            return Some(position);
        }
        if region.children.is_empty() {
            let (x, y, width, height) = (region.x, region.y, region.width, region.height);
            let (remaining_width, remaining_height) =
                (width - holder.width, height - holder.height);
            let mut children = vec![Region::new(x, y, holder.width, holder.height)];
            if remaining_width > 0 && remaining_height > 0 {
                if height.max(remaining_width) >= width.max(remaining_height) {
                    children.push(Region::new(
                        x,
                        y + holder.height,
                        holder.width,
                        remaining_height,
                    ));
                    children.push(Region::new(x + holder.width, y, remaining_width, height));
                } else {
                    children.push(Region::new(
                        x + holder.width,
                        y,
                        remaining_width,
                        holder.height,
                    ));
                    children.push(Region::new(x, y + holder.height, width, remaining_height));
                }
            } else if remaining_width == 0 {
                children.push(Region::new(
                    x,
                    y + holder.height,
                    holder.width,
                    remaining_height,
                ));
            } else {
                children.push(Region::new(
                    x + holder.width,
                    y,
                    remaining_width,
                    holder.height,
                ));
            }
            let start = regions.len();
            regions.extend(children);
            regions[index].children = (start..regions.len()).collect();
        }
        todo.extend(regions[index].children.iter().rev().copied());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_or_oversized_sprites_and_respects_mipmap_alignment() {
        let sprite = SpriteSize {
            id: ResourceId::parse("example:one").unwrap(),
            width: 16,
            height: 16,
        };
        assert!(stitch(&[sprite.clone(), sprite.clone()], 128, 128, 0, 0).is_err());
        assert!(stitch(std::slice::from_ref(&sprite), 8, 8, 0, 0).is_err());
        let layout = stitch(std::slice::from_ref(&sprite), 128, 128, 4, 0).unwrap();
        let placement = layout.placements[&sprite.id];
        assert_eq!(placement.padding, 16);
        assert_eq!(
            (placement.x, placement.y, layout.width, layout.height),
            (0, 0, 64, 64)
        );
        assert_eq!(effective_mip_level(&[sprite], 4).unwrap(), 4);
    }
}
