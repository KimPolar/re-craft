//! Shared clipping checks for entities stored in structure templates.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::{Tag, TagType};
use native_resources::structure_transform::{Mirror, Rotation};

// Java clips template entities using their transformed integer blockPos,
// independently of their floating-point Pos. Reject records whose location
// cannot be established instead of silently omitting a possible entity.
pub(super) fn template_entities_intersect(
    entities: &[Tag],
    origin: [i32; 3],
    mirror: Mirror,
    rotation: Rotation,
    pivot: [i32; 3],
    clip: BoundingBox,
) -> Result<bool> {
    ensure!(
        (0..3).all(|axis| clip.min[axis] <= clip.max[axis]),
        "invalid template entity clipping box"
    );
    for entity in entities {
        let world = template_entity_anchor(entity, origin, mirror, rotation, pivot)?;
        if (0..3).all(|axis| (clip.min[axis]..=clip.max[axis]).contains(&world[axis])) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn template_entity_anchor(
    entity: &Tag,
    origin: [i32; 3],
    mirror: Mirror,
    rotation: Rotation,
    pivot: [i32; 3],
) -> Result<[i32; 3]> {
    let Tag::Compound(fields) = entity else {
        anyhow::bail!("invalid template entity");
    };
    let mut positions = fields.iter().filter(|(name, _)| name.matches("blockPos"));
    let (_, position) = positions
        .next()
        .context("template entity blockPos missing")?;
    ensure!(
        positions.next().is_none(),
        "duplicate template entity blockPos"
    );
    let local = match position {
        Tag::IntArray(values) => match values.as_slice() {
            [x, y, z] => [*x, *y, *z],
            _ => anyhow::bail!("invalid template entity blockPos length"),
        },
        Tag::List {
            element_type: TagType::Int,
            values,
        } => match values.as_slice() {
            [Tag::Int(x), Tag::Int(y), Tag::Int(z)] => [*x, *y, *z],
            _ => anyhow::bail!("invalid template entity blockPos"),
        },
        _ => anyhow::bail!("invalid template entity blockPos"),
    };
    Ok(native_resources::structure_transform::block_world_position(
        local, origin, mirror, rotation, pivot,
    ))
}

/// Transform only entities whose integer anchor intersects this placement pass.
/// Chunk ownership is currently supported when floating and integer positions
/// resolve to the same block; reject other cases rather than lose an entity.
pub(super) fn template_entity_location(
    entity: &Tag,
    origin: [i32; 3],
    rotation: Rotation,
    pivot: [i32; 3],
    clip: BoundingBox,
) -> Result<Option<([i32; 3], [f64; 3])>> {
    ensure!(
        (0..3).all(|axis| clip.min[axis] <= clip.max[axis]),
        "invalid template entity clipping box"
    );
    let anchor = template_entity_anchor(entity, origin, Mirror::None, rotation, pivot)?;
    if !(0..3).all(|axis| (clip.min[axis]..=clip.max[axis]).contains(&anchor[axis])) {
        return Ok(None);
    }
    let Tag::Compound(outer) = entity else {
        anyhow::bail!("invalid template entity")
    };
    for key in ["pos", "nbt"] {
        ensure!(
            outer.iter().filter(|(name, _)| name.matches(key)).count() == 1,
            "missing or duplicate template entity {key}"
        );
    }
    let local = match entity.get("pos") {
        Some(Tag::List {
            element_type: TagType::Double,
            values,
        }) => match values.as_slice() {
            [Tag::Double(x), Tag::Double(y), Tag::Double(z)] => [*x, *y, *z],
            _ => anyhow::bail!("invalid template entity pos"),
        },
        _ => anyhow::bail!("invalid template entity pos"),
    };
    ensure!(
        local.iter().all(|value| value.is_finite()),
        "nonfinite template entity position"
    );
    let transformed = native_resources::structure_transform::transform_entity(
        local,
        Mirror::None,
        rotation,
        pivot,
    );
    let position =
        std::array::from_fn::<_, 3, _>(|axis| transformed[axis] + f64::from(origin[axis]));
    ensure!(
        position.iter().all(|v| v.is_finite())
            && position[0].abs() <= 30_000_512.0
            && position[2].abs() <= 30_000_512.0
            && position[1].abs() <= 20_000_000.0,
        "template entity position is outside supported bounds"
    );
    ensure!(
        (0..3).all(|axis| position[axis].floor() == f64::from(anchor[axis])),
        "template entity position differs from its anchor"
    );
    Ok(Some((anchor, position)))
}

pub(super) fn template_entity_yaw(yaw: f32, rotation: Rotation) -> f32 {
    let loaded = yaw % 360.0;
    let mut wrapped = loaded % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    let rotated = wrapped
        + match rotation {
            Rotation::None => 0.0,
            Rotation::Clockwise90 => 90.0,
            Rotation::Clockwise180 => 180.0,
            Rotation::Counterclockwise90 => 270.0,
        };
    rotated + (wrapped - loaded)
}

pub(super) fn draft_template_entity(
    entity: &Tag,
    origin: [i32; 3],
    rotation: Rotation,
    pivot: [i32; 3],
    clip: BoundingBox,
    uuid: impl FnOnce() -> Result<[i32; 4]>,
) -> Result<Option<([i32; 3], Tag)>> {
    let is_armor_stand = matches!(entity.get("nbt").and_then(|data| data.get("id")), Some(Tag::String(id)) if id.matches("minecraft:armor_stand"));
    if is_armor_stand {
        super::generated_armor_stand::draft_template_armor_stand(
            entity, origin, rotation, pivot, clip, uuid,
        )
    } else {
        super::generated_cushion::draft_template_cushion(
            entity, origin, rotation, pivot, clip, uuid,
        )
    }
}

#[cfg(test)]
mod entity_clip_tests {
    use super::*;

    fn entity(position: Tag) -> Tag {
        Tag::Compound(vec![("blockPos".into(), position)])
    }

    #[test]
    fn template_entity_clipping_uses_transformed_integer_anchor() -> Result<()> {
        let list = Tag::List {
            element_type: TagType::Int,
            values: vec![Tag::Int(3), Tag::Int(4), Tag::Int(-2)],
        };
        let args = (
            [-32, 60, 48],
            Mirror::FrontBack,
            Rotation::Clockwise90,
            [2, 0, 3],
        );
        let point = BoundingBox::new([-25, 64, 46], [-25, 64, 46]).unwrap();
        let outside = BoundingBox::new([-24, 64, 46], [-24, 64, 46]).unwrap();
        for position in [list, Tag::IntArray(vec![3, 4, -2])] {
            let entities = [entity(position)];
            assert!(template_entities_intersect(
                &entities, args.0, args.1, args.2, args.3, point
            )?);
            assert!(!template_entities_intersect(
                &entities, args.0, args.1, args.2, args.3, outside
            )?);
        }
        assert!(!template_entities_intersect(
            &[],
            args.0,
            args.1,
            args.2,
            args.3,
            point
        )?);
        Ok(())
    }

    #[test]
    fn jigsaw_entity_clipping_tracks_rotation_and_inclusive_boundaries() -> Result<()> {
        let entities = [entity(Tag::IntArray(vec![3, 4, -2]))];
        for (rotation, world) in [
            (Rotation::None, [103, 24, 198]),
            (Rotation::Clockwise90, [102, 24, 203]),
            (Rotation::Clockwise180, [97, 24, 202]),
            (Rotation::Counterclockwise90, [98, 24, 197]),
        ] {
            let point = BoundingBox::new(world, world).map_err(anyhow::Error::msg)?;
            let excluded = BoundingBox::new(
                [world[0] + 1, world[1], world[2]],
                [world[0] + 1, world[1], world[2]],
            )
            .map_err(anyhow::Error::msg)?;
            assert!(template_entities_intersect(
                &entities,
                [100, 20, 200],
                Mirror::None,
                rotation,
                [0, 0, 0],
                point,
            )?);
            assert!(!template_entities_intersect(
                &entities,
                [100, 20, 200],
                Mirror::None,
                rotation,
                [0, 0, 0],
                excluded,
            )?);
        }
        Ok(())
    }

    #[test]
    fn template_entity_clipping_rejects_unknown_anchors() {
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let cases = [
            Tag::Compound(vec![]),
            entity(Tag::IntArray(vec![1, 2])),
            entity(Tag::String("1 2 3".into())),
            Tag::Compound(vec![
                ("blockPos".into(), Tag::IntArray(vec![1, 2, 3])),
                ("blockPos".into(), Tag::IntArray(vec![30, 2, 3])),
            ]),
        ];
        for value in cases {
            assert!(
                template_entities_intersect(
                    &[value],
                    [0, 0, 0],
                    Mirror::None,
                    Rotation::None,
                    [0, 0, 0],
                    clip
                )
                .is_err()
            );
        }
    }
}
