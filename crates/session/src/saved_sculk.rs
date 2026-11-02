//! Read and stage sculk sensor block entities without discarding vibrations.
//! The resulting partial edit does not authorize a world write.

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

    /// Preserve unrelated block-entity fields while updating the listener.
    /// Unknown listener fields cannot be safely carried to a different event.
    pub fn draft(
        &self,
        original: &Tag,
        registry: &BTreeSet<String>,
    ) -> Result<native_persistence::edit::BlockEntityChange> {
        ensure!(
            original == self.source(),
            "sensor block entity changed since loading"
        );
        let initial = decode(original.clone(), self.position, registry)?;
        ensure!(
            initial.calibrated == self.calibrated,
            "sensor block entity type changed"
        );
        if initial.last_frequency == self.last_frequency && initial.vibration == self.vibration {
            return Ok(native_persistence::edit::BlockEntityChange {
                expected: original.clone(),
                replacement: original.clone(),
            });
        }
        let mut replacement = original.clone();
        if initial.last_frequency != self.last_frequency {
            replace_field(
                &mut replacement,
                "last_vibration_frequency",
                Tag::Int(self.last_frequency),
            )?;
        }
        if initial.vibration == self.vibration {
            return Ok(native_persistence::edit::BlockEntityChange {
                expected: original.clone(),
                replacement,
            });
        }
        if let Some(listener) = unique(original, "listener")? {
            validate_listener_fields(listener)?;
        }
        ensure!(self.vibration.remaining() >= 0, "negative vibration delay");
        let mut selector = vec![(
            "tick".into(),
            Tag::Long(self.vibration.pending().map_or(-1, |(tick, _)| tick)),
        )];
        if let Some((_, event)) = self.vibration.pending() {
            selector.push(("event".into(), encode_event(event, registry)?));
        }
        let mut listener = vec![("selector".into(), Tag::Compound(selector))];
        if let Some(event) = self.vibration.current() {
            listener.push(("event".into(), encode_event(event, registry)?));
        }
        if self.vibration.remaining() != 0 {
            listener.push(("event_delay".into(), Tag::Int(self.vibration.remaining())));
        }
        replace_field(&mut replacement, "listener", Tag::Compound(listener))?;
        Ok(native_persistence::edit::BlockEntityChange {
            expected: original.clone(),
            replacement,
        })
    }
}

fn replace_field(tag: &mut Tag, name: &str, value: Tag) -> Result<()> {
    let Tag::Compound(fields) = tag else {
        bail!("expected a compound for {name}")
    };
    let mut positions = fields
        .iter()
        .enumerate()
        .filter_map(|(index, (key, _))| key.matches(name).then_some(index));
    let index = positions.next();
    ensure!(positions.next().is_none(), "duplicate field {name}");
    if let Some(index) = index {
        fields[index].1 = value;
    } else {
        fields.push((name.into(), value));
    }
    Ok(())
}

fn known_fields(tag: &Tag, allowed: &[&str]) -> Result<()> {
    let Tag::Compound(fields) = tag else {
        bail!("expected a compound")
    };
    let mut seen = BTreeSet::new();
    for (key, _) in fields {
        let name = String::from_utf16(&key.0).context("invalid listener field name")?;
        ensure!(
            allowed.contains(&name.as_str()),
            "unsupported listener field {name}"
        );
        ensure!(seen.insert(name), "duplicate listener field");
    }
    Ok(())
}

fn validate_listener_fields(listener: &Tag) -> Result<()> {
    known_fields(listener, &["event", "event_delay", "selector"])?;
    if let Some(event) = unique(listener, "event")? {
        known_fields(
            event,
            &[
                "game_event",
                "distance",
                "pos",
                "source",
                "projectile_owner",
            ],
        )?;
    }
    if let Some(selector) = unique(listener, "selector")? {
        known_fields(selector, &["event", "tick"])?;
        if let Some(event) = unique(selector, "event")? {
            known_fields(
                event,
                &[
                    "game_event",
                    "distance",
                    "pos",
                    "source",
                    "projectile_owner",
                ],
            )?;
        }
    }
    Ok(())
}

fn encode_event(event: &Vibration, registry: &BTreeSet<String>) -> Result<Tag> {
    let parsed = native_resources::packs::ResourceId::parse(&event.event)?;
    let id = format!("{}:{}", parsed.namespace, parsed.path);
    ensure!(
        registry.contains(&id),
        "game event is absent from the active registry"
    );
    ensure!(
        event.distance.is_finite() && event.distance >= 0.0,
        "invalid vibration distance"
    );
    ensure!(
        event.position.is_finite(),
        "vibration position is not finite"
    );
    let mut fields = vec![
        ("game_event".into(), Tag::String(id.as_str().into())),
        ("distance".into(), Tag::Float(event.distance)),
        (
            "pos".into(),
            Tag::List {
                element_type: TagType::Double,
                values: event
                    .position
                    .to_array()
                    .into_iter()
                    .map(Tag::Double)
                    .collect(),
            },
        ),
    ];
    if let Some(source) = event.source {
        fields.push(("source".into(), Tag::IntArray(source.to_vec())));
    }
    if let Some(owner) = event.projectile_owner {
        fields.push(("projectile_owner".into(), Tag::IntArray(owner.to_vec())));
    }
    Ok(Tag::Compound(fields))
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
    for (name, expected) in [("x", position.x), ("y", position.y), ("z", position.z)] {
        ensure!(
            matches!(unique(&entity,name)?, Some(Tag::Int(value)) if *value == expected),
            "sensor block entity coordinates changed"
        );
    }
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
    fn draft_preserves_original_and_roundtrips_pending_and_current_vibrations() {
        let original = sensor();
        let chunk = ChunkPos { x: 0, z: 0 };
        let position = IVec3::new(1, 4, 2);
        let registry = BTreeSet::from(["minecraft:splash".into(), "minecraft:step".into()]);
        let mut saved = read(&payload(vec![original.clone()]), chunk, position, &registry)
            .unwrap()
            .unwrap();
        let current = Vibration {
            event: "minecraft:splash".into(),
            distance: 4.25,
            position: DVec3::new(-1.25, 64.0, 3.75),
            source: Some([1, 2, 3, 4]),
            projectile_owner: Some([5, 6, 7, 8]),
        };
        let pending = Vibration {
            event: "minecraft:step".into(),
            distance: 2.5,
            position: DVec3::new(9.0, 1.5, -4.0),
            source: None,
            projectile_owner: None,
        };
        saved.vibration =
            VibrationData::from_saved(Some(current.clone()), 3, Some((99, pending.clone())));
        saved.last_frequency = 7;
        let change = saved.draft(&original, &registry).unwrap();
        assert_eq!(change.expected, original);
        assert_eq!(change.replacement.get("unknown"), Some(&Tag::Int(37)));
        let decoded = decode(change.replacement, position, &registry).unwrap();
        assert_eq!(decoded.last_frequency, 7);
        assert_eq!(decoded.vibration.current(), Some(&current));
        assert_eq!(decoded.vibration.pending(), Some((99, &pending)));
        assert_eq!(decoded.vibration.remaining(), 3);
        assert!(saved.draft(&sensor_with_extra_field(), &registry).is_err());
        let mut relocated = saved.clone();
        relocated.position.x += 1;
        assert!(relocated.draft(&original, &registry).is_err());
        assert!(
            saved
                .draft(&original, &BTreeSet::from(["minecraft:step".into()]))
                .is_err()
        );
        saved.vibration = VibrationData::from_saved(None, -1, None);
        assert!(saved.draft(&original, &registry).is_err());
    }

    fn sensor_with_extra_field() -> Tag {
        let Tag::Compound(mut fields) = sensor() else {
            unreachable!()
        };
        fields.push(("extra".into(), Tag::Int(1)));
        Tag::Compound(fields)
    }

    #[test]
    fn draft_refuses_unknown_listener_fields_when_listener_changes() {
        let Tag::Compound(mut fields) = sensor() else {
            unreachable!()
        };
        let listener = fields
            .iter_mut()
            .find(|(name, _)| name.matches("listener"))
            .unwrap();
        let Tag::Compound(listener_fields) = &mut listener.1 else {
            unreachable!()
        };
        listener_fields.push(("future".into(), Tag::Int(42)));
        let original = Tag::Compound(fields);
        let registry = BTreeSet::from(["minecraft:splash".into()]);
        let mut saved = decode(original.clone(), IVec3::new(1, 4, 2), &registry).unwrap();
        assert_eq!(
            saved.draft(&original, &registry).unwrap().replacement,
            original
        );
        saved.last_frequency = 2;
        let frequency_only = saved.draft(&original, &registry).unwrap().replacement;
        assert_eq!(frequency_only.get("listener"), original.get("listener"));
        assert_eq!(
            frequency_only.get("last_vibration_frequency"),
            Some(&Tag::Int(2))
        );
        saved.vibration.offer(
            Vibration {
                event: "minecraft:splash".into(),
                distance: 1.0,
                position: DVec3::ZERO,
                source: None,
                projectile_owner: None,
            },
            10,
        );
        assert!(saved.draft(&original, &registry).is_err());
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
