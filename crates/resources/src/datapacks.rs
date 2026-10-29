//! Read-only world data pack selection and resource lookup for 26.3.
//!
//! This is a resource layer, not a complete data pack implementation: registries,
//! commands, recipes and world generation are not interpreted here.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use serde_json::Value;
use zip::ZipArchive;

use crate::packs::{Pack, ResourceId, collect_pack_data, collect_zip_data, read_zip};

const DATA_FORMAT: (i64, i64) = (121, 0);
const LAST_PRE_MINOR_FORMAT: i64 = 81;
const FEATURES: &[&str] = &[
    "minecraft:vanilla",
    "minecraft:trade_rebalance",
    "minecraft:redstone_experiments",
    "minecraft:minecart_improvements",
];

#[derive(Clone, Debug)]
pub struct DataPackSettings {
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
    pub enabled_features: Vec<String>,
}

struct Pattern {
    namespace: Option<Regex>,
    path: Option<Regex>,
}

#[derive(Default)]
struct Metadata {
    features: BTreeSet<String>,
    overlays: Vec<String>,
    filters: Vec<Pattern>,
}

enum Source {
    Vanilla,
    Builtin(String),
    File(Pack),
}

impl Source {
    fn read(&mut self, base: &mut ZipArchive<File>, path: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Vanilla => read_zip(base, path),
            Self::Builtin(prefix) => read_zip(base, &format!("{prefix}/{path}")),
            Self::File(pack) => pack.read(path),
        }
    }

    fn collect(
        &self,
        base: &ZipArchive<File>,
        overlay: &str,
        folder: &str,
        out: &mut BTreeSet<ResourceId>,
    ) -> Result<()> {
        match self {
            Self::Vanilla => collect_zip_data(base, overlay, folder, out),
            Self::Builtin(prefix) => {
                let prefix = if overlay.is_empty() {
                    prefix.clone()
                } else {
                    format!("{prefix}/{overlay}")
                };
                collect_zip_data(base, &prefix, folder, out)
            }
            Self::File(pack) => collect_pack_data(pack, overlay, folder, out),
        }
    }
}

struct Candidate {
    source: Source,
    metadata: Metadata,
    auto_enable: bool,
    feature_pack: bool,
}

impl Candidate {
    fn filtered(&self, namespace: &str, path: &str) -> bool {
        self.metadata
            .filters
            .iter()
            .any(|p| p.namespace.as_ref().is_none_or(|r| r.is_match(namespace)))
            && self
                .metadata
                .filters
                .iter()
                .any(|p| p.path.as_ref().is_none_or(|r| r.is_match(path)))
    }

    fn read(&mut self, base: &mut ZipArchive<File>, path: &str) -> Result<Option<Vec<u8>>> {
        for overlay in self.metadata.overlays.iter().rev() {
            if let Some(bytes) = self.source.read(base, &format!("{overlay}/{path}"))? {
                return Ok(Some(bytes));
            }
        }
        self.source.read(base, path)
    }
}

pub(crate) struct DataPackStack {
    selected: Vec<(String, Candidate)>,
}

impl DataPackStack {
    pub(crate) fn open(
        world_dir: &Path,
        settings: &DataPackSettings,
        base: &mut ZipArchive<File>,
    ) -> Result<(Self, Vec<String>)> {
        let root = world_dir
            .canonicalize()
            .context("cannot open world directory for data packs")?;
        let mut warnings = Vec::new();
        let mut available = BTreeMap::new();
        available.insert(
            "vanilla".to_owned(),
            Candidate {
                source: Source::Vanilla,
                metadata: Metadata {
                    features: BTreeSet::from(["minecraft:vanilla".into()]),
                    ..Default::default()
                },
                auto_enable: false,
                feature_pack: false,
            },
        );

        let builtin_names: Vec<_> = base
            .file_names()
            .filter_map(|name| {
                name.strip_prefix("data/minecraft/datapacks/")?
                    .strip_suffix("/pack.mcmeta")
            })
            .filter(|name| !name.contains('/'))
            .map(str::to_owned)
            .collect();
        for name in builtin_names {
            let prefix = format!("data/minecraft/datapacks/{name}");
            let mut source = Source::Builtin(prefix);
            match load_metadata(&mut source, base) {
                Ok(metadata) => {
                    available.insert(
                        name,
                        Candidate {
                            source,
                            metadata,
                            auto_enable: false,
                            feature_pack: true,
                        },
                    );
                }
                Err(error) => {
                    warnings.push(format!("cannot read built-in data pack {name}: {error:#}"))
                }
            }
        }

        let directory = root.join("datapacks");
        match directory.canonicalize() {
            Ok(directory) => {
                ensure!(
                    directory.starts_with(&root),
                    "data pack directory is outside the world"
                );
                for entry in
                    std::fs::read_dir(&directory).context("cannot list world data packs")?
                {
                    let entry = entry?;
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        warnings.push("data pack name contains invalid Unicode; ignored".into());
                        continue;
                    };
                    let id = format!("file/{name}");
                    let path = entry.path();
                    let file_type = entry.file_type()?;
                    if file_type.is_symlink() {
                        warnings.push(format!(
                            "data pack {id} is a symbolic link and was not loaded"
                        ));
                        continue;
                    }
                    let canonical = path.canonicalize()?;
                    if !canonical.starts_with(&directory) {
                        warnings.push(format!(
                            "data pack {id} resolves outside the data pack directory"
                        ));
                        continue;
                    }
                    let pack = if file_type.is_dir() {
                        Pack::Directory(canonical)
                    } else if file_type.is_file() && name.ends_with(".zip") {
                        match File::open(&canonical)
                            .and_then(|file| ZipArchive::new(file).map_err(std::io::Error::other))
                        {
                            Ok(archive) => Pack::Archive(archive),
                            Err(error) => {
                                warnings.push(format!("cannot open data pack {id}: {error}"));
                                continue;
                            }
                        }
                    } else {
                        continue;
                    };
                    let mut source = Source::File(pack);
                    match load_metadata(&mut source, base) {
                        Ok(metadata) => {
                            available.insert(
                                id,
                                Candidate {
                                    source,
                                    metadata,
                                    auto_enable: true,
                                    feature_pack: false,
                                },
                            );
                        }
                        Err(error) => {
                            warnings.push(format!("cannot read data pack {id}: {error:#}"))
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("cannot access world data packs"),
        }

        let features: BTreeSet<_> = settings.enabled_features.iter().cloned().collect();
        for feature in &features {
            ensure!(
                FEATURES.contains(&feature.as_str()),
                "unsupported enabled world feature {feature}"
            );
        }
        let disabled: BTreeSet<_> = settings.disabled.iter().map(String::as_str).collect();
        let mut selected = Vec::new();
        for id in &settings.enabled {
            if available.contains_key(id) {
                if !selected.contains(id) {
                    selected.push(id.clone());
                }
            } else {
                warnings.push(format!("enabled data pack {id} is missing or invalid"));
            }
        }
        let mut ids: Vec<_> = available.keys().cloned().collect();
        ids.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        for id in &ids {
            if disabled.contains(id.as_str()) {
                continue;
            }
            let candidate = &available[id];
            let selected_index = selected.iter().position(|item| item == id);
            if !candidate.metadata.features.is_subset(&features) {
                if let Some(index) = selected_index {
                    selected.remove(index);
                    warnings.push(format!(
                        "data pack {id} requires features that are not enabled"
                    ));
                }
            } else if selected_index.is_none() && candidate.auto_enable {
                selected.push(id.clone());
            }
        }
        if !selected.iter().any(|id| id == "vanilla") {
            selected.insert(0, "vanilla".into());
        }
        let mut requested: BTreeSet<String> = selected
            .iter()
            .flat_map(|id| available[id].metadata.features.iter().cloned())
            .collect();
        for id in &ids {
            let candidate = &available[id];
            if candidate.feature_pack
                && !selected.contains(id)
                && candidate
                    .metadata
                    .features
                    .iter()
                    .any(|feature| features.contains(feature) && !requested.contains(feature))
            {
                selected.push(id.clone());
                requested.extend(candidate.metadata.features.iter().cloned());
            }
        }
        let selected = selected
            .into_iter()
            .map(|id| {
                let pack = available.remove(&id).expect("selected pack was discovered");
                (id, pack)
            })
            .collect();
        Ok((Self { selected }, warnings))
    }

    pub(crate) fn selected_ids(&self) -> Vec<&str> {
        self.selected.iter().map(|(id, _)| id.as_str()).collect()
    }

    pub(crate) fn read(
        &mut self,
        base: &mut ZipArchive<File>,
        path: &str,
    ) -> Result<Option<Vec<u8>>> {
        let (namespace, resource_path) = path
            .strip_prefix("data/")
            .context("invalid data path")?
            .split_once('/')
            .context("invalid data resource")?;
        for (_, pack) in self.selected.iter_mut().rev() {
            if let Some(bytes) = pack.read(base, path)? {
                return Ok(Some(bytes));
            }
            if pack.filtered(namespace, resource_path) {
                return Ok(None);
            }
        }
        Ok(None)
    }

    pub(crate) fn read_stack(
        &mut self,
        base: &mut ZipArchive<File>,
        path: &str,
    ) -> Result<Vec<Vec<u8>>> {
        let (namespace, resource_path) = path
            .strip_prefix("data/")
            .context("invalid data path")?
            .split_once('/')
            .context("invalid data resource")?;
        let mut resources = Vec::new();
        for (_, pack) in self.selected.iter_mut().rev() {
            if let Some(bytes) = pack.read(base, path)? {
                resources.push(bytes);
            }
            if pack.filtered(namespace, resource_path) {
                break;
            }
        }
        resources.reverse();
        Ok(resources)
    }

    pub(crate) fn list(
        &self,
        base: &ZipArchive<File>,
        folder: &str,
    ) -> Result<BTreeSet<ResourceId>> {
        let mut visible = BTreeSet::new();
        for (_, pack) in &self.selected {
            visible.retain(|id: &ResourceId| {
                !pack.filtered(&id.namespace, &format!("{folder}/{}.json", id.path))
            });
            let mut found = BTreeSet::new();
            pack.source.collect(base, "", folder, &mut found)?;
            for overlay in &pack.metadata.overlays {
                pack.source.collect(base, overlay, folder, &mut found)?;
            }
            visible.extend(found);
            ensure!(
                visible.len() <= 100_000,
                "too many data resources in {folder}"
            );
        }
        Ok(visible)
    }
}

fn load_metadata(source: &mut Source, base: &mut ZipArchive<File>) -> Result<Metadata> {
    let bytes = source
        .read(base, "pack.mcmeta")?
        .context("missing pack.mcmeta")?;
    let json: Value = serde_json::from_slice(&bytes).context("invalid pack.mcmeta JSON")?;
    let pack = json
        .get("pack")
        .and_then(Value::as_object)
        .context("missing pack metadata")?;
    ensure!(
        pack.get("description").is_some(),
        "pack metadata is missing description"
    );
    let mut metadata = Metadata::default();
    if let Some(features) = json.get("features") {
        for value in features
            .get("enabled")
            .and_then(Value::as_array)
            .context("features.enabled must be a list")?
        {
            let feature = value.as_str().context("feature must be a string")?;
            ensure!(
                FEATURES.contains(&feature),
                "unsupported pack feature {feature}"
            );
            metadata.features.insert(feature.to_owned());
        }
    }
    if let Some(overlays) = json.get("overlays") {
        let entries = overlays
            .get("entries")
            .and_then(Value::as_array)
            .context("overlays.entries must be a list")?;
        let parsed: Vec<_> = entries
            .iter()
            .map(parse_overlay_range)
            .collect::<Result<_>>()?;
        let needs_legacy = parsed
            .iter()
            .flatten()
            .any(|range| range.minimum_major() <= LAST_PRE_MINOR_FORMAT);
        for (entry, range) in entries.iter().zip(parsed) {
            let directory = entry
                .get("directory")
                .and_then(Value::as_str)
                .context("overlay directory must be a string")?;
            ensure!(
                !directory.is_empty()
                    && directory != "."
                    && directory != ".."
                    && directory
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
                "invalid overlay directory"
            );
            if let Some(range) = range
                && range.applies(needs_legacy)?
            {
                metadata.overlays.push(directory.to_owned());
            }
        }
    }
    if let Some(filters) = json.get("filter") {
        for value in filters
            .get("block")
            .and_then(Value::as_array)
            .context("filter.block must be a list")?
        {
            let compile = |key: &str| -> Result<Option<Regex>> {
                value
                    .get(key)
                    .map(|value| {
                        let pattern = value
                            .as_str()
                            .with_context(|| format!("filter {key} must be a string"))?;
                        Regex::new(&format!(r"\A(?:{pattern})\z"))
                            .with_context(|| format!("unsupported filter {key} pattern {pattern}"))
                    })
                    .transpose()
            };
            metadata.filters.push(Pattern {
                namespace: compile("namespace")?,
                path: compile("path")?,
            });
        }
    }
    Ok(metadata)
}

type Version = (i64, i64);
struct OverlayRange {
    modern: Option<(Version, Version)>,
    legacy: Option<(i64, i64)>,
}

impl OverlayRange {
    fn minimum_major(&self) -> i64 {
        self.modern
            .map_or(i64::MAX, |(min, _)| min.0)
            .min(self.legacy.map_or(i64::MAX, |(min, _)| min))
    }

    fn applies(&self, needs_legacy: bool) -> Result<bool> {
        if needs_legacy {
            ensure!(
                self.legacy.is_some(),
                "overlays supporting older formats must provide formats for every entry"
            );
        }
        if let Some((min, max)) = self.modern {
            ensure!(min <= max, "overlay minimum format exceeds maximum");
            if min.0 > LAST_PRE_MINOR_FORMAT && !needs_legacy {
                ensure!(
                    self.legacy.is_none(),
                    "modern overlay must omit legacy formats"
                );
            } else if let Some((legacy_min, legacy_max)) = self.legacy {
                ensure!(
                    legacy_min == min.0
                        && (legacy_max == max.0 || legacy_max == LAST_PRE_MINOR_FORMAT),
                    "legacy and modern overlay formats differ"
                );
            } else {
                bail!("overlay supporting older formats must provide legacy formats");
            }
            return Ok(min <= DATA_FORMAT && DATA_FORMAT <= max);
        }
        let (min, max) = self.legacy.context("overlay formats are missing")?;
        ensure!(
            min <= max && max <= LAST_PRE_MINOR_FORMAT,
            "legacy overlay format exceeds the last pre-minor version"
        );
        Ok((min, 0) <= DATA_FORMAT && DATA_FORMAT <= (max, 0))
    }
}

fn parse_overlay_range(entry: &Value) -> Result<Option<OverlayRange>> {
    fn integer(value: &Value) -> Result<i64> {
        let value = value.as_i64().context("pack format must be an integer")?;
        ensure!(
            i32::try_from(value).is_ok(),
            "pack format exceeds 32-bit range"
        );
        Ok(value)
    }
    fn endpoint(value: &Value, top: bool) -> Result<Version> {
        if value.is_number() {
            return Ok((integer(value)?, if top { i64::from(i32::MAX) } else { 0 }));
        }
        let values = value
            .as_array()
            .context("pack format must be an integer or [major, minor]")?;
        ensure!(values.len() == 2, "pack format must have two values");
        Ok((integer(&values[0])?, integer(&values[1])?))
    }
    let modern = match (entry.get("min_format"), entry.get("max_format")) {
        (Some(min), Some(max)) => Some((endpoint(min, false)?, endpoint(max, true)?)),
        (None, None) => None,
        _ => bail!("overlay must contain both min_format and max_format"),
    };
    let legacy = entry
        .get("formats")
        .map(|formats| -> Result<(i64, i64)> {
            if formats.is_number() {
                let n = integer(formats)?;
                Ok((n, n))
            } else if let Some(range) = formats.as_array() {
                ensure!(
                    range.len() == 2,
                    "legacy overlay format range must have two values"
                );
                Ok((integer(&range[0])?, integer(&range[1])?))
            } else {
                Ok((
                    integer(
                        formats
                            .get("min_inclusive")
                            .context("missing legacy minimum format")?,
                    )?,
                    integer(
                        formats
                            .get("max_inclusive")
                            .context("missing legacy maximum format")?,
                    )?,
                ))
            }
        })
        .transpose()?;
    if modern.is_none() && legacy.is_none() {
        return Ok(None);
    }
    Ok(Some(OverlayRange { modern, legacy }))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn overlay_ranges_validate_modern_and_pre_minor_formats() {
        let modern = serde_json::json!({"min_format": [121, 0], "max_format": [121, 0]});
        let modern = parse_overlay_range(&modern).unwrap().unwrap();
        assert!(modern.applies(false).unwrap());
        assert!(modern.applies(true).is_err());
        let old = serde_json::json!({"formats": [80, 81]});
        assert!(
            !parse_overlay_range(&old)
                .unwrap()
                .unwrap()
                .applies(true)
                .unwrap()
        );
        let invalid = serde_json::json!({"formats": [80, 121]});
        assert!(
            parse_overlay_range(&invalid)
                .unwrap()
                .unwrap()
                .applies(true)
                .is_err()
        );
        let mixed =
            serde_json::json!({"min_format": 121, "max_format": 121, "formats": [121, 121]});
        assert!(
            parse_overlay_range(&mixed)
                .unwrap()
                .unwrap()
                .applies(true)
                .unwrap()
        );
        assert!(
            parse_overlay_range(&serde_json::json!({"directory": "ignored"}))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn world_packs_resolve_overlays_filters_and_automatic_selection_offline() {
        let temp = tempfile::tempdir().unwrap();
        let base_path = temp.path().join("base.jar");
        let mut writer = zip::ZipWriter::new(File::create(&base_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (path, value) in [
            ("data/example/worldgen/biome/shared.json", "base"),
            ("data/example/worldgen/biome/hidden.json", "hidden"),
        ] {
            writer.start_file(path, options).unwrap();
            writer.write_all(value.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        let world = temp.path().join("world");
        let pack_dir = world.join("datapacks/alpha/data/example/worldgen/biome");
        std::fs::create_dir_all(&pack_dir).unwrap();
        std::fs::write(
            world.join("datapacks/alpha/pack.mcmeta"),
            br#"{"pack":{"description":"alpha","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        std::fs::write(pack_dir.join("shared.json"), b"alpha").unwrap();
        std::fs::create_dir_all(pack_dir.join("nested")).unwrap();
        std::fs::write(pack_dir.join("nested/only.json"), b"nested").unwrap();
        let zip_path = world.join("datapacks/z.zip");
        let mut zip = zip::ZipWriter::new(File::create(zip_path).unwrap());
        for (path, bytes) in [
            ("pack.mcmeta", br#"{"pack":{"description":"zip","min_format":121,"max_format":121},"overlays":{"entries":[{"directory":"new","min_format":121,"max_format":121}]},"filter":{"block":[{"namespace":"example","path":"worldgen/biome/hidden\\.json"}]}}"#.as_slice()),
            ("data/example/worldgen/biome/shared.json", b"zip".as_slice()),
            ("new/data/example/worldgen/biome/shared.json", b"overlay".as_slice()),
            ("new/data/example/worldgen/biome/overlay_only.json", b"extra".as_slice()),
        ] {
            zip.start_file(path, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let mut base = ZipArchive::new(File::open(base_path).unwrap()).unwrap();
        let settings = DataPackSettings {
            enabled: vec!["vanilla".into(), "file/alpha".into()],
            disabled: vec![],
            enabled_features: vec!["minecraft:vanilla".into()],
        };
        let (mut stack, warnings) = DataPackStack::open(&world, &settings, &mut base).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            stack.selected_ids(),
            ["vanilla", "file/alpha", "file/z.zip"]
        );
        assert_eq!(
            stack
                .read(&mut base, "data/example/worldgen/biome/shared.json")
                .unwrap()
                .as_deref(),
            Some(b"overlay".as_slice())
        );
        assert_eq!(
            stack
                .read_stack(&mut base, "data/example/worldgen/biome/shared.json")
                .unwrap(),
            [b"base".to_vec(), b"alpha".to_vec(), b"overlay".to_vec()]
        );
        assert!(
            stack
                .read_stack(&mut base, "data/example/worldgen/biome/hidden.json")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            stack
                .list(&base, "worldgen/biome")
                .unwrap()
                .into_iter()
                .map(|id| id.path)
                .collect::<Vec<_>>(),
            ["nested/only", "overlay_only", "shared"]
        );
        assert_eq!(
            stack
                .read(&mut base, "data/example/worldgen/biome/hidden.json")
                .unwrap(),
            None
        );
        let disabled = DataPackSettings {
            disabled: vec!["file/z.zip".into()],
            ..settings
        };
        let (mut stack, warnings) = DataPackStack::open(&world, &disabled, &mut base).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            stack
                .read(&mut base, "data/example/worldgen/biome/shared.json")
                .unwrap()
                .as_deref(),
            Some(b"alpha".as_slice())
        );
        assert_eq!(
            stack
                .read(&mut base, "data/example/worldgen/biome/hidden.json")
                .unwrap()
                .as_deref(),
            Some(b"hidden".as_slice())
        );
    }
}
