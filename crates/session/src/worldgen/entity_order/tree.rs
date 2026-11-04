//! Red-black collision bins for Java's position HashSet.
use anyhow::{Result, bail};

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
    root: usize,
    head: Option<usize>,
    tail: Option<usize>,
}

impl TreeBin {
    pub(super) fn from_chain(positions: Vec<Pos>) -> Result<Self> {
        let mut tree = Self {
            nodes: Vec::new(),
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
        let node = self.nodes.len();
        self.nodes.push(Node {
            pos,
            key: value,
            parent,
            children: [None; 2],
            prev: None,
            next: None,
            red: true,
        });
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
