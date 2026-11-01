//! In-memory drafts of the shared 26.3 `random_sequences` saved data.
//!
//! The caller must commit this data atomically with every world change that
//! consumes a sequence. This module never writes a world file.
use crate::{
    nbt::{NamedTag, Tag},
    world::{DimensionId, TARGET_DATA_VERSION},
};
use anyhow::{Context, Result, bail, ensure};
use native_core::random::XoroshiroRandom;
use std::collections::BTreeMap;

fn unique<'a>(value: &'a Tag, key: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = value else {
        bail!("random sequence value is not a compound")
    };
    let mut found = fields.iter().filter(|(name, _)| name.matches(key));
    let value = found.next().map(|(_, value)| value);
    ensure!(found.next().is_none(), "duplicate {key} field");
    Ok(value)
}
fn id(value: &str) -> Result<DimensionId> {
    DimensionId::parse(value).context("invalid random sequence identifier")
}
fn boolean(value: Option<&Tag>, name: &str) -> Result<bool> {
    match value {
        None | Some(Tag::Byte(1)) => Ok(true),
        Some(Tag::Byte(0)) => Ok(false),
        _ => bail!("invalid {name} flag"),
    }
}

#[derive(Clone, Debug)]
pub struct RandomSequences {
    source: Option<NamedTag>,
    salt: i32,
    include_world_seed: bool,
    include_sequence_id: bool,
    states: BTreeMap<DimensionId, (u64, u64)>,
    changed: BTreeMap<DimensionId, (u64, u64)>,
}

impl RandomSequences {
    /// Decode a 26.3 saved-data document, or the vanilla defaults for an absent file.
    pub fn read(source: Option<NamedTag>) -> Result<Self> {
        let mut result = Self {
            source: None,
            salt: 0,
            include_world_seed: true,
            include_sequence_id: true,
            states: BTreeMap::new(),
            changed: BTreeMap::new(),
        };
        let Some(document) = source else {
            return Ok(result);
        };
        ensure!(
            matches!(
                unique(&document.value, "DataVersion")?,
                Some(Tag::Int(TARGET_DATA_VERSION))
            ),
            "unsupported random sequence DataVersion"
        );
        let data = unique(&document.value, "data")?.context("random sequence data is missing")?;
        result.salt = match unique(data, "salt")? {
            Some(Tag::Int(value)) => *value,
            _ => bail!("invalid random sequence salt"),
        };
        result.include_world_seed =
            boolean(unique(data, "include_world_seed")?, "include_world_seed")?;
        result.include_sequence_id =
            boolean(unique(data, "include_sequence_id")?, "include_sequence_id")?;
        let Some(Tag::Compound(sequences)) = unique(data, "sequences")? else {
            bail!("invalid random sequences map")
        };
        for (name, sequence) in sequences {
            let name = String::from_utf16(&name.0).context("invalid random sequence name")?;
            let name = id(&name)?;
            let Some(Tag::LongArray(state)) = unique(sequence, "source")? else {
                bail!("invalid random sequence source")
            };
            ensure!(
                state.len() == 2,
                "random sequence source must contain two longs"
            );
            ensure!(
                result
                    .states
                    .insert(name, (state[0] as u64, state[1] as u64))
                    .is_none(),
                "duplicate random sequence identifier"
            );
        }
        result.source = Some(document);
        Ok(result)
    }

    pub fn source(&self) -> Option<&NamedTag> {
        self.source.as_ref()
    }
    pub fn is_dirty(&self) -> bool {
        !self.changed.is_empty()
    }

    /// Obtain an independent copy; call `set` only once the consuming action succeeds.
    pub fn get(&self, name: &str, world_seed: i64) -> Result<XoroshiroRandom> {
        let name = id(name)?;
        if let Some(&(low, high)) = self.changed.get(&name).or_else(|| self.states.get(&name)) {
            return Ok(XoroshiroRandom::from_state(low, high));
        }
        let canonical = format!("{}:{}", name.namespace, name.path);
        Ok(XoroshiroRandom::for_world_sequence(
            world_seed,
            self.salt,
            self.include_world_seed,
            self.include_sequence_id.then_some(canonical.as_str()),
        ))
    }

    pub fn set(&mut self, name: &str, random: &XoroshiroRandom) -> Result<()> {
        let name = id(name)?;
        let state = random.state();
        if self.states.get(&name) == Some(&state) {
            self.changed.remove(&name);
        } else {
            self.changed.insert(name, state);
        }
        Ok(())
    }

    /// Build an NBT document while retaining unrecognized data and entry fields.
    pub fn draft(&self) -> Result<NamedTag> {
        let mut document = self.source.clone().unwrap_or_else(|| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                (
                    "data".into(),
                    Tag::Compound(vec![
                        ("salt".into(), Tag::Int(0)),
                        ("sequences".into(), Tag::Compound(Vec::new())),
                    ]),
                ),
            ]),
        });
        let Tag::Compound(root) = &mut document.value else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut root
            .iter_mut()
            .find(|(key, _)| key.matches("data"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(sequences) = &mut data
            .iter_mut()
            .find(|(key, _)| key.matches("sequences"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        for (name, &(low, high)) in &self.changed {
            let mut existing = None;
            for (key, value) in sequences.iter_mut() {
                let parsed = id(&String::from_utf16(&key.0).context("invalid sequence name")?)?;
                if &parsed == name {
                    existing = Some(value);
                    break;
                }
            }
            let state = Tag::LongArray(vec![low as i64, high as i64]);
            if let Some(Tag::Compound(fields)) = existing {
                fields
                    .iter_mut()
                    .find(|(key, _)| key.matches("source"))
                    .unwrap()
                    .1 = state;
            } else {
                let canonical = format!("{}:{}", name.namespace, name.path);
                sequences.push((
                    canonical.as_str().into(),
                    Tag::Compound(vec![("source".into(), state)]),
                ));
            }
        }
        Ok(document)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> NamedTag {
        NamedTag {
            name: "root".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("future_root".into(), Tag::Int(9)),
                (
                    "data".into(),
                    Tag::Compound(vec![
                        ("salt".into(), Tag::Int(-31)),
                        ("include_world_seed".into(), Tag::Byte(0)),
                        ("future_data".into(), Tag::String("preserve".into())),
                        (
                            "sequences".into(),
                            Tag::Compound(vec![
                                (
                                    "example:existing".into(),
                                    Tag::Compound(vec![
                                        ("source".into(), Tag::LongArray(vec![10, -20])),
                                        ("future_entry".into(), Tag::Byte(7)),
                                    ]),
                                ),
                                (
                                    "example:unrelated".into(),
                                    Tag::Compound(vec![(
                                        "source".into(),
                                        Tag::LongArray(vec![30, 40]),
                                    )]),
                                ),
                            ]),
                        ),
                    ]),
                ),
            ]),
        }
    }

    #[test]
    fn draft_preserves_unknown_data_and_keeps_other_sequences() {
        let original = document();
        let mut saved = RandomSequences::read(Some(original.clone())).unwrap();
        assert!(!saved.is_dirty());
        assert_eq!(saved.draft().unwrap(), original);
        let mut random = saved.get("example:existing", 42).unwrap();
        assert_eq!(random.state(), (10, (-20_i64) as u64));
        random.next_long();
        saved.set("example:existing", &random).unwrap();
        assert!(saved.is_dirty());
        let draft = saved.draft().unwrap();
        assert_eq!(saved.source(), Some(&original));
        assert_eq!(draft.value.get("future_root"), Some(&Tag::Int(9)));
        let data = draft.value.get("data").unwrap();
        assert_eq!(
            data.get("future_data"),
            Some(&Tag::String("preserve".into()))
        );
        let entries = data.get("sequences").unwrap();
        assert_eq!(
            entries.get("example:existing").unwrap().get("future_entry"),
            Some(&Tag::Byte(7))
        );
        assert_eq!(
            entries.get("example:unrelated").unwrap().get("source"),
            Some(&Tag::LongArray(vec![30, 40]))
        );
        let restored = RandomSequences::read(Some(draft)).unwrap();
        assert_eq!(
            restored.get("example:existing", 99).unwrap().state(),
            random.state()
        );
        assert_eq!(
            restored.get("example:new", 1).unwrap().state(),
            restored.get("example:new", 999).unwrap().state()
        );
    }

    #[test]
    fn defaults_stage_new_sequences_and_reject_ambiguous_data() {
        let mut sequences = RandomSequences::read(None).unwrap();
        let mut random = sequences
            .get("minecraft:chests/buried_treasure", 123)
            .unwrap();
        random.next_long();
        sequences
            .set("minecraft:chests/buried_treasure", &random)
            .unwrap();
        let restored = RandomSequences::read(Some(sequences.draft().unwrap())).unwrap();
        assert_eq!(
            restored
                .get("minecraft:chests/buried_treasure", 123)
                .unwrap()
                .state(),
            random.state()
        );
        let mut bad = document();
        let Tag::Compound(root) = &mut bad.value else {
            unreachable!()
        };
        root.push(("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)));
        assert!(RandomSequences::read(Some(bad)).is_err());
        let mut bad = document();
        let Tag::Compound(root) = &mut bad.value else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut root
            .iter_mut()
            .find(|(key, _)| key.matches("data"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(entries) = &mut data
            .iter_mut()
            .find(|(key, _)| key.matches("sequences"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        entries.push((
            "example:existing".into(),
            Tag::Compound(vec![("source".into(), Tag::LongArray(vec![0, 1]))]),
        ));
        assert!(RandomSequences::read(Some(bad)).is_err());
    }
}
