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
    block_descriptions: HashSet<String>,
    rarity: HashMap<String, ItemRarity>,
    fire_resistant: HashSet<String>,
    other_resistance: HashSet<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemRarity {
    Common,
    Uncommon,
    Rare,
    Epic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DamageResistancePrototype {
    FireTag,
    ExplosionTag,
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
        let mut block_description_flag = false;
        let mut rarity_constant = None;
        let mut item_rarity = ItemRarity::Common;
        let mut fire_resistant = HashSet::new();
        let mut other_resistance = HashSet::new();
        let mut ids = HashMap::new();
        let mut block_descriptions = HashSet::new();
        let mut rarities = HashMap::new();
        let mut next = 0_i32;
        let mut insert = |id: String, block_description: bool, rarity: ItemRarity| -> Result<()> {
            ensure!(
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'),
                "invalid item registry ID"
            );
            let qualified = format!("minecraft:{id}");
            if block_description {
                block_descriptions.insert(qualified.clone());
            }
            if rarity != ItemRarity::Common {
                rarities.insert(qualified.clone(), rarity);
            }
            ensure!(
                ids.insert(qualified, next).is_none(),
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
                if owner == "net/minecraft/world/item/Item$Properties"
                    && method == "useBlockDescriptionPrefix"
                {
                    ensure!(
                        pending.is_some() && !block_description_flag,
                        "unexpected block description initializer"
                    );
                    block_description_flag = true;
                }
                if owner == "net/minecraft/world/item/Item$Properties" && method == "rarity" {
                    item_rarity = rarity_constant
                        .take()
                        .context("missing rarity initializer")?;
                }
            }
            if ins.op == 0xb2 {
                let (owner, name, _) =
                    items.member(ins.index.context("missing field reference")?)?;
                if owner == "net/minecraft/world/item/Rarity" {
                    rarity_constant = Some(match name {
                        "COMMON" => ItemRarity::Common,
                        "UNCOMMON" => ItemRarity::Uncommon,
                        "RARE" => ItemRarity::Rare,
                        "EPIC" => ItemRarity::Epic,
                        _ => bail!("unknown item rarity"),
                    });
                }
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
                insert(
                    name,
                    method == "registerBlock" || block_description_flag,
                    item_rarity,
                )?;
            } else if color {
                ensure!(
                    !block_description_flag,
                    "unsupported collection description initializer"
                );
                for prefix in COLORS {
                    insert(
                        format!("{prefix}_{name}"),
                        method == "registerBlockItems",
                        item_rarity,
                    )?;
                }
            } else {
                ensure!(
                    !block_description_flag,
                    "unsupported collection description initializer"
                );
                for prefix in WEATHER {
                    let suffix = if name == "copper_block" && !matches!(prefix, "" | "waxed_") {
                        "copper"
                    } else {
                        &name
                    };
                    insert(format!("{prefix}{suffix}"), true, item_rarity)?;
                }
            }
            block_description_flag = false;
            rarity_constant = None;
            item_rarity = ItemRarity::Common;
        }
        ensure!(
            pending.is_none() && !fire_flag && !other_flag && !block_description_flag,
            "unfinished item registration"
        );
        ensure!(
            next > 0 && ids.get("minecraft:air") == Some(&0),
            "invalid item registry"
        );
        Ok(Self {
            ids,
            block_descriptions,
            rarity: rarities,
            fire_resistant,
            other_resistance,
        })
    }
    pub fn id(&self, id: &str) -> Option<i32> {
        self.ids.get(id).copied()
    }
    /// The default translatable description key decoded from item registration.
    pub fn description_key(&self, id: &str) -> Option<String> {
        self.ids.contains_key(id).then(|| {
            let prefix = if self.block_descriptions.contains(id) {
                "block"
            } else {
                "item"
            };
            format!("{prefix}.{}", id.replace(':', "."))
        })
    }
    pub fn default_rarity(&self, id: &str) -> Option<ItemRarity> {
        self.ids.contains_key(id).then(|| {
            self.rarity.get(id).copied().unwrap_or_else(|| {
                // In 26.3 these two registrations set rarity in deferred property callbacks.
                match id {
                    "minecraft:light" => ItemRarity::Epic,
                    "minecraft:sniffer_egg" => ItemRarity::Uncommon,
                    _ => ItemRarity::Common,
                }
            })
        })
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
        if !self.ids.contains_key(id) {
            DamageResistancePrototype::Unknown
        } else if self.other_resistance.contains(id) {
            // In 26.3 the nether star's delayed component resolves to this tag.
            // Other delayed initializers remain unknown until decoded.
            if id == "minecraft:nether_star" {
                DamageResistancePrototype::ExplosionTag
            } else {
                DamageResistancePrototype::Unknown
            }
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
    MethodHandle(u8, u16),
    InvokeDynamic(u16, u16),
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
pub(crate) struct Ins {
    pub(crate) op: u8,
    pub(crate) index: Option<u16>,
}
pub(crate) struct Class {
    pub(crate) name: String,
    pub(crate) superclass: Option<String>,
    constants: Vec<Constant>,
    code: Option<Vec<u8>>,
    methods: std::collections::BTreeSet<(String, String)>,
    bootstraps: Vec<(u16, Vec<u16>)>,
    block_factory_lambdas: HashMap<String, Vec<u8>>,
}
impl Class {
    pub(crate) fn read(bytes: &[u8]) -> Result<Self> {
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
                18 => Constant::InvokeDynamic(r.u16()?, r.u16()?),
                3 | 4 | 17 => {
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
                15 => Constant::MethodHandle(r.u8()?, r.u16()?),
                tag => bail!("unsupported constant tag {tag}"),
            };
            constants.push(value);
        }
        ensure!(constants.len() == count, "invalid constant pool");
        r.u16()?;
        let this = r.u16()?;
        let parent = r.u16()?;
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
        let superclass = if parent == 0 {
            None
        } else {
            Some(class(parent)?.to_owned())
        };
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
        let mut block_factory_lambdas = HashMap::new();
        let mut methods = std::collections::BTreeSet::new();
        for _ in 0..r.u16()? {
            r.u16()?;
            let name_index = r.u16()?;
            let descriptor_index = r.u16()?;
            let method_name = match constants.get(name_index as usize) {
                Some(Constant::Utf8(value)) => std::str::from_utf8(value)?,
                _ => bail!("invalid method name"),
            };
            let descriptor = match constants.get(descriptor_index as usize) {
                Some(Constant::Utf8(value)) => std::str::from_utf8(value)?,
                _ => bail!("invalid method descriptor"),
            };
            ensure!(
                methods.insert((method_name.to_owned(), descriptor.to_owned())),
                "duplicate class method"
            );
            for _ in 0..r.u16()? {
                let attribute = r.u16()?;
                let size = r.u32()? as usize;
                let content = r.take(size)?;
                let attr = match constants.get(attribute as usize) {
                    Some(Constant::Utf8(value)) => value.as_slice(),
                    _ => bail!("invalid attribute name"),
                };
                let factory_lambda = method_name.starts_with("lambda$static$")
                    && descriptor
                        == "(Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)Lnet/minecraft/world/level/block/Block;";
                if (method_name == "<clinit>" || factory_lambda) && attr == b"Code" {
                    let mut inner = Reader {
                        bytes: content,
                        pos: 0,
                    };
                    inner.take(4)?;
                    let len = inner.u32()? as usize;
                    let method_code = inner.take(len)?.to_vec();
                    if method_name == "<clinit>" {
                        ensure!(
                            code.replace(method_code).is_none(),
                            "duplicate class initializer"
                        );
                    } else {
                        ensure!(
                            block_factory_lambdas
                                .insert(method_name.to_owned(), method_code)
                                .is_none(),
                            "duplicate block factory lambda"
                        );
                    }
                }
            }
        }
        let mut bootstraps = Vec::new();
        let mut has_bootstraps = false;
        for _ in 0..r.u16()? {
            let name_index = r.u16()?;
            let size = r.u32()? as usize;
            let content = r.take(size)?;
            if matches!(constants.get(name_index as usize), Some(Constant::Utf8(value)) if value == b"BootstrapMethods")
            {
                ensure!(!has_bootstraps, "duplicate bootstrap methods");
                has_bootstraps = true;
                let mut inner = Reader {
                    bytes: content,
                    pos: 0,
                };
                for _ in 0..inner.u16()? {
                    let method = inner.u16()?;
                    let mut args = Vec::new();
                    for _ in 0..inner.u16()? {
                        args.push(inner.u16()?);
                    }
                    bootstraps.push((method, args));
                }
                ensure!(
                    inner.pos == content.len(),
                    "invalid bootstrap methods length"
                );
            }
        }
        ensure!(r.pos == bytes.len(), "trailing class bytes");
        Ok(Self {
            name,
            superclass,
            constants,
            code,
            methods,
            bootstraps,
            block_factory_lambdas,
        })
    }
    fn utf8(&self, index: u16) -> Result<&str> {
        let Some(Constant::Utf8(value)) = self.constants.get(index as usize) else {
            bail!("invalid UTF-8 reference")
        };
        Ok(std::str::from_utf8(value)?)
    }
    pub(crate) fn member(&self, index: u16) -> Result<(&str, &str, &str)> {
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
    pub(crate) fn instructions(&self) -> Result<Vec<Ins>> {
        let mut r = Reader {
            bytes: self.code.as_deref().context("missing class initializer")?,
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
    pub(crate) fn declares_method(&self, name: &str, descriptor: &str) -> bool {
        self.methods
            .contains(&(name.to_owned(), descriptor.to_owned()))
    }

    /// Extract a constructor or a simple uncaptured constructor lambda.
    pub(crate) fn block_factory_constructor(&self, index: u16) -> Result<Option<String>> {
        let Some(Constant::InvokeDynamic(bootstrap, name_type)) =
            self.constants.get(index as usize)
        else {
            bail!("invalid invokedynamic reference")
        };
        let Some(Constant::NameType(_, descriptor)) = self.constants.get(*name_type as usize)
        else {
            bail!("invalid invokedynamic signature")
        };
        if self.utf8(*descriptor)? != "()Ljava/util/function/Function;" {
            return Ok(None);
        }
        let (bootstrap_method, args) = self
            .bootstraps
            .get(*bootstrap as usize)
            .context("missing bootstrap entry")?;
        let Some(Constant::MethodHandle(6, target)) =
            self.constants.get(*bootstrap_method as usize)
        else {
            return Ok(None);
        };
        let (owner, name, _) = self.member(*target)?;
        if owner != "java/lang/invoke/LambdaMetafactory" || name != "metafactory" || args.len() != 3
        {
            return Ok(None);
        }
        let Some(Constant::MethodHandle(kind, target)) = self.constants.get(args[1] as usize)
        else {
            return Ok(None);
        };
        let (class, name, descriptor) = self.member(*target)?;
        match *kind {
            8 if name == "<init>"
                && descriptor
                    == "(Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)V" =>
            {
                Ok(Some(class.to_owned()))
            }
            6 if class == self.name
                && descriptor
                    == "(Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)Lnet/minecraft/world/level/block/Block;" =>
            {
                self.block_lambda_constructor(name)
            }
            _ => Ok(None),
        }
    }

    /// Accept straight-line constructors returning their first allocated
    /// object. Reject branches, mutations, stack shuffling and unrecognized helpers.
    fn block_lambda_constructor(&self, name: &str) -> Result<Option<String>> {
        let Some(code) = self.block_factory_lambdas.get(name) else {
            return Ok(None);
        };
        let mut r = Reader {
            bytes: code,
            pos: 0,
        };
        if r.u8()? != 0xbb {
            return Ok(None);
        }
        let Some(Constant::Class(index)) = self.constants.get(r.u16()? as usize) else {
            bail!("invalid block constructor class")
        };
        let class = self.utf8(*index)?.to_owned();
        if r.u8()? != 0x59 {
            return Ok(None);
        }
        let mut initialized = false;
        while r.pos < r.bytes.len() {
            let op = r.u8()?;
            if initialized && op != 0xb0 {
                return Ok(None);
            }
            match op {
                0x03..=0x08 | 0x0b..=0x0d | 0x2a | 0x59 => {}
                0x10 | 0x12 => {
                    r.u8()?;
                }
                0x11 | 0x13 | 0xb2 | 0xbb => {
                    r.u16()?;
                }
                0xba if class == "net/minecraft/world/level/block/ChestBlock" => {
                    let index = r.u16()?;
                    if r.u16()? != 0 || !self.is_no_capture_supplier(index)? {
                        return Ok(None);
                    }
                }
                0xb7 => {
                    let (owner, method, _) = self.member(r.u16()?)?;
                    if method != "<init>" {
                        return Ok(None);
                    }
                    if owner == class {
                        if initialized {
                            return Ok(None);
                        }
                        initialized = true;
                    }
                }
                0xb8 => {
                    let (owner, method, descriptor) = self.member(r.u16()?)?;
                    let constant = owner == "net/minecraft/util/valueproviders/ConstantInt"
                        && descriptor == "(I)Lnet/minecraft/util/valueproviders/ConstantInt;";
                    let uniform = owner == "net/minecraft/util/valueproviders/UniformInt"
                        && descriptor == "(II)Lnet/minecraft/util/valueproviders/UniformInt;";
                    let ambient_leaves = matches!(
                        class.as_str(),
                        "net/minecraft/world/level/block/LeavesBlock"
                            | "net/minecraft/world/level/block/UntintedParticleLeavesBlock"
                    ) && owner
                        == "net/minecraft/world/level/block/sounds/AmbientLeavesBlockSoundPlayer"
                        && method == "noAmbientSound"
                        && descriptor
                            == "()Lnet/minecraft/world/level/block/sounds/AmbientLeavesBlockSoundPlayer;";
                    let colored_particle = class
                        == "net/minecraft/world/level/block/UntintedParticleLeavesBlock"
                        && owner == "net/minecraft/core/particles/ColorParticleOption"
                        && method == "create"
                        && descriptor
                            == "(Lnet/minecraft/core/particles/ParticleType;I)Lnet/minecraft/core/particles/ColorParticleOption;";
                    if !(method == "of" && (constant || uniform))
                        && !ambient_leaves
                        && !colored_particle
                    {
                        return Ok(None);
                    }
                }
                0xb0 if initialized && r.pos == r.bytes.len() => return Ok(Some(class)),
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// A no-capture LambdaMetafactory supplier creates a value on the stack;
    /// it does not execute the supplier target while the block is constructed.
    fn is_no_capture_supplier(&self, index: u16) -> Result<bool> {
        let Some(Constant::InvokeDynamic(bootstrap, name_type)) =
            self.constants.get(index as usize)
        else {
            bail!("invalid supplier invocation")
        };
        let Some(Constant::NameType(_, descriptor)) = self.constants.get(*name_type as usize)
        else {
            bail!("invalid supplier signature")
        };
        if self.utf8(*descriptor)? != "()Ljava/util/function/Supplier;" {
            return Ok(false);
        }
        let (method, args) = self
            .bootstraps
            .get(*bootstrap as usize)
            .context("missing supplier bootstrap")?;
        let Some(Constant::MethodHandle(6, target)) = self.constants.get(*method as usize) else {
            return Ok(false);
        };
        let (owner, name, _) = self.member(*target)?;
        Ok(owner == "java/lang/invoke/LambdaMetafactory"
            && name == "metafactory"
            && args.len() == 3)
    }
    pub(crate) fn reference_names(&self) -> Result<HashMap<String, String>> {
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

    /// BlockItemId can use different names for the block and its item (for
    /// example powder snow and its bucket); take the first create argument.
    pub(crate) fn block_reference_names(&self) -> Result<HashMap<String, String>> {
        const ID_CLASS: &str = "net/minecraft/references/BlockItemId";
        const FIELD_TYPE: &str = "Lnet/minecraft/references/BlockItemId;";
        const ONE: &str = "(Ljava/lang/String;)Lnet/minecraft/references/BlockItemId;";
        const TWO: &str =
            "(Ljava/lang/String;Ljava/lang/String;)Lnet/minecraft/references/BlockItemId;";
        ensure!(self.name == BLOCK_IDS, "not a block item reference class");
        let mut strings = Vec::new();
        let mut created = None;
        let mut values = HashMap::new();
        for ins in self.instructions()? {
            if matches!(ins.op, 0x12 | 0x13) {
                if let Some(Constant::String(index)) =
                    self.constants.get(ins.index.unwrap() as usize)
                {
                    strings.push(self.utf8(*index)?.to_owned());
                }
            } else if ins.op == 0xb8 {
                let (owner, method, descriptor) = self.member(ins.index.unwrap())?;
                if owner == ID_CLASS && method == "create" {
                    let count = match descriptor {
                        ONE => 1,
                        TWO => 2,
                        _ => bail!("unsupported block reference factory"),
                    };
                    ensure!(
                        created.is_none() && strings.len() == count,
                        "ambiguous block reference arguments"
                    );
                    created = Some(strings[0].clone());
                    strings.clear();
                }
            } else if ins.op == 0xb3 {
                let (owner, field, descriptor) = self.member(ins.index.unwrap())?;
                ensure!(owner == self.name, "unexpected block reference owner");
                if descriptor == FIELD_TYPE {
                    let name = created
                        .take()
                        .context("block reference was not created directly")?;
                    ensure!(
                        values.insert(field.to_owned(), name).is_none(),
                        "duplicate block reference"
                    );
                } else {
                    ensure!(
                        created.is_none(),
                        "block reference assigned to another field type"
                    );
                }
                strings.clear();
            }
        }
        ensure!(created.is_none(), "unfinished block reference");
        Ok(values)
    }

    /// Block-only keys use a single literal argument and have no item ID.
    pub(crate) fn block_key_reference_names(&self) -> Result<HashMap<String, String>> {
        const BLOCK_KEYS: &str = "net/minecraft/references/BlockIds";
        const KEY_TYPE: &str = "Lnet/minecraft/resources/ResourceKey;";
        const FACTORY: &str = "(Ljava/lang/String;)Lnet/minecraft/resources/ResourceKey;";
        ensure!(self.name == BLOCK_KEYS, "not a block key reference class");
        let mut literal = None;
        let mut created = None;
        let mut values = HashMap::new();
        for ins in self.instructions()? {
            if matches!(ins.op, 0x12 | 0x13) {
                if let Some(Constant::String(index)) =
                    self.constants.get(ins.index.unwrap() as usize)
                {
                    ensure!(literal.is_none(), "ambiguous block key literal");
                    literal = Some(self.utf8(*index)?.to_owned());
                }
            } else if ins.op == 0xb8 {
                let (owner, method, descriptor) = self.member(ins.index.unwrap())?;
                if owner == self.name && method == "create" && descriptor == FACTORY {
                    ensure!(created.is_none(), "ambiguous block key factory");
                    created = Some(literal.take().context("missing block key literal")?);
                }
            } else if ins.op == 0xb3 {
                let (owner, field, descriptor) = self.member(ins.index.unwrap())?;
                ensure!(owner == self.name, "unexpected block key owner");
                if descriptor == KEY_TYPE {
                    let name = created
                        .take()
                        .context("block key was not created directly")?;
                    ensure!(
                        values.insert(field.to_owned(), name).is_none(),
                        "duplicate block key"
                    );
                } else {
                    ensure!(
                        created.is_none(),
                        "block key assigned to another field type"
                    );
                }
                literal = None;
            }
        }
        ensure!(created.is_none(), "unfinished block key reference");
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_lambda_only_accepts_straight_line_return_of_constructed_block() {
        let class = Class {
            name: "net/minecraft/world/level/block/Blocks".into(),
            superclass: None,
            constants: vec![
                Constant::Other,
                Constant::Utf8(b"net/minecraft/world/level/block/SandBlock".to_vec()),
                Constant::Class(1),
                Constant::Utf8(b"<init>".to_vec()),
                Constant::Utf8(
                    b"(Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)V"
                        .to_vec(),
                ),
                Constant::NameType(3, 4),
                Constant::Member(2, 5),
                Constant::Utf8(b"net/minecraft/util/valueproviders/ConstantInt".to_vec()),
                Constant::Class(7),
                Constant::Utf8(b"of".to_vec()),
                Constant::Utf8(b"(I)Lnet/minecraft/util/valueproviders/ConstantInt;".to_vec()),
                Constant::NameType(9, 10),
                Constant::Member(8, 11),
                Constant::Utf8(b"example/UnexpectedHelper".to_vec()),
                Constant::Class(13),
                Constant::Member(14, 11),
            ],
            code: None,
            methods: Default::default(),
            bootstraps: vec![],
            block_factory_lambdas: HashMap::from([
                (
                    "good".into(),
                    vec![0xbb, 0, 2, 0x59, 0x2a, 0xb7, 0, 6, 0xb0],
                ),
                (
                    "branch".into(),
                    vec![0xbb, 0, 2, 0x59, 0x2a, 0x99, 0, 3, 0xb7, 0, 6, 0xb0],
                ),
                (
                    "helper".into(),
                    vec![0xbb, 0, 2, 0x59, 0x2a, 0xb8, 0, 6, 0xb7, 0, 6, 0xb0],
                ),
                (
                    "after_init".into(),
                    vec![0xbb, 0, 2, 0x59, 0x2a, 0xb7, 0, 6, 0x59, 0xb0],
                ),
                (
                    "constant".into(),
                    vec![0xbb, 0, 2, 0x59, 0x03, 0xb8, 0, 12, 0x2a, 0xb7, 0, 6, 0xb0],
                ),
                (
                    "foreign".into(),
                    vec![0xbb, 0, 2, 0x59, 0x03, 0xb8, 0, 15, 0x2a, 0xb7, 0, 6, 0xb0],
                ),
            ]),
        };
        assert_eq!(
            class.block_lambda_constructor("good").unwrap().as_deref(),
            Some("net/minecraft/world/level/block/SandBlock")
        );
        assert_eq!(
            class
                .block_lambda_constructor("constant")
                .unwrap()
                .as_deref(),
            Some("net/minecraft/world/level/block/SandBlock")
        );
        for name in ["branch", "helper", "foreign", "after_init", "missing"] {
            assert_eq!(
                class.block_lambda_constructor(name).unwrap(),
                None,
                "{name}"
            );
        }
    }

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
