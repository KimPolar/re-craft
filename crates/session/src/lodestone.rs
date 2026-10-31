//! The saved compass target and its POI-driven invalidation.
//!
//! A missing POI lookup is distinct from an absent POI: callers must not
//! invalidate a target merely because its chunk has not been read yet.

use glam::IVec3;
use native_persistence::nbt::Tag;
use native_resources::packs::ResourceId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub dimension: ResourceId,
    pub position: IVec3,
}

impl Target {
    /// Decode the GlobalPos representation used by player and item data.
    pub fn parse(tag: &Tag) -> Option<Self> {
        parse_target(tag)
    }

    pub fn to_nbt(&self) -> Tag {
        Tag::Compound(vec![
            (
                "dimension".into(),
                Tag::String(
                    format!("{}:{}", self.dimension.namespace, self.dimension.path)
                        .as_str()
                        .into(),
                ),
            ),
            (
                "pos".into(),
                Tag::IntArray(self.position.to_array().to_vec()),
            ),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tracker {
    pub target: Option<Target>,
    pub tracked: bool,
}

impl Tracker {
    pub fn linked(dimension: ResourceId, position: IVec3) -> Self {
        Self {
            target: Some(Target {
                dimension,
                position,
            }),
            tracked: true,
        }
    }

    /// Parse only data that can be rewritten without discarding unknown fields.
    pub fn parse(tag: &Tag) -> Option<Self> {
        let Tag::Compound(fields) = tag else {
            return None;
        };
        let mut target = None;
        let mut tracked = None;
        for (name, value) in fields {
            if name.matches("target") && target.is_none() {
                target = Some(parse_target(value)?);
            } else if name.matches("tracked") && tracked.is_none() {
                tracked = Some(match value {
                    Tag::Byte(0) => false,
                    Tag::Byte(1) => true,
                    _ => return None,
                });
            } else {
                return None;
            }
        }
        Some(Self {
            target,
            tracked: tracked.unwrap_or(true),
        })
    }

    pub fn to_nbt(&self) -> Tag {
        let mut fields = Vec::new();
        if let Some(target) = &self.target {
            fields.push(("target".into(), target.to_nbt()));
        }
        if !self.tracked {
            fields.push(("tracked".into(), Tag::Byte(0)));
        }
        Tag::Compound(fields)
    }

    /// Apply Java's tick result after the caller has resolved bounds and POI.
    /// `poi_exists` is None when the authoritative POI state is unavailable.
    pub fn update_target(
        &mut self,
        current_dimension: &ResourceId,
        in_bounds: bool,
        poi_exists: Option<bool>,
    ) -> bool {
        let Some(target) = &self.target else {
            return false;
        };
        if !self.tracked || &target.dimension != current_dimension {
            return false;
        }
        if !in_bounds || poi_exists == Some(false) {
            self.target = None;
            return true;
        }
        false
    }
}

fn parse_target(tag: &Tag) -> Option<Target> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let mut dimension = None;
    let mut position = None;
    for (name, value) in fields {
        if name.matches("dimension") && dimension.is_none() {
            let Tag::String(id) = value else { return None };
            dimension = Some(ResourceId::parse(&String::from_utf16(&id.0).ok()?).ok()?);
        } else if name.matches("pos") && position.is_none() {
            let coords: Vec<i32> = match value {
                Tag::IntArray(values) => values.clone(),
                Tag::ByteArray(values) => {
                    values.iter().map(|value| i32::from(*value as i8)).collect()
                }
                Tag::LongArray(values) => values.iter().map(|value| *value as i32).collect(),
                Tag::List { values, .. } => {
                    values.iter().map(Tag::as_java_int).collect::<Option<_>>()?
                }
                _ => return None,
            };
            let [x, y, z] = coords.as_slice() else {
                return None;
            };
            position = Some(IVec3::new(*x, *y, *z));
        } else {
            return None;
        }
    }
    Some(Target {
        dimension: dimension?,
        position: position?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_requires_a_known_absent_poi_in_the_same_dimension() {
        let overworld = ResourceId::parse("minecraft:overworld").unwrap();
        let nether = ResourceId::parse("minecraft:the_nether").unwrap();
        let original = Tracker::linked(overworld.clone(), IVec3::new(-16, 64, 42));
        for (dimension, bounds, poi, changed) in [
            (&overworld, true, None, false),
            (&overworld, true, Some(true), false),
            (&nether, false, Some(false), false),
            (&overworld, true, Some(false), true),
            (&overworld, false, None, true),
        ] {
            let mut tracker = original.clone();
            assert_eq!(tracker.update_target(dimension, bounds, poi), changed);
            assert_eq!(tracker.target.is_none(), changed);
        }
        let mut untracked = original;
        untracked.tracked = false;
        assert!(!untracked.update_target(&overworld, false, Some(false)));
    }

    #[test]
    fn malformed_and_unknown_components_are_not_rewritten() {
        let target = Tracker::linked(
            ResourceId::parse("example:world").unwrap(),
            IVec3::new(-1, 2, 3),
        );
        assert_eq!(Tracker::parse(&target.to_nbt()), Some(target));
        assert!(Tracker::parse(&Tag::Compound(vec![("other".into(), Tag::Int(1))])).is_none());
        assert!(Tracker::parse(&Tag::Compound(vec![("tracked".into(), Tag::Byte(2))])).is_none());
        assert!(
            Tracker::parse(&Tag::Compound(vec![
                ("tracked".into(), Tag::Byte(0)),
                ("tracked".into(), Tag::Byte(1))
            ]))
            .is_none()
        );
    }

    #[test]
    fn supported_compass_components_are_resolved_for_inventory_operations() {
        let tracker = Tracker::linked(
            ResourceId::parse("minecraft:overworld").unwrap(),
            IVec3::new(1, 2, 3),
        );
        let components = Tag::Compound(vec![(
            "minecraft:lodestone_tracker".into(),
            tracker.to_nbt(),
        )]);
        assert_eq!(
            crate::inventory::saved_item_max("minecraft:compass", &components),
            Some(64)
        );
        assert_eq!(
            crate::inventory::saved_item_max("minecraft:stone", &components),
            None
        );
        let stack =
            crate::inventory::ItemStack::new("minecraft:compass".into(), 2, 64, components.clone())
                .unwrap();
        assert!(stack.has_glint());
        let mut inventory = crate::inventory::PlayerInventory::default();
        let mut source = Some(stack);
        assert!(inventory.pickup_stack(&mut source, false));
        assert!(source.is_none());
        let dropped = inventory.drop_selected(false).unwrap();
        assert_eq!(dropped.count(), 1);
        assert_eq!(dropped.components(), &components);
        let conflict = Tag::Compound(vec![
            ("minecraft:lodestone_tracker".into(), tracker.to_nbt()),
            ("!minecraft:lodestone_tracker".into(), Tag::Compound(vec![])),
        ]);
        assert_eq!(
            crate::inventory::saved_item_max("minecraft:compass", &conflict),
            None
        );
    }

    #[test]
    fn binding_preserves_custom_data_and_defers_unknown_components() {
        use crate::inventory::{ItemStack, PlayerInventory, PlayerSlot};
        let tracker = Tracker::linked(
            ResourceId::parse("minecraft:overworld").unwrap(),
            IVec3::new(9, 4, -2),
        );
        let custom = Tag::Compound(vec![("marker".into(), Tag::Int(42))]);
        let components = Tag::Compound(vec![("minecraft:custom_data".into(), custom.clone())]);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:compass".into(), 2, 64, components),
        );
        assert_eq!(
            inventory.bind_lodestone_compass(PlayerSlot::Storage(0), &tracker, false),
            Some(None)
        );
        let original = inventory.slot(PlayerSlot::Storage(0)).unwrap();
        let linked = inventory.slot(PlayerSlot::Storage(1)).unwrap();
        assert_eq!(original.count(), 1);
        assert_eq!(
            linked.components().get("minecraft:custom_data"),
            Some(&custom)
        );
        assert_eq!(
            linked
                .components()
                .get("minecraft:lodestone_tracker")
                .and_then(Tracker::parse),
            Some(tracker.clone())
        );
        let unsupported = ItemStack::new(
            "minecraft:compass".into(),
            1,
            64,
            Tag::Compound(vec![("example:unknown".into(), Tag::Int(1))]),
        )
        .unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), Some(unsupported.clone()));
        assert_eq!(
            inventory.bind_lodestone_compass(PlayerSlot::Storage(0), &tracker, false),
            None
        );
        assert_eq!(inventory.slot(PlayerSlot::Storage(0)), Some(&unsupported));
    }
}
