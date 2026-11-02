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
    free: Vec<usize>,
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
        let node = TreeNode {
            pos,
            parent,
            left: None,
            right: None,
            red: true,
        };
        let index = if let Some(index) = self.free.pop() {
            self.nodes[index] = node;
            index
        } else {
            self.nodes.push(node);
            self.nodes.len() - 1
        };
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

    fn replace(&mut self, old: usize, new: Option<usize>) {
        let parent = self.nodes[old].parent;
        if let Some(parent) = parent {
            if self.nodes[parent].left == Some(old) {
                self.nodes[parent].left = new;
            } else {
                self.nodes[parent].right = new;
            }
        } else {
            self.root = new;
        }
        if let Some(new) = new {
            self.nodes[new].parent = parent;
        }
    }

    fn is_red(&self, node: Option<usize>) -> bool {
        node.is_some_and(|node| self.nodes[node].red)
    }

    fn child(&self, node: Option<usize>, left: bool) -> Option<usize> {
        node.and_then(|node| {
            if left {
                self.nodes[node].left
            } else {
                self.nodes[node].right
            }
        })
    }

    fn fix_removal(
        &mut self,
        mut node: Option<usize>,
        mut parent: Option<usize>,
        mut absent_left: bool,
    ) {
        while node != self.root && !self.is_red(node) {
            let Some(p) = parent else {
                break;
            };
            let left = node.map_or(absent_left, |node| self.nodes[p].left == Some(node));
            let mut sibling = self.child(Some(p), !left);
            if self.is_red(sibling) {
                self.nodes[sibling.unwrap()].red = false;
                self.nodes[p].red = true;
                self.rotate(p, left);
                sibling = self.child(Some(p), !left);
            }
            if !self.is_red(self.child(sibling, true)) && !self.is_red(self.child(sibling, false)) {
                if let Some(sibling) = sibling {
                    self.nodes[sibling].red = true;
                }
                node = Some(p);
                parent = self.nodes[p].parent;
                absent_left = false;
            } else {
                if !self.is_red(self.child(sibling, !left)) {
                    if let Some(near) = self.child(sibling, left) {
                        self.nodes[near].red = false;
                    }
                    if let Some(s) = sibling {
                        self.nodes[s].red = true;
                        self.rotate(s, !left);
                    }
                    sibling = self.child(Some(p), !left);
                }
                if let Some(s) = sibling {
                    self.nodes[s].red = self.nodes[p].red;
                }
                self.nodes[p].red = false;
                if let Some(far) = self.child(sibling, !left) {
                    self.nodes[far].red = false;
                }
                self.rotate(p, left);
                node = self.root;
                parent = None;
            }
        }
        if let Some(node) = node {
            self.nodes[node].red = false;
        }
    }

    /// Remove a node from the balanced tree without changing its bucket's
    /// iteration order, matching removal through a Java HashSet iterator.
    fn remove(&mut self, pos: Pos) {
        let hash = PositionSet::hash(pos) as i32;
        let mut node = self.root;
        let index = loop {
            let i = node.expect("tree position is present");
            let h = PositionSet::hash(self.nodes[i].pos) as i32;
            if hash == h {
                assert_eq!(pos, self.nodes[i].pos);
                break i;
            }
            node = if hash < h {
                self.nodes[i].left
            } else {
                self.nodes[i].right
            };
        };
        let left = self.nodes[index].left;
        let right = self.nodes[index].right;
        let mut removed_red = self.nodes[index].red;
        let replacement;
        let replacement_parent;
        let replacement_left;
        if left.is_none() || right.is_none() {
            replacement = left.or(right);
            replacement_parent = self.nodes[index].parent;
            replacement_left =
                replacement_parent.is_some_and(|p| self.nodes[p].left == Some(index));
            self.replace(index, replacement);
        } else {
            let mut successor = right.unwrap();
            while let Some(next) = self.nodes[successor].left {
                successor = next;
            }
            removed_red = self.nodes[successor].red;
            replacement = self.nodes[successor].right;
            if self.nodes[successor].parent == Some(index) {
                replacement_parent = Some(successor);
                replacement_left = false;
                if let Some(replacement) = replacement {
                    self.nodes[replacement].parent = Some(successor);
                }
            } else {
                replacement_parent = self.nodes[successor].parent;
                replacement_left = true;
                self.replace(successor, replacement);
                self.nodes[successor].right = right;
                self.nodes[right.unwrap()].parent = Some(successor);
            }
            self.replace(index, Some(successor));
            self.nodes[successor].left = left;
            self.nodes[left.unwrap()].parent = Some(successor);
            self.nodes[successor].red = self.nodes[index].red;
        }
        if !removed_red {
            self.fix_removal(replacement, replacement_parent, replacement_left);
        }
        // Reuse the detached slot without changing any other node's index.
        self.nodes[index].parent = None;
        self.nodes[index].left = None;
        self.nodes[index].right = None;
        self.free.push(index);
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
                let pos = bucket.order.remove(0);
                if let Some(tree) = &mut bucket.tree {
                    tree.remove(pos);
                }
                if bucket.order.is_empty() {
                    bucket.tree = None;
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
    fn identity_dependent_tree_collisions_are_rejected() {
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
        set.insert([64 * 10, 0, 0]).unwrap();
        assert!(set.values().any(|p| p == [64 * 10, 0, 0]));
    }

    #[test]
    #[ignore = "requires Java 25 JDK"]
    fn tree_bin_insertions_and_iterator_removals_match_java() {
        use std::{path::PathBuf, process::Command};
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin/java.exe");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("PositionSetOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/PositionSetOracle.java"),
        )
        .unwrap();
        let output = Command::new(java).arg(source).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let java = String::from_utf8(output.stdout).unwrap();
        let mut expected = Vec::new();
        for scenario in 0..4 {
            let mut set = PositionSet::default();
            let mut step = 0;
            let point = |i: i32| match scenario {
                0 => [i * 64, 0, 0],
                1 => [(i - 150) * 64, 0, 0],
                2 => [i * 256, 0, 0],
                3 => [if i % 2 == 0 { i * 256 } else { -i * 256 }, 0, 0],
                _ => unreachable!(),
            };
            let record = |set: &PositionSet, step: &mut i32, output: &mut Vec<String>| {
                let parts = set
                    .values()
                    .map(|p| format!("{},{},{}", p[0], p[1], p[2]))
                    .collect::<Vec<_>>()
                    .join(";");
                output.push(format!("C {scenario} {step} {parts}"));
                *step += 1;
            };
            for i in 0..100 {
                set.insert(point(i)).unwrap();
                record(&set, &mut step, &mut expected);
            }
            for i in 100..240 {
                set.pop_first().unwrap();
                record(&set, &mut step, &mut expected);
                set.insert(point(i)).unwrap();
                record(&set, &mut step, &mut expected);
                if i % 7 == 0 {
                    set.insert(point(i)).unwrap();
                    record(&set, &mut step, &mut expected);
                }
                if i % 9 == 0 {
                    set.pop_first().unwrap();
                    record(&set, &mut step, &mut expected);
                }
            }
            while set.len > 0 {
                set.pop_first().unwrap();
                record(&set, &mut step, &mut expected);
            }
            for i in 300..320 {
                set.insert(point(i)).unwrap();
                record(&set, &mut step, &mut expected);
            }
        }
        let actual: Vec<_> = java.lines().collect();
        assert_eq!(actual.len(), expected.len());
        for (i, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
            assert_eq!(*actual, expected, "operation {i}");
        }
    }
}
