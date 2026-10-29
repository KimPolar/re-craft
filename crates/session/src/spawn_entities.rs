//! Distinguish entities known never to collide from ones needing simulation.

use anyhow::{Context, Result, bail, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::{entity_collision::EntityCollisionKind, packs::ResourceId};

pub(crate) fn require_noncolliding(
    entities: &[Tag],
    mut kind: impl FnMut(&str) -> Option<EntityCollisionKind>,
) -> Result<()> {
    let mut pending: Vec<_> = entities.iter().collect();
    while let Some(entity) = pending.pop() {
        ensure!(
            matches!(entity, Tag::Compound(_)),
            "saved entity is not a compound"
        );
        let Some(Tag::String(id)) = entity.get("id") else {
            bail!("saved entity type is missing")
        };
        let id = String::from_utf16(&id.0).context("saved entity type is invalid UTF-16")?;
        let id = ResourceId::parse(&id)?;
        let id = format!("{}:{}", id.namespace, id.path);
        match kind(&id).with_context(|| format!("unknown saved entity type {id}"))? {
            EntityCollisionKind::Never => {}
            EntityCollisionKind::RequiresEntityState => {
                bail!("collision behavior of {id} requires a simulated entity")
            }
        }
        match entity.get("Passengers") {
            None => {}
            Some(Tag::List {
                element_type: TagType::Compound,
                values,
            }) => pending.extend(values),
            Some(Tag::List {
                element_type: TagType::End,
                values,
            }) if values.is_empty() => {}
            _ => bail!("entity passengers are not a compound list"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entity(id: &str, passengers: Vec<Tag>) -> Tag {
        Tag::Compound(vec![
            ("id".into(), Tag::String(id.into())),
            (
                "Passengers".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: passengers,
                },
            ),
        ])
    }
    fn kind(id: &str) -> Option<EntityCollisionKind> {
        match id {
            "minecraft:pig" | "minecraft:item" => Some(EntityCollisionKind::Never),
            "minecraft:oak_boat" => Some(EntityCollisionKind::RequiresEntityState),
            _ => None,
        }
    }
    #[test]
    fn passenger_trees_and_unknown_types_are_never_assumed_safe() {
        let safe = entity("pig", vec![entity("minecraft:item", vec![])]);
        assert!(require_noncolliding(&[safe], kind).is_ok());
        let boat = entity("pig", vec![entity("minecraft:oak_boat", vec![])]);
        assert!(require_noncolliding(&[boat], kind).is_err());
        assert!(require_noncolliding(&[entity("example:unknown", vec![])], kind).is_err());
        assert!(require_noncolliding(&[Tag::Compound(vec![])], kind).is_err());
        let malformed = Tag::Compound(vec![
            ("id".into(), Tag::String("pig".into())),
            ("Passengers".into(), Tag::Int(0)),
        ]);
        assert!(require_noncolliding(&[malformed], kind).is_err());
    }
}
