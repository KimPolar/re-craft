//! Inspect the verified client's entity type declarations and inheritance.
//!
//! Java's null-entity collision query ignores types inheriting Entity's default
//! `canBeCollidedWith` implementation. Types overriding it need their own
//! runtime simulation before they can be classified safely.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};

const TYPES_CLASS: &str = "net/minecraft/world/entity/EntityTypes";
const ENTITY_CLASS: &str = "net/minecraft/world/entity/Entity";
const TYPE_DESCRIPTOR: &str = "Lnet/minecraft/world/entity/EntityType;";
const TYPE_SIGNATURE_PREFIX: &str = "Lnet/minecraft/world/entity/EntityType<L";
const COLLISION_DESCRIPTOR: &str = "(Lnet/minecraft/world/entity/Entity;)Z";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntityCollisionKind {
    Never,
    RequiresEntityState,
}

#[derive(Clone, Debug)]
pub struct EntityCollisionRegistry {
    kinds: BTreeMap<String, EntityCollisionKind>,
}

impl EntityCollisionRegistry {
    /// `read_class` must read only from the verified client JAR, not a pack.
    pub fn load(mut read_class: impl FnMut(&str) -> Result<Option<Vec<u8>>>) -> Result<Self> {
        let types = read_class(&format!("{TYPES_CLASS}.class"))?
            .context("entity type declarations are missing")?;
        let types = Class::read(&types, true).context("invalid entity type declarations")?;
        let mut cache = BTreeMap::new();
        let mut kinds = BTreeMap::new();
        for field in types.fields {
            ensure!(
                !field.name.is_empty()
                    && field
                        .name
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'),
                "unsupported entity field name"
            );
            let signature = field
                .signature
                .context("entity type has no generic signature")?;
            let class = signature
                .strip_prefix(TYPE_SIGNATURE_PREFIX)
                .and_then(|value| value.strip_suffix(";>;"))
                .context("unsupported entity type signature")?;
            let kind = resolve(class, &mut read_class, &mut cache, &mut BTreeSet::new())?;
            let id = format!("minecraft:{}", field.name.to_ascii_lowercase());
            ensure!(
                kinds.insert(id, kind).is_none(),
                "duplicate entity type declaration"
            );
        }
        ensure!(!kinds.is_empty(), "entity type registry is empty");
        Ok(Self { kinds })
    }

    pub fn kind(&self, id: &str) -> Option<EntityCollisionKind> {
        self.kinds.get(id).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, EntityCollisionKind)> {
        self.kinds.iter().map(|(id, kind)| (id.as_str(), *kind))
    }
}

fn resolve(
    name: &str,
    read_class: &mut impl FnMut(&str) -> Result<Option<Vec<u8>>>,
    cache: &mut BTreeMap<String, EntityCollisionKind>,
    visiting: &mut BTreeSet<String>,
) -> Result<EntityCollisionKind> {
    if let Some(kind) = cache.get(name) {
        return Ok(*kind);
    }
    ensure!(
        name.starts_with("net/minecraft/world/entity/")
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/_$".contains(&c)),
        "invalid entity class name"
    );
    ensure!(
        visiting.insert(name.to_owned()) && visiting.len() <= 64,
        "cyclic entity inheritance"
    );
    let result = (|| {
        let bytes = read_class(&format!("{name}.class"))?
            .with_context(|| format!("entity class {name} is missing"))?;
        let class =
            Class::read(&bytes, false).with_context(|| format!("invalid entity class {name}"))?;
        ensure!(
            class.name == name,
            "entity class name does not match its path"
        );
        if name == ENTITY_CLASS {
            ensure!(
                class.has_collision_method,
                "base entity collision method is missing"
            );
            Ok(EntityCollisionKind::Never)
        } else if class.has_collision_method {
            Ok(EntityCollisionKind::RequiresEntityState)
        } else {
            let parent = class.superclass.context("entity superclass is missing")?;
            resolve(&parent, read_class, cache, visiting)
        }
    })();
    visiting.remove(name);
    if let Ok(kind) = result {
        cache.insert(name.to_owned(), kind);
    }
    result
}

#[derive(Debug)]
enum Constant<'a> {
    Utf8(&'a [u8]),
    Class(u16),
    Other,
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .context("class length overflow")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .context("truncated class")?;
        self.position = end;
        Ok(bytes)
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

struct Field {
    name: String,
    signature: Option<String>,
}
struct Class {
    name: String,
    superclass: Option<String>,
    fields: Vec<Field>,
    has_collision_method: bool,
}

impl Class {
    fn read(bytes: &[u8], collect_fields: bool) -> Result<Self> {
        let mut input = Reader { bytes, position: 0 };
        ensure!(input.u32()? == 0xcafe_babe, "invalid class magic");
        input.take(4)?; // minor and major versions
        let count = input.u16()? as usize;
        ensure!(count > 0, "invalid constant pool length");
        let mut constants = Vec::with_capacity(count);
        constants.push(Constant::Other);
        while constants.len() < count {
            let tag = input.u8()?;
            let constant = match tag {
                1 => {
                    let length = input.u16()? as usize;
                    Constant::Utf8(input.take(length)?)
                }
                7 => Constant::Class(input.u16()?),
                3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => {
                    input.take(4)?;
                    Constant::Other
                }
                5 | 6 => {
                    input.take(8)?;
                    constants.push(Constant::Other);
                    Constant::Other
                }
                8 | 16 | 19 | 20 => {
                    input.take(2)?;
                    Constant::Other
                }
                15 => {
                    input.take(3)?;
                    Constant::Other
                }
                _ => bail!("unsupported class constant tag {tag}"),
            };
            constants.push(constant);
        }
        ensure!(constants.len() == count, "invalid wide constant");
        let utf8 = |index: u16| -> Result<&str> {
            let Some(Constant::Utf8(bytes)) = constants.get(index as usize) else {
                bail!("invalid class UTF-8 reference")
            };
            std::str::from_utf8(bytes).context("class identifier is not UTF-8")
        };
        let class_name = |index: u16| -> Result<String> {
            let Some(Constant::Class(name)) = constants.get(index as usize) else {
                bail!("invalid class reference")
            };
            Ok(utf8(*name)?.to_owned())
        };
        input.u16()?; // access flags
        let name = class_name(input.u16()?)?;
        let parent = input.u16()?;
        let superclass = if parent == 0 {
            None
        } else {
            Some(class_name(parent)?)
        };
        let interfaces = input.u16()? as usize;
        input.take(interfaces * 2)?;
        let mut fields = Vec::new();
        for _ in 0..input.u16()? {
            let access = input.u16()?;
            let name_index = input.u16()?;
            let descriptor_index = input.u16()?;
            let wanted = collect_fields
                && utf8(descriptor_index)? == TYPE_DESCRIPTOR
                && access & 0x0019 == 0x0019;
            let mut signature = None;
            for _ in 0..input.u16()? {
                let attribute = input.u16()?;
                let size = input.u32()? as usize;
                if wanted && utf8(attribute)? == "Signature" {
                    ensure!(
                        size == 2 && signature.is_none(),
                        "invalid class signature attribute"
                    );
                    signature = Some(utf8(input.u16()?)?.to_owned());
                } else {
                    input.take(size)?;
                }
            }
            if wanted {
                fields.push(Field {
                    name: utf8(name_index)?.to_owned(),
                    signature,
                });
            }
        }
        let mut has_collision_method = false;
        for _ in 0..input.u16()? {
            input.u16()?; // access flags
            let name = utf8(input.u16()?)?;
            let descriptor = utf8(input.u16()?)?;
            has_collision_method |=
                name == "canBeCollidedWith" && descriptor == COLLISION_DESCRIPTOR;
            for _ in 0..input.u16()? {
                input.u16()?;
                let size = input.u32()? as usize;
                input.take(size)?;
            }
        }
        for _ in 0..input.u16()? {
            input.u16()?;
            let size = input.u32()? as usize;
            input.take(size)?;
        }
        ensure!(input.position == bytes.len(), "trailing class bytes");
        Ok(Self {
            name,
            superclass,
            fields,
            has_collision_method,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_class(
        name: &str,
        parent: &str,
        fields: &[(&str, &str)],
        collision: bool,
    ) -> Vec<u8> {
        let mut constants = Vec::<Vec<u8>>::new();
        fn utf8(pool: &mut Vec<Vec<u8>>, value: &str) -> u16 {
            let mut entry = vec![1];
            entry.extend_from_slice(&(value.len() as u16).to_be_bytes());
            entry.extend_from_slice(value.as_bytes());
            pool.push(entry);
            pool.len() as u16
        }
        fn class(pool: &mut Vec<Vec<u8>>, name: &str) -> u16 {
            let name = utf8(pool, name);
            let mut entry = vec![7];
            entry.extend_from_slice(&name.to_be_bytes());
            pool.push(entry);
            pool.len() as u16
        }
        let this = class(&mut constants, name);
        let superclass = class(&mut constants, parent);
        let descriptor = utf8(&mut constants, TYPE_DESCRIPTOR);
        let signature_name = utf8(&mut constants, "Signature");
        let collision_name = utf8(&mut constants, "canBeCollidedWith");
        let collision_descriptor = utf8(&mut constants, COLLISION_DESCRIPTOR);
        let fields: Vec<_> = fields
            .iter()
            .map(|(name, class)| {
                let name = utf8(&mut constants, name);
                let signature = utf8(
                    &mut constants,
                    &format!("{TYPE_SIGNATURE_PREFIX}{class};>;"),
                );
                (name, signature)
            })
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(&0xcafe_babeu32.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 69]);
        out.extend_from_slice(&((constants.len() + 1) as u16).to_be_bytes());
        for constant in constants {
            out.extend_from_slice(&constant);
        }
        out.extend_from_slice(&0x0021u16.to_be_bytes());
        out.extend_from_slice(&this.to_be_bytes());
        out.extend_from_slice(&superclass.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // interfaces
        out.extend_from_slice(&(fields.len() as u16).to_be_bytes());
        for (name, signature) in fields {
            out.extend_from_slice(&0x0019u16.to_be_bytes());
            out.extend_from_slice(&name.to_be_bytes());
            out.extend_from_slice(&descriptor.to_be_bytes());
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&signature_name.to_be_bytes());
            out.extend_from_slice(&2u32.to_be_bytes());
            out.extend_from_slice(&signature.to_be_bytes());
        }
        out.extend_from_slice(&u16::from(collision).to_be_bytes());
        if collision {
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&collision_name.to_be_bytes());
            out.extend_from_slice(&collision_descriptor.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
        }
        out.extend_from_slice(&0u16.to_be_bytes()); // class attributes
        out
    }

    #[test]
    fn malformed_class_data_is_rejected() {
        assert!(Class::read(&[], false).is_err());
        assert!(Class::read(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 69, 0, 1], false).is_err());
        assert!(EntityCollisionRegistry::load(|_| Ok(None)).is_err());
    }

    #[test]
    fn registry_follows_superclasses_and_distinguishes_overrides() {
        let animal = "net/minecraft/world/entity/TestAnimal";
        let special = "net/minecraft/world/entity/TestSpecial";
        let types = synthetic_class(
            TYPES_CLASS,
            "java/lang/Object",
            &[("PIG", animal), ("BOAT", special)],
            false,
        );
        let base = synthetic_class(ENTITY_CLASS, "java/lang/Object", &[], true);
        let animal_class = synthetic_class(animal, ENTITY_CLASS, &[], false);
        let special_class = synthetic_class(special, animal, &[], true);
        let classes = BTreeMap::from([
            (format!("{TYPES_CLASS}.class"), types),
            (format!("{ENTITY_CLASS}.class"), base),
            (format!("{animal}.class"), animal_class),
            (format!("{special}.class"), special_class),
        ]);
        let registry =
            EntityCollisionRegistry::load(|path| Ok(classes.get(path).cloned())).unwrap();
        assert_eq!(
            registry.kind("minecraft:pig"),
            Some(EntityCollisionKind::Never)
        );
        assert_eq!(
            registry.kind("minecraft:boat"),
            Some(EntityCollisionKind::RequiresEntityState)
        );
        assert_eq!(registry.kind("example:unknown"), None);
    }
}
