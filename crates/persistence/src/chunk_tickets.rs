//! Read 26.3 dimension-scoped chunk tickets without changing their source.
//!
//! Decoded tickets are initially inactive in Java. Activation visits a
//! fastutil map grouped by chunk, so file order alone is not the activation
//! order. This module does not expire tickets or write saved data.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};

use crate::{
    nbt::{NamedTag, Tag, TagType},
    world::{ChunkPos, TARGET_DATA_VERSION},
};

/// Built-in 26.3 ticket types. Their flags come from Java's TicketType.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum TicketKind {
    PlayerSpawn,
    SpawnSearch,
    Dragon,
    PlayerLoading,
    PlayerSimulation,
    Forced,
    Portal,
    EnderPearl,
    Unknown,
}

impl TicketKind {
    fn parse(value: &str) -> Result<Self> {
        let (namespace, path) = value.split_once(':').unwrap_or(("minecraft", value));
        ensure!(
            namespace == "minecraft",
            "unsupported chunk ticket type {value}"
        );
        Ok(match path {
            "player_spawn" => Self::PlayerSpawn,
            "spawn_search" => Self::SpawnSearch,
            "dragon" => Self::Dragon,
            "player_loading" => Self::PlayerLoading,
            "player_simulation" => Self::PlayerSimulation,
            "forced" => Self::Forced,
            "portal" => Self::Portal,
            "ender_pearl" => Self::EnderPearl,
            "unknown" => Self::Unknown,
            _ => bail!("unsupported chunk ticket type {value}"),
        })
    }

    pub fn identifier(self) -> &'static str {
        match self {
            Self::PlayerSpawn => "minecraft:player_spawn",
            Self::SpawnSearch => "minecraft:spawn_search",
            Self::Dragon => "minecraft:dragon",
            Self::PlayerLoading => "minecraft:player_loading",
            Self::PlayerSimulation => "minecraft:player_simulation",
            Self::Forced => "minecraft:forced",
            Self::Portal => "minecraft:portal",
            Self::EnderPearl => "minecraft:ender_pearl",
            Self::Unknown => "minecraft:unknown",
        }
    }

    /// Stable key for distinguishing ticket types in the simulation tracker.
    pub fn simulation_key(self) -> u64 {
        self as u64
    }

    pub fn simulates(self) -> bool {
        matches!(
            self,
            Self::Dragon | Self::PlayerSimulation | Self::Forced | Self::Portal | Self::EnderPearl
        )
    }

    pub fn timeout(self) -> Option<i64> {
        match self {
            Self::PlayerSpawn => Some(20),
            Self::SpawnSearch | Self::Unknown => Some(1),
            Self::Portal => Some(300),
            Self::EnderPearl => Some(40),
            _ => None,
        }
    }

    /// Java's unknown loading ticket may expire before its chunk is ready.
    pub fn can_expire_if_unloaded(self) -> bool {
        matches!(self, Self::Unknown)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SavedChunkTicket {
    pub position: ChunkPos,
    pub kind: TicketKind,
    pub level: i32,
    pub ticks_left: i64,
}

#[derive(Clone, Debug, Default)]
pub struct SavedChunkTickets {
    source: Option<NamedTag>,
    entries: Vec<SavedChunkTicket>,
    activation: Vec<usize>,
}

fn unique<'a>(tag: &'a Tag, key: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = tag else {
        bail!("chunk ticket value is not a compound")
    };
    let mut matches = fields.iter().filter(|(name, _)| name.matches(key));
    let value = matches.next().map(|(_, value)| value);
    ensure!(
        matches.next().is_none(),
        "duplicate chunk ticket {key} field"
    );
    Ok(value)
}

fn position(tag: &Tag) -> Result<ChunkPos> {
    let (x, z) = match tag {
        Tag::IntArray(values) if values.len() == 2 => (values[0], values[1]),
        Tag::ByteArray(values) if values.len() == 2 => {
            (i32::from(values[0] as i8), i32::from(values[1] as i8))
        }
        Tag::LongArray(values) if values.len() == 2 => (values[0] as i32, values[1] as i32),
        Tag::List {
            element_type,
            values,
        } if values.len() == 2 && values.iter().all(|value| value.tag_type() == *element_type) => (
            values[0]
                .as_java_int()
                .context("chunk position is not numeric")?,
            values[1]
                .as_java_int()
                .context("chunk position is not numeric")?,
        ),
        _ => bail!("chunk position must contain two numbers"),
    };
    Ok(ChunkPos { x, z })
}

impl SavedChunkTickets {
    /// Decode a 26.3 saved-data document, retaining all original fields. An
    /// absent file represents an initially empty ticket store.
    pub fn read(source: Option<NamedTag>) -> Result<Self> {
        let Some(document) = source else {
            return Ok(Self::default());
        };
        ensure!(
            matches!(
                unique(&document.value, "DataVersion")?,
                Some(Tag::Int(TARGET_DATA_VERSION))
            ),
            "unsupported chunk ticket DataVersion"
        );
        let data = unique(&document.value, "data")?.context("chunk ticket data is missing")?;
        let values = match unique(data, "tickets")? {
            None => &[][..],
            Some(Tag::List {
                element_type: TagType::Compound,
                values,
            }) => values.as_slice(),
            Some(Tag::List {
                element_type: TagType::End,
                values,
            }) if values.is_empty() => &[][..],
            _ => bail!("chunk tickets must be a list of compounds"),
        };
        let mut entries = Vec::with_capacity(values.len());
        let mut groups: BTreeMap<ChunkPos, Vec<usize>> = BTreeMap::new();
        let mut order = ActivationOrder::default();
        for (index, value) in values.iter().enumerate() {
            let parse = || -> Result<SavedChunkTicket> {
                let position = position(
                    unique(value, "chunk_pos")?.context("chunk ticket position is missing")?,
                )?;
                let Some(Tag::String(kind)) = unique(value, "type")? else {
                    bail!("chunk ticket type is missing or invalid");
                };
                let kind = String::from_utf16(&kind.0).context("invalid chunk ticket type")?;
                let kind = TicketKind::parse(&kind)?;
                let level = unique(value, "level")?
                    .and_then(Tag::as_java_int)
                    .context("chunk ticket level is missing or invalid")?;
                ensure!(level >= 0, "chunk ticket level must not be negative");
                let ticks_left = unique(value, "ticks_left")?
                    .map(|tag| {
                        tag.as_java_long()
                            .context("invalid chunk ticket ticks_left")
                    })
                    .transpose()?
                    .unwrap_or(0);
                Ok(SavedChunkTicket {
                    position,
                    kind,
                    level,
                    ticks_left,
                })
            };
            let ticket = parse().with_context(|| format!("invalid chunk ticket {index}"))?;
            groups.entry(ticket.position).or_default().push(index);
            order.insert(ticket.position);
            entries.push(ticket);
        }
        let activation = order
            .positions()
            .flat_map(|pos| groups[&pos].iter().copied())
            .collect();
        Ok(Self {
            source: Some(document),
            entries,
            activation,
        })
    }

    pub fn source(&self) -> Option<&NamedTag> {
        self.source.as_ref()
    }

    pub fn entries(&self) -> &[SavedChunkTicket] {
        &self.entries
    }

    /// Entries in the order Java activates decoded tickets. Repeated tickets
    /// are retained here; Java deduplicates them while activating the store.
    pub fn activation_order(&self) -> impl Iterator<Item = &SavedChunkTicket> {
        self.activation.iter().map(|&index| &self.entries[index])
    }
}

fn pack(pos: ChunkPos) -> u64 {
    u64::from(pos.x as u32) | (u64::from(pos.z as u32) << 32)
}

fn unpack(value: u64) -> ChunkPos {
    ChunkPos {
        x: value as u32 as i32,
        z: (value >> 32) as u32 as i32,
    }
}

fn mix(value: u64) -> u64 {
    let hash = value.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let hash = hash ^ (hash >> 32);
    hash ^ (hash >> 16)
}

/// Insert-only fastutil Long2ObjectOpenHashMap used by TicketStorage.fromPacked.
struct ActivationOrder {
    slots: Vec<Option<u64>>,
    zero: bool,
    len: usize,
}

impl Default for ActivationOrder {
    fn default() -> Self {
        Self {
            slots: vec![None; 32],
            zero: false,
            len: 0,
        }
    }
}

impl ActivationOrder {
    fn find(&self, key: u64) -> usize {
        let mask = self.slots.len() - 1;
        let mut index = mix(key) as usize & mask;
        while self.slots[index].is_some_and(|entry| entry != key) {
            index = (index + 1) & mask;
        }
        index
    }

    fn insert(&mut self, pos: ChunkPos) {
        let key = pack(pos);
        if key == 0 {
            if self.zero {
                return;
            }
            self.zero = true;
        } else {
            let index = self.find(key);
            if self.slots[index].is_some() {
                return;
            }
            self.slots[index] = Some(key);
        }
        let previous = self.len;
        self.len += 1;
        if previous >= self.slots.len() * 3 / 4 {
            let size = self.slots.len() * 2;
            let previous = std::mem::replace(&mut self.slots, vec![None; size]);
            for key in previous.into_iter().rev().flatten() {
                let index = self.find(key);
                self.slots[index] = Some(key);
            }
        }
    }

    fn positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.zero
            .then_some(0)
            .into_iter()
            .chain(self.slots.iter().rev().filter_map(|entry| *entry))
            .map(unpack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pos: Tag, kind: &str, level: Tag, ticks: Option<Tag>) -> Tag {
        let mut fields = vec![
            ("chunk_pos".into(), pos),
            ("type".into(), Tag::String(kind.into())),
            ("level".into(), level),
        ];
        if let Some(ticks) = ticks {
            fields.push(("ticks_left".into(), ticks));
        }
        Tag::Compound(fields)
    }

    fn document(values: Vec<Tag>) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                (
                    "data".into(),
                    Tag::Compound(vec![
                        (
                            "tickets".into(),
                            Tag::List {
                                element_type: TagType::Compound,
                                values,
                            },
                        ),
                        ("future_field".into(), Tag::String("retained".into())),
                    ]),
                ),
            ]),
        }
    }

    #[test]
    fn retains_ticket_values_duplicates_and_unknown_fields() {
        let source = document(vec![
            entry(Tag::IntArray(vec![0, -5]), "forced", Tag::Int(31), None),
            entry(
                Tag::IntArray(vec![0, -5]),
                "minecraft:portal",
                Tag::Byte(30),
                Some(Tag::Long(77)),
            ),
            entry(Tag::IntArray(vec![0, -5]), "forced", Tag::Int(31), None),
        ]);
        let tickets = SavedChunkTickets::read(Some(source.clone())).unwrap();
        assert_eq!(tickets.source(), Some(&source));
        assert_eq!(tickets.entries().len(), 3);
        assert_eq!(
            tickets.activation_order().copied().collect::<Vec<_>>(),
            tickets.entries()
        );
        assert_eq!(tickets.entries()[0].kind, TicketKind::Forced);
        assert_eq!(tickets.entries()[1].ticks_left, 77);
        assert_eq!(tickets.entries()[1].kind.timeout(), Some(300));
        assert!(tickets.entries()[0].kind.simulates());
        assert!(!TicketKind::PlayerLoading.simulates());
    }

    #[test]
    fn rejects_unsupported_or_ambiguous_saved_tickets() {
        let good = || entry(Tag::IntArray(vec![1, 2]), "forced", Tag::Int(31), None);
        assert!(SavedChunkTickets::read(None).unwrap().entries().is_empty());
        let empty = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("data".into(), Tag::Compound(vec![])),
            ]),
        };
        assert!(
            SavedChunkTickets::read(Some(empty))
                .unwrap()
                .entries()
                .is_empty()
        );
        for invalid in [
            entry(Tag::IntArray(vec![1]), "forced", Tag::Int(31), None),
            entry(Tag::IntArray(vec![1, 2]), "forced", Tag::Int(-1), None),
            entry(
                Tag::IntArray(vec![1, 2]),
                "custom:forced",
                Tag::Int(31),
                None,
            ),
            entry(
                Tag::IntArray(vec![1, 2]),
                "portal",
                Tag::Int(31),
                Some(Tag::String("bad".into())),
            ),
        ] {
            assert!(SavedChunkTickets::read(Some(document(vec![invalid]))).is_err());
        }
        let mut duplicate = good();
        let Tag::Compound(fields) = &mut duplicate else {
            unreachable!()
        };
        fields.push(("level".into(), Tag::Int(30)));
        assert!(SavedChunkTickets::read(Some(document(vec![duplicate]))).is_err());
        let mut wrong_version = document(vec![good()]);
        let Tag::Compound(fields) = &mut wrong_version.value else {
            unreachable!()
        };
        fields[0].1 = Tag::Int(0);
        assert!(SavedChunkTickets::read(Some(wrong_version)).is_err());
    }
}
