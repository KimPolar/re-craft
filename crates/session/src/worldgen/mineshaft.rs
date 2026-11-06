//! Prepare mineshaft starts and their save data; decoration is not implemented.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::TerrainAdjustment;
use native_persistence::nbt::{Tag, TagType};
use native_resources::{
    mineshaft::{MineshaftElevation, MineshaftGraph, MineshaftVariant},
    packs::ResourceId,
    structure_start::{BiomeQuery, Heightmap, StartEnvironment, decode_terrain_adjustment},
};
use serde_json::Value;

use super::{SavedMineshaftPiece, structure_references::StructureStartBounds};

/// Java's generator minimum can differ from the dimension's height accessor.
pub trait MineshaftEnvironment: StartEnvironment {
    fn generator_min_y(&self) -> i32;
}

/// All pieces of a fresh start, in Java's decoration order. A prepared start
/// does not imply that its blocks, entities or chunk save are complete.
#[derive(Clone, Debug)]
pub struct PreparedStart {
    pub chunk: [i32; 2],
    pub generation_point: [i32; 3],
    pub terrain_adjustment: TerrainAdjustment,
    pub variant: MineshaftVariant,
    pub pieces: Vec<SavedMineshaftPiece>,
}

impl PreparedStart {
    /// Evaluate the actual generation point before its final biome check.
    /// Unlike on-top-of-chunk-center structures, mineshafts do not use a
    /// preliminary vertical biome-column filter.
    pub fn generate(
        definition: &Value,
        env: &mut impl MineshaftEnvironment,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<Option<Self>> {
        let kind = ResourceId::parse(
            definition
                .get("type")
                .and_then(Value::as_str)
                .context("missing structure type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "mineshaft",
            "not a mineshaft structure"
        );
        let variant = match definition.get("mineshaft_type").and_then(Value::as_str) {
            Some("normal") => MineshaftVariant::Normal,
            Some("mesa") => MineshaftVariant::Mesa,
            _ => anyhow::bail!("invalid mineshaft type"),
        };
        let terrain_adjustment = decode_terrain_adjustment(definition)?;
        let (min_y, max_y) = env.vertical_bounds();
        ensure!(
            min_y <= max_y && i64::from(max_y) - i64::from(min_y) <= 4096,
            "invalid dimension bounds"
        );
        let graph = MineshaftGraph::new(seed, chunk, variant)?;
        let sea_level = env.sea_level();
        let elevation = match variant {
            MineshaftVariant::Normal => MineshaftElevation::Normal {
                sea_level,
                min_y: env.generator_min_y(),
            },
            MineshaftVariant::Mesa => {
                let center = graph.center();
                // Java getBaseHeight is one above getFirstOccupiedHeight.
                let surface_height = env
                    .first_occupied_height(center[0], center[2], Heightmap::WorldSurface)?
                    .checked_add(1)
                    .context("mineshaft surface height overflow")?;
                MineshaftElevation::Mesa {
                    sea_level,
                    surface_height,
                }
            }
        };
        let placed = graph.place(elevation)?;
        let generation_point = placed.generation_point;
        if !env.biome_allowed(generation_point.map(|v| v.div_euclid(4)), BiomeQuery::Point)? {
            return Ok(None);
        }
        let pieces = placed
            .pieces
            .iter()
            .map(|piece| SavedMineshaftPiece::capture(piece, variant, false))
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(Self {
            chunk,
            generation_point,
            terrain_adjustment,
            variant,
            pieces,
        }))
    }

    pub fn reference_bounds(&self, structure: ResourceId) -> Result<StructureStartBounds> {
        let bounds: Vec<_> = self.pieces.iter().map(|piece| piece.piece.bounds).collect();
        StructureStartBounds::from_pieces(structure, &bounds, self.terrain_adjustment)
    }

    /// Form a new start compound. This alone does not authorize a world write.
    pub fn to_tag(&self, structure: &ResourceId) -> Result<Tag> {
        ensure!(!self.pieces.is_empty(), "mineshaft has no pieces");
        ensure!(
            self.pieces
                .iter()
                .all(|piece| piece.variant == self.variant),
            "mineshaft piece variant mismatch"
        );
        let root = &self.pieces[0].piece;
        let x = self.chunk[0]
            .checked_mul(16)
            .and_then(|x| x.checked_add(2))
            .context("mineshaft chunk X overflow")?;
        let z = self.chunk[1]
            .checked_mul(16)
            .and_then(|z| z.checked_add(2))
            .context("mineshaft chunk Z overflow")?;
        ensure!(
            matches!(
                root.kind,
                native_resources::mineshaft::MineshaftGraphPieceKind::Room { .. }
            ) && root.bounds.min[0] == x
                && root.bounds.min[2] == z,
            "mineshaft root does not match chunk"
        );
        ensure!(
            self.generation_point == [x + 6, root.bounds.min[1], z - 2]
                && self.pieces[1..].iter().all(|piece| !matches!(
                    piece.piece.kind,
                    native_resources::mineshaft::MineshaftGraphPieceKind::Room { .. }
                )),
            "mineshaft generation point or room count is invalid"
        );
        let name = format!("{}:{}", structure.namespace, structure.path);
        Ok(Tag::Compound(vec![
            ("id".into(), Tag::String(name.as_str().into())),
            ("ChunkX".into(), Tag::Int(self.chunk[0])),
            ("ChunkZ".into(), Tag::Int(self.chunk[1])),
            ("references".into(), Tag::Int(0)),
            (
                "Children".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: self
                        .pieces
                        .iter()
                        .map(SavedMineshaftPiece::to_tag)
                        .collect::<Result<Vec<_>>>()?,
                },
            ),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::beardifier::BoundingBox;
    use serde_json::json;

    struct Environment {
        allow: bool,
        queried: Vec<(BiomeQuery, [i32; 3])>,
        sampled: Vec<(i32, i32, Heightmap)>,
        occupied: i32,
    }
    impl StartEnvironment for Environment {
        fn vertical_bounds(&self) -> (i32, i32) {
            (-64, 319)
        }
        fn sea_level(&self) -> i32 {
            63
        }
        fn first_occupied_height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
            self.sampled.push((x, z, map));
            Ok(self.occupied)
        }
        fn biome_allowed(&mut self, q: [i32; 3], query: BiomeQuery) -> Result<bool> {
            self.queried.push((query, q));
            Ok(self.allow)
        }
    }
    impl MineshaftEnvironment for Environment {
        fn generator_min_y(&self) -> i32 {
            -32
        }
    }
    fn environment(allow: bool) -> Environment {
        Environment {
            allow,
            queried: vec![],
            sampled: vec![],
            occupied: 63,
        }
    }

    #[test]
    fn mesa_samples_base_height_and_only_final_generation_biome() {
        let definition = json!({"type":"minecraft:mineshaft", "mineshaft_type":"mesa"});
        let graph = MineshaftGraph::new(42, [4, -1], MineshaftVariant::Mesa).unwrap();
        let center = graph.center();
        let expected = graph
            .place(MineshaftElevation::Mesa {
                sea_level: 63,
                surface_height: 64,
            })
            .unwrap();
        let mut env = environment(true);
        let actual = PreparedStart::generate(&definition, &mut env, 42, [4, -1])
            .unwrap()
            .unwrap();
        assert_eq!(actual.generation_point, expected.generation_point);
        assert_eq!(
            env.sampled,
            vec![(center[0], center[2], Heightmap::WorldSurface)]
        );
        assert_eq!(
            env.queried,
            vec![(
                BiomeQuery::Point,
                actual.generation_point.map(|v| v.div_euclid(4))
            )]
        );
        assert_eq!(actual.pieces.len(), expected.pieces.len());
        for (actual, expected) in actual.pieces.iter().zip(expected.pieces) {
            assert_eq!(actual.piece, expected);
        }
    }

    #[test]
    fn normal_rejects_final_biome_without_sampling_height_and_can_pack_a_start() {
        let definition = json!({"type":"minecraft:mineshaft", "mineshaft_type":"normal"});
        let mut env = environment(false);
        assert!(
            PreparedStart::generate(&definition, &mut env, 42, [4, -1])
                .unwrap()
                .is_none()
        );
        assert!(env.sampled.is_empty());
        assert_eq!(env.queried.len(), 1);
        assert_eq!(env.queried[0].0, BiomeQuery::Point);
        let mut env = environment(true);
        let mut start = PreparedStart::generate(&definition, &mut env, 42, [4, -1])
            .unwrap()
            .unwrap();
        let expected = MineshaftGraph::new(42, [4, -1], MineshaftVariant::Normal)
            .unwrap()
            .place(MineshaftElevation::Normal {
                sea_level: 63,
                min_y: -32,
            })
            .unwrap();
        assert_eq!(start.generation_point, expected.generation_point);
        let id = ResourceId::parse("minecraft:mineshaft").unwrap();
        let bounds = start.reference_bounds(id.clone()).unwrap().bounds;
        for piece in &start.pieces {
            let b: BoundingBox = piece.piece.bounds;
            assert!(
                (0..3)
                    .all(|axis| bounds.min[axis] <= b.min[axis] && bounds.max[axis] >= b.max[axis])
            );
        }
        let tag = start.to_tag(&id).unwrap();
        assert!(
            matches!(tag.get("id"), Some(Tag::String(name)) if name.matches("minecraft:mineshaft"))
        );
        assert!(
            matches!(tag.get("Children"), Some(Tag::List { values, .. }) if values.len() == start.pieces.len())
        );
        start.generation_point[0] += 1;
        assert!(start.to_tag(&id).is_err());
        start.generation_point[0] -= 1;
        start.chunk[0] += 1;
        assert!(start.to_tag(&id).is_err());
    }
}
