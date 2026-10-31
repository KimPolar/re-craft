//! Saved `minecraft:tool` component patches.

use native_persistence::nbt::Tag;
use native_resources::{mining_tags::MiningTags, packs::ResourceId};

use crate::mining::{Material, MiningProperties};

enum Blocks {
    Tag(String),
    Direct(Vec<String>),
}

struct Rule {
    blocks: Blocks,
    speed: Option<f32>,
    correct: Option<bool>,
}

pub(crate) struct Tool {
    rules: Vec<Rule>,
    default_speed: f32,
    damage: u32,
    creative: bool,
}

fn identifier(value: &Tag) -> Option<(bool, String)> {
    let Tag::String(value) = value else {
        return None;
    };
    let value = String::from_utf16(&value.0).ok()?;
    let (tag, value) = value
        .strip_prefix('#')
        .map_or((false, value.as_str()), |id| (true, id));
    let id = ResourceId::parse(value).ok()?;
    Some((tag, format!("{}:{}", id.namespace, id.path)))
}

fn float(value: &Tag) -> Option<f32> {
    let n = match value {
        Tag::Byte(n) => *n as f32,
        Tag::Short(n) => *n as f32,
        Tag::Int(n) => *n as f32,
        Tag::Long(n) => *n as f32,
        Tag::Float(n) => *n,
        Tag::Double(n) => *n as f32,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

fn boolean(value: &Tag) -> Option<bool> {
    match value {
        Tag::Byte(0) => Some(false),
        Tag::Byte(1) => Some(true),
        _ => None,
    }
}

fn fields<'a>(
    tag: &'a Tag,
    allowed: &[&str],
) -> Option<&'a [(native_persistence::nbt::JavaString, Tag)]> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    for (i, (name, _)) in fields.iter().enumerate() {
        if !allowed.iter().any(|key| name.matches(key))
            || fields[..i].iter().any(|(other, _)| name == other)
        {
            return None;
        }
    }
    Some(fields)
}

impl Tool {
    pub(crate) fn parse(tag: &Tag) -> Option<Self> {
        fields(
            tag,
            &[
                "rules",
                "default_mining_speed",
                "damage_per_block",
                "can_destroy_blocks_in_creative",
            ],
        )?;
        let Tag::List { values, .. } = tag.get("rules")? else {
            return None;
        };
        let mut rules = Vec::with_capacity(values.len());
        for rule in values {
            fields(rule, &["blocks", "speed", "correct_for_drops"])?;
            let blocks = match rule.get("blocks")? {
                value @ Tag::String(_) => {
                    let (is_tag, id) = identifier(value)?;
                    if is_tag {
                        Blocks::Tag(id)
                    } else {
                        Blocks::Direct(vec![id])
                    }
                }
                Tag::List { values, .. } => {
                    let mut ids = Vec::with_capacity(values.len());
                    for value in values {
                        let (is_tag, id) = identifier(value)?;
                        if is_tag {
                            return None;
                        }
                        ids.push(id);
                    }
                    Blocks::Direct(ids)
                }
                _ => return None,
            };
            let speed = rule
                .get("speed")
                .map(|v| float(v).filter(|v| *v > 0.0))
                .transpose_option()?;
            let correct = rule
                .get("correct_for_drops")
                .map(boolean)
                .transpose_option()?;
            rules.push(Rule {
                blocks,
                speed,
                correct,
            });
        }
        let default_speed = tag
            .get("default_mining_speed")
            .map(float)
            .transpose_option()?
            .unwrap_or(1.0);
        let damage = tag
            .get("damage_per_block")
            .map(|value| u32::try_from(value.as_java_int()?).ok())
            .transpose_option()?
            .unwrap_or(1);
        let creative = tag
            .get("can_destroy_blocks_in_creative")
            .map(boolean)
            .transpose_option()?
            .unwrap_or(true);
        Some(Self {
            rules,
            default_speed,
            damage,
            creative,
        })
    }

    pub(crate) fn creative(&self) -> bool {
        self.creative
    }

    pub(crate) fn properties(
        &self,
        material: Material,
        block: &str,
        tags: Option<&MiningTags>,
    ) -> Option<MiningProperties> {
        let tags = tags?;
        tags.for_block(block)?;
        let mut speed = None;
        let mut correct = None;
        for rule in &self.rules {
            let matches = match &rule.blocks {
                Blocks::Tag(tag) => tags.contains(tag, block)?,
                Blocks::Direct(ids) => {
                    for id in ids {
                        tags.for_block(id)?;
                    }
                    ids.iter().any(|id| id == block)
                }
            };
            if matches {
                if speed.is_none() {
                    speed = rule.speed;
                }
                if correct.is_none() {
                    correct = rule.correct;
                }
            }
        }
        Some(MiningProperties {
            speed: speed.unwrap_or(self.default_speed),
            correct_for_drops: !material.needs_tool() || correct.unwrap_or(false),
            damage_per_block: self.damage,
            can_destroy_in_creative: self.creative,
        })
    }
}

trait TransposeOption<T> {
    fn transpose_option(self) -> Option<Option<T>>;
}
impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Option<Option<T>> {
        match self {
            None => Some(None),
            Some(Some(value)) => Some(Some(value)),
            Some(None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::TagType;
    use native_resources::packs::ResourceProvider;
    use std::collections::BTreeSet;

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn list_data(&mut self, _: &str) -> anyhow::Result<Vec<ResourceId>> {
            Ok(vec![ResourceId::parse("example:custom").unwrap()])
        }
        fn read_data_stack(&mut self, path: &str) -> anyhow::Result<Vec<Vec<u8>>> {
            Ok(vec![if path.ends_with("/example/tags/block/custom.json") {
                br#"{"values":["minecraft:stone"]}"#.to_vec()
            } else {
                br#"{"values":[]}"#.to_vec()
            }])
        }
    }
    fn rule(blocks: &str, speed: Option<f32>, correct: Option<bool>) -> Tag {
        let mut fields = vec![("blocks".into(), Tag::String(blocks.into()))];
        if let Some(speed) = speed {
            fields.push(("speed".into(), Tag::Float(speed)));
        }
        if let Some(correct) = correct {
            fields.push(("correct_for_drops".into(), Tag::Byte(i8::from(correct))));
        }
        Tag::Compound(fields)
    }
    #[test]
    fn custom_tag_rules_resolve_independently_and_missing_tags_stop_mining() {
        let tags = MiningTags::load(
            &mut Data,
            &BTreeSet::from(["minecraft:stone".into(), "minecraft:dirt".into()]),
        )
        .unwrap();
        let component = |name| {
            Tag::Compound(vec![(
                "rules".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![
                        rule(name, Some(8.0), None),
                        rule("minecraft:stone", Some(2.0), Some(true)),
                        rule(name, None, Some(false)),
                    ],
                },
            )])
        };
        let tool = Tool::parse(&component("#example:custom")).unwrap();
        let stone = tool
            .properties(Material::Stone, "minecraft:stone", Some(&tags))
            .unwrap();
        assert_eq!(stone.speed, 8.0);
        assert!(stone.correct_for_drops);
        let dirt = tool
            .properties(Material::Dirt, "minecraft:dirt", Some(&tags))
            .unwrap();
        assert_eq!(dirt.speed, 1.0);
        assert!(dirt.correct_for_drops);
        assert!(
            Tool::parse(&component("#example:missing"))
                .unwrap()
                .properties(Material::Stone, "minecraft:stone", Some(&tags))
                .is_none()
        );
        assert!(
            tool.properties(Material::Stone, "example:unregistered", Some(&tags))
                .is_none()
        );
    }
}
