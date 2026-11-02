//! Read sculk sensor block entities without discarding their pending vibration.
//! This does not yet register listeners or authorize world writes.

use anyhow::{Context, Result, bail, ensure};
use glam::{DVec3, IVec3};
use native_persistence::nbt::{self, Tag, TagType};
use native_world::loaded::{ChunkPos, SavedBlockEntities};
use std::{collections::BTreeSet, sync::Arc};

use crate::vibration::{Vibration, VibrationData};

#[derive(Clone, Debug)]
pub struct SavedSculkSensor {
    pub position: IVec3,
    pub calibrated: bool,
    pub last_frequency: i32,
    pub vibration: VibrationData,
    /// A decoded Java listener requests particle reconstruction after loading.
    pub reload_vibration_particle: bool,
    source: Arc<Tag>,
}

impl SavedSculkSensor {
    pub fn source(&self) -> &Tag {
        &self.source
    }
}

fn unique<'a>(tag: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = tag else {
        bail!("expected a compound for {name}")
    };
    let mut found = fields.iter().filter(|(key, _)| key.matches(name));
    let value = found.next().map(|(_, value)| value);
    ensure!(found.next().is_none(), "duplicate field {name}");
    Ok(value)
}

fn number(tag: &Tag) -> Option<f64> {
    match tag {
        Tag::Byte(v) => Some(f64::from(*v)),
        Tag::Short(v) => Some(f64::from(*v)),
        Tag::Int(v) => Some(f64::from(*v)),
        Tag::Long(v) => Some(*v as f64),
        Tag::Float(v) => Some(f64::from(*v)),
        Tag::Double(v) => Some(*v),
        _ => None,
    }
}

fn event(tag: &Tag, registry: &BTreeSet<String>) -> Result<Vibration> {
    let Tag::String(name) = unique(tag, "game_event")?.context("missing game event")? else {
        bail!("invalid game event")
    };
    let name = String::from_utf16(&name.0).context("invalid event identifier")?;
    let parsed = native_resources::packs::ResourceId::parse(&name)?;
    let name = format!("{}:{}", parsed.namespace, parsed.path);
    ensure!(
        registry.contains(&name),
        "game event is absent from the active registry"
    );
    let distance = number(unique(tag, "distance")?.context("missing vibration distance")?)
        .context("invalid vibration distance")? as f32;
    ensure!(
        distance.is_finite() && distance >= 0.0,
        "invalid vibration distance"
    );
    let Tag::List { values, .. } = unique(tag, "pos")?.context("missing vibration position")?
    else {
        bail!("invalid vibration position")
    };
    ensure!(
        values.len() == 3,
        "vibration position must have three coordinates"
    );
    let position = DVec3::new(
        number(&values[0]).context("invalid x")?,
        number(&values[1]).context("invalid y")?,
        number(&values[2]).context("invalid z")?,
    );
    ensure!(position.is_finite(), "vibration position is not finite");
    let uuid = |field: &str| -> Result<Option<[i32; 4]>> {
        match unique(tag, field)? {
            None => Ok(None),
            Some(Tag::IntArray(values)) if values.len() == 4 => {
                Ok(Some([values[0], values[1], values[2], values[3]]))
            }
            _ => bail!("invalid vibration {field}"),
        }
    };
    Ok(Vibration {
        event: name,
        distance,
        position,
        source: uuid("source")?,
        projectile_owner: uuid("projectile_owner")?,
    })
}

fn listener(tag: &Tag, registry: &BTreeSet<String>) -> Result<VibrationData> {
    let current = unique(tag, "event")?
        .map(|value| event(value, registry))
        .transpose()?;
    let delay = match unique(tag, "event_delay")? {
        None => 0,
        Some(value) => value.as_java_int().context("invalid vibration delay")?,
    };
    ensure!(delay >= 0, "negative vibration delay");
    let selector = unique(tag, "selector")?.context("missing vibration selector")?;
    let tick = unique(selector, "tick")?
        .and_then(Tag::as_java_long)
        .context("invalid selector tick")?;
    let pending = unique(selector, "event")?
        .map(|value| event(value, registry).map(|event| (tick, event)))
        .transpose()?;
    Ok(VibrationData::from_saved(current, delay, pending))
}

/// The registry must be the complete set of game-event IDs in the active world.
fn entities(payload: &SavedBlockEntities, chunk: ChunkPos) -> Result<Vec<(IVec3, Tag)>> {
    let bytes = match payload {
        SavedBlockEntities::Empty => return Ok(Vec::new()),
        SavedBlockEntities::Unavailable => bail!("block entity data is unavailable"),
        SavedBlockEntities::Encoded(bytes) => bytes,
    };
    let document = nbt::read(bytes)?;
    let Tag::List {
        element_type: TagType::Compound,
        values,
    } = document.value
    else {
        bail!("block entities are not a compound list")
    };
    let mut positions = BTreeSet::new();
    let mut found = Vec::with_capacity(values.len());
    for entity in values {
        let coord = |field: &str| -> Result<i32> {
            match unique(&entity, field)? {
                Some(Tag::Int(v)) => Ok(*v),
                _ => bail!("invalid block entity {field}"),
            }
        };
        let point = IVec3::new(coord("x")?, coord("y")?, coord("z")?);
        ensure!(
            point.x.div_euclid(16) == chunk.x && point.z.div_euclid(16) == chunk.z,
            "block entity outside chunk"
        );
        ensure!(
            positions.insert(point.to_array()),
            "duplicate block entity position"
        );
        found.push((point, entity));
    }
    Ok(found)
}

fn decode(entity: Tag, position: IVec3, registry: &BTreeSet<String>) -> Result<SavedSculkSensor> {
    let calibrated = match unique(&entity, "id")? {
        Some(Tag::String(id))
            if id.matches("minecraft:sculk_sensor") || id.matches("sculk_sensor") =>
        {
            false
        }
        Some(Tag::String(id))
            if id.matches("minecraft:calibrated_sculk_sensor")
                || id.matches("calibrated_sculk_sensor") =>
        {
            true
        }
        _ => bail!("block entity is not a sculk sensor"),
    };
    let last_frequency = match unique(&entity, "last_vibration_frequency")? {
        None => 0,
        Some(v) => v
            .as_java_int()
            .context("invalid last vibration frequency")?,
    };
    let saved_listener = unique(&entity, "listener")?;
    let vibration = saved_listener
        .map(|tag| listener(tag, registry))
        .transpose()?
        .unwrap_or_default();
    Ok(SavedSculkSensor {
        position,
        calibrated,
        last_frequency,
        vibration,
        reload_vibration_particle: saved_listener.is_some(),
        source: Arc::new(entity),
    })
}

/// The registry must be the complete set of game-event IDs in the active world.
pub fn read(
    payload: &SavedBlockEntities,
    chunk: ChunkPos,
    position: IVec3,
    registry: &BTreeSet<String>,
) -> Result<Option<SavedSculkSensor>> {
    ensure!(
        position.x.div_euclid(16) == chunk.x && position.z.div_euclid(16) == chunk.z,
        "sensor is outside chunk"
    );
    let Some((_, entity)) = entities(payload, chunk)?
        .into_iter()
        .find(|(point, _)| *point == position)
    else {
        return Ok(None);
    };
    Ok(Some(decode(entity, position, registry)?))
}

/// Decode a chunk's block entities once, preserving their stored order.
pub fn read_chunk(
    payload: &SavedBlockEntities,
    chunk: ChunkPos,
    registry: &BTreeSet<String>,
) -> Result<Vec<SavedSculkSensor>> {
    let mut sensors = Vec::new();
    for (position, entity) in entities(payload, chunk)? {
        let is_sensor = match unique(&entity, "id")? {
            Some(Tag::String(id)) => [
                "minecraft:sculk_sensor",
                "sculk_sensor",
                "minecraft:calibrated_sculk_sensor",
                "calibrated_sculk_sensor",
            ]
            .iter()
            .any(|name| id.matches(name)),
            _ => bail!("invalid block entity id"),
        };
        if is_sensor {
            sensors.push(decode(entity, position, registry)?);
        }
    }
    Ok(sensors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::NamedTag;

    fn payload(values: Vec<Tag>) -> SavedBlockEntities {
        SavedBlockEntities::Encoded(
            nbt::write(&NamedTag {
                name: "".into(),
                value: Tag::List {
                    element_type: TagType::Compound,
                    values,
                },
            })
            .unwrap()
            .into(),
        )
    }
    fn sensor() -> Tag {
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:sculk_sensor".into())),
            ("x".into(), Tag::Int(1)),
            ("y".into(), Tag::Int(4)),
            ("z".into(), Tag::Int(2)),
            (
                "listener".into(),
                Tag::Compound(vec![
                    (
                        "selector".into(),
                        Tag::Compound(vec![("tick".into(), Tag::Long(-1))]),
                    ),
                    ("event_delay".into(), Tag::Int(0)),
                ]),
            ),
            ("unknown".into(), Tag::Int(37)),
        ])
    }
    #[test]
    fn unavailable_duplicate_and_invalid_listener_are_not_treated_as_empty() {
        let chunk = ChunkPos { x: 0, z: 0 };
        let pos = IVec3::new(1, 4, 2);
        let registry = BTreeSet::new();
        assert!(
            read(&SavedBlockEntities::Empty, chunk, pos, &registry)
                .unwrap()
                .is_none()
        );
        assert!(read(&SavedBlockEntities::Unavailable, chunk, pos, &registry).is_err());
        let original = sensor();
        let saved = read(&payload(vec![original.clone()]), chunk, pos, &registry)
            .unwrap()
            .unwrap();
        assert_eq!(saved.source(), &original);
        assert_eq!(saved.last_frequency, 0);
        assert!(saved.vibration.current().is_none() && saved.vibration.pending().is_none());
        assert!(
            read(
                &payload(vec![original.clone(), original.clone()]),
                chunk,
                pos,
                &registry
            )
            .is_err()
        );
        let Tag::Compound(mut fields) = original else {
            unreachable!()
        };
        fields.push(("listener".into(), Tag::Compound(vec![])));
        assert!(read(&payload(vec![Tag::Compound(fields)]), chunk, pos, &registry).is_err());
    }

    #[test]
    fn listener_rejects_unknown_events_and_malformed_vibration_data() {
        let event = || {
            Tag::Compound(vec![
                ("game_event".into(), Tag::String("minecraft:splash".into())),
                ("distance".into(), Tag::Float(2.5)),
                (
                    "pos".into(),
                    Tag::List {
                        element_type: TagType::Double,
                        values: vec![Tag::Double(1.0), Tag::Double(2.0), Tag::Double(3.0)],
                    },
                ),
            ])
        };
        let build = |value: Tag, delay: i32| {
            Tag::Compound(vec![
                ("event".into(), value),
                ("event_delay".into(), Tag::Int(delay)),
                (
                    "selector".into(),
                    Tag::Compound(vec![("tick".into(), Tag::Long(-1))]),
                ),
            ])
        };
        let registry = BTreeSet::from(["minecraft:splash".into()]);
        assert_eq!(
            listener(&build(event(), 2), &registry)
                .unwrap()
                .current()
                .unwrap()
                .distance,
            2.5
        );
        assert!(listener(&build(event(), 2), &BTreeSet::new()).is_err());
        assert!(listener(&build(event(), -1), &registry).is_err());
        let Tag::Compound(mut fields) = event() else {
            unreachable!()
        };
        fields.push(("source".into(), Tag::IntArray(vec![1, 2, 3])));
        assert!(listener(&build(Tag::Compound(fields), 2), &registry).is_err());
    }

    #[test]
    fn chunk_loading_preserves_sensor_order_and_checks_all_positions() {
        let first = sensor();
        let Tag::Compound(mut fields) = sensor() else {
            unreachable!()
        };
        for (key, value) in &mut fields {
            if key.matches("x") {
                *value = Tag::Int(3);
            }
            if key.matches("id") {
                *value = Tag::String("minecraft:calibrated_sculk_sensor".into());
            }
        }
        let second = Tag::Compound(fields);
        let chest = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:chest".into())),
            ("x".into(), Tag::Int(5)),
            ("y".into(), Tag::Int(4)),
            ("z".into(), Tag::Int(2)),
        ]);
        let chunk = ChunkPos { x: 0, z: 0 };
        let result = read_chunk(
            &payload(vec![second.clone(), chest, first.clone()]),
            chunk,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .iter()
                .map(|sensor| (sensor.position.x, sensor.calibrated))
                .collect::<Vec<_>>(),
            vec![(3, true), (1, false)]
        );
        assert!(
            read_chunk(
                &payload(vec![first.clone(), first]),
                chunk,
                &BTreeSet::new()
            )
            .is_err()
        );
    }
}
