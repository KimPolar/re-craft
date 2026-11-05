//! Position ordering used by a fresh protochunk's materialized block entities.
use std::collections::BTreeSet;

use anyhow::{Result, ensure};

mod tree;
use tree::TreeBin;

type Pos = [i32; 3];

fn hash([x, y, z]: Pos) -> u32 {
    z.wrapping_mul(31)
        .wrapping_add(y)
        .wrapping_mul(31)
        .wrapping_add(x) as u32
}

fn fast_hash(pos: Pos) -> u32 {
    let h = hash(pos).wrapping_mul(0x9e37_79b9);
    h ^ (h >> 16)
}

/// The non-null keys of fastutil's default Object2ObjectOpenHashMap.
#[derive(Clone, Debug)]
pub(super) struct LivePositions {
    slots: Vec<Option<Pos>>,
    len: usize,
}

impl Default for LivePositions {
    fn default() -> Self {
        Self {
            slots: vec![None; 32],
            len: 0,
        }
    }
}

impl LivePositions {
    fn find(&self, pos: Pos) -> usize {
        let mask = self.slots.len() - 1;
        let mut i = fast_hash(pos) as usize & mask;
        while self.slots[i].is_some_and(|other| other != pos) {
            i = (i + 1) & mask;
        }
        i
    }

    pub(super) fn insert(&mut self, pos: Pos) {
        let i = self.find(pos);
        if self.slots[i].is_some() {
            return;
        }
        self.slots[i] = Some(pos);
        let old_len = self.len;
        self.len += 1;
        if old_len >= self.slots.len() * 3 / 4 {
            self.rehash(self.slots.len() * 2);
        }
    }

    pub(super) fn remove(&mut self, pos: Pos) {
        let mut hole = self.find(pos);
        if self.slots[hole].is_none() {
            return;
        }
        self.len -= 1;
        let mask = self.slots.len() - 1;
        let mut cursor = (hole + 1) & mask;
        while let Some(candidate) = self.slots[cursor] {
            let home = fast_hash(candidate) as usize & mask;
            let move_to_hole = if hole <= cursor {
                hole >= home || home > cursor
            } else {
                hole >= home && home > cursor
            };
            if move_to_hole {
                self.slots[hole] = Some(candidate);
                hole = cursor;
            }
            cursor = (cursor + 1) & mask;
        }
        self.slots[hole] = None;
        if self.slots.len() > 32 && self.len < (self.slots.len() * 3 / 4) / 4 {
            self.rehash(self.slots.len() / 2);
        }
    }

    fn rehash(&mut self, capacity: usize) {
        let old = std::mem::replace(&mut self.slots, vec![None; capacity]);
        for pos in old.into_iter().rev().flatten() {
            let i = self.find(pos);
            self.slots[i] = Some(pos);
        }
    }

    /// ChunkAccess copies pending keys into a HashSet, then adds fastutil keys.
    pub(super) fn saving_order(&self) -> Result<Vec<Pos>> {
        self.saving_order_with_pending(None)
    }

    pub(super) fn saving_order_with_pending(
        &self,
        pending: Option<&PendingPositions>,
    ) -> Result<Vec<Pos>> {
        let count = pending.map_or(0, |positions| positions.members.len());
        // Java 25 HashSet(Collection) reserves ceil(max(size, 12) / .75).
        let requested = count
            .max(12)
            .checked_mul(4)
            .and_then(|n| n.checked_add(2))
            .map(|n| n / 3)
            .and_then(usize::checked_next_power_of_two)
            .ok_or_else(|| anyhow::anyhow!("generated block entity order is too large"))?;
        let mut buckets = vec![Bin::default(); requested];
        let mut size = 0;
        // Live keys are already unique; track duplicates only for the mixed case.
        let mut inserted = (count != 0).then(BTreeSet::new);
        if let Some(pending) = pending {
            for pos in pending.positions()? {
                if inserted.as_mut().is_none_or(|keys| keys.insert(pos)) {
                    insert_set(&mut buckets, &mut size, pos)?;
                }
            }
        }
        for pos in self.slots.iter().rev().flatten().copied() {
            if inserted.as_mut().is_none_or(|keys| keys.insert(pos)) {
                insert_set(&mut buckets, &mut size, pos)?;
            }
        }
        Ok(bucket_positions(&buckets))
    }
}

/// Java HashMap insertion history for protochunk block entity placeholders.
#[derive(Clone, Debug)]
pub(super) struct PendingPositions {
    buckets: Vec<Bin>,
    members: BTreeSet<Pos>,
    exact: bool,
}

impl Default for PendingPositions {
    fn default() -> Self {
        Self {
            buckets: vec![Bin::default(); 16],
            members: BTreeSet::new(),
            exact: true,
        }
    }
}

impl PendingPositions {
    pub(super) fn insert(&mut self, pos: Pos) {
        if self.members.insert(pos) && self.exact {
            let mut size = self.members.len() - 1;
            if insert_set(&mut self.buckets, &mut size, pos).is_err() {
                self.exact = false;
            }
        }
    }

    pub(super) fn remove(&mut self, pos: Pos) {
        if !self.members.remove(&pos) || !self.exact {
            return;
        }
        let h = hash(pos);
        let index = (h ^ (h >> 16)) as usize & (self.buckets.len() - 1);
        match &mut self.buckets[index] {
            Bin::List(chain) => {
                if let Some(i) = chain.iter().position(|&p| p == pos) {
                    chain.remove(i);
                } else {
                    self.exact = false;
                }
                if chain.is_empty() {
                    self.buckets[index] = Bin::Empty;
                }
            }
            Bin::Tree(tree) => match tree.remove(pos) {
                Ok(Some(chain)) => {
                    self.buckets[index] = if chain.is_empty() {
                        Bin::Empty
                    } else {
                        Bin::List(chain)
                    };
                }
                Ok(None) => {}
                Err(_) => self.exact = false,
            },
            Bin::Empty => self.exact = false,
        }
    }

    pub(super) fn members(&self) -> &BTreeSet<Pos> {
        &self.members
    }

    fn positions(&self) -> Result<Vec<Pos>> {
        if self.members.is_empty() {
            return Ok(Vec::new());
        }
        ensure!(
            self.exact,
            "pending block entity hash tree order is unavailable"
        );
        Ok(bucket_positions(&self.buckets))
    }
}

fn bucket_positions(buckets: &[Bin]) -> Vec<Pos> {
    let mut result = Vec::new();
    for bucket in buckets {
        match bucket {
            Bin::Empty => {}
            Bin::List(chain) => result.extend(chain.iter().copied()),
            Bin::Tree(tree) => result.extend(tree.positions()),
        }
    }
    result
}

fn insert_set(buckets: &mut Vec<Bin>, size: &mut usize, pos: Pos) -> Result<()> {
    let h = hash(pos);
    let index = (h ^ (h >> 16)) as usize & (buckets.len() - 1);
    let capacity = buckets.len();
    let mut grow = false;
    match &mut buckets[index] {
        Bin::Empty => buckets[index] = Bin::List(vec![pos]),
        Bin::List(chain) => {
            let treeify = chain.len() >= 8;
            chain.push(pos);
            if treeify {
                if capacity < 64 {
                    grow = true;
                } else {
                    buckets[index] = Bin::Tree(TreeBin::from_chain(std::mem::take(chain))?);
                }
            }
        }
        Bin::Tree(tree) => tree.insert(pos)?,
    }
    *size += 1;
    if grow {
        resize_set(buckets)?;
    }
    if *size > buckets.len() * 3 / 4 {
        resize_set(buckets)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
enum Bin {
    #[default]
    Empty,
    List(Vec<Pos>),
    Tree(TreeBin),
}

fn resize_set(buckets: &mut Vec<Bin>) -> Result<()> {
    let old_len = buckets.len();
    let old = std::mem::replace(buckets, vec![Bin::default(); old_len * 2]);
    for (index, bucket) in old.into_iter().enumerate() {
        match bucket {
            Bin::Empty => {}
            Bin::List(chain) => {
                for pos in chain {
                    let h = hash(pos);
                    let target = (h ^ (h >> 16)) as usize & (buckets.len() - 1);
                    match &mut buckets[target] {
                        Bin::Empty => buckets[target] = Bin::List(vec![pos]),
                        Bin::List(chain) => chain.push(pos),
                        Bin::Tree(_) => unreachable!(),
                    }
                }
            }
            Bin::Tree(tree) => {
                let (low, high): (Vec<_>, Vec<_>) = tree.positions().partition(|&pos| {
                    let h = hash(pos);
                    ((h ^ (h >> 16)) as usize & old_len) == 0
                });
                if low.is_empty() {
                    buckets[index + old_len] = Bin::Tree(tree);
                } else if high.is_empty() {
                    buckets[index] = Bin::Tree(tree);
                } else {
                    buckets[index] = split_tree(low)?;
                    buckets[index + old_len] = split_tree(high)?;
                }
            }
        }
    }
    Ok(())
}

fn split_tree(chain: Vec<Pos>) -> Result<Bin> {
    if chain.len() <= 6 {
        Ok(Bin::List(chain))
    } else {
        Ok(Bin::Tree(TreeBin::from_chain(chain)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_identity_dependent_java_hash_tree_collisions() {
        let mut positions = LivePositions::default();
        for z in 0..11 {
            positions.insert([0, -31 * z, z]);
        }
        assert!(positions.saving_order().is_err());
    }

    #[test]
    fn pending_tree_removal_keeps_remaining_positions() {
        let mut pending = PendingPositions::default();
        let positions: Vec<_> = (-10000..10000)
            .map(|y| [0, y, 0])
            .filter(|&pos| {
                let h = hash(pos);
                (h ^ (h >> 16)) & 63 == 0
            })
            .take(12)
            .collect();
        for &pos in &positions {
            pending.insert(pos);
        }
        assert!(
            LivePositions::default()
                .saving_order_with_pending(Some(&pending))
                .is_ok()
        );
        pending.remove(positions[0]);
        assert_eq!(pending.positions().unwrap().len(), positions.len() - 1);
        assert!(
            LivePositions::default()
                .saving_order_with_pending(Some(&pending))
                .is_ok()
        );
        for &pos in &positions[1..] {
            pending.remove(pos);
        }
        assert!(
            LivePositions::default()
                .saving_order_with_pending(Some(&pending))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn protochunk_order_matches_java_during_inserts_removals_and_resizes() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("GeneratedEntityOrderOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GeneratedEntityOrderOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        let mut ops = Vec::new();
        for i in 0i32..180 {
            let p = [(i * 7 % 16) - 16, (i * 71 % 384) - 64, (i * 11 % 16) + 32];
            ops.push((true, p));
            if i % 5 == 0 {
                ops.push((true, p));
            }
        }
        for i in (0i32..180).step_by(2) {
            ops.push((
                false,
                [(i * 7 % 16) - 16, (i * 71 % 384) - 64, (i * 11 % 16) + 32],
            ));
        }
        for i in (1i32..180).step_by(2) {
            ops.push((
                false,
                [(i * 7 % 16) - 16, (i * 71 % 384) - 64, (i * 11 % 16) + 32],
            ));
        }
        // Force long collision bins, treeification, subsequent insertions,
        // and splitting on several later HashSet resizes.
        let mut collision_positions = Vec::new();
        for y in -10000..10000 {
            let p = [0, y, 0];
            let h = hash(p);
            if (h ^ (h >> 16)) & 63 == 0 {
                collision_positions.push(p);
                if collision_positions.len() == 100 {
                    break;
                }
            }
        }
        assert_eq!(collision_positions.len(), 100);
        for &p in &collision_positions {
            ops.push((true, p));
        }
        for &p in collision_positions.iter().step_by(2) {
            ops.push((false, p));
        }
        for &p in collision_positions.iter().skip(1).step_by(2) {
            ops.push((false, p));
        }
        let mut mixed = Vec::new();
        for y in -10000i32..10000 {
            let p = [-11, y, 39];
            let h = hash(p);
            if (h ^ (h >> 16)) & 63 == 13 {
                mixed.push(p);
            }
        }
        // Alternate widely separated positive and negative hashes.
        let chosen = mixed.iter().take(55).zip(mixed.iter().rev().take(55));
        for (&a, &b) in chosen {
            ops.push((true, a));
            ops.push((true, b));
        }
        let input = temp.path().join("operations.txt");
        std::fs::write(
            &input,
            ops.iter()
                .map(|(insert, p)| {
                    format!(
                        "{} {} {} {}\n",
                        if *insert { "I" } else { "R" },
                        p[0],
                        p[1],
                        p[2]
                    )
                })
                .collect::<String>(),
        )
        .unwrap();
        let expected = run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GeneratedEntityOrderOracle")
            .arg(&input));
        let rows: Vec<_> = expected.lines().collect();
        assert_eq!(rows.len(), ops.len());
        let mut positions = LivePositions::default();
        for (index, ((insert, p), expected)) in ops.into_iter().zip(rows).enumerate() {
            if insert {
                positions.insert(p);
            } else {
                positions.remove(p);
            }
            let actual = positions.saving_order().unwrap();
            let actual = actual
                .iter()
                .map(|p| format!("{},{},{}", p[0], p[1], p[2]))
                .collect::<Vec<_>>()
                .join(";");
            assert_eq!(actual, expected, "operation {index}");
        }

        // Pending HashMap retains its capacity after removal. Interleave
        // resolution and removals across several resizes and compare each step.
        let mut operations = Vec::new();
        let position = |i: i32| [(i * 7 % 16) - 16, (i * 71 % 384) - 64, (i * 11 % 16) + 32];
        for i in 0..180 {
            operations.push(('P', position(i)));
            if i % 4 == 0 {
                operations.push(('P', position(i)));
            }
            if i > 8 && i % 3 == 0 {
                operations.push(('C', position(i - 8)));
            }
            if i > 20 && i % 7 == 0 {
                operations.push(('X', position(i - 19)));
            }
        }
        for i in 0..180 {
            operations.push(('C', position(i)));
        }
        for i in 180..280 {
            operations.push(('P', position(i)));
        }
        std::fs::write(
            &input,
            operations
                .iter()
                .map(|(op, p)| format!("{op} {} {} {}\n", p[0], p[1], p[2]))
                .collect::<String>(),
        )
        .unwrap();
        let expected = run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GeneratedEntityOrderOracle")
            .arg(&input));
        let rows: Vec<_> = expected.lines().collect();
        assert_eq!(rows.len(), operations.len());
        let mut live = LivePositions::default();
        let mut pending = PendingPositions::default();
        let mut live_members = BTreeSet::new();
        for (index, ((op, p), expected)) in operations.into_iter().zip(rows).enumerate() {
            match op {
                'P' if !live_members.contains(&p) => pending.insert(p),
                'C' => {
                    pending.remove(p);
                    live.insert(p);
                    live_members.insert(p);
                }
                'X' => pending.remove(p),
                _ => {}
            }
            let actual = live.saving_order_with_pending(Some(&pending)).unwrap();
            let actual = actual
                .iter()
                .map(|p| format!("{},{},{}", p[0], p[1], p[2]))
                .collect::<Vec<_>>()
                .join(";");
            assert_eq!(actual, expected, "mixed operation {index}: {op} {p:?}");
        }

        // Drive pending bins through treeification, deletion, untreeification,
        // subsequent insertions, and table splits. Inspect the HashMap order
        // directly as well as the final copied HashSet order.
        let collision: Vec<_> = (-30000..30000)
            .map(|y| [0, y, 0])
            .filter(|&p| {
                let h = hash(p);
                (h ^ (h >> 16)) & 63 == 0
            })
            .take(120)
            .collect();
        assert_eq!(collision.len(), 120);
        let mut operations = Vec::new();
        for &p in &collision[..40] {
            operations.push(('P', p));
        }
        for &p in collision[..40].iter().step_by(2) {
            operations.push(('X', p));
        }
        for &p in &collision[40..100] {
            operations.push(('P', p));
        }
        for &p in collision[1..100].iter().step_by(3) {
            operations.push(('C', p));
        }
        for &p in collision[..100].iter().rev() {
            operations.push(('X', p));
        }
        for &p in &collision[100..] {
            operations.push(('P', p));
        }
        for &p in collision[100..].iter().step_by(2) {
            operations.push(('C', p));
        }
        for &p in &collision {
            operations.push(('X', p));
            operations.push(('R', p));
        }
        for round in 0..4 {
            for i in 0..120 {
                operations.push(('P', collision[(i * 37 + round * 13) % 120]));
            }
            for i in 0..120 {
                let p = collision[(i * 53 + round * 17) % 120];
                operations.push((if i % 3 == 0 { 'C' } else { 'X' }, p));
                if i % 7 == 0 {
                    operations.push(('P', p));
                }
            }
            for &p in &collision {
                operations.push(('X', p));
                operations.push(('R', p));
            }
        }
        std::fs::write(
            &input,
            operations
                .iter()
                .map(|(op, p)| format!("{op} {} {} {}\n", p[0], p[1], p[2]))
                .collect::<String>(),
        )
        .unwrap();
        let expected = run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GeneratedEntityOrderOracle")
            .arg(&input)
            .arg("--pending"));
        let rows: Vec<_> = expected.lines().collect();
        assert_eq!(rows.len(), operations.len());
        let mut live = LivePositions::default();
        let mut live_members = BTreeSet::new();
        let mut pending = PendingPositions::default();
        let format_positions = |positions: Vec<Pos>| {
            positions
                .iter()
                .map(|p| format!("{},{},{}", p[0], p[1], p[2]))
                .collect::<Vec<_>>()
                .join(";")
        };
        for (index, ((op, p), expected)) in operations.into_iter().zip(rows).enumerate() {
            match op {
                'P' if !live_members.contains(&p) => pending.insert(p),
                'C' => {
                    pending.remove(p);
                    live.insert(p);
                    live_members.insert(p);
                }
                'X' => pending.remove(p),
                'R' => {
                    live.remove(p);
                    live_members.remove(&p);
                }
                _ => {}
            }
            let actual = format!(
                "{}|{}",
                format_positions(live.saving_order_with_pending(Some(&pending)).unwrap()),
                format_positions(pending.positions().unwrap())
            );
            assert_eq!(actual, expected, "collision operation {index}: {op} {p:?}");
        }
    }
}
