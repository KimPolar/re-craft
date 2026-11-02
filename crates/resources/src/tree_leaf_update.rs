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
// important when a pre-existing leaf already has a short distance.
#[derive(Default)]
struct PositionSet {
    buckets: Vec<Bucket>,
    len: usize,
}

#[derive(Default)]
struct Bucket {
    order: Vec<Pos>,
    tree: Option<TreeBin>,
    removed_from_tree: bool,
}

struct TreeNode {
    pos: Pos,
    parent: Option<usize>,
    left: Option<usize>,
    right: Option<usize>,
    red: bool,
}

#[derive(Default)]
struct TreeBin {
    nodes: Vec<TreeNode>,
    root: Option<usize>,
}
impl TreeBin {
    fn rotate(&mut self, at: usize, left: bool) {
        let child = if left {
            self.nodes[at].right.unwrap()
        } else {
            self.nodes[at].left.unwrap()
        };
        let middle = if left {
            self.nodes[child].left
        } else {
            self.nodes[child].right
        };
        if left {
            self.nodes[at].right = middle;
        } else {
            self.nodes[at].left = middle;
        }
        if let Some(m) = middle {
            self.nodes[m].parent = Some(at);
        }
        let parent = self.nodes[at].parent;
        self.nodes[child].parent = parent;
        if let Some(p) = parent {
            if self.nodes[p].left == Some(at) {
                self.nodes[p].left = Some(child);
            } else {
                self.nodes[p].right = Some(child);
            }
        } else {
            self.root = Some(child);
        }
        if left {
            self.nodes[child].left = Some(at);
        } else {
            self.nodes[child].right = Some(at);
        }
        self.nodes[at].parent = Some(child);
    }
    /// Insert by Java's signed spread hash. Equal hashes use JVM identity and
    /// cannot be reproduced from coordinates alone.
    fn insert(&mut self, pos: Pos) -> Result<Option<Pos>> {
        let mut parent = None;
        let mut cursor = self.root;
        let hash = PositionSet::hash(pos) as i32;
        while let Some(i) = cursor {
            parent = Some(i);
            let other = PositionSet::hash(self.nodes[i].pos) as i32;
            ensure!(
                hash != other,
                "unsupported Java identity-dependent hash collision"
            );
            cursor = if hash < other {
                self.nodes[i].left
            } else {
                self.nodes[i].right
            };
        }
        let index = self.nodes.len();
        self.nodes.push(TreeNode {
            pos,
            parent,
            left: None,
            right: None,
            red: true,
        });
        if let Some(p) = parent {
            if hash < PositionSet::hash(self.nodes[p].pos) as i32 {
                self.nodes[p].left = Some(index);
            } else {
                self.nodes[p].right = Some(index);
            }
        } else {
            self.root = Some(index);
        }
        let parent_pos = parent.map(|p| self.nodes[p].pos);
        let mut x = index;
        while let Some(p) = self.nodes[x].parent {
            if !self.nodes[p].red {
                break;
            }
            let Some(g) = self.nodes[p].parent else { break };
            let parent_on_left = self.nodes[g].left == Some(p);
            let uncle = if parent_on_left {
                self.nodes[g].right
            } else {
                self.nodes[g].left
            };
            if uncle.is_some_and(|u| self.nodes[u].red) {
                self.nodes[p].red = false;
                self.nodes[uncle.unwrap()].red = false;
                self.nodes[g].red = true;
                x = g;
                continue;
            }
            let inner = if parent_on_left {
                self.nodes[p].right == Some(x)
            } else {
                self.nodes[p].left == Some(x)
            };
            if inner {
                x = p;
                self.rotate(x, parent_on_left);
            }
            let p = self.nodes[x].parent.unwrap();
            let g = self.nodes[p].parent.unwrap();
            self.nodes[p].red = false;
            self.nodes[g].red = true;
            self.rotate(g, !parent_on_left);
        }
        self.nodes[self.root.unwrap()].red = false;
        Ok(parent_pos)
    }
    fn root_pos(&self) -> Pos {
        self.nodes[self.root.unwrap()].pos
    }
}

impl Bucket {
    fn move_root_front(&mut self) {
        let root = self.tree.as_ref().unwrap().root_pos();
        let index = self.order.iter().position(|p| *p == root).unwrap();
        if index != 0 {
            self.order.remove(index);
            self.order.insert(0, root);
        }
    }
    fn treeify(&mut self) -> Result<()> {
        let mut tree = TreeBin::default();
        for &p in &self.order {
            tree.insert(p)?;
        }
        self.tree = Some(tree);
        self.move_root_front();
        Ok(())
    }
}
impl PositionSet {
    fn hash(p: Pos) -> u32 {
        let h = p[1]
            .wrapping_add(p[2].wrapping_mul(31))
            .wrapping_mul(31)
            .wrapping_add(p[0]) as u32;
        h ^ (h >> 16)
    }
    fn resize(&mut self) -> Result<()> {
        let old_capacity = self.buckets.len();
        let n = if self.buckets.is_empty() {
            16
        } else {
            self.buckets.len() * 2
        };
        let mut buckets: Vec<Bucket> = (0..n).map(|_| Bucket::default()).collect();
        for (index, bucket) in self.buckets.drain(..).enumerate() {
            if bucket.tree.is_none() {
                for p in bucket.order {
                    buckets[Self::hash(p) as usize & (n - 1)].order.push(p);
                }
                continue;
            }
            let (mut low, mut high) = (Vec::new(), Vec::new());
            for p in bucket.order {
                if Self::hash(p) as usize & old_capacity == 0 {
                    low.push(p)
                } else {
                    high.push(p)
                }
            }
            let split = !low.is_empty() && !high.is_empty();
            let mut tree = bucket.tree;
            for (target, order) in [(index, low), (index + old_capacity, high)] {
                if order.is_empty() {
                    continue;
                }
                let entry = &mut buckets[target];
                entry.order = order;
                if entry.order.len() <= 6 {
                    continue;
                }
                if split {
                    entry.treeify()?;
                } else {
                    entry.tree = tree.take();
                    entry.removed_from_tree = bucket.removed_from_tree;
                }
            }
        }
        self.buckets = buckets;
        Ok(())
    }
    fn insert(&mut self, p: Pos) -> Result<()> {
        if self.buckets.is_empty() {
            self.resize()?;
        }
        let i = Self::hash(p) as usize & (self.buckets.len() - 1);
        if self.buckets[i].order.contains(&p) {
            return Ok(());
        }
        if self.buckets[i].tree.is_some() {
            ensure!(
                !self.buckets[i].removed_from_tree,
                "unsupported insertion after Java tree-bin removal"
            );
            let parent = self.buckets[i]
                .tree
                .as_mut()
                .unwrap()
                .insert(p)?
                .expect("nonempty tree");
            let index = self.buckets[i]
                .order
                .iter()
                .position(|v| *v == parent)
                .unwrap();
            self.buckets[i].order.insert(index + 1, p);
            self.buckets[i].move_root_front();
        } else {
            self.buckets[i].order.push(p);
        }
        self.len += 1;
        if self.buckets[i].tree.is_none() && self.buckets[i].order.len() >= 9 {
            if self.buckets.len() < 64 {
                self.resize()?;
            } else {
                self.buckets[i].treeify()?;
            }
        }
        if self.len > self.buckets.len() * 3 / 4 {
            self.resize()?;
        }
        Ok(())
    }
    fn pop_first(&mut self) -> Option<Pos> {
        for bucket in &mut self.buckets {
            if !bucket.order.is_empty() {
                self.len -= 1;
                if bucket.tree.is_some() {
                    bucket.removed_from_tree = true;
                }
                let pos = bucket.order.remove(0);
                if bucket.order.is_empty() {
                    bucket.tree = None;
                    bucket.removed_from_tree = false;
                }
                return Some(pos);
            }
        }
        None
    }
    fn values(&self) -> impl Iterator<Item = Pos> + '_ {
        self.buckets
            .iter()
            .flat_map(|bucket| bucket.order.iter().copied())
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

#[cfg(test)]
mod tests {
    use super::PositionSet;

    fn capacity_64() -> PositionSet {
        let mut set = PositionSet::default();
        set.resize().unwrap();
        set.resize().unwrap();
        set.resize().unwrap();
        set
    }

    #[test]
    fn identity_dependent_tree_collisions_and_stale_tree_inserts_are_rejected() {
        let mut collisions = capacity_64();
        for k in 0..8 {
            collisions.insert([31 + 64 * k, 0, 0]).unwrap();
        }
        assert!(
            collisions
                .insert([0, 1, 0])
                .unwrap_err()
                .to_string()
                .contains("identity-dependent")
        );

        let mut set = capacity_64();
        for k in 0..9 {
            set.insert([64 * k, 0, 0]).unwrap();
        }
        set.pop_first().unwrap();
        assert!(
            set.insert([64 * 10, 0, 0])
                .unwrap_err()
                .to_string()
                .contains("after Java tree-bin removal")
        );
    }
}
