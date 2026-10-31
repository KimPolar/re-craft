//! Mining properties shared by vanilla slabs and their stair counterparts.

use crate::mining::{Material, StairTool};

pub(crate) fn material(id: &str) -> Option<Material> {
    let base = id.strip_prefix("minecraft:")?.strip_suffix("_slab")?;
    let (hardness_tenths, tool) = match base {
        // These slabs have different properties from the similarly named stairs
        // or have no stair counterpart.
        "blackstone"
        | "cut_red_sandstone"
        | "cut_sandstone"
        | "petrified_oak"
        | "polished_blackstone_brick"
        | "purpur"
        | "quartz"
        | "red_sandstone"
        | "sandstone"
        | "smooth_stone"
        | "stone_brick"
        | "stone" => (20, StairTool::Pickaxe),
        _ => match crate::stair_mining::material(&format!("minecraft:{base}_stairs"))? {
            Material::Stair {
                hardness_tenths,
                tool,
            } => (hardness_tenths, tool),
            _ => return None,
        },
    };
    Some(Material::Slab {
        hardness_tenths,
        tool,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slab_exceptions_and_unrecognized_names() {
        for name in [
            "stone_slab",
            "petrified_oak_slab",
            "quartz_slab",
            "cut_sandstone_slab",
        ] {
            let material = material(&format!("minecraft:{name}")).unwrap();
            assert_eq!(material.hardness(), 2.0);
            assert!(material.needs_tool());
        }
        let wooden = material("minecraft:oak_slab").unwrap();
        assert_eq!(wooden.hardness(), 2.0);
        assert!(!wooden.needs_tool());
        assert!(material("minecraft:not_a_real_slab").is_none());
        assert!(material("example:stone_slab").is_none());
    }
}
