use std::collections::BTreeMap;

use native_world::{
    collision::{block_outline, block_shape, tripwire_attached, tripwire_hook_facing},
    loaded::BlockState,
};

#[test]
fn rejects_malformed_tripwires_and_preserves_attached_outline() {
    let mut wire = BlockState {
        name: "minecraft:tripwire".into(),
        properties: [
            "attached", "disarmed", "east", "north", "powered", "south", "west",
        ]
        .into_iter()
        .map(|name| (name.into(), "false".into()))
        .collect(),
    };
    assert_eq!(tripwire_attached(&wire), Some(false));
    assert_eq!(block_outline(&wire).unwrap()[0].max.y, 0.5);
    wire.properties.insert("attached".into(), "true".into());
    let outline = block_outline(&wire).unwrap()[0];
    assert_eq!((outline.min.y, outline.max.y), (1.0 / 16.0, 2.5 / 16.0));
    assert!(block_shape(&wire).unwrap().is_empty());
    wire.properties.insert("north".into(), "invalid".into());
    assert!(tripwire_attached(&wire).is_none());
    assert!(block_outline(&wire).is_none());

    let mut hook = BlockState {
        name: "minecraft:tripwire_hook".into(),
        properties: BTreeMap::from([
            ("attached".into(), "true".into()),
            ("facing".into(), "north".into()),
            ("powered".into(), "false".into()),
        ]),
    };
    assert_eq!(tripwire_hook_facing(&hook), Some("north"));
    hook.properties.insert("facing".into(), "up".into());
    assert!(tripwire_hook_facing(&hook).is_none());
    assert!(block_shape(&hook).is_none());
}
