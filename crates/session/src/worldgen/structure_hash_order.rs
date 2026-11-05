//! Java HashMap ordering for structure identities. Identity hashes must be
//! supplied by the caller; they cannot be derived from a world seed.
use anyhow::{Context, Result, bail};
use native_resources::packs::ResourceId;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) enum Insertion {
    Put,
    ComputeIfAbsent,
}

/// Insert-only HashMap histories used by ChunkAccess for starts and references.
/// Tree bins are rejected until their identity-dependent ordering is modeled.
pub(super) struct StructureIdentityMap<'a> {
    hashes: &'a BTreeMap<ResourceId, i32>,
    bins: Vec<Vec<ResourceId>>,
    members: BTreeSet<ResourceId>,
}

impl<'a> StructureIdentityMap<'a> {
    pub(super) fn new(hashes: &'a BTreeMap<ResourceId, i32>) -> Self {
        Self {
            hashes,
            bins: vec![Vec::new(); 16],
            members: BTreeSet::new(),
        }
    }

    fn hash(&self, key: &ResourceId) -> Result<usize> {
        let hash = *self
            .hashes
            .get(key)
            .context("structure identity hash is missing")? as u32;
        Ok((hash ^ (hash >> 16)) as usize)
    }

    fn resize(&mut self) -> Result<()> {
        let next = self
            .bins
            .len()
            .checked_mul(2)
            .context("structure hash table is too large")?;
        let old = std::mem::replace(&mut self.bins, vec![Vec::new(); next]);
        for bin in old {
            for key in bin {
                let index = self.hash(&key)? & (next - 1);
                self.bins[index].push(key);
            }
        }
        Ok(())
    }

    pub(super) fn insert(&mut self, key: &ResourceId, insertion: Insertion) -> Result<()> {
        let hash = self.hash(key)?;
        if matches!(insertion, Insertion::ComputeIfAbsent)
            && self.members.len() > self.bins.len() * 3 / 4
        {
            self.resize()?;
        }
        if self.members.contains(key) {
            return Ok(());
        }
        let index = hash & (self.bins.len() - 1);
        let previous = self.bins[index].len();
        let treeify = match insertion {
            Insertion::Put => previous >= 8,
            Insertion::ComputeIfAbsent => previous >= 7,
        };
        if treeify && self.bins.len() >= 64 {
            bail!("structure identity hash tree order is unavailable");
        }
        match insertion {
            Insertion::Put => self.bins[index].push(key.clone()),
            Insertion::ComputeIfAbsent => self.bins[index].insert(0, key.clone()),
        }
        self.members.insert(key.clone());
        if treeify {
            self.resize()?;
        }
        if matches!(insertion, Insertion::Put) && self.members.len() > self.bins.len() * 3 / 4 {
            self.resize()?;
        }
        Ok(())
    }

    pub(super) fn order(&self) -> Vec<ResourceId> {
        self.bins.iter().flatten().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn put_appends_but_compute_prepends_collisions() {
        let a = ResourceId::parse("test:a").unwrap();
        let b = ResourceId::parse("test:b").unwrap();
        let hashes = BTreeMap::from([(a.clone(), 1), (b.clone(), 17)]);
        let mut put = StructureIdentityMap::new(&hashes);
        put.insert(&a, Insertion::Put).unwrap();
        put.insert(&b, Insertion::Put).unwrap();
        assert_eq!(put.order(), vec![a.clone(), b.clone()]);
        let mut compute = StructureIdentityMap::new(&hashes);
        compute.insert(&a, Insertion::ComputeIfAbsent).unwrap();
        compute.insert(&b, Insertion::ComputeIfAbsent).unwrap();
        assert_eq!(compute.order(), vec![b, a]);
    }

    #[test]
    fn missing_hash_and_unmodeled_tree_fail_without_guessing() {
        let keys: Vec<_> = (0..10)
            .map(|i| ResourceId::parse(&format!("test:k{i}")).unwrap())
            .collect();
        let hashes = keys.iter().cloned().map(|key| (key, 0)).collect();
        let mut map = StructureIdentityMap::new(&hashes);
        for key in &keys[..9] {
            map.insert(key, Insertion::ComputeIfAbsent).unwrap();
        }
        let before = map.order();
        assert!(map.insert(&keys[9], Insertion::ComputeIfAbsent).is_err());
        assert_eq!(map.order(), before);
        assert!(
            map.insert(&ResourceId::parse("test:unknown").unwrap(), Insertion::Put)
                .is_err()
        );
        assert_eq!(map.order(), before);
    }

    #[test]
    #[ignore = "requires Java 25 JDK"]
    fn structure_identity_hash_map_order_matches_java() {
        use std::{path::PathBuf, process::Command};
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("StructureHashOrderOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StructureHashOrderOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(temp.path())
            .arg("StructureHashOrderOracle")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout).unwrap();
        let lines: Vec<_> = output.lines().collect();
        let mut cursor = 0;
        for mode in 0..2 {
            for scenario in 0..3 {
                let count = if scenario == 0 {
                    80
                } else if scenario == 1 {
                    8
                } else {
                    15
                };
                let keys: Vec<_> = (0..count)
                    .map(|i| ResourceId::parse(&format!("test:k{i}")).unwrap())
                    .collect();
                let hashes = keys
                    .iter()
                    .enumerate()
                    .map(|(i, key)| {
                        let value = if scenario == 1 {
                            (i as i32).wrapping_mul(64)
                        } else if scenario == 2 {
                            (i as i32).wrapping_mul(16)
                        } else {
                            (i as i32).wrapping_mul(0x9e37_79b9_u32 as i32)
                        };
                        (key.clone(), value)
                    })
                    .collect();
                let mut map = StructureIdentityMap::new(&hashes);
                let insertion = if mode == 0 {
                    Insertion::Put
                } else {
                    Insertion::ComputeIfAbsent
                };
                for (i, key) in keys.iter().enumerate() {
                    for duplicate in 0..=usize::from(i % 3 == 0) {
                        map.insert(key, insertion).unwrap();
                        let words: Vec<_> = lines[cursor].split_whitespace().collect();
                        cursor += 1;
                        assert_eq!(
                            &words[..5],
                            &[
                                "M",
                                &mode.to_string(),
                                &scenario.to_string(),
                                &i.to_string(),
                                &duplicate.to_string()
                            ]
                        );
                        let expected: Vec<_> = words[5..]
                            .iter()
                            .map(|id| ResourceId::parse(&format!("test:k{id}")).unwrap())
                            .collect();
                        assert_eq!(map.order(), expected, "{}", lines[cursor - 1]);
                    }
                }
            }
        }
        assert_eq!(cursor, lines.len());
    }
}
