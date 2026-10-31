//! Compare center fluid status with the real 26.3 aquifer under synthetic densities.
use native_core::aquifer::{
    AquiferBlock, AquiferFluidLevels, AquiferGenerator, AquiferSampler, FluidStatus,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Block(i32);
impl AquiferBlock for Block {
    fn air() -> Self {
        Self(0)
    }
    fn lava() -> Self {
        Self(2)
    }
    fn is_air(self) -> bool {
        self.0 == 0
    }
    fn is_water(self) -> bool {
        self.0 == 1
    }
    fn is_lava(self) -> bool {
        self.0 == 2
    }
}
struct Sampler {
    base: i32,
    sea: i32,
    mode: i32,
    exclusion: f32,
    flooded: f32,
    spread: f32,
    lava: f32,
}
impl AquiferSampler for Sampler {
    type Block = Block;
    fn barrier(&mut self, _: i32, _: i32, _: i32) -> f32 {
        0.0
    }
    fn global(&mut self, x: i32, y: i32, z: i32) -> FluidStatus<Block> {
        if self.mode == 1 && y < self.sea.min(-54) {
            return FluidStatus {
                level: -54,
                block: Block(2),
            };
        }
        if self.mode == 2 {
            let cell = x.div_euclid(16) ^ z.div_euclid(16);
            return FluidStatus {
                level: self.sea + cell.rem_euclid(17) - 8,
                block: Block(if cell & 3 == 0 { 2 } else { 1 }),
            };
        }
        FluidStatus {
            level: self.sea,
            block: Block(1),
        }
    }
    fn surface(&mut self, x: i32, z: i32) -> f32 {
        self.base as f32 + (((x / 4 * 31 + z / 4 * 17) & 31) - 16) as f32 * 0.25
    }
    fn exclusion(&mut self, _: i32, _: i32, _: i32) -> f32 {
        self.exclusion
    }
    fn floodedness(&mut self, _: i32, _: i32, _: i32) -> f32 {
        self.flooded
    }
    fn spread(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.spread + (((x * 7 + y * 11 + z * 13) & 7) - 3) as f32 * 0.03125
    }
    fn lava(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.lava + (((x * 3 + y * 5 + z * 7) & 7) - 3) as f32 * 0.03125
    }
}
fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "aquifer oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn center_fluid_matches_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("AquiferFluidOracle.java");
    std::fs::write(&source, include_str!("java/AquiferFluidOracle.java")).unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("AquiferFluidOracle"));
    let mut count = 0;
    let mut lava_count = 0;
    let mut dry_count = 0;
    let mut batch = None;
    let mut decisions = 0;
    for line in output.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.first() == Some(&"B") {
            let i = |n: usize| f[n].parse::<i32>().unwrap();
            let fl = |n: usize| f32::from_bits(i(n) as u32);
            batch = Some((
                Sampler {
                    base: i(1),
                    sea: i(2),
                    mode: i(3),
                    exclusion: fl(4),
                    flooded: fl(5),
                    spread: fl(6),
                    lava: fl(7),
                },
                AquiferGenerator::new(0, false, i(8)),
            ));
            continue;
        }
        if f.first() == Some(&"D") {
            let i = |n: usize| f[n].parse::<i32>().unwrap();
            let density = f64::from_bits(f[4].parse::<i64>().unwrap() as u64);
            let (sampler, generator) = batch.as_mut().unwrap();
            let actual = generator.sample(sampler, i(1), i(2), i(3), density);
            assert_eq!(
                (
                    actual.block.map_or(-1, |b| b.0),
                    actual.schedule_fluid_update
                ),
                (i(5), i(6) != 0),
                "decision: {line}"
            );
            decisions += 1;
            continue;
        }
        if f.first() != Some(&"F") {
            continue;
        }
        let i = |n: usize| f[n].parse::<i32>().unwrap();
        let fl = |n: usize| f32::from_bits(i(n) as u32);
        let mut sampler = Sampler {
            base: i(4),
            sea: i(5),
            mode: i(6),
            exclusion: fl(7),
            flooded: fl(8),
            spread: fl(9),
            lava: fl(10),
        };
        let actual = AquiferFluidLevels::new().compute(&mut sampler, i(1), i(2), i(3));
        assert_eq!(
            (actual.level, actual.block.0),
            (i(11), i(12)),
            "case: {line}"
        );
        lava_count += usize::from(actual.block.0 == 2);
        dry_count += usize::from(actual.level == native_core::aquifer::WAY_BELOW_MIN_Y);
        count += 1;
    }
    assert_eq!(count, 8000);
    assert_eq!(decisions, 2000);
    assert!(lava_count > 0 && dry_count > 0);
}
