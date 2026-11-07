//! Java 26.3 door neighbor updates for verified built-in door states.
use glam::IVec3;
use std::collections::BTreeMap;

use crate::{collision::door_open, loaded::BlockState};

/// Update one half of a door after a neighbor changes. `below_sturdy` is
/// required only when the lower half receives an update from below. Unknown
/// door states or directions return `None` instead of guessing.
pub fn update_shape(
    state: &BlockState,
    direction: IVec3,
    neighbor: &BlockState,
    below_sturdy: Option<bool>,
) -> Option<BlockState> {
    door_open(state)?;
    if !matches!(
        direction,
        IVec3::X | IVec3::NEG_X | IVec3::Y | IVec3::NEG_Y | IVec3::Z | IVec3::NEG_Z
    ) {
        return None;
    }
    let half = state.properties.get("half")?.as_str();
    let pair = (half == "lower" && direction == IVec3::Y)
        || (half == "upper" && direction == IVec3::NEG_Y);
    if pair {
        if neighbor.name.ends_with("_door") && door_open(neighbor).is_none() {
            return None;
        }
        if door_open(neighbor).is_some() && neighbor.properties.get("half")?.as_str() != half {
            let mut result = neighbor.clone();
            result.properties.insert("half".into(), half.into());
            return Some(result);
        }
        return Some(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
    }
    if half == "lower" && direction == IVec3::NEG_Y && !below_sturdy? {
        return Some(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
    }
    Some(state.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn syncs_neighbor_half_and_rejects_unknown_support() {
        let state = BlockState {
            name: "minecraft:oak_door".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("half".into(), "lower".into()),
                ("hinge".into(), "left".into()),
                ("open".into(), "false".into()),
                ("powered".into(), "false".into()),
            ]),
        };
        let mut other = state.clone();
        other.name = "minecraft:birch_door".into();
        other.properties.insert("half".into(), "upper".into());
        other.properties.insert("open".into(), "true".into());
        let synced = update_shape(&state, IVec3::Y, &other, None).unwrap();
        assert_eq!(synced.name, "minecraft:birch_door");
        assert_eq!(synced.properties["half"], "lower");
        assert_eq!(synced.properties["open"], "true");
        assert!(update_shape(&state, IVec3::NEG_Y, &other, None).is_none());
        assert_eq!(
            update_shape(&state, IVec3::NEG_Y, &other, Some(false))
                .unwrap()
                .name,
            "minecraft:air"
        );
    }
}
