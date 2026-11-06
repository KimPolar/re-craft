//! Compare fresh piece NBT with tags produced by the installed 26.3 client.
use native_persistence::nbt::{self, Tag};
use native_resources::{
    mineshaft::MineshaftVariant,
    packs::ResourceId,
    structure_start::{BiomeQuery, Heightmap, StartEnvironment},
};
use native_session::worldgen::{
    SavedMineshaftPiece,
    mineshaft::{MineshaftEnvironment, PreparedStart},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

struct Environment;
impl StartEnvironment for Environment {
    fn vertical_bounds(&self) -> (i32, i32) {
        (-64, 319)
    }
    fn sea_level(&self) -> i32 {
        63
    }
    fn first_occupied_height(&mut self, _: i32, _: i32, map: Heightmap) -> anyhow::Result<i32> {
        assert_eq!(map, Heightmap::WorldSurface);
        Ok(79)
    }
    fn biome_allowed(&mut self, _: [i32; 3], query: BiomeQuery) -> anyhow::Result<bool> {
        assert_eq!(query, BiomeQuery::Point);
        Ok(true)
    }
}
impl MineshaftEnvironment for Environment {
    fn generator_min_y(&self) -> i32 {
        -64
    }
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn canonical(tag: &Tag) -> Tag {
    match tag {
        Tag::Compound(fields) => {
            let mut fields: Vec<_> = fields
                .iter()
                .map(|(name, value)| (name.clone(), canonical(value)))
                .collect();
            fields.sort_by(|a, b| a.0.0.cmp(&b.0.0));
            Tag::Compound(fields)
        }
        Tag::List {
            element_type,
            values,
        } => Tag::List {
            element_type: *element_type,
            values: values.iter().map(canonical).collect(),
        },
        value => value.clone(),
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn fresh_mineshaft_piece_tags_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("MineshaftPieceOracle.java");
    std::fs::write(&source, include_str!("java/MineshaftPieceOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(source));
    let output = temp.path().join("pieces.nbt");
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("MineshaftPieceOracle")
        .arg(&output));
    let root = nbt::read(&std::fs::read(output).unwrap()).unwrap();
    let Some(Tag::List { values: graphs, .. }) = root.value.get("graphs") else {
        panic!("missing graphs")
    };
    assert_eq!(graphs.len(), 56);
    let mut piece_count = 0;
    for graph in graphs {
        let Some(Tag::Long(seed)) = graph.get("seed") else {
            panic!("seed")
        };
        let Some(Tag::Int(x)) = graph.get("x") else {
            panic!("x")
        };
        let Some(Tag::Int(z)) = graph.get("z") else {
            panic!("z")
        };
        let Some(Tag::String(name)) = graph.get("variant") else {
            panic!("variant")
        };
        let variant = if name.matches("NORMAL") {
            MineshaftVariant::Normal
        } else {
            assert!(name.matches("MESA"));
            MineshaftVariant::Mesa
        };
        let definition = serde_json::json!({"type":"minecraft:mineshaft", "mineshaft_type": if variant == MineshaftVariant::Normal { "normal" } else { "mesa" }});
        let start = PreparedStart::generate(&definition, &mut Environment, *seed, [*x, *z])
            .unwrap()
            .unwrap();
        let Some(Tag::IntArray(point)) = graph.get("point") else {
            panic!("point")
        };
        assert_eq!(start.generation_point.as_slice(), point);
        let Some(Tag::List {
            values: expected, ..
        }) = graph.get("pieces")
        else {
            panic!("pieces")
        };
        assert_eq!(start.pieces.len(), expected.len());
        let tag = start
            .to_tag(&ResourceId::parse("minecraft:mineshaft").unwrap())
            .unwrap();
        let Some(Tag::List { values: packed, .. }) = tag.get("Children") else {
            panic!("children")
        };
        assert_eq!(packed.len(), expected.len());
        for (index, ((saved, actual), expected)) in
            start.pieces.iter().zip(packed).zip(expected).enumerate()
        {
            assert_eq!(
                canonical(actual),
                canonical(expected),
                "seed {seed} chunk {x},{z}, {variant:?} piece {index}"
            );
            assert_eq!(
                &SavedMineshaftPiece::from_tag(expected).unwrap(),
                saved,
                "seed {seed} chunk {x},{z}, {variant:?} piece {index}"
            );
            piece_count += 1;
        }
    }
    assert!(piece_count > 1000, "only {piece_count} pieces checked");
    println!(
        "verified {piece_count} mineshaft pieces across {} graphs",
        graphs.len()
    );
}
