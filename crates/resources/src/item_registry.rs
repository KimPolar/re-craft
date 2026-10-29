//! Reconstruct the built-in numeric item registry from the verified 26.3 client.
//!
//! The client registers ordinary items individually, then expands color and
//! copper collections in a defined order. Resource packs cannot change these
//! Java registrations. Unknown class structures fail closed.

use anyhow::{Context, Result, bail, ensure};
use std::collections::{HashMap, HashSet};

const ITEMS: &str = "net/minecraft/world/item/Items";
const ITEM_IDS: &str = "net/minecraft/references/ItemIds";
const BLOCK_IDS: &str = "net/minecraft/references/BlockItemIds";
const COLOR: &str = "net/minecraft/world/level/block/ColorCollection";
const COPPER: &str = "net/minecraft/world/level/block/WeatheringCopperCollection";
const COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];
const WEATHER: [&str; 8] = [
    "",
    "exposed_",
    "weathered_",
    "oxidized_",
    "waxed_",
    "waxed_exposed_",
    "waxed_weathered_",
    "waxed_oxidized_",
];

pub struct ItemRegistry {
    ids: HashMap<String, i32>,
    fire_resistant: HashSet<String>,
    other_resistance: HashSet<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DamageResistancePrototype {
    FireTag,
    Absent,
    Unknown,
}
impl ItemRegistry {
    pub fn load(mut read_class: impl FnMut(&str) -> Result<Option<Vec<u8>>>) -> Result<Self> {
        let load =
            |read: &mut dyn FnMut(&str) -> Result<Option<Vec<u8>>>, name: &str| -> Result<Class> {
                let bytes = read(&format!("{name}.class"))?
                    .with_context(|| format!("missing class {name}"))?;
                let class = Class::read(&bytes)?;
                ensure!(class.name == name, "class name does not match {name}");
                Ok(class)
            };
        let block = load(&mut read_class, BLOCK_IDS)?.reference_names()?;
        let item = load(&mut read_class, ITEM_IDS)?.reference_names()?;
        let items = load(&mut read_class, ITEMS)?;
        let mut pending: Option<(String, String)> = None;
        let mut fire_flag = false;
        let mut other_flag = false;
        let mut fire_resistant = HashSet::new();
        let mut other_resistance = HashSet::new();
        let mut ids = HashMap::new();
        let mut next = 0_i32;
        let mut insert = |id: String| -> Result<()> {
            ensure!(
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'),
                "invalid item registry ID"
            );
            ensure!(
                ids.insert(format!("minecraft:{id}"), next).is_none(),
                "duplicate item registry ID"
            );
            next = next.checked_add(1).context("item registry overflow")?;
            Ok(())
        };
        for ins in items.instructions()? {
            if ins.op == 0xb6 {
                let (owner, method, _) =
                    items.member(ins.index.context("missing method reference")?)?;
                if owner == "net/minecraft/world/item/Item$Properties" && method == "fireResistant"
                {
                    ensure!(
                        pending.is_some() && !fire_flag,
                        "unexpected fire-resistant item initializer"
                    );
                    fire_flag = true;
                }
            }
            if ins.op == 0xb2 {
                let (owner, name, _) =
                    items.member(ins.index.context("missing field reference")?)?;
                if owner == BLOCK_IDS || owner == ITEM_IDS {
                    pending = Some((owner.to_owned(), name.to_owned()));
                }
                if owner == "net/minecraft/core/component/DataComponents"
                    && name == "DAMAGE_RESISTANT"
                {
                    ensure!(
                        pending.is_some() && !other_flag,
                        "unexpected resistance initializer"
                    );
                    other_flag = true;
                }
            }
            if ins.op != 0xb8 {
                continue;
            }
            let (owner, method, _) =
                items.member(ins.index.context("missing method reference")?)?;
            let direct = owner == ITEMS
                && matches!(
                    method,
                    "registerBlock" | "registerItem" | "registerSpawnEgg"
                );
            let color = owner == COLOR && matches!(method, "registerBlockItems" | "registerItems");
            let copper = owner == COPPER && method == "registerItems";
            if !direct && !color && !copper {
                ensure!(
                    !(method.starts_with("register")
                        && (owner == ITEMS || owner == COLOR || owner == COPPER)),
                    "unsupported item registration {owner}.{method}"
                );
                continue;
            }
            let (owner, field) = pending
                .take()
                .context("item registration has no reference")?;
            let names = if owner == BLOCK_IDS { &block } else { &item };
            ensure!(
                field
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'),
                "invalid reference field"
            );
            let name = names
                .get(&field)
                .cloned()
                .with_context(|| format!("unknown item reference {owner}.{field}"))?;
            if fire_flag {
                ensure!(direct, "unsupported fire-resistant collection initializer");
                ensure!(
                    fire_resistant.insert(format!("minecraft:{name}")),
                    "duplicate fire-resistant item"
                );
                fire_flag = false;
            }
            if other_flag {
                ensure!(
                    direct,
                    "unsupported damage-resistant collection initializer"
                );
                ensure!(
                    other_resistance.insert(format!("minecraft:{name}")),
                    "duplicate damage resistance item"
                );
                other_flag = false;
            }
            if direct {
                insert(name)?;
            } else if color {
                for prefix in COLORS {
                    insert(format!("{prefix}_{name}"))?;
                }
            } else {
                for prefix in WEATHER {
                    let suffix = if name == "copper_block" && !matches!(prefix, "" | "waxed_") {
                        "copper"
                    } else {
                        &name
                    };
                    insert(format!("{prefix}{suffix}"))?;
                }
            }
        }
        ensure!(
            pending.is_none() && !fire_flag && !other_flag,
            "unfinished item registration"
        );
        ensure!(
            next > 0 && ids.get("minecraft:air") == Some(&0),
            "invalid item registry"
        );
        Ok(Self {
            ids,
            fire_resistant,
            other_resistance,
        })
    }
    pub fn id(&self, id: &str) -> Option<i32> {
        self.ids.get(id).copied()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, i32)> {
        self.ids.iter().map(|(name, id)| (name.as_str(), *id))
    }
    /// Only explicitly recognized fire-resistant prototypes are returned.
    /// Absence is unknown until other initializer paths are decoded.
    pub fn explicitly_fire_resistant(&self, id: &str) -> bool {
        self.fire_resistant.contains(id)
    }
    /// Recognized default component state in the verified 26.3 registry.
    /// Initializers whose resistance value is not decoded remain unknown.
    pub fn damage_resistance_prototype(&self, id: &str) -> DamageResistancePrototype {
        if !self.ids.contains_key(id) || self.other_resistance.contains(id) {
            DamageResistancePrototype::Unknown
        } else if self.fire_resistant.contains(id) {
            DamageResistancePrototype::FireTag
        } else {
            DamageResistancePrototype::Absent
        }
    }
}

#[derive(Clone)]
enum Constant {
    Utf8(Vec<u8>),
    Class(u16),
    String(u16),
    NameType(u16, u16),
    Member(u16, u16),
    Other,
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(count).context("class size overflow")?;
        let data = self.bytes.get(self.pos..end).context("truncated class")?;
        self.pos = end;
        Ok(data)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }
}
struct Ins {
    op: u8,
    index: Option<u16>,
}
struct Class {
    name: String,
    constants: Vec<Constant>,
    code: Vec<u8>,
}
impl Class {
    fn read(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader { bytes, pos: 0 };
        ensure!(r.u32()? == 0xcafe_babe, "invalid class magic");
        r.take(4)?;
        let count = r.u16()? as usize;
        ensure!(count > 0, "empty constant pool");
        let mut constants = vec![Constant::Other];
        while constants.len() < count {
            let value = match r.u8()? {
                1 => {
                    let len = r.u16()? as usize;
                    Constant::Utf8(r.take(len)?.to_vec())
                }
                7 => Constant::Class(r.u16()?),
                8 => Constant::String(r.u16()?),
                9..=11 => Constant::Member(r.u16()?, r.u16()?),
                12 => Constant::NameType(r.u16()?, r.u16()?),
                3 | 4 | 17 | 18 => {
                    r.take(4)?;
                    Constant::Other
                }
                5 | 6 => {
                    r.take(8)?;
                    constants.push(Constant::Other);
                    Constant::Other
                }
                16 | 19 | 20 => {
                    r.take(2)?;
                    Constant::Other
                }
                15 => {
                    r.take(3)?;
                    Constant::Other
                }
                tag => bail!("unsupported constant tag {tag}"),
            };
            constants.push(value);
        }
        ensure!(constants.len() == count, "invalid constant pool");
        r.u16()?;
        let this = r.u16()?;
        r.u16()?;
        let class = |index: u16| -> Result<&str> {
            let Some(Constant::Class(name)) = constants.get(index as usize) else {
                bail!("invalid class reference")
            };
            let Some(Constant::Utf8(value)) = constants.get(*name as usize) else {
                bail!("invalid class name")
            };
            Ok(std::str::from_utf8(value)?)
        };
        let name = class(this)?.to_owned();
        let interfaces = r.u16()? as usize;
        r.take(interfaces * 2)?;
        for _ in 0..r.u16()? {
            r.take(6)?;
            for _ in 0..r.u16()? {
                r.u16()?;
                let size = r.u32()? as usize;
                r.take(size)?;
            }
        }
        let mut code = None;
        for _ in 0..r.u16()? {
            r.u16()?;
            let name_index = r.u16()?;
            r.u16()?;
            let method_name = match constants.get(name_index as usize) {
                Some(Constant::Utf8(value)) => std::str::from_utf8(value)?,
                _ => bail!("invalid method name"),
            };
            for _ in 0..r.u16()? {
                let attribute = r.u16()?;
                let size = r.u32()? as usize;
                let content = r.take(size)?;
                let attr = match constants.get(attribute as usize) {
                    Some(Constant::Utf8(value)) => value.as_slice(),
                    _ => bail!("invalid attribute name"),
                };
                if method_name == "<clinit>" && attr == b"Code" {
                    ensure!(code.is_none(), "duplicate class initializer");
                    let mut inner = Reader {
                        bytes: content,
                        pos: 0,
                    };
                    inner.take(4)?;
                    let len = inner.u32()? as usize;
                    code = Some(inner.take(len)?.to_vec());
                }
            }
        }
        for _ in 0..r.u16()? {
            r.u16()?;
            let size = r.u32()? as usize;
            r.take(size)?;
        }
        ensure!(r.pos == bytes.len(), "trailing class bytes");
        Ok(Self {
            name,
            constants,
            code: code.context("missing class initializer")?,
        })
    }
    fn utf8(&self, index: u16) -> Result<&str> {
        let Some(Constant::Utf8(value)) = self.constants.get(index as usize) else {
            bail!("invalid UTF-8 reference")
        };
        Ok(std::str::from_utf8(value)?)
    }
    fn member(&self, index: u16) -> Result<(&str, &str, &str)> {
        let Some(Constant::Member(owner, name)) = self.constants.get(index as usize) else {
            bail!("invalid member reference")
        };
        let Some(Constant::Class(owner)) = self.constants.get(*owner as usize) else {
            bail!("invalid owner reference")
        };
        let Some(Constant::NameType(name, descriptor)) = self.constants.get(*name as usize) else {
            bail!("invalid name/type reference")
        };
        Ok((
            self.utf8(*owner)?,
            self.utf8(*name)?,
            self.utf8(*descriptor)?,
        ))
    }
    fn instructions(&self) -> Result<Vec<Ins>> {
        let mut r = Reader {
            bytes: &self.code,
            pos: 0,
        };
        let mut out = Vec::new();
        while r.pos < r.bytes.len() {
            let op = r.u8()?;
            let index = match op {
                0x12 => Some(u16::from(r.u8()?)),
                0x13 | 0xb2..=0xb8 | 0xbb | 0xbd => Some(r.u16()?),
                0xba => {
                    let index = r.u16()?;
                    ensure!(r.u16()? == 0, "invalid invokedynamic");
                    Some(index)
                }
                0x10 => {
                    r.u8()?;
                    None
                }
                0x11 => {
                    r.u16()?;
                    None
                }
                0x03..=0x08 | 0x0b..=0x0d | 0x53 | 0x59 | 0xb1 => None,
                _ => bail!("unsupported initializer opcode {op:#x}"),
            };
            out.push(Ins { op, index });
        }
        ensure!(
            out.last().is_some_and(|ins| ins.op == 0xb1),
            "initializer does not return"
        );
        Ok(out)
    }
    fn reference_names(&self) -> Result<HashMap<String, String>> {
        let mut values = HashMap::new();
        let mut last = None;
        let mut generator = None;
        for ins in self.instructions()? {
            if matches!(ins.op, 0x12 | 0x13) {
                if let Some(Constant::String(index)) =
                    self.constants.get(ins.index.unwrap() as usize)
                {
                    last = Some(self.utf8(*index)?.to_owned());
                }
            } else if ins.op == 0xb8 {
                let (owner, method, _) = self.member(ins.index.unwrap())?;
                if owner == self.name {
                    generator = Some(method);
                }
            } else if ins.op == 0xb3 {
                let (owner, field, _) = self.member(ins.index.unwrap())?;
                ensure!(owner == self.name, "unexpected reference owner");
                let value = if let Some(value) = last.take() {
                    value
                } else if self.name == BLOCK_IDS && field == "COPPER_BLOCK" {
                    "copper_block".to_owned()
                } else if self.name == ITEM_IDS
                    && match generator {
                        Some("createSpawnEgg") => field.ends_with("_SPAWN_EGG"),
                        Some("createMusicDisc") => field.starts_with("MUSIC_DISC_"),
                        Some("createPotterySherd") => field.ends_with("_POTTERY_SHERD"),
                        Some("createArmorTrimSmithingTemplate") => {
                            field.ends_with("_ARMOR_TRIM_SMITHING_TEMPLATE")
                        }
                        _ => false,
                    }
                {
                    field.to_ascii_lowercase()
                } else {
                    bail!("unsupported item reference generator for {owner}.{field}");
                };
                generator = None;
                ensure!(
                    values.insert(field.to_owned(), value).is_none(),
                    "duplicate reference field"
                );
            }
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_or_missing_registry_classes_fail_closed() {
        assert!(ItemRegistry::load(|_| Ok(None)).is_err());
        for bytes in [
            vec![],
            vec![0xca, 0xfe, 0xba, 0xbe],
            vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0, 0, 1],
        ] {
            assert!(ItemRegistry::load(|_| Ok(Some(bytes.clone()))).is_err());
        }
    }
}
