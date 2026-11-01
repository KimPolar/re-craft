//! Data-pack block predicates used by location effects such as `replace_disk`.

use std::{collections::BTreeSet, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use super::{
    load_single_tag,
    location::{check_keys, id, object},
};
use crate::packs::{ResourceId, ResourceProvider};

/// Implementations must return an error when a block, fluid, entity collision,
/// or loaded chunk is unknown, rather than assuming the predicate is false.
pub trait PredicateWorld {
    fn matches_block(&mut self, position: [i32; 3], ids: &BTreeSet<String>) -> Result<bool>;
    fn matches_fluid(&mut self, position: [i32; 3], ids: &BTreeSet<String>) -> Result<bool>;
    fn unobstructed(&mut self, position: [i32; 3]) -> Result<bool>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum DiskPredicate {
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
    Blocks {
        offset: [i32; 3],
        ids: Arc<BTreeSet<String>>,
    },
    Fluids {
        offset: [i32; 3],
        ids: Arc<BTreeSet<String>>,
    },
    Unobstructed {
        offset: [i32; 3],
    },
}

fn offset(value: Option<&Value>) -> Result<[i32; 3]> {
    let Some(value) = value else {
        return Ok([0; 3]);
    };
    let values = value
        .as_array()
        .context("predicate offset is not an array")?;
    ensure!(
        values.len() == 3,
        "predicate offset must have three coordinates"
    );
    let mut result = [0; 3];
    for (output, value) in result.iter_mut().zip(values) {
        *output = i32::try_from(value.as_i64().context("invalid predicate offset")?)?;
    }
    Ok(result)
}

fn canonical(id: ResourceId) -> String {
    format!("{}:{}", id.namespace, id.path)
}

fn ids(
    provider: &mut impl ResourceProvider,
    folder: &str,
    value: &Value,
) -> Result<Arc<BTreeSet<String>>> {
    if let Some(name) = value.as_str() {
        if let Some(tag) = name.strip_prefix('#') {
            return Ok(Arc::new(load_single_tag(
                provider,
                folder,
                &ResourceId::parse(tag)?,
            )?));
        }
        return Ok(Arc::new(BTreeSet::from([canonical(ResourceId::parse(
            name,
        )?)])));
    }
    let values = value
        .as_array()
        .context("predicate registry set is not a list")?;
    ensure!(values.len() <= 4096, "predicate registry set is too large");
    let ids = values
        .iter()
        .map(|v| Ok(canonical(id(v, "predicate registry entry")?)))
        .collect::<Result<_>>()?;
    Ok(Arc::new(ids))
}

impl DiskPredicate {
    pub fn parse(provider: &mut impl ResourceProvider, value: &Value) -> Result<Self> {
        Self::parse_inner(provider, value, 0)
    }

    fn parse_inner(
        provider: &mut impl ResourceProvider,
        value: &Value,
        depth: usize,
    ) -> Result<Self> {
        ensure!(depth <= 32, "disk predicate nesting is too deep");
        let value = object(value, "disk block predicate")?;
        let kind = id(
            value
                .get("type")
                .context("disk predicate type is missing")?,
            "predicate type",
        )?;
        ensure!(
            kind.namespace == "minecraft",
            "unsupported disk predicate namespace"
        );
        match kind.path.as_str() {
            "all_of" | "any_of" => {
                check_keys(value, &["type", "predicates"])?;
                let terms = value
                    .get("predicates")
                    .and_then(Value::as_array)
                    .context("disk predicates are missing")?;
                ensure!(terms.len() <= 256, "too many disk predicates");
                let terms = terms
                    .iter()
                    .map(|term| Self::parse_inner(provider, term, depth + 1))
                    .collect::<Result<_>>()?;
                Ok(if kind.path == "all_of" {
                    Self::All(terms)
                } else {
                    Self::Any(terms)
                })
            }
            "not" => {
                check_keys(value, &["type", "predicate"])?;
                Ok(Self::Not(Box::new(Self::parse_inner(
                    provider,
                    value
                        .get("predicate")
                        .context("negated predicate is missing")?,
                    depth + 1,
                )?)))
            }
            "matching_blocks" | "matching_fluids" => {
                let is_block = kind.path == "matching_blocks";
                let key = if is_block { "blocks" } else { "fluids" };
                check_keys(value, &["type", "offset", key])?;
                let values = ids(
                    provider,
                    if is_block { "block" } else { "fluid" },
                    value
                        .get(key)
                        .context("predicate registry set is missing")?,
                )?;
                let offset = offset(value.get("offset"))?;
                Ok(if is_block {
                    Self::Blocks {
                        offset,
                        ids: values,
                    }
                } else {
                    Self::Fluids {
                        offset,
                        ids: values,
                    }
                })
            }
            "matching_block_tag" => {
                check_keys(value, &["type", "offset", "tag"])?;
                let tag = id(
                    value.get("tag").context("block tag is missing")?,
                    "block tag",
                )?;
                Ok(Self::Blocks {
                    offset: offset(value.get("offset"))?,
                    ids: Arc::new(load_single_tag(provider, "block", &tag)?),
                })
            }
            "unobstructed" => {
                check_keys(value, &["type", "offset"])?;
                Ok(Self::Unobstructed {
                    offset: offset(value.get("offset"))?,
                })
            }
            _ => bail!("unsupported disk block predicate {}", kind.path),
        }
    }

    pub fn test(&self, world: &mut impl PredicateWorld, position: [i32; 3]) -> Result<bool> {
        let shifted = |offset: [i32; 3]| -> Result<[i32; 3]> {
            Ok([
                position[0]
                    .checked_add(offset[0])
                    .context("predicate x overflow")?,
                position[1]
                    .checked_add(offset[1])
                    .context("predicate y overflow")?,
                position[2]
                    .checked_add(offset[2])
                    .context("predicate z overflow")?,
            ])
        };
        match self {
            Self::All(terms) => {
                for term in terms {
                    if !term.test(world, position)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::Any(terms) => {
                for term in terms {
                    if term.test(world, position)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Not(term) => Ok(!term.test(world, position)?),
            Self::Blocks { offset, ids } => world.matches_block(shifted(*offset)?, ids),
            Self::Fluids { offset, ids } => world.matches_fluid(shifted(*offset)?, ids),
            // Java 26.3's UnobstructedPredicate.test ignores its stored offset.
            Self::Unobstructed { .. } => world.unobstructed(position),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Tags;
    impl ResourceProvider for Tags {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(match path {
                "data/example/tags/block/air.json" => vec![
                    br#"{"values":["minecraft:stone"]}"#.to_vec(),
                    br##"{"replace":true,"values":["#example:nested"]}"##.to_vec(),
                ],
                "data/example/tags/block/nested.json" => {
                    vec![br#"{"values":["minecraft:air","minecraft:cave_air"]}"#.to_vec()]
                }
                _ => vec![],
            })
        }
    }
    struct World {
        calls: Vec<[i32; 3]>,
        clear: bool,
        missing: bool,
    }
    impl PredicateWorld for World {
        fn matches_block(&mut self, position: [i32; 3], ids: &BTreeSet<String>) -> Result<bool> {
            self.calls.push(position);
            ensure!(!self.missing, "missing chunk");
            Ok(ids.contains("minecraft:air"))
        }
        fn matches_fluid(&mut self, position: [i32; 3], _: &BTreeSet<String>) -> Result<bool> {
            self.calls.push(position);
            ensure!(!self.missing, "missing chunk");
            Ok(false)
        }
        fn unobstructed(&mut self, position: [i32; 3]) -> Result<bool> {
            self.calls.push(position);
            ensure!(!self.missing, "unknown entities");
            Ok(self.clear)
        }
    }

    #[test]
    fn resolves_replaced_nested_tags_and_short_circuits_in_order() {
        let value = json!({"type":"minecraft:all_of","predicates":[
            {"type":"minecraft:matching_block_tag","tag":"example:air","offset":[0,1,0]},
            {"type":"minecraft:unobstructed","offset":[8,4,2]},
            {"type":"minecraft:matching_fluids","fluids":"minecraft:water"}
        ]});
        let predicate = DiskPredicate::parse(&mut Tags, &value).unwrap();
        let DiskPredicate::All(terms) = &predicate else {
            panic!()
        };
        let DiskPredicate::Blocks { ids, .. } = &terms[0] else {
            panic!()
        };
        assert_eq!(
            ids.as_ref(),
            &BTreeSet::from([
                "minecraft:air".to_string(),
                "minecraft:cave_air".to_string()
            ])
        );
        let mut world = World {
            calls: vec![],
            clear: false,
            missing: false,
        };
        assert!(!predicate.test(&mut world, [2, 10, 3]).unwrap());
        assert_eq!(world.calls, vec![[2, 11, 3], [2, 10, 3]]);
        world.calls.clear();
        world.missing = true;
        assert!(predicate.test(&mut world, [2, 10, 3]).is_err());
        assert_eq!(world.calls, vec![[2, 11, 3]]);
        assert!(predicate.test(&mut world, [2, i32::MAX, 3]).is_err());
    }
}
