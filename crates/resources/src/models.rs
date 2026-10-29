use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value};

use crate::packs::{ResourceId, ResourceProvider};

const MAX_PARENT_DEPTH: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextureReference {
    pub sprite: ResourceId,
    pub force_translucent: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ResolvedModel {
    /// All inherited fields are retained for later geometry and rendering stages.
    pub fields: Map<String, Value>,
    /// True when the effective geometry inherits Minecraft's generated sprite model.
    pub generated: bool,
}

impl ResolvedModel {
    pub fn texture(&self, name: &str) -> Result<TextureReference> {
        let textures = self
            .fields
            .get("textures")
            .and_then(Value::as_object)
            .context("model has no textures object")?;
        let mut key = name.trim_start_matches('#').to_owned();
        let mut visited = BTreeSet::new();
        let mut force_translucent = false;
        loop {
            ensure!(
                visited.insert(key.clone()),
                "cyclic model texture reference: {key}"
            );
            let value = textures
                .get(&key)
                .with_context(|| format!("model texture #{key} is not defined"))?;
            let raw = match value {
                Value::String(value) => value.as_str(),
                Value::Object(value) => {
                    if let Some(flag) = value.get("force_translucent") {
                        force_translucent |= flag
                            .as_bool()
                            .context("force_translucent must be a boolean")?;
                    }
                    value
                        .get("sprite")
                        .and_then(Value::as_str)
                        .context("texture object is missing sprite")?
                }
                _ => bail!("model texture #{key} is not a string or texture object"),
            };
            if let Some(next) = raw.strip_prefix('#') {
                ensure!(!next.is_empty(), "empty model texture reference");
                key = next.to_owned();
            } else {
                return Ok(TextureReference {
                    sprite: ResourceId::parse(raw)?,
                    force_translucent,
                });
            }
        }
    }
}

pub fn resolve_model(source: &mut impl ResourceProvider, id: &str) -> Result<ResolvedModel> {
    let id = ResourceId::parse(id)?;
    resolve_inner(source, &id, &mut Vec::new())
}

fn resolve_inner(
    source: &mut impl ResourceProvider,
    id: &ResourceId,
    stack: &mut Vec<ResourceId>,
) -> Result<ResolvedModel> {
    ensure!(
        stack.len() < MAX_PARENT_DEPTH,
        "model parent chain exceeds {MAX_PARENT_DEPTH}"
    );
    ensure!(
        !stack.contains(id),
        "cyclic model parent: {}:{}",
        id.namespace,
        id.path
    );
    if id.namespace == "minecraft" && id.path == "builtin/generated" {
        return Ok(ResolvedModel {
            fields: serde_json::json!({"textures":{"particle":"#layer0"}, "gui_light":"front"})
                .as_object()
                .expect("builtin object")
                .clone(),
            generated: true,
        });
    }
    stack.push(id.clone());
    let path = id.asset_path("models", "json");
    let bytes = source
        .read_asset(&path)?
        .with_context(|| format!("missing model {path}"))?;
    let value: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("invalid model JSON {path}"))?;
    let object = value
        .as_object()
        .with_context(|| format!("model {path} must be an object"))?;
    let mut result = match object.get("parent") {
        Some(Value::String(parent)) => resolve_inner(source, &ResourceId::parse(parent)?, stack)?,
        Some(_) => bail!("model parent must be a resource identifier"),
        None => ResolvedModel::default(),
    };
    if object.contains_key("elements") {
        result.generated = false;
    }
    let merged = &mut result.fields;
    for (name, value) in object {
        if name == "parent" {
            continue;
        }
        if name == "textures" || name == "display" {
            let child = value
                .as_object()
                .with_context(|| format!("model {name} must be an object"))?;
            let existing = merged
                .entry(name.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            let parent = existing
                .as_object_mut()
                .with_context(|| format!("inherited model {name} must be an object"))?;
            // ItemTransforms substitutes a model's right-hand transform for its
            // missing left-hand transform *before* walking up the parent chain.
            if name == "display" {
                for (right, left) in [
                    ("thirdperson_righthand", "thirdperson_lefthand"),
                    ("firstperson_righthand", "firstperson_lefthand"),
                ] {
                    if !child.contains_key(left)
                        && let Some(transform) = child.get(right)
                    {
                        parent.insert(left.to_owned(), transform.clone());
                    }
                }
            }
            for (key, value) in child {
                parent.insert(key.clone(), value.clone());
            }
        } else {
            merged.insert(name.clone(), value.clone());
        }
    }
    stack.pop();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct MemoryResources(HashMap<String, Vec<u8>>);

    impl ResourceProvider for MemoryResources {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }

    fn insert(source: &mut MemoryResources, path: &str, json: &str) {
        source.0.insert(
            format!("assets/minecraft/models/{path}.json"),
            json.as_bytes().to_vec(),
        );
    }

    #[test]
    fn child_textures_override_parent_and_resolve_late() {
        let mut source = MemoryResources::default();
        insert(
            &mut source,
            "block/base",
            r##"{"textures":{"particle":"#all","all":"block/stone"},"elements":[{"from":[0,0,0]}]}"##,
        );
        insert(
            &mut source,
            "block/child",
            r##"{"parent":"block/base","textures":{"all":"#side","side":{"sprite":"block/dirt","force_translucent":true}}}"##,
        );
        let model = resolve_model(&mut source, "block/child").unwrap();
        let texture = model.texture("particle").unwrap();
        assert_eq!(texture.sprite, ResourceId::parse("block/dirt").unwrap());
        assert!(texture.force_translucent);
        assert_eq!(model.fields["elements"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn parent_and_texture_cycles_are_errors() {
        let mut source = MemoryResources::default();
        insert(&mut source, "block/a", r##"{"parent":"block/b"}"##);
        insert(&mut source, "block/b", r##"{"parent":"block/a"}"##);
        assert!(resolve_model(&mut source, "block/a").is_err());
        insert(
            &mut source,
            "block/c",
            r##"{"textures":{"a":"#b","b":"#a"}}"##,
        );
        assert!(
            resolve_model(&mut source, "block/c")
                .unwrap()
                .texture("a")
                .is_err()
        );
    }

    #[test]
    fn display_contexts_inherit_independently_with_local_left_hand_fallback() {
        let mut source = MemoryResources::default();
        insert(
            &mut source,
            "block/base",
            r#"{"display":{"gui":{"scale":[1,1,1]},"thirdperson_lefthand":{"translation":[1,0,0]},"thirdperson_righthand":{"translation":[2,0,0]}}}"#,
        );
        insert(
            &mut source,
            "block/child",
            r#"{"parent":"block/base","display":{"thirdperson_righthand":{"translation":[3,0,0]}}}"#,
        );
        let model = resolve_model(&mut source, "block/child").unwrap();
        let display = &model.fields["display"];
        assert_eq!(display["gui"]["scale"], serde_json::json!([1, 1, 1]));
        assert_eq!(
            display["thirdperson_righthand"]["translation"],
            serde_json::json!([3, 0, 0])
        );
        assert_eq!(
            display["thirdperson_lefthand"]["translation"],
            serde_json::json!([3, 0, 0])
        );
    }

    #[test]
    fn generated_parent_is_builtin_but_explicit_elements_override_it() {
        let mut source = MemoryResources::default();
        insert(
            &mut source,
            "item/generated",
            r#"{"parent":"minecraft:builtin/generated","display":{"firstperson_righthand":{"scale":[0.5,0.5,0.5]}}}"#,
        );
        insert(
            &mut source,
            "item/test",
            r#"{"parent":"minecraft:item/generated","textures":{"layer0":"minecraft:item/test"}}"#,
        );
        let model = resolve_model(&mut source, "item/test").unwrap();
        assert!(model.generated);
        assert_eq!(
            model.texture("particle").unwrap().sprite,
            ResourceId::parse("item/test").unwrap()
        );
        assert_eq!(model.fields["gui_light"], "front");
        assert_eq!(
            model.fields["display"]["firstperson_lefthand"]["scale"],
            serde_json::json!([0.5, 0.5, 0.5])
        );
        insert(
            &mut source,
            "item/cuboid",
            r#"{"parent":"minecraft:item/test","elements":[]}"#,
        );
        assert!(!resolve_model(&mut source, "item/cuboid").unwrap().generated);
        assert!(resolve_model(&mut source, "example:builtin/generated").is_err());
    }
}
