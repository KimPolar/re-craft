//! Red-black collision bins for Java's position HashSet.
use anyhow::{Result, bail, ensure};

use super::{Pos, hash};

fn key(pos: Pos) -> i32 {
    let h = hash(pos);
    (h ^ (h >> 16)) as i32
}

#[derive(Clone, Debug)]
struct Node {
    pos: Pos,
    key: i32,
    parent: Option<usize>,
    children: [Option<usize>; 2],
    prev: Option<usize>,
    next: Option<usize>,
    red: bool,
}

/// A red-black tree plus the independent linked iteration order of its bin.
#[derive(Clone, Debug)]
pub(super) struct TreeBin {
    nodes: Vec<Node>,
    free: Vec<usize>,
    root: usize,
    head: Option<usize>,
    tail: Option<usize>,
}

impl TreeBin {
    pub(super) fn from_chain(positions: Vec<Pos>) -> Result<Self> {
        let mut tree = Self {
            nodes: Vec::new(),
            free: Vec::new(),
            root: 0,
            head: None,
            tail: None,
        };
        for pos in positions {
            let (node, _) = tree.attach(pos)?;
            tree.nodes[node].prev = tree.tail;
            if let Some(tail) = tree.tail {
                tree.nodes[tail].next = Some(node);
            } else {
                tree.head = Some(node);
            }
            tree.tail = Some(node);
            tree.rebalance(node);
        }
        tree.move_root_first();
        Ok(tree)
    }

    pub(super) fn insert(&mut self, pos: Pos) -> Result<()> {
        let (node, parent) = self.attach(pos)?;
        let parent = parent.expect("tree bin is nonempty");
        let next = self.nodes[parent].next;
        self.nodes[parent].next = Some(node);
        self.nodes[node].prev = Some(parent);
        self.nodes[node].next = next;
        if let Some(next) = next {
            self.nodes[next].prev = Some(node);
        } else {
            self.tail = Some(node);
        }
        self.rebalance(node);
        self.move_root_first();
        Ok(())
    }

    /// Remove a key, returning a list when Java converts this bin back to a list.
    pub(super) fn remove(&mut self, pos: Pos) -> Result<Option<Vec<Pos>>> {
        let mut index = self.root;
        loop {
            let node = &self.nodes[index];
            if node.pos == pos {
                break;
            }
            let side = usize::from(key(pos) > node.key);
            ensure!(
                key(pos) != node.key,
                "Java block entity identity hash ordering is unavailable"
            );
            index = node.children[side]
                .ok_or_else(|| anyhow::anyhow!("pending tree position is missing"))?;
        }

        // The linked list is independent of the tree and is visited on save.
        self.unlink(index);
        if self.head.is_none() {
            return Ok(Some(Vec::new()));
        }
        let root = &self.nodes[self.root];
        if root.children[1].is_none()
            || root.children[0].is_none_or(|left| self.nodes[left].children[0].is_none())
        {
            return Ok(Some(self.positions().collect()));
        }

        let mut removed = index;
        if let [Some(_), Some(right)] = self.nodes[index].children {
            let mut successor = right;
            while let Some(left) = self.nodes[successor].children[0] {
                successor = left;
            }
            // Keep the successor at its original linked-list location while
            // swapping only keys in the tree. The node to delete moves down.
            self.replace_link(successor, index);
            let (new_pos, new_key) = (self.nodes[successor].pos, self.nodes[successor].key);
            self.nodes[successor].pos = self.nodes[index].pos;
            self.nodes[successor].key = self.nodes[index].key;
            self.nodes[index].pos = new_pos;
            self.nodes[index].key = new_key;
            removed = successor;
        }
        let child = self.nodes[removed].children[0].or(self.nodes[removed].children[1]);
        let replacement = child.unwrap_or(removed);
        if child.is_some() {
            let parent = self.nodes[removed].parent;
            self.nodes[replacement].parent = parent;
            if let Some(parent) = parent {
                let side = usize::from(self.nodes[parent].children[1] == Some(removed));
                self.nodes[parent].children[side] = Some(replacement);
            } else {
                self.root = replacement;
                self.nodes[replacement].red = false;
            }
            self.nodes[removed].parent = None;
            self.nodes[removed].children = [None, None];
        }
        if !self.nodes[removed].red {
            self.rebalance_after_removal(replacement);
        }
        if replacement == removed {
            if let Some(parent) = self.nodes[removed].parent {
                let side = usize::from(self.nodes[parent].children[1] == Some(removed));
                self.nodes[parent].children[side] = None;
            }
            self.nodes[removed].parent = None;
        }
        self.free.push(removed);
        self.move_root_first();
        Ok(None)
    }

    fn unlink(&mut self, index: usize) {
        let (prev, next) = (self.nodes[index].prev, self.nodes[index].next);
        if let Some(prev) = prev {
            self.nodes[prev].next = next;
        } else {
            self.head = next;
        }
        if let Some(next) = next {
            self.nodes[next].prev = prev;
        } else {
            self.tail = prev;
        }
        self.nodes[index].prev = None;
        self.nodes[index].next = None;
    }

    fn replace_link(&mut self, old: usize, new: usize) {
        let (prev, next) = (self.nodes[old].prev, self.nodes[old].next);
        if let Some(prev) = prev {
            self.nodes[prev].next = Some(new);
        } else {
            self.head = Some(new);
        }
        if let Some(next) = next {
            self.nodes[next].prev = Some(new);
        } else {
            self.tail = Some(new);
        }
        self.nodes[new].prev = prev;
        self.nodes[new].next = next;
        self.nodes[old].prev = None;
        self.nodes[old].next = None;
    }

    fn rebalance_after_removal(&mut self, mut node: usize) {
        while node != self.root && !self.nodes[node].red {
            let Some(parent) = self.nodes[node].parent else {
                break;
            };
            let side = usize::from(self.nodes[parent].children[1] == Some(node));
            let other = 1 - side;
            let Some(mut sibling) = self.nodes[parent].children[other] else {
                node = parent;
                continue;
            };
            if self.nodes[sibling].red {
                self.nodes[sibling].red = false;
                self.nodes[parent].red = true;
                self.rotate(parent, other);
                let Some(updated) = self.nodes[parent].children[other] else {
                    node = parent;
                    continue;
                };
                sibling = updated;
            }
            let near = self.nodes[sibling].children[side];
            let far = self.nodes[sibling].children[other];
            if !near.is_some_and(|i| self.nodes[i].red) && !far.is_some_and(|i| self.nodes[i].red) {
                self.nodes[sibling].red = true;
                node = parent;
                continue;
            }
            if !far.is_some_and(|i| self.nodes[i].red) {
                if let Some(near) = near {
                    self.nodes[near].red = false;
                }
                self.nodes[sibling].red = true;
                self.rotate(sibling, side);
                sibling = self.nodes[parent].children[other].expect("sibling after rotation");
            }
            self.nodes[sibling].red = self.nodes[parent].red;
            self.nodes[parent].red = false;
            if let Some(far) = self.nodes[sibling].children[other] {
                self.nodes[far].red = false;
            }
            self.rotate(parent, other);
            node = self.root;
        }
        self.nodes[node].red = false;
    }

    fn attach(&mut self, pos: Pos) -> Result<(usize, Option<usize>)> {
        let value = key(pos);
        let mut parent = None;
        let mut direction = 0;
        if !self.nodes.is_empty() {
            let mut current = self.root;
            loop {
                match value.cmp(&self.nodes[current].key) {
                    std::cmp::Ordering::Equal => {
                        bail!("Java block entity identity hash ordering is unavailable")
                    }
                    std::cmp::Ordering::Less => direction = 0,
                    std::cmp::Ordering::Greater => direction = 1,
                }
                if let Some(next) = self.nodes[current].children[direction] {
                    current = next;
                } else {
                    parent = Some(current);
                    break;
                }
            }
        }
        let value = Node {
            pos,
            key: value,
            parent,
            children: [None; 2],
            prev: None,
            next: None,
            red: true,
        };
        let node = if let Some(index) = self.free.pop() {
            self.nodes[index] = value;
            index
        } else {
            self.nodes.push(value);
            self.nodes.len() - 1
        };
        if let Some(parent) = parent {
            self.nodes[parent].children[direction] = Some(node);
        } else {
            self.root = node;
        }
        Ok((node, parent))
    }

    fn rotate(&mut self, old: usize, promoted_side: usize) {
        let new = self.nodes[old].children[promoted_side].expect("rotation child");
        let opposite = 1 - promoted_side;
        let transferred = self.nodes[new].children[opposite];
        self.nodes[old].children[promoted_side] = transferred;
        if let Some(child) = transferred {
            self.nodes[child].parent = Some(old);
        }
        let parent = self.nodes[old].parent;
        self.nodes[new].parent = parent;
        if let Some(parent) = parent {
            let side = usize::from(self.nodes[parent].children[1] == Some(old));
            self.nodes[parent].children[side] = Some(new);
        } else {
            self.root = new;
        }
        self.nodes[new].children[opposite] = Some(old);
        self.nodes[old].parent = Some(new);
    }

    fn rebalance(&mut self, mut node: usize) {
        while let Some(parent) = self.nodes[node].parent {
            if !self.nodes[parent].red {
                break;
            }
            let grand = self.nodes[parent]
                .parent
                .expect("red parent has grandparent");
            let parent_side = usize::from(self.nodes[grand].children[1] == Some(parent));
            let uncle = self.nodes[grand].children[1 - parent_side];
            if uncle.is_some_and(|u| self.nodes[u].red) {
                self.nodes[parent].red = false;
                self.nodes[uncle.unwrap()].red = false;
                self.nodes[grand].red = true;
                node = grand;
                continue;
            }
            if self.nodes[parent].children[1 - parent_side] == Some(node) {
                self.rotate(parent, 1 - parent_side);
                node = parent;
            }
            let parent = self.nodes[node].parent.expect("parent after rotation");
            let grand = self.nodes[parent]
                .parent
                .expect("grandparent after rotation");
            self.nodes[parent].red = false;
            self.nodes[grand].red = true;
            self.rotate(grand, parent_side);
        }
        self.nodes[self.root].red = false;
    }

    fn move_root_first(&mut self) {
        let root = self.root;
        let Some(previous) = self.nodes[root].prev else {
            return;
        };
        let next = self.nodes[root].next;
        self.nodes[previous].next = next;
        if let Some(next) = next {
            self.nodes[next].prev = Some(previous);
        } else {
            self.tail = Some(previous);
        }
        self.nodes[root].prev = None;
        self.nodes[root].next = self.head;
        if let Some(head) = self.head {
            self.nodes[head].prev = Some(root);
        }
        self.head = Some(root);
    }

    pub(super) fn positions(&self) -> impl Iterator<Item = Pos> + '_ {
        std::iter::successors(self.head, |&index| self.nodes[index].next)
            .map(|index| self.nodes[index].pos)
    }
}
