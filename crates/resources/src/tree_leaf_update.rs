//! Tree leaf-distance propagation and its occupied shape, before edge updates.
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

type Pos = [i32; 3];

#[derive(Clone, Copy)]
pub struct TreeBounds {
    pub min: Pos,
    pub max: Pos,
}
impl TreeBounds {
    fn contains(self, p: Pos) -> bool {
        (0..3).all(|i| self.min[i] <= p[i] && p[i] <= self.max[i])
    }
}

/// The world implements `LeavesBlock.getOptionalDistanceAt` using the active
/// `prevents_nearby_leaf_decay` tag and actual block-state property schema.
pub trait LeafUpdateWorld {
    fn optional_leaf_distance(&mut self, pos: Pos) -> Result<Option<u8>>;
    /// Apply Java's known-shape update (flags 19), preserving all other state.
    fn set_leaf_distance(&mut self, pos: Pos, distance: u8) -> Result<()>;
}

// The vanilla queues are java.util.HashSets. Retaining their bucket order is
// important when a pre-existing leaf already has a short distance. Tree bins
// are deliberately rejected until their ordering has also been implemented.
#[derive(Default)]
struct PositionSet {
    buckets: Vec<Vec<Pos>>,
    len: usize,
}
impl PositionSet {
    fn hash(p: Pos) -> u32 {
        let h = p[1]
            .wrapping_add(p[2].wrapping_mul(31))
            .wrapping_mul(31)
            .wrapping_add(p[0]) as u32;
        h ^ (h >> 16)
    }
    fn resize(&mut self) {
        let n = if self.buckets.is_empty() {
            16
        } else {
            self.buckets.len() * 2
        };
        let mut buckets = vec![Vec::new(); n];
        for bucket in self.buckets.drain(..) {
            for p in bucket {
                buckets[Self::hash(p) as usize & (n - 1)].push(p);
            }
        }
        self.buckets = buckets;
    }
    fn insert(&mut self, p: Pos) -> Result<()> {
        if self.buckets.is_empty() {
            self.resize();
        }
        let i = Self::hash(p) as usize & (self.buckets.len() - 1);
        if self.buckets[i].contains(&p) {
            return Ok(());
        }
        self.buckets[i].push(p);
        self.len += 1;
        if self.buckets[i].len() >= 9 {
            ensure!(
                self.buckets.len() < 64,
                "unsupported Java treeified position set"
            );
            self.resize();
        }
        if self.len > self.buckets.len() * 3 / 4 {
            self.resize();
        }
        Ok(())
    }
    fn pop_first(&mut self) -> Option<Pos> {
        for bucket in &mut self.buckets {
            if !bucket.is_empty() {
                self.len -= 1;
                return Some(bucket.remove(0));
            }
        }
        None
    }
    fn values(&self) -> impl Iterator<Item = Pos> + '_ {
        self.buckets
            .iter()
            .flat_map(|bucket| bucket.iter().copied())
    }
}

/// Update reachable leaves and return the occupied world positions used by
/// `StructureTemplate.updateShapeAtEdge`. The caller must still run that edge
/// update and must discard the enclosing transaction on errors.
pub fn update_tree_leaves(
    world: &mut impl LeafUpdateWorld,
    bounds: TreeBounds,
    logs: &[Pos],
    decorations: &[Pos],
    roots: &[Pos],
) -> Result<BTreeSet<Pos>> {
    ensure!(
        (0..3).all(|i| bounds.min[i] <= bounds.max[i]),
        "invalid tree bounding box"
    );
    let mut shape: BTreeSet<Pos> = decorations
        .iter()
        .chain(roots)
        .copied()
        .filter(|p| bounds.contains(*p))
        .collect();
    let mut seed = PositionSet::default();
    for &p in logs {
        seed.insert(p)?;
    }
    let mut queues: [PositionSet; 7] = std::array::from_fn(|_| PositionSet::default());
    for p in seed.values() {
        queues[0].insert(p)?;
    }
    let mut distance = 0usize;
    loop {
        while distance < 7 && queues[distance].len == 0 {
            distance += 1;
        }
        if distance >= 7 {
            break;
        }
        let p = queues[distance].pop_first().expect("nonempty queue");
        if !bounds.contains(p) {
            continue;
        }
        if distance != 0 {
            world.set_leaf_distance(p, distance as u8)?;
        }
        shape.insert(p);
        for [dx, dy, dz] in [
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
            [-1, 0, 0],
            [1, 0, 0],
        ] {
            let q = [
                p[0].wrapping_add(dx),
                p[1].wrapping_add(dy),
                p[2].wrapping_add(dz),
            ];
            if !bounds.contains(q) || shape.contains(&q) {
                continue;
            }
            if let Some(old) = world.optional_leaf_distance(q)? {
                ensure!(old <= 7, "invalid leaf distance");
                let next = usize::from(old).min(distance + 1);
                if next < 7 {
                    queues[next].insert(q)?;
                    distance = distance.min(next);
                }
            }
        }
    }
    Ok(shape)
}
