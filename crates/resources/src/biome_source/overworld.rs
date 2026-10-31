//! Build the 26.3 overworld climate parameter list in registration order.
use super::{DIMENSIONS, Interval, ResourceId, quantize};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

fn span(min: f32, max: f32) -> Interval {
    Interval {
        min: quantize(min),
        max: quantize(max),
    }
}
fn join(a: Interval, b: Interval) -> Interval {
    Interval {
        min: a.min.min(b.min),
        max: a.max.max(b.max),
    }
}
fn point(x: f32) -> Interval {
    span(x, x)
}

const OCEANS: [[&str; 5]; 2] = [
    [
        "deep_frozen_ocean",
        "deep_cold_ocean",
        "deep_ocean",
        "deep_lukewarm_ocean",
        "warm_ocean",
    ],
    [
        "frozen_ocean",
        "cold_ocean",
        "ocean",
        "lukewarm_ocean",
        "warm_ocean",
    ],
];
const MIDDLE: [[&str; 5]; 5] = [
    [
        "snowy_plains",
        "snowy_plains",
        "snowy_plains",
        "snowy_taiga",
        "taiga",
    ],
    [
        "plains",
        "plains",
        "forest",
        "taiga",
        "old_growth_spruce_taiga",
    ],
    [
        "flower_forest",
        "plains",
        "forest",
        "birch_forest",
        "dark_forest",
    ],
    ["savanna", "savanna", "forest", "jungle", "jungle"],
    ["desert", "desert", "desert", "desert", "desert"],
];
const MIDDLE_VARIANT: [[&str; 5]; 5] = [
    ["ice_spikes", "", "snowy_taiga", "", ""],
    ["dappled_forest", "", "", "", "old_growth_pine_taiga"],
    ["sunflower_plains", "", "", "old_growth_birch_forest", ""],
    ["", "", "plains", "sparse_jungle", "bamboo_jungle"],
    ["", "", "", "", ""],
];
const PLATEAU: [[&str; 5]; 5] = [
    [
        "snowy_plains",
        "snowy_plains",
        "snowy_plains",
        "snowy_taiga",
        "snowy_taiga",
    ],
    [
        "meadow",
        "meadow",
        "forest",
        "taiga",
        "old_growth_spruce_taiga",
    ],
    ["meadow", "meadow", "meadow", "meadow", "pale_garden"],
    [
        "savanna_plateau",
        "savanna_plateau",
        "forest",
        "forest",
        "jungle",
    ],
    [
        "badlands",
        "badlands",
        "badlands",
        "wooded_badlands",
        "wooded_badlands",
    ],
];
const PLATEAU_VARIANT: [[&str; 5]; 5] = [
    ["ice_spikes", "", "", "", ""],
    [
        "cherry_grove",
        "",
        "meadow",
        "meadow",
        "old_growth_pine_taiga",
    ],
    ["cherry_grove", "cherry_grove", "forest", "birch_forest", ""],
    ["", "", "", "", ""],
    ["eroded_badlands", "eroded_badlands", "", "", ""],
];
const SHATTERED: [[&str; 5]; 5] = [
    [
        "windswept_gravelly_hills",
        "windswept_gravelly_hills",
        "windswept_hills",
        "windswept_forest",
        "windswept_forest",
    ],
    [
        "windswept_gravelly_hills",
        "windswept_gravelly_hills",
        "windswept_hills",
        "windswept_forest",
        "windswept_forest",
    ],
    [
        "windswept_hills",
        "windswept_hills",
        "windswept_hills",
        "windswept_forest",
        "windswept_forest",
    ],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
];

struct Builder {
    full: Interval,
    temperatures: [Interval; 5],
    humidities: [Interval; 5],
    erosions: [Interval; 7],
    mushroom: Interval,
    deep_ocean: Interval,
    ocean: Interval,
    coast: Interval,
    inland: Interval,
    near: Interval,
    mid: Interval,
    far: Interval,
    result: Vec<([Interval; DIMENSIONS], &'static str)>,
}
impl Builder {
    fn new() -> Self {
        Self {
            full: span(-1.0, 1.0),
            temperatures: [
                span(-1.0, -0.45),
                span(-0.45, -0.15),
                span(-0.15, 0.2),
                span(0.2, 0.55),
                span(0.55, 1.0),
            ],
            humidities: [
                span(-1.0, -0.35),
                span(-0.35, -0.1),
                span(-0.1, 0.1),
                span(0.1, 0.3),
                span(0.3, 1.0),
            ],
            erosions: [
                span(-1.0, -0.78),
                span(-0.78, -0.375),
                span(-0.375, -0.2225),
                span(-0.2225, 0.05),
                span(0.05, 0.45),
                span(0.45, 0.55),
                span(0.55, 1.0),
            ],
            mushroom: span(-1.2, -1.05),
            deep_ocean: span(-1.05, -0.455),
            ocean: span(-0.455, -0.19),
            coast: span(-0.19, -0.11),
            inland: span(-0.11, 0.55),
            near: span(-0.11, 0.03),
            mid: span(0.03, 0.3),
            far: span(0.3, 1.0),
            result: Vec::new(),
        }
    }
    fn add(
        &mut self,
        t: Interval,
        h: Interval,
        c: Interval,
        e: Interval,
        d: Interval,
        w: Interval,
        id: &'static str,
    ) {
        self.result.push(([t, h, c, e, d, w, point(0.0)], id));
    }
    fn surface(
        &mut self,
        t: Interval,
        h: Interval,
        c: Interval,
        e: Interval,
        w: Interval,
        id: &'static str,
    ) {
        self.add(t, h, c, e, point(0.0), w, id);
        self.add(t, h, c, e, point(1.0), w, id);
    }
    fn middle(t: usize, h: usize, w: Interval) -> &'static str {
        if w.max >= 0 && !MIDDLE_VARIANT[t][h].is_empty() {
            MIDDLE_VARIANT[t][h]
        } else {
            MIDDLE[t][h]
        }
    }
    fn badlands(h: usize, w: Interval) -> &'static str {
        if h < 2 {
            if w.max < 0 {
                "badlands"
            } else {
                "eroded_badlands"
            }
        } else if h < 3 {
            "badlands"
        } else {
            "wooded_badlands"
        }
    }
    fn plateau(t: usize, h: usize, w: Interval) -> &'static str {
        if w.max >= 0 && !PLATEAU_VARIANT[t][h].is_empty() {
            PLATEAU_VARIANT[t][h]
        } else {
            PLATEAU[t][h]
        }
    }
    fn slope(t: usize, h: usize, w: Interval) -> &'static str {
        if t >= 3 {
            Self::plateau(t, h, w)
        } else if h <= 1 {
            "snowy_slopes"
        } else {
            "grove"
        }
    }
    fn peak(t: usize, h: usize, w: Interval) -> &'static str {
        if t <= 2 {
            if w.max < 0 {
                "jagged_peaks"
            } else {
                "frozen_peaks"
            }
        } else if t == 3 {
            "stony_peaks"
        } else {
            Self::badlands(h, w)
        }
    }
    fn middle_or_badlands(t: usize, h: usize, w: Interval) -> &'static str {
        if t == 4 {
            Self::badlands(h, w)
        } else {
            Self::middle(t, h, w)
        }
    }
    fn middle_or_slope(t: usize, h: usize, w: Interval) -> &'static str {
        if t == 0 {
            Self::slope(t, h, w)
        } else {
            Self::middle_or_badlands(t, h, w)
        }
    }
    fn shattered(t: usize, h: usize, w: Interval) -> &'static str {
        if SHATTERED[t][h].is_empty() {
            Self::middle(t, h, w)
        } else {
            SHATTERED[t][h]
        }
    }
    fn wind(t: usize, h: usize, w: Interval, base: &'static str) -> &'static str {
        if t > 1 && h < 4 && w.max >= 0 {
            "windswept_savanna"
        } else {
            base
        }
    }
    fn beach(t: usize) -> &'static str {
        if t == 0 {
            "snowy_beach"
        } else if t == 4 {
            "desert"
        } else {
            "beach"
        }
    }
    fn shattered_coast(t: usize, h: usize, w: Interval) -> &'static str {
        Self::wind(
            t,
            h,
            w,
            if w.max >= 0 {
                Self::middle(t, h, w)
            } else {
                Self::beach(t)
            },
        )
    }
    fn oceans(&mut self) {
        self.surface(
            self.full,
            self.full,
            self.mushroom,
            self.full,
            self.full,
            "mushroom_fields",
        );
        for (i, _) in OCEANS[0].iter().enumerate() {
            self.surface(
                self.temperatures[i],
                self.full,
                self.deep_ocean,
                self.full,
                self.full,
                OCEANS[0][i],
            );
            self.surface(
                self.temperatures[i],
                self.full,
                self.ocean,
                self.full,
                self.full,
                OCEANS[1][i],
            );
        }
    }
    fn peaks(&mut self, w: Interval) {
        for ti in 0..5 {
            for hi in 0..5 {
                let (t, h) = (self.temperatures[ti], self.humidities[hi]);
                let e = self.erosions;
                let middle = Self::middle(ti, hi, w);
                let bad = Self::middle_or_badlands(ti, hi, w);
                let cold = Self::middle_or_slope(ti, hi, w);
                let plateau = Self::plateau(ti, hi, w);
                let shattered = Self::shattered(ti, hi, w);
                let wind = Self::wind(ti, hi, w, shattered);
                let peak = Self::peak(ti, hi, w);
                self.surface(t, h, join(self.coast, self.far), e[0], w, peak);
                self.surface(t, h, join(self.coast, self.near), e[1], w, cold);
                self.surface(t, h, join(self.mid, self.far), e[1], w, peak);
                self.surface(
                    t,
                    h,
                    join(self.coast, self.near),
                    join(e[2], e[3]),
                    w,
                    middle,
                );
                self.surface(t, h, join(self.mid, self.far), e[2], w, plateau);
                self.surface(t, h, self.mid, e[3], w, bad);
                self.surface(t, h, self.far, e[3], w, plateau);
                self.surface(t, h, join(self.coast, self.far), e[4], w, middle);
                self.surface(t, h, join(self.coast, self.near), e[5], w, wind);
                self.surface(t, h, join(self.mid, self.far), e[5], w, shattered);
                self.surface(t, h, join(self.coast, self.far), e[6], w, middle);
            }
        }
    }
    fn high(&mut self, w: Interval) {
        for ti in 0..5 {
            for hi in 0..5 {
                let (t, h) = (self.temperatures[ti], self.humidities[hi]);
                let e = self.erosions;
                let middle = Self::middle(ti, hi, w);
                let bad = Self::middle_or_badlands(ti, hi, w);
                let cold = Self::middle_or_slope(ti, hi, w);
                let plateau = Self::plateau(ti, hi, w);
                let shattered = Self::shattered(ti, hi, w);
                let wind = Self::wind(ti, hi, w, middle);
                let slope = Self::slope(ti, hi, w);
                let peak = Self::peak(ti, hi, w);
                self.surface(t, h, self.coast, join(e[0], e[1]), w, middle);
                self.surface(t, h, self.near, e[0], w, slope);
                self.surface(t, h, join(self.mid, self.far), e[0], w, peak);
                self.surface(t, h, self.near, e[1], w, cold);
                self.surface(t, h, join(self.mid, self.far), e[1], w, slope);
                self.surface(
                    t,
                    h,
                    join(self.coast, self.near),
                    join(e[2], e[3]),
                    w,
                    middle,
                );
                self.surface(t, h, join(self.mid, self.far), e[2], w, plateau);
                self.surface(t, h, self.mid, e[3], w, bad);
                self.surface(t, h, self.far, e[3], w, plateau);
                self.surface(t, h, join(self.coast, self.far), e[4], w, middle);
                self.surface(t, h, join(self.coast, self.near), e[5], w, wind);
                self.surface(t, h, join(self.mid, self.far), e[5], w, shattered);
                self.surface(t, h, join(self.coast, self.far), e[6], w, middle);
            }
        }
    }
}

impl Builder {
    fn common_low_mid(&mut self, w: Interval) {
        let e = self.erosions;
        let t = self.temperatures;
        self.surface(
            self.full,
            self.full,
            self.coast,
            join(e[0], e[2]),
            w,
            "stony_shore",
        );
        self.surface(
            join(t[1], t[2]),
            self.full,
            join(self.near, self.far),
            e[6],
            w,
            "swamp",
        );
        self.surface(
            join(t[3], t[4]),
            self.full,
            join(self.near, self.far),
            e[6],
            w,
            "mangrove_swamp",
        );
    }
    fn mid_slice(&mut self, w: Interval) {
        self.common_low_mid(w);
        for ti in 0..5 {
            for hi in 0..5 {
                let (t, h) = (self.temperatures[ti], self.humidities[hi]);
                let e = self.erosions;
                let middle = Self::middle(ti, hi, w);
                let bad = Self::middle_or_badlands(ti, hi, w);
                let cold = Self::middle_or_slope(ti, hi, w);
                let shattered = Self::shattered(ti, hi, w);
                let plateau = Self::plateau(ti, hi, w);
                let beach = Self::beach(ti);
                let wind = Self::wind(ti, hi, w, middle);
                let coast = Self::shattered_coast(ti, hi, w);
                let slope = Self::slope(ti, hi, w);
                self.surface(t, h, join(self.near, self.far), e[0], w, slope);
                self.surface(t, h, join(self.near, self.mid), e[1], w, cold);
                self.surface(
                    t,
                    h,
                    self.far,
                    e[1],
                    w,
                    if ti == 0 { slope } else { plateau },
                );
                self.surface(t, h, self.near, e[2], w, middle);
                self.surface(t, h, self.mid, e[2], w, bad);
                self.surface(t, h, self.far, e[2], w, plateau);
                self.surface(t, h, join(self.coast, self.near), e[3], w, middle);
                self.surface(t, h, join(self.mid, self.far), e[3], w, bad);
                if w.max < 0 {
                    self.surface(t, h, self.coast, e[4], w, beach);
                    self.surface(t, h, join(self.near, self.far), e[4], w, middle);
                } else {
                    self.surface(t, h, join(self.coast, self.far), e[4], w, middle);
                }
                self.surface(t, h, self.coast, e[5], w, coast);
                self.surface(t, h, self.near, e[5], w, wind);
                self.surface(t, h, join(self.mid, self.far), e[5], w, shattered);
                self.surface(
                    t,
                    h,
                    self.coast,
                    e[6],
                    w,
                    if w.max < 0 { beach } else { middle },
                );
                if ti == 0 {
                    self.surface(t, h, join(self.near, self.far), e[6], w, middle);
                }
            }
        }
    }
    fn low(&mut self, w: Interval) {
        self.common_low_mid(w);
        for ti in 0..5 {
            for hi in 0..5 {
                let (t, h) = (self.temperatures[ti], self.humidities[hi]);
                let e = self.erosions;
                let middle = Self::middle(ti, hi, w);
                let bad = Self::middle_or_badlands(ti, hi, w);
                let cold = Self::middle_or_slope(ti, hi, w);
                let beach = Self::beach(ti);
                let wind = Self::wind(ti, hi, w, middle);
                let coast = Self::shattered_coast(ti, hi, w);
                self.surface(t, h, self.near, join(e[0], e[1]), w, bad);
                self.surface(t, h, join(self.mid, self.far), join(e[0], e[1]), w, cold);
                self.surface(t, h, self.near, join(e[2], e[3]), w, middle);
                self.surface(t, h, join(self.mid, self.far), join(e[2], e[3]), w, bad);
                self.surface(t, h, self.coast, join(e[3], e[4]), w, beach);
                self.surface(t, h, join(self.near, self.far), e[4], w, middle);
                self.surface(t, h, self.coast, e[5], w, coast);
                self.surface(t, h, self.near, e[5], w, wind);
                self.surface(t, h, join(self.mid, self.far), e[5], w, middle);
                self.surface(t, h, self.coast, e[6], w, beach);
                if ti == 0 {
                    self.surface(t, h, join(self.near, self.far), e[6], w, middle);
                }
            }
        }
    }
    fn valleys(&mut self, w: Interval) {
        let e = self.erosions;
        let t = self.temperatures;
        let frozen = t[0];
        let unfrozen = join(t[1], t[4]);
        self.surface(
            frozen,
            self.full,
            self.coast,
            join(e[0], e[1]),
            w,
            if w.max < 0 {
                "stony_shore"
            } else {
                "frozen_river"
            },
        );
        self.surface(
            unfrozen,
            self.full,
            self.coast,
            join(e[0], e[1]),
            w,
            if w.max < 0 { "stony_shore" } else { "river" },
        );
        self.surface(
            frozen,
            self.full,
            self.near,
            join(e[0], e[1]),
            w,
            "frozen_river",
        );
        self.surface(unfrozen, self.full, self.near, join(e[0], e[1]), w, "river");
        self.surface(
            frozen,
            self.full,
            join(self.coast, self.far),
            join(e[2], e[5]),
            w,
            "frozen_river",
        );
        self.surface(
            unfrozen,
            self.full,
            join(self.coast, self.far),
            join(e[2], e[5]),
            w,
            "river",
        );
        self.surface(frozen, self.full, self.coast, e[6], w, "frozen_river");
        self.surface(unfrozen, self.full, self.coast, e[6], w, "river");
        self.surface(
            join(t[1], t[2]),
            self.full,
            join(self.inland, self.far),
            e[6],
            w,
            "swamp",
        );
        self.surface(
            join(t[3], t[4]),
            self.full,
            join(self.inland, self.far),
            e[6],
            w,
            "mangrove_swamp",
        );
        self.surface(
            frozen,
            self.full,
            join(self.inland, self.far),
            e[6],
            w,
            "frozen_river",
        );
        for ti in 0..5 {
            for hi in 0..5 {
                self.surface(
                    t[ti],
                    self.humidities[hi],
                    join(self.mid, self.far),
                    join(e[0], e[1]),
                    w,
                    Self::middle_or_badlands(ti, hi, w),
                );
            }
        }
    }
    fn build(mut self) -> Vec<([Interval; DIMENSIONS], &'static str)> {
        self.oceans();
        self.mid_slice(span(-1.0, -0.93333334));
        self.high(span(-0.93333334, -0.7666667));
        self.peaks(span(-0.7666667, -0.56666666));
        self.high(span(-0.56666666, -0.4));
        self.mid_slice(span(-0.4, -0.26666668));
        self.low(span(-0.26666668, -0.05));
        self.valleys(span(-0.05, 0.05));
        self.low(span(0.05, 0.26666668));
        self.mid_slice(span(0.26666668, 0.4));
        self.high(span(0.4, 0.56666666));
        self.peaks(span(0.56666666, 0.7666667));
        self.high(span(0.7666667, 0.93333334));
        self.mid_slice(span(0.93333334, 1.0));
        let e = self.erosions;
        self.add(
            self.full,
            self.full,
            span(0.8, 1.0),
            self.full,
            span(0.2, 0.9),
            self.full,
            "dripstone_caves",
        );
        self.add(
            self.full,
            span(0.7, 1.0),
            self.full,
            self.full,
            span(0.2, 0.9),
            self.full,
            "lush_caves",
        );
        self.add(
            self.full,
            self.full,
            join(self.coast, self.inland),
            join(e[5], e[6]),
            span(0.2, 0.9),
            span(-1.1, -0.85),
            "sulfur_caves",
        );
        self.add(
            self.full,
            self.full,
            self.full,
            join(e[0], e[1]),
            point(1.1),
            self.full,
            "deep_dark",
        );
        self.result
    }
}

pub(super) fn entries(
    registry: &BTreeSet<ResourceId>,
) -> Result<Vec<([Interval; DIMENSIONS], ResourceId)>> {
    Builder::new()
        .build()
        .into_iter()
        .map(|(parameters, name)| {
            let biome = ResourceId::parse(name)?;
            ensure!(
                registry.contains(&biome),
                "overworld biome {name} is missing from the active registry"
            );
            Ok((parameters, biome))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::{path::PathBuf, process::Command};
    #[test]
    fn includes_surface_and_underground_entries() {
        let entries = Builder::new().build();
        assert!(entries.len() > 1000);
        assert_eq!(entries[0].1, "mushroom_fields");
        assert_eq!(entries[1].1, "mushroom_fields");
        assert_eq!(entries[0].0[4].min, quantize(0.0));
        assert_eq!(entries[1].0[4].min, quantize(1.0));
        assert_eq!(entries.last().unwrap().1, "deep_dark");
        assert!(entries.iter().any(|entry| entry.1 == "sulfur_caves"));
    }
    fn run(command: &mut Command) -> String {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "Java overworld oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn parameter_registration_matches_vanilla() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
                let p = game.join("libraries").join(p);
                if p.is_file() {
                    paths.push(p);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("OverworldPresetOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/OverworldPresetOracle.java"),
        )
        .unwrap();
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("OverworldPresetOracle"));
        let expected: Vec<_> = output
            .lines()
            .filter(|line| line.starts_with("P "))
            .collect();
        let actual = Builder::new().build();
        assert_eq!(actual.len(), expected.len(), "overworld parameter count");
        for (i, ((parameters, name), line)) in actual.iter().zip(expected).enumerate() {
            let fields: Vec<_> = line.split_whitespace().collect();
            for axis in 0..6 {
                assert_eq!(
                    parameters[axis].min,
                    fields[1 + axis * 2].parse::<i64>().unwrap(),
                    "entry {i} axis {axis} min"
                );
                assert_eq!(
                    parameters[axis].max,
                    fields[2 + axis * 2].parse::<i64>().unwrap(),
                    "entry {i} axis {axis} max"
                );
            }
            assert_eq!(
                parameters[6].min,
                fields[13].parse::<i64>().unwrap(),
                "entry {i} offset"
            );
            assert_eq!(format!("minecraft:{name}"), fields[14], "entry {i} biome");
        }
    }
}
