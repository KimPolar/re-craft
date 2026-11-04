//! Position ordering used by a fresh protochunk's materialized block entities.
use anyhow::Result;

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

    /// ChunkAccess copies the empty pending key set to a Java HashSet, then
    /// adds this fastutil map's keys in descending slot order.
    pub(super) fn saving_order(&self) -> Result<Vec<Pos>> {
        let mut buckets = vec![Bin::default(); 16];
        let mut size = 0;
        for pos in self.slots.iter().rev().flatten().copied() {
            let h = hash(pos);
            let spread = h ^ (h >> 16);
            let index = spread as usize & (buckets.len() - 1);
            let capacity = buckets.len();
            let mut grow = false;
            match &mut buckets[index] {
                Bin::Empty => buckets[index] = Bin::List(vec![pos]),
                Bin::List(chain) => {
                    if chain.contains(&pos) {
                        continue;
                    }
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
            size += 1;
            if grow {
                resize_set(&mut buckets)?;
            }
            if size > buckets.len() * 3 / 4 {
                resize_set(&mut buckets)?;
            }
        }
        let mut result = Vec::with_capacity(size);
        for bucket in buckets {
            match bucket {
                Bin::Empty => {}
                Bin::List(chain) => result.extend(chain),
                Bin::Tree(tree) => result.extend(tree.positions()),
            }
        }
        Ok(result)
    }
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
    }
}
