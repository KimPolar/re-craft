//! Resolve Minecraft sound definitions and OGG assets without bundling game data.
//!
//! This module describes resources only. Playback and spatialization are separate.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::{
    asset_index::AssetIndex,
    packs::{ResourceId, ResourceProvider, ResourceStack},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKind {
    File,
    Event,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoundEntry {
    pub id: ResourceId,
    pub kind: SoundKind,
    /// Positive numeric constants from sounds.json, retained in their original form.
    pub volume: Value,
    pub pitch: Value,
    pub weight: u32,
    pub stream: bool,
    pub preload: bool,
    pub attenuation_distance: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundEvent {
    pub subtitle: Option<String>,
    pub sounds: Vec<SoundEntry>,
}

#[derive(Clone, Debug, Default)]
pub struct SoundDefinitions {
    events: BTreeMap<ResourceId, SoundEvent>,
}

/// A file chosen from an event. Providers are ordered from the file outwards;
/// each enclosing event multiplies the preceding volume and pitch.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectedSound {
    pub id: ResourceId,
    pub volume_factors: Vec<Value>,
    pub pitch_factors: Vec<Value>,
    pub weight: u32,
    pub stream: bool,
    pub preload: bool,
    pub attenuation_distance: u32,
}

impl SoundDefinitions {
    /// Build vanilla Minecraft definitions and apply the selected packs in
    /// increasing priority. Sound files are read separately on demand.
    pub fn load_minecraft(
        index: &AssetIndex,
        root: &Path,
        stack: &mut ResourceStack,
    ) -> Result<Self> {
        let vanilla = index.read(root, "minecraft/sounds.json")?;
        let mut out = Self::default();
        out.apply("minecraft", &vanilla)?;
        for namespace in stack.asset_namespaces()? {
            for (layer_index, layer) in stack
                .read_asset_stack(&format!("assets/{namespace}/sounds.json"))?
                .into_iter()
                .enumerate()
            {
                if let Err(error) = out.apply(&namespace, &layer) {
                    eprintln!(
                        "Ignoring invalid {namespace} sounds.json layer {layer_index}: {error:#}"
                    );
                }
            }
        }
        Ok(out)
    }

    pub fn get(&self, id: &ResourceId) -> Option<&SoundEvent> {
        self.events.get(id)
    }

    /// Select as Java's weighted event graph does. `available` must reflect
    /// the set of files registered at reload time and remain stable for the
    /// duration of this call. `random` must return an integer in 0..bound.
    /// Missing events and unregistered files have zero weight. Cycles and
    /// excessive depth or weight return an error rather than overflowing.
    pub fn select(
        &self,
        id: &ResourceId,
        available: &impl Fn(&ResourceId) -> bool,
        random: &mut impl FnMut(i32) -> i32,
    ) -> Result<Option<SelectedSound>> {
        let mut weights = BTreeMap::new();
        let mut files = BTreeMap::new();
        self.event_weight(id, available, &mut weights, &mut files, &mut Vec::new())?;
        self.select_inner(id, &weights, &files, random, 0)
    }

    fn event_weight(
        &self,
        id: &ResourceId,
        available: &impl Fn(&ResourceId) -> bool,
        weights: &mut BTreeMap<ResourceId, i32>,
        files: &mut BTreeMap<ResourceId, bool>,
        visiting: &mut Vec<ResourceId>,
    ) -> Result<i32> {
        if let Some(weight) = weights.get(id) {
            return Ok(*weight);
        }
        ensure!(visiting.len() < 64, "sound event nesting is too deep");
        ensure!(!visiting.contains(id), "cyclic sound event reference");
        visiting.push(id.clone());
        let mut total = 0_i32;
        if let Some(event) = self.events.get(id) {
            for sound in &event.sounds {
                let weight = match sound.kind {
                    SoundKind::File => {
                        if *files
                            .entry(sound.id.clone())
                            .or_insert_with(|| available(&sound.id))
                        {
                            i32::try_from(sound.weight).context("sound weight is too large")?
                        } else {
                            0
                        }
                    }
                    SoundKind::Event => {
                        self.event_weight(&sound.id, available, weights, files, visiting)?
                    }
                };
                total = total
                    .checked_add(weight)
                    .context("sound event weight overflow")?;
            }
        }
        visiting.pop();
        weights.insert(id.clone(), total);
        Ok(total)
    }

    fn select_inner(
        &self,
        id: &ResourceId,
        weights: &BTreeMap<ResourceId, i32>,
        files: &BTreeMap<ResourceId, bool>,
        random: &mut impl FnMut(i32) -> i32,
        depth: usize,
    ) -> Result<Option<SelectedSound>> {
        ensure!(depth < 64, "sound event nesting is too deep");
        let total = *weights.get(id).unwrap_or(&0);
        if total == 0 {
            return Ok(None);
        }
        let mut pick = random(total);
        ensure!(
            (0..total).contains(&pick),
            "sound random value out of range"
        );
        let event = self
            .events
            .get(id)
            .context("weighted sound event missing")?;
        for sound in &event.sounds {
            let weight = match sound.kind {
                SoundKind::File => {
                    if *files.get(&sound.id).unwrap_or(&false) {
                        i32::try_from(sound.weight).context("sound weight is too large")?
                    } else {
                        0
                    }
                }
                SoundKind::Event => *weights.get(&sound.id).unwrap_or(&0),
            };
            pick -= weight;
            if pick < 0 {
                return match sound.kind {
                    SoundKind::File => Ok(Some(SelectedSound {
                        id: sound.id.clone(),
                        volume_factors: vec![sound.volume.clone()],
                        pitch_factors: vec![sound.pitch.clone()],
                        weight: sound.weight,
                        stream: sound.stream,
                        preload: sound.preload,
                        attenuation_distance: sound.attenuation_distance,
                    })),
                    SoundKind::Event => {
                        let mut selected = self
                            .select_inner(&sound.id, weights, files, random, depth + 1)?
                            .context("selected empty nested sound event")?;
                        selected.volume_factors.push(sound.volume.clone());
                        selected.pitch_factors.push(sound.pitch.clone());
                        selected.weight = sound.weight;
                        selected.stream |= sound.stream;
                        Ok(Some(selected))
                    }
                };
            }
        }
        bail!("weighted sound event had no matching entry")
    }

    /// Parse the complete layer before mutating the catalog, so an invalid
    /// layer cannot leave a partially applied resource pack.
    pub fn apply(&mut self, namespace: &str, bytes: &[u8]) -> Result<()> {
        ResourceId::parse(&format!("{namespace}:sound"))?;
        let root: Value = serde_json::from_slice(bytes).context("invalid sounds.json")?;
        let object = root.as_object().context("sounds.json must be an object")?;
        let mut parsed = Vec::with_capacity(object.len());
        for (name, value) in object {
            let id = ResourceId::parse(&format!("{namespace}:{name}"))?;
            let definition = value
                .as_object()
                .with_context(|| format!("sound event {name} must be an object"))?;
            let replace = match definition.get("replace") {
                None => false,
                Some(v) => v.as_bool().context("sound replace must be a boolean")?,
            };
            let subtitle = match definition.get("subtitle") {
                None => None,
                Some(v) => Some(
                    v.as_str()
                        .context("sound subtitle must be a string")?
                        .to_owned(),
                ),
            };
            let mut sounds = Vec::new();
            if let Some(entries) = definition.get("sounds") {
                for entry in entries.as_array().context("sounds must be an array")? {
                    sounds.push(parse_entry(entry)?);
                }
            }
            parsed.push((id, replace, SoundEvent { subtitle, sounds }));
        }
        for (id, replace, event) in parsed {
            if replace || !self.events.contains_key(&id) {
                self.events.insert(id, event);
            } else {
                self.events
                    .get_mut(&id)
                    .expect("event exists")
                    .sounds
                    .extend(event.sounds);
            }
        }
        Ok(())
    }
}

fn parse_entry(value: &Value) -> Result<SoundEntry> {
    let mut name = value.as_str();
    let mut kind = SoundKind::File;
    let mut volume = Value::from(1.0);
    let mut pitch = Value::from(1.0);
    let mut weight = 1_u32;
    let mut stream = false;
    let mut preload = false;
    let mut attenuation_distance = 16_u32;
    if let Some(object) = value.as_object() {
        name = Some(
            object
                .get("name")
                .and_then(Value::as_str)
                .context("sound entry needs a name")?,
        );
        let entry_type = match object.get("type") {
            None => "file",
            Some(value) => value.as_str().context("sound type must be a string")?,
        };
        kind = match entry_type {
            "file" => SoundKind::File,
            "event" => SoundKind::Event,
            other => bail!("unsupported sound type {other}"),
        };
        if let Some(v) = object.get("volume") {
            validate_sound_float(v, "volume")?;
            volume = v.clone();
        }
        if let Some(v) = object.get("pitch") {
            validate_sound_float(v, "pitch")?;
            pitch = v.clone();
        }
        if let Some(v) = object.get("weight") {
            weight = u32::try_from(v.as_u64().context("invalid sound weight")?)
                .context("sound weight is too large")?;
            ensure!(weight > 0, "sound weight must be positive");
        }
        if let Some(v) = object.get("stream") {
            stream = v.as_bool().context("invalid sound stream flag")?;
        }
        if let Some(v) = object.get("preload") {
            preload = v.as_bool().context("invalid sound preload flag")?;
        }
        if let Some(v) = object.get("attenuation_distance") {
            attenuation_distance =
                u32::try_from(v.as_u64().context("invalid attenuation distance")?)
                    .context("attenuation distance is too large")?;
        }
    }
    let id = ResourceId::parse(name.context("sound entry must be a string or object")?)?;
    Ok(SoundEntry {
        id,
        kind,
        volume,
        pitch,
        weight,
        stream,
        preload,
        attenuation_distance,
    })
}

fn validate_sound_float(value: &Value, name: &str) -> Result<()> {
    let number = value
        .as_f64()
        .with_context(|| format!("sound {name} must be a number"))? as f32;
    ensure!(number > 0.0, "sound {name} must be positive");
    Ok(())
}

/// A pack's OGG file overrides the launcher's content-addressed copy.
pub fn read_sound_file(
    index: &AssetIndex,
    root: &Path,
    stack: &mut ResourceStack,
    id: &ResourceId,
) -> Result<Vec<u8>> {
    let id = ResourceId::parse(&format!("{}:{}", id.namespace, id.path))?;
    let name = format!("{}/sounds/{}.ogg", id.namespace, id.path);
    match stack.read_asset(&format!("assets/{name}"))? {
        Some(bytes) => Ok(bytes),
        None => index.read(root, &name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SELECTION_FIXTURE: &[u8] = br#"{
      "test.outer":{"sounds":[{"name":"test/a","weight":2,"volume":0.7},
       {"name":"test.inner","type":"event","weight":19,"volume":0.5,"pitch":0.8,"stream":true}]},
      "test.inner":{"sounds":[{"name":"test/b","weight":1,"volume":0.3,"preload":true,"attenuation_distance":9},
       {"name":"test/c","weight":3,"pitch":1.2}]}}
    "#;

    #[test]
    fn invalid_volume_or_pitch_rejects_the_entire_layer() {
        for field in ["volume", "pitch"] {
            for value in ["0", "-1", "1e-100", "null", "true", "\"0.5\"", "{}"] {
                let mut catalog = SoundDefinitions::default();
                catalog
                    .apply("minecraft", br#"{"test":{"sounds":["original"]}}"#)
                    .unwrap();
                let json = format!(
                    r#"{{"test":{{"replace":true,"sounds":[{{"name":"replacement","{field}":{value}}}]}},"other":{{"sounds":["partial"]}}}}"#
                );
                assert!(
                    catalog.apply("minecraft", json.as_bytes()).is_err(),
                    "{field}: {value}"
                );
                assert_eq!(
                    catalog
                        .get(&ResourceId::parse("test").unwrap())
                        .unwrap()
                        .sounds[0]
                        .id,
                    ResourceId::parse("original").unwrap()
                );
                assert!(catalog.get(&ResourceId::parse("other").unwrap()).is_none());
            }
        }
    }

    #[test]
    fn selection_handles_missing_files_nested_events_and_cycles() {
        let mut catalog = SoundDefinitions::default();
        catalog.apply("minecraft", SELECTION_FIXTURE).unwrap();
        let outer = ResourceId::parse("test.outer").unwrap();
        let available = |id: &ResourceId| id.path != "test/a";
        let mut bounds = Vec::new();
        let selected = catalog
            .select(&outer, &available, &mut |bound| {
                bounds.push(bound);
                0
            })
            .unwrap()
            .unwrap();
        assert_eq!(bounds, vec![4, 4]);
        assert_eq!(selected.id, ResourceId::parse("test/b").unwrap());
        assert_eq!(selected.weight, 19);
        assert!(selected.stream && selected.preload);
        assert_eq!(selected.attenuation_distance, 9);
        assert_eq!(
            selected.volume_factors,
            vec![Value::from(0.3), Value::from(0.5)]
        );
        assert_eq!(
            selected.pitch_factors,
            vec![Value::from(1.0), Value::from(0.8)]
        );
        assert!(
            catalog
                .select(&outer, &|_| false, &mut |_| panic!(
                    "no random call expected"
                ))
                .unwrap()
                .is_none()
        );
        assert!(
            catalog
                .select(&outer, &|_| true, &mut |bound| bound)
                .is_err()
        );
        catalog.apply("minecraft", br#"{"test.inner":{"replace":true,"sounds":[{"name":"test.outer","type":"event"}]}}"#).unwrap();
        assert!(catalog.select(&outer, &|_| true, &mut |_| 0).is_err());
        catalog.apply("minecraft", br#"{"test.inner":{"replace":true,"sounds":[{"name":"test/b","weight":2147483647},"test/c"]}}"#).unwrap();
        assert!(catalog.select(&outer, &|_| true, &mut |_| 0).is_err());
    }

    #[test]
    fn sound_pack_layers_append_replace_and_preserve_values() {
        let mut definitions = SoundDefinitions::default();
        definitions
            .apply(
                "minecraft",
                br#"{"block.test":{"subtitle":"first","sounds":["random/a"]}}"#,
            )
            .unwrap();
        definitions.apply("minecraft", br#"{"block.test":{"subtitle":"ignored","sounds":[{"name":"other:events.b","type":"event","pitch":0.5,"weight":3,"stream":true}]}}"#).unwrap();
        let id = ResourceId::parse("minecraft:block.test").unwrap();
        let event = definitions.get(&id).unwrap();
        assert_eq!(event.subtitle.as_deref(), Some("first"));
        assert_eq!(event.sounds.len(), 2);
        assert_eq!(event.sounds[1].kind, SoundKind::Event);
        assert_eq!(
            event.sounds[1].id,
            ResourceId::parse("other:events.b").unwrap()
        );
        assert_eq!(event.sounds[1].weight, 3);
        assert!(event.sounds[1].stream);
        assert_eq!(event.sounds[1].pitch.as_f64(), Some(0.5));
        let before = event.clone();
        assert!(definitions.apply("minecraft", br#"{"block.test":{"replace":true,"sounds":[{"name":"a","weight":0}]},"block.other":{"sounds":["b"]}}"#).is_err());
        assert_eq!(definitions.get(&id), Some(&before));
        assert!(
            definitions
                .get(&ResourceId::parse("block.other").unwrap())
                .is_none()
        );
        definitions.apply("minecraft", br#"{"block.test":{"replace":true,"subtitle":"replacement","sounds":["random/c"]}}"#).unwrap();
        let event = definitions.get(&id).unwrap();
        assert_eq!(event.subtitle.as_deref(), Some("replacement"));
        assert_eq!(event.sounds.len(), 1);
        assert_eq!(event.sounds[0].id, ResourceId::parse("random/c").unwrap());
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_button_sounds_and_pack_overrides_resolve() {
        let root = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let index = AssetIndex::load(&root).unwrap();
        let mut stack = ResourceStack::open(&root).unwrap();
        let catalog = SoundDefinitions::load_minecraft(&index, &root, &mut stack).unwrap();
        #[cfg(target_os = "windows")]
        {
            use std::{path::PathBuf, process::Command};
            let temp = tempfile::tempdir().unwrap();
            let version = root.join("versions/26.3");
            let manifest: Value =
                serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
            let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
            for library in manifest["libraries"].as_array().unwrap() {
                if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                    let path = root.join("libraries").join(path);
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
            let cp = std::env::join_paths(paths).unwrap();
            let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
            let source = temp.path().join("SoundDefinitionOracle.java");
            let input = temp.path().join("sounds.json");
            let output = temp.path().join("expected.json");
            std::fs::write(
                &source,
                include_str!("../tests/java/SoundDefinitionOracle.java"),
            )
            .unwrap();
            std::fs::write(&input, index.read(&root, "minecraft/sounds.json").unwrap()).unwrap();
            let run = |command: &mut Command| {
                let result = command.output().unwrap();
                assert!(
                    result.status.success(),
                    "Java sound oracle failed:\n{}\n{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            };
            run(Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source));
            run(Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("SoundDefinitionOracle")
                .arg(&input)
                .arg(&output));
            let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
            for row in expected["validation"].as_array().unwrap() {
                let field = row["field"].as_str().unwrap();
                let value = row["value"].as_str().unwrap();
                let json =
                    format!(r#"{{"test":{{"sounds":[{{"name":"random/a","{field}":{value}}}]}}}}"#);
                let mut definitions = SoundDefinitions::default();
                let result = definitions.apply("minecraft", json.as_bytes());
                assert_eq!(
                    result.is_ok(),
                    row["accepted"].as_bool().unwrap(),
                    "{field}: {value}"
                );
                if result.is_ok() {
                    let sound = &definitions
                        .get(&ResourceId::parse("test").unwrap())
                        .unwrap()
                        .sounds[0];
                    let number = if field == "volume" {
                        &sound.volume
                    } else {
                        &sound.pitch
                    };
                    assert_eq!(
                        (number.as_f64().unwrap() as f32).to_bits(),
                        row["bits"].as_i64().unwrap() as i32 as u32,
                        "{field}: {value}"
                    );
                }
            }
            let mut fixture = SoundDefinitions::default();
            fixture.apply("minecraft", SELECTION_FIXTURE).unwrap();
            for (seed, row) in expected["selection"].as_array().unwrap().iter().enumerate() {
                let mut random = native_core::random::LegacyRandom::new(seed as i64);
                let selected = fixture
                    .select(
                        &ResourceId::parse("test.outer").unwrap(),
                        &|_| true,
                        &mut |bound| random.next_int_bound(bound).unwrap(),
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    format!("{}:{}", selected.id.namespace, selected.id.path),
                    row["name"].as_str().unwrap()
                );
                let product = |values: &[Value]| {
                    values
                        .iter()
                        .map(|v| v.as_f64().unwrap() as f32)
                        .reduce(|a, b| a * b)
                        .unwrap()
                        .to_bits()
                };
                assert_eq!(
                    product(&selected.volume_factors),
                    row["volume"].as_i64().unwrap() as i32 as u32
                );
                assert_eq!(
                    product(&selected.pitch_factors),
                    row["pitch"].as_i64().unwrap() as i32 as u32
                );
                assert_eq!(u64::from(selected.weight), row["weight"].as_u64().unwrap());
                assert_eq!(selected.stream, row["stream"].as_bool().unwrap());
                assert_eq!(selected.preload, row["preload"].as_bool().unwrap());
                assert_eq!(
                    u64::from(selected.attenuation_distance),
                    row["attenuation"].as_u64().unwrap()
                );
                assert_eq!(random.next_int(), row["next"].as_i64().unwrap() as i32);
            }
            for id in [
                "block.stone_button.click_on",
                "block.stone_button.click_off",
            ] {
                let event = catalog.get(&ResourceId::parse(id).unwrap()).unwrap();
                let sound = &event.sounds[0];
                let row = &expected[id];
                assert_eq!(
                    format!("{}:{}", sound.id.namespace, sound.id.path),
                    row["name"].as_str().unwrap()
                );
                assert_eq!(
                    (sound.volume.as_f64().unwrap() as f32).to_bits(),
                    row["volume"].as_i64().unwrap() as i32 as u32
                );
                assert_eq!(
                    (sound.pitch.as_f64().unwrap() as f32).to_bits(),
                    row["pitch"].as_i64().unwrap() as i32 as u32
                );
                assert_eq!(u64::from(sound.weight), row["weight"].as_u64().unwrap());
                assert_eq!(sound.stream, row["stream"].as_bool().unwrap());
                assert_eq!(sound.preload, row["preload"].as_bool().unwrap());
                assert_eq!(
                    u64::from(sound.attenuation_distance),
                    row["attenuation"].as_u64().unwrap()
                );
                assert_eq!(event.subtitle.as_deref(), row["subtitle"].as_str());
            }
        }
        for (id, pitch) in [
            ("block.stone_button.click_on", 0.6),
            ("block.stone_button.click_off", 0.5),
        ] {
            let event = catalog.get(&ResourceId::parse(id).unwrap()).unwrap();
            assert_eq!(event.sounds.len(), 1);
            let sound = &event.sounds[0];
            assert_eq!(sound.id, ResourceId::parse("random/click").unwrap());
            assert_eq!(sound.kind, SoundKind::File);
            assert_eq!(sound.pitch.as_f64(), Some(pitch));
            assert_eq!(sound.volume.as_f64(), Some(0.3));
            assert!(
                read_sound_file(&index, &root, &mut stack, &sound.id)
                    .unwrap()
                    .starts_with(b"OggS")
            );
        }
        let pack = tempfile::tempdir().unwrap();
        let dir = pack.path().join("assets/minecraft/sounds/random");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("click.ogg"), b"test-override").unwrap();
        std::fs::write(
            pack.path().join("assets/minecraft/sounds.json"),
            br#"{"block.stone_button.click_on":{"replace":true,"sounds":["random/click"]}}"#,
        )
        .unwrap();
        let custom_assets = pack.path().join("assets/example");
        std::fs::create_dir_all(&custom_assets).unwrap();
        std::fs::write(
            custom_assets.join("sounds.json"),
            br#"{"impact":{"sounds":["example:low"]},"append":{"sounds":["example:first"]}}"#,
        )
        .unwrap();
        stack.push_pack(pack.path()).unwrap();
        let invalid = tempfile::tempdir().unwrap();
        let invalid_assets = invalid.path().join("assets/minecraft");
        std::fs::create_dir_all(&invalid_assets).unwrap();
        std::fs::write(invalid_assets.join("sounds.json"), br#"{"block.stone_button.click_on":{"replace":true,"sounds":[{"name":"broken","volume":0}]},"test.partial":{"sounds":["should_not_apply"]}}"#).unwrap();
        stack.push_pack(invalid.path()).unwrap();
        let high = tempfile::tempdir().unwrap();
        let high_assets = high.path().join("assets/minecraft");
        std::fs::create_dir_all(&high_assets).unwrap();
        std::fs::write(
            high_assets.join("sounds.json"),
            br#"{"test.high":{"sounds":["high"]}}"#,
        )
        .unwrap();
        let custom_high = high.path().join("assets/example");
        std::fs::create_dir_all(&custom_high).unwrap();
        std::fs::write(
            custom_high.join("sounds.json"),
            br#"{"impact":{"replace":true,"sounds":["example:high"]},"append":{"sounds":["example:second"]}}"#,
        )
        .unwrap();
        stack.push_pack(high.path()).unwrap();
        let archive_dir = tempfile::tempdir().unwrap();
        let archive_path = archive_dir.path().join("custom.zip");
        {
            use std::io::Write;
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
            archive
                .start_file(
                    "assets/zipmod/sounds.json",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            archive
                .write_all(br#"{"impact":{"sounds":["example:high"]}}"#)
                .unwrap();
            archive.finish().unwrap();
        }
        stack.push_pack(&archive_path).unwrap();
        let catalog = SoundDefinitions::load_minecraft(&index, &root, &mut stack).unwrap();
        let impact = catalog
            .get(&ResourceId::parse("example:impact").unwrap())
            .unwrap();
        assert_eq!(impact.sounds.len(), 1);
        assert_eq!(
            impact.sounds[0].id,
            ResourceId::parse("example:high").unwrap()
        );
        let append = catalog
            .get(&ResourceId::parse("example:append").unwrap())
            .unwrap();
        assert_eq!(
            append
                .sounds
                .iter()
                .map(|sound| sound.id.clone())
                .collect::<Vec<_>>(),
            ["example:first", "example:second"].map(|id| ResourceId::parse(id).unwrap())
        );
        assert_eq!(
            catalog
                .get(&ResourceId::parse("zipmod:impact").unwrap())
                .unwrap()
                .sounds[0]
                .id,
            ResourceId::parse("example:high").unwrap()
        );
        assert!(
            catalog
                .get(&ResourceId::parse("test.partial").unwrap())
                .is_none()
        );
        assert!(
            catalog
                .get(&ResourceId::parse("test.high").unwrap())
                .is_some()
        );
        let event = catalog
            .get(&ResourceId::parse("block.stone_button.click_on").unwrap())
            .unwrap();
        assert_eq!(event.sounds[0].pitch.as_f64(), Some(1.0));
        assert_eq!(
            read_sound_file(&index, &root, &mut stack, &event.sounds[0].id).unwrap(),
            b"test-override"
        );
    }
}
