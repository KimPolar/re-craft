//! Java 26.3 structure terrain adjustments for already known pieces.
//! Structure generation and old-chunk blending are separate stages.
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundingBox {
    pub min: [i32; 3],
    pub max: [i32; 3],
}
impl BoundingBox {
    pub fn new(min: [i32; 3], max: [i32; 3]) -> Result<Self, &'static str> {
        if (0..3).any(|i| min[i] > max[i]) {
            return Err("invalid bounding box");
        }
        Ok(Self { min, max })
    }
    fn include(&mut self, other: Self) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(other.min[i]);
            self.max[i] = self.max[i].max(other.max[i]);
        }
    }
    fn contains(&self, p: [i32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }
    fn intersects(&self, other: &Self) -> bool {
        (0..3).all(|i| self.max[i] >= other.min[i] && self.min[i] <= other.max[i])
    }
    fn inflate(&self, n: i32) -> Result<Self, &'static str> {
        let mut min = [0; 3];
        let mut max = [0; 3];
        for i in 0..3 {
            min[i] = self.min[i].checked_sub(n).ok_or("bounding box overflow")?;
            max[i] = self.max[i].checked_add(n).ok_or("bounding box overflow")?;
        }
        Ok(Self { min, max })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerrainAdjustment {
    None,
    Bury,
    BeardThin,
    BeardBox,
    Encapsulate,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RigidPiece {
    pub bounds: BoundingBox,
    pub adjustment: TerrainAdjustment,
    pub ground_delta: i32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Junction {
    pub position: [i32; 3],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PieceProjection {
    NonPool,
    Rigid,
    TerrainMatching,
}
/// Pool-piece junctions must be supplied in their original generation order.
#[derive(Clone, Debug)]
pub struct StructurePiece {
    pub bounds: BoundingBox,
    pub adjustment: TerrainAdjustment,
    pub ground_delta: i32,
    pub projection: PieceProjection,
    pub junctions: Vec<Junction>,
}
#[derive(Clone, Debug)]
pub struct Beardifier {
    pieces: Vec<RigidPiece>,
    junctions: Vec<Junction>,
    affected: Option<BoundingBox>,
}
impl Beardifier {
    /// Direct Java constructor equivalent. `affected` controls clipping and
    /// may intentionally be absent even when lists are nonempty.
    pub fn new(
        pieces: Vec<RigidPiece>,
        junctions: Vec<Junction>,
        affected: Option<BoundingBox>,
    ) -> Self {
        Self {
            pieces,
            junctions,
            affected,
        }
    }
    /// Collect the already-generated pieces referenced by a target chunk.
    /// This does not discover starts or generate structure pieces.
    pub fn for_chunk(chunk: [i32; 2], pieces: &[StructurePiece]) -> Result<Self, &'static str> {
        let x = chunk[0].checked_mul(16).ok_or("chunk X overflow")?;
        let z = chunk[1].checked_mul(16).ok_or("chunk Z overflow")?;
        let min_x = x.checked_sub(12).ok_or("chunk X overflow")?;
        let min_z = z.checked_sub(12).ok_or("chunk Z overflow")?;
        let max_x = x.checked_add(27).ok_or("chunk X overflow")?;
        let max_z = z.checked_add(27).ok_or("chunk Z overflow")?;
        let mut rigids = Vec::new();
        let mut junctions = Vec::new();
        let mut affected: Option<BoundingBox> = None;
        let mut include = |b: BoundingBox| {
            if let Some(a) = &mut affected {
                a.include(b)
            } else {
                affected = Some(b)
            }
        };
        for piece in pieces {
            if piece.adjustment == TerrainAdjustment::None
                || piece.bounds.max[0] < min_x
                || piece.bounds.min[0] > max_x
                || piece.bounds.max[2] < min_z
                || piece.bounds.min[2] > max_z
            {
                continue;
            }
            if piece.projection != PieceProjection::TerrainMatching {
                rigids.push(RigidPiece {
                    bounds: piece.bounds,
                    adjustment: piece.adjustment,
                    ground_delta: if piece.projection == PieceProjection::NonPool {
                        0
                    } else {
                        piece.ground_delta
                    },
                });
                include(piece.bounds);
            }
            if piece.projection != PieceProjection::NonPool {
                for &junction in &piece.junctions {
                    let [jx, jy, jz] = junction.position;
                    if jx > min_x && jz > min_z && jx < max_x && jz < max_z {
                        junctions.push(junction);
                        include(BoundingBox {
                            min: [jx, jy, jz],
                            max: [jx, jy, jz],
                        });
                    }
                }
            }
        }
        let affected = affected.map(|bounds| bounds.inflate(24)).transpose()?;
        Ok(Self::new(rigids, junctions, affected))
    }
    pub fn affected_bounds(&self) -> Option<BoundingBox> {
        self.affected
    }
    pub fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        if self.affected.is_some_and(|b| b.contains([x, y, z])) {
            self.sample_unchecked(x, y, z)
        } else {
            0.0
        }
    }
    fn sample_unchecked(&self, x: i32, y: i32, z: i32) -> f32 {
        let mut value = 0.0f32;
        for piece in &self.pieces {
            let b = piece.bounds;
            let dx = 0.max(b.min[0].wrapping_sub(x).max(x.wrapping_sub(b.max[0])));
            let dz = 0.max(b.min[2].wrapping_sub(z).max(z.wrapping_sub(b.max[2])));
            let ground = b.min[1].wrapping_add(piece.ground_delta);
            let dy_ground = y.wrapping_sub(ground);
            let dy = match piece.adjustment {
                TerrainAdjustment::None => 0,
                TerrainAdjustment::Bury | TerrainAdjustment::BeardThin => dy_ground,
                TerrainAdjustment::BeardBox => {
                    0.max(ground.wrapping_sub(y).max(y.wrapping_sub(b.max[1])))
                }
                TerrainAdjustment::Encapsulate => {
                    0.max(b.min[1].wrapping_sub(y).max(y.wrapping_sub(b.max[1])))
                }
            };
            value += match piece.adjustment {
                TerrainAdjustment::None => 0.0,
                TerrainAdjustment::Bury => bury(dx as f32, dy as f32 / 2.0, dz as f32),
                TerrainAdjustment::BeardThin | TerrainAdjustment::BeardBox => {
                    beard(dx, dy, dz, dy_ground) * 0.8
                }
                TerrainAdjustment::Encapsulate => {
                    bury(dx as f32 / 2.0, dy as f32 / 2.0, dz as f32 / 2.0) * 0.8
                }
            };
        }
        for junction in &self.junctions {
            let dx = x.wrapping_sub(junction.position[0]);
            let dy = y.wrapping_sub(junction.position[1]);
            let dz = z.wrapping_sub(junction.position[2]);
            value += beard(dx, dy, dz, dy) * 0.4;
        }
        value
    }
    /// Java-order Z, X, Y sampling. Stepped volumes intentionally include the
    /// first sample just before an affected-box boundary, as Java does.
    pub fn sample_volume(
        &self,
        min: [i32; 3],
        size: [u32; 3],
        step: [i32; 3],
    ) -> Result<Vec<f32>, &'static str> {
        if size.contains(&0) || step.iter().any(|n| *n <= 0) {
            return Err("invalid density volume");
        }
        let count = size
            .iter()
            .try_fold(1usize, |n, &v| n.checked_mul(v as usize))
            .ok_or("volume overflow")?;
        if count > 1_048_576 {
            return Err("volume too large");
        }
        let mut max = [0; 3];
        for i in 0..3 {
            max[i] = i32::try_from(i64::from(min[i]) + i64::from(size[i] - 1) * i64::from(step[i]))
                .map_err(|_| "volume coordinate overflow")?;
        }
        let mut output = vec![0.0; count];
        let Some(affected) = self.affected else {
            return Ok(output);
        };
        if !affected.intersects(&BoundingBox { min, max }) {
            return Ok(output);
        }
        let mut lo = [0usize; 3];
        let mut hi = [0usize; 3];
        for i in 0..3 {
            let start = i64::from(affected.min[i]) - i64::from(min[i]);
            let end = i64::from(affected.max[i]) - i64::from(min[i]);
            lo[i] = (start.max(0) / i64::from(step[i])) as usize;
            hi[i] = ((end.div_euclid(i64::from(step[i]))).min(i64::from(size[i] - 1))) as usize;
        }
        for z in lo[2]..=hi[2] {
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    let indices = [x, y, z];
                    let p: [i32; 3] = std::array::from_fn(|i| {
                        (i64::from(min[i]) + indices[i] as i64 * i64::from(step[i])) as i32
                    });
                    output[(z * size[0] as usize + x) * size[1] as usize + y] =
                        self.sample_unchecked(p[0], p[1], p[2]);
                }
            }
        }
        Ok(output)
    }
}

fn bury(x: f32, y: f32, z: f32) -> f32 {
    let squared = x * x + y * y + z * z;
    if squared >= 36.0 {
        0.0
    } else {
        1.0 - (f64::from(squared).sqrt() as f32) / 6.0
    }
}
fn fast_inv_sqrt(x: f64) -> f64 {
    let half = 0.5 * x;
    let bits = x.to_bits() as i64;
    let guess = 6910469410427058090i64.wrapping_sub(bits >> 1);
    let mut value = f64::from_bits(guess as u64);
    value *= 1.5 - half * value * value;
    value
}
fn beard(dx: i32, dy: i32, dz: i32, ground_y: i32) -> f32 {
    let x = dx.wrapping_add(12);
    let y = dy.wrapping_add(12);
    let z = dz.wrapping_add(12);
    if !(0..24).contains(&x) || !(0..24).contains(&y) || !(0..24).contains(&z) {
        return 0.0;
    }
    static KERNEL: OnceLock<Box<[f32]>> = OnceLock::new();
    let kernel = KERNEL.get_or_init(|| {
        let mut values = Vec::with_capacity(24 * 24 * 24);
        for z in -12..12 {
            for x in -12..12 {
                for y in -12..12 {
                    let yd = f64::from(y) + 0.5;
                    let squared =
                        f64::from(x) * f64::from(x) + yd * yd + f64::from(z) * f64::from(z);
                    values.push(std::f64::consts::E.powf(-squared / 16.0) as f32);
                }
            }
        }
        values.into_boxed_slice()
    });
    let dy = ground_y as f32 + 0.5;
    let dx = dx as f32;
    let dz = dz as f32;
    let squared = dx * dx + dy * dy + dz * dz;
    let value = -dy * (fast_inv_sqrt(f64::from(squared / 2.0)) as f32) / 2.0;
    value * kernel[(z as usize * 24 + x as usize) * 24 + y as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(min: [i32; 3], max: [i32; 3]) -> BoundingBox {
        BoundingBox::new(min, max).unwrap()
    }

    #[test]
    fn collects_only_nearby_pieces_and_strictly_inside_junctions() {
        let junction = |x, y, z| Junction {
            position: [x, y, z],
        };
        let pieces = vec![
            StructurePiece {
                bounds: bounds([-12, 5, 0], [-12, 8, 1]),
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 99,
                projection: PieceProjection::NonPool,
                junctions: vec![junction(0, 3, 0)],
            },
            StructurePiece {
                bounds: bounds([27, 10, 0], [27, 12, 1]),
                adjustment: TerrainAdjustment::BeardThin,
                ground_delta: 2,
                projection: PieceProjection::Rigid,
                junctions: vec![junction(-12, 1, 0), junction(27, 1, 0), junction(-11, 1, 0)],
            },
            StructurePiece {
                bounds: bounds([0, 0, 0], [1, 2, 1]),
                adjustment: TerrainAdjustment::BeardBox,
                ground_delta: 4,
                projection: PieceProjection::TerrainMatching,
                junctions: vec![junction(0, -4, 26), junction(0, 0, 27)],
            },
            StructurePiece {
                bounds: bounds([28, 0, 0], [29, 2, 1]),
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 0,
                projection: PieceProjection::Rigid,
                junctions: vec![junction(0, 0, 0)],
            },
            StructurePiece {
                bounds: bounds([0, 0, 0], [1, 2, 1]),
                adjustment: TerrainAdjustment::None,
                ground_delta: 0,
                projection: PieceProjection::Rigid,
                junctions: vec![junction(0, 0, 0)],
            },
        ];
        let actual = Beardifier::for_chunk([0, 0], &pieces).unwrap();
        let expected = Beardifier::new(
            vec![
                RigidPiece {
                    bounds: pieces[0].bounds,
                    adjustment: TerrainAdjustment::Bury,
                    ground_delta: 0,
                },
                RigidPiece {
                    bounds: pieces[1].bounds,
                    adjustment: TerrainAdjustment::BeardThin,
                    ground_delta: 2,
                },
            ],
            vec![junction(-11, 1, 0), junction(0, -4, 26)],
            Some(bounds([-36, -28, -24], [51, 36, 50])),
        );
        assert_eq!(actual.affected_bounds(), expected.affected_bounds());
        for x in -40..55 {
            for y in -10..20 {
                for z in -2..30 {
                    assert_eq!(
                        actual.sample(x, y, z).to_bits(),
                        expected.sample(x, y, z).to_bits()
                    );
                }
            }
        }
        assert!(
            Beardifier::for_chunk([0, 0], &[])
                .unwrap()
                .affected_bounds()
                .is_none()
        );
    }

    #[test]
    fn stepped_volume_handles_large_intermediate_offsets() {
        let adjustment = Beardifier::new(vec![], vec![], Some(bounds([-1, 0, 0], [1, 0, 0])));
        assert_eq!(
            adjustment
                .sample_volume([i32::MIN, 0, 0], [3, 1, 1], [1_073_741_824, 1, 1])
                .unwrap(),
            vec![0.0; 3]
        );
        assert!(
            adjustment
                .sample_volume([i32::MAX, 0, 0], [2, 1, 1], [1, 1, 1])
                .is_err()
        );
    }
}
