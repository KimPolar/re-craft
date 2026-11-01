//! The non-interpolated 26.3 moon phase environment attribute.
use anyhow::{Context, Result, ensure};
use native_resources::packs::ResourceId;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::preview_lighting::TrackEasing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    FullMoon,
    WaningGibbous,
    ThirdQuarter,
    WaningCrescent,
    NewMoon,
    WaxingCrescent,
    FirstQuarter,
    WaxingGibbous,
}

impl Phase {
    pub const ALL: [Self; 8] = [
        Self::FullMoon,
        Self::WaningGibbous,
        Self::ThirdQuarter,
        Self::WaningCrescent,
        Self::NewMoon,
        Self::WaxingCrescent,
        Self::FirstQuarter,
        Self::WaxingGibbous,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::FullMoon => "full_moon",
            Self::WaningGibbous => "waning_gibbous",
            Self::ThirdQuarter => "third_quarter",
            Self::WaningCrescent => "waning_crescent",
            Self::NewMoon => "new_moon",
            Self::WaxingCrescent => "waxing_crescent",
            Self::FirstQuarter => "first_quarter",
            Self::WaxingGibbous => "waxing_gibbous",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|phase| *phase == self).unwrap()
    }

    pub fn parse(value: &Value) -> Result<Self> {
        let value = value.as_str().context("invalid moon phase")?;
        Self::ALL
            .into_iter()
            .find(|phase| phase.name() == value)
            .context("unknown moon phase")
    }
}

pub fn layer(value: &Value) -> Result<Phase> {
    if value.is_string() {
        return Phase::parse(value);
    }
    let modifier = ResourceId::parse(
        value
            .get("modifier")
            .and_then(Value::as_str)
            .context("missing moon phase modifier")?,
    )?;
    ensure!(
        modifier.namespace == "minecraft" && modifier.path == "override",
        "unsupported moon phase modifier"
    );
    Phase::parse(
        value
            .get("argument")
            .context("missing moon phase argument")?,
    )
}

pub struct Track {
    clock: ResourceId,
    period: Option<i32>,
    easing: TrackEasing,
    frames: Vec<(i32, Phase)>,
}

impl Track {
    pub fn parse(timeline: &Value, track: &Value) -> Result<Self> {
        let clock = ResourceId::parse(
            timeline
                .get("clock")
                .and_then(Value::as_str)
                .context("missing moon phase clock")?,
        )?;
        let period = timeline
            .get("period_ticks")
            .map(|value| {
                let period = value
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .context("invalid period")?;
                ensure!(period > 0, "invalid period");
                Ok(period)
            })
            .transpose()?;
        if let Some(modifier) = track.get("modifier") {
            let id = ResourceId::parse(modifier.as_str().context("invalid modifier")?)?;
            ensure!(
                id.namespace == "minecraft" && id.path == "override",
                "unsupported moon phase modifier"
            );
        }
        let easing = TrackEasing::parse(track.get("ease"))?;
        let values = track
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing moon phase keyframes")?;
        ensure!(!values.is_empty(), "empty moon phase keyframes");
        let mut frames: Vec<(i32, Phase)> = Vec::with_capacity(values.len());
        for frame in values {
            let tick = frame
                .get("ticks")
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok())
                .context("invalid keyframe tick")?;
            if let Some(period) = period {
                ensure!((0..=period).contains(&tick), "keyframe outside period");
            }
            if let Some((previous, _)) = frames.last() {
                ensure!(tick >= *previous, "unordered keyframes");
            }
            if frames.len() >= 2 {
                ensure!(
                    frames[frames.len() - 2].0 != tick,
                    "three equal keyframe ticks"
                );
            }
            frames.push((
                tick,
                Phase::parse(frame.get("value").context("missing keyframe value")?)?,
            ));
        }
        Ok(Self {
            clock,
            period,
            easing,
            frames,
        })
    }

    pub fn sample(&self, mut clock: impl FnMut(&ResourceId) -> Option<i64>) -> Option<Phase> {
        let ticks = clock(&self.clock)?;
        let ticks = self
            .period
            .map_or(ticks, |period| ticks.rem_euclid(i64::from(period)));
        let first = self.frames[0];
        let last = self.frames[self.frames.len() - 1];
        if self.frames.len() == 1 {
            return Some(first.1);
        }
        let segments = self
            .period
            .map(|p| ((last.0.wrapping_sub(p), last.1), first))
            .into_iter()
            .chain(self.frames.windows(2).map(|pair| (pair[0], pair[1])))
            .chain(
                self.period
                    .map(|p| (last, (first.0.wrapping_add(p), first.1))),
            );
        let ((start, a), (end, b)) = segments
            .clone()
            .find(|(_, end)| ticks < i64::from(end.0))
            .unwrap_or_else(|| segments.last().unwrap());
        if ticks <= i64::from(start) {
            return Some(a);
        }
        if ticks >= i64::from(end) {
            return Some(b);
        }
        let fraction = ticks.wrapping_sub(i64::from(start)) as f32 / end.wrapping_sub(start) as f32;
        Some(if self.easing.apply(fraction) >= 1.0 {
            b
        } else {
            a
        })
    }
}

pub struct Environment {
    pub base: Phase,
    pub biomes: BTreeMap<String, Option<Option<Phase>>>,
    pub has_biome_layer: bool,
    pub tracks: Vec<Track>,
}

impl Environment {
    pub fn sample(
        &self,
        biome: Option<&str>,
        mut clock: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Option<Phase> {
        let mut value = self.base;
        if self.has_biome_layer {
            value = self.biomes.get(biome?)?.as_ref()?.unwrap_or(value);
        }
        for track in &self.tracks {
            value = track.sample(&mut clock)?;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn phases_apply_dimension_biome_and_timeline_in_order() {
        let mut environment = Environment {
            base: layer(&json!("new_moon")).unwrap(),
            biomes: BTreeMap::from([("test:plains".into(), Some(Some(Phase::FirstQuarter)))]),
            has_biome_layer: true,
            tracks: Vec::new(),
        };
        assert_eq!(
            environment.sample(Some("test:plains"), |_| None),
            Some(Phase::FirstQuarter)
        );
        assert_eq!(environment.sample(None, |_| None), None);
        environment.tracks.push(Track::parse(&json!({"clock":"test:clock", "period_ticks":20}),
            &json!({"keyframes":[{"ticks":0,"value":"full_moon"},{"ticks":10,"value":"new_moon"}]})).unwrap());
        for (tick, expected) in [
            (-1, Phase::NewMoon),
            (0, Phase::FullMoon),
            (9, Phase::FullMoon),
            (10, Phase::NewMoon),
            (19, Phase::NewMoon),
            (20, Phase::FullMoon),
        ] {
            assert_eq!(
                environment.sample(Some("test:plains"), |_| Some(tick)),
                Some(expected)
            );
        }
        assert_eq!(environment.sample(Some("test:plains"), |_| None), None);
        assert!(layer(&json!({"modifier":"add","argument":"new_moon"})).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn phase_timeline_matches_java() {
        use native_resources::packs::{ResourceProvider, ResourceStack};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let timeline: Value = serde_json::from_slice(
            &resources
                .read_data("data/minecraft/timeline/moon.json")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let value = &timeline["tracks"]["minecraft:visual/moon_phase"];
        let track = Track::parse(&timeline, value).unwrap();
        let mut ticks: Vec<i64> = (-1000..=193000).step_by(293).collect();
        for i in -1..=9 {
            for delta in [-1, 0, 1] {
                ticks.push(i * 24000 + delta);
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("MoonPhaseOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/MoonPhaseOracle.java")).unwrap();
        std::fs::write(
            &input,
            serde_json::to_vec(&json!({"track":value,"ticks":ticks})).unwrap(),
        )
        .unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("MoonPhaseOracle")
                .arg(&input)
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let expected: Vec<usize> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), ticks.len() + 4);
        for (&tick, &index) in ticks.iter().zip(&expected) {
            assert_eq!(
                track.sample(|_| Some(tick)),
                Some(Phase::ALL[index]),
                "tick {tick}"
            );
        }
        assert_eq!(&expected[ticks.len()..], &[4, 4, 4, 4]);
    }
}
