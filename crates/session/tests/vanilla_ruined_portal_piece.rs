use anyhow::{Context, Result, ensure};
use native_persistence::nbt::{self, NamedTag, Tag, TagType};
use native_resources::{
    packs::{ResourceId, ResourceProvider},
    ruined_portal::{
        GenerationPoint, InitialSelection, PortalEnvironment, Setup, VerticalPlacement,
    },
    structure_start::{BiomeQuery, Heightmap, StartEnvironment},
    structure_transform::{Mirror, Rotation, bounding_box},
};
use native_session::worldgen::ruined_portal::{
    PortalStartEnvironment, PreparedStart, SavedRuinedPortalPiece,
};
use std::{path::PathBuf, process::Command};

fn fixtures() -> Result<Vec<SavedRuinedPortalPiece>> {
    let mut pieces = Vec::new();
    for placement in [
        VerticalPlacement::OnLandSurface,
        VerticalPlacement::PartlyBuried,
        VerticalPlacement::OnOceanFloor,
        VerticalPlacement::InMountain,
        VerticalPlacement::Underground,
        VerticalPlacement::InNether,
    ] {
        for rotation in [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::Counterclockwise90,
        ] {
            for mirror in [Mirror::None, Mirror::LeftRight, Mirror::FrontBack] {
                for flags in 0..64 {
                    let setup = Setup {
                        placement,
                        air_pocket_probability: 0.0,
                        mossiness: if flags & 32 != 0 { 0.8 } else { 0.2 },
                        overgrown: flags & 4 != 0,
                        vines: flags & 8 != 0,
                        can_be_cold: true,
                        replace_with_blackstone: flags & 16 != 0,
                        weight: 1.0,
                    };
                    let selection = InitialSelection {
                        setup_index: 0,
                        air_pocket: flags & 2 != 0,
                        template: ResourceId::parse("minecraft:ruined_portal/portal_1")?,
                        rotation,
                        mirror,
                    };
                    let position = [-37, 90, 52];
                    let size = [7, 4, 5];
                    let pivot = [3, 0, 2];
                    let point = GenerationPoint {
                        position,
                        pivot,
                        bounds: bounding_box(size, position, mirror, rotation, pivot)?,
                    };
                    pieces.push(SavedRuinedPortalPiece::capture(
                        &selection,
                        &setup,
                        &point,
                        size,
                        flags & 1 != 0,
                    )?);
                }
            }
        }
    }
    Ok(pieces)
}

#[test]
fn generated_portal_piece_round_trip_rejects_unpreserved_fields() -> Result<()> {
    let pieces = fixtures()?;
    for piece in &pieces {
        let tag = piece.to_tag()?;
        assert_eq!(&SavedRuinedPortalPiece::from_tag(&tag, [7, 4, 5])?, piece);
    }
    let Tag::Compound(mut fields) = pieces[0].to_tag()? else {
        unreachable!()
    };
    fields.push(("unknown".into(), Tag::Int(1)));
    assert!(SavedRuinedPortalPiece::from_tag(&Tag::Compound(fields), [7, 4, 5]).is_err());
    assert!(SavedRuinedPortalPiece::from_tag(&pieces[0].to_tag()?, [6, 4, 5]).is_err());
    let Tag::Compound(mut fields) = pieces[0].to_tag()? else {
        unreachable!()
    };
    let (_, Tag::Compound(properties)) = fields
        .iter_mut()
        .find(|(key, _)| key.matches("Properties"))
        .unwrap()
    else {
        unreachable!()
    };
    properties.push(("future".into(), Tag::Byte(1)));
    assert!(SavedRuinedPortalPiece::from_tag(&Tag::Compound(fields), [7, 4, 5]).is_err());
    Ok(())
}

#[test]
fn prepared_start_preserves_piece_and_checks_cold_only_after_biome() -> Result<()> {
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;
    struct Provider {
        template: Option<Vec<u8>>,
    }
    impl ResourceProvider for Provider {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            assert!(path.starts_with("data/minecraft/structure/ruined_portal/"));
            Ok(self.template.clone())
        }
    }
    struct Environment {
        allowed: bool,
        cold_calls: usize,
    }
    impl StartEnvironment for Environment {
        fn vertical_bounds(&self) -> (i32, i32) {
            (-64, 319)
        }
        fn sea_level(&self) -> i32 {
            63
        }
        fn first_occupied_height(&mut self, _: i32, _: i32, _: Heightmap) -> Result<i32> {
            Ok(70)
        }
        fn biome_allowed(&mut self, _: [i32; 3], query: BiomeQuery) -> Result<bool> {
            assert_eq!(query, BiomeQuery::Point);
            Ok(self.allowed)
        }
    }
    impl PortalEnvironment for Environment {
        fn heightmap_opaque(&mut self, _: [i32; 3], _: Heightmap) -> Result<bool> {
            Ok(true)
        }
    }
    impl PortalStartEnvironment for Environment {
        fn cold_enough_to_snow(
            &mut self,
            _: &mut impl ResourceProvider,
            _: [i32; 3],
        ) -> Result<bool> {
            self.cold_calls += 1;
            Ok(true)
        }
    }
    let list = |element_type, values| Tag::List {
        element_type,
        values,
    };
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            (
                "size".into(),
                list(TagType::Int, vec![Tag::Int(7), Tag::Int(4), Tag::Int(5)]),
            ),
            (
                "palette".into(),
                list(TagType::Compound, vec![Tag::Compound(vec![])]),
            ),
            ("blocks".into(), list(TagType::Compound, vec![])),
            ("entities".into(), list(TagType::Compound, vec![])),
        ]),
    };
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&nbt::write(&document)?)?;
    let mut provider = Provider {
        template: Some(encoder.finish()?),
    };
    let mut definition = serde_json::json!({"type":"minecraft:ruined_portal", "setups":[{
        "placement":"on_land_surface", "air_pocket_probability":1.0, "mossiness":0.2,
        "overgrown":false, "vines":true, "can_be_cold":true, "replace_with_blackstone":false, "weight":1.0
    }]});
    let mut environment = Environment {
        allowed: false,
        cold_calls: 0,
    };
    assert!(
        PreparedStart::prepare(&mut provider, &definition, &mut environment, 42, [0, 0])?.is_none()
    );
    assert_eq!(environment.cold_calls, 0);
    environment.allowed = true;
    let start =
        PreparedStart::prepare(&mut provider, &definition, &mut environment, 42, [0, 0])?.unwrap();
    assert_eq!(environment.cold_calls, 1);
    assert!(
        start.piece.properties.cold
            && start.piece.properties.air_pocket
            && start.piece.properties.vines
    );
    assert_eq!(
        SavedRuinedPortalPiece::from_tag(&start.piece.to_tag()?, start.template.size)?,
        start.piece
    );
    assert_eq!(
        start
            .reference_bounds(ResourceId::parse("minecraft:ruined_portal")?)?
            .bounds,
        start.piece.bounds
    );
    definition["setups"][0]["can_be_cold"] = false.into();
    let start =
        PreparedStart::prepare(&mut provider, &definition, &mut environment, 42, [0, 0])?.unwrap();
    assert!(!start.piece.properties.cold);
    assert_eq!(environment.cold_calls, 1);
    provider.template = None;
    assert!(
        PreparedStart::prepare(&mut provider, &definition, &mut environment, 42, [0, 0]).is_err()
    );
    Ok(())
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn generated_portal_piece_nbt_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("RuinedPortalSavedPieceOracle.java");
    std::fs::write(
        &source,
        include_str!("java/RuinedPortalSavedPieceOracle.java"),
    )?;
    let compile = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .output()?;
    ensure!(
        compile.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let pieces = fixtures()?;
    let root = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "pieces".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: pieces
                    .iter()
                    .map(SavedRuinedPortalPiece::to_tag)
                    .collect::<Result<_>>()?,
            },
        )]),
    };
    let rust_path = temp.path().join("rust.nbt");
    let java_path = temp.path().join("java.nbt");
    std::fs::write(&rust_path, nbt::write(&root)?)?;
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("RuinedPortalSavedPieceOracle")
        .arg(&java_path)
        .arg(&rust_path)
        .output()?;
    ensure!(
        output.status.success(),
        "Java comparison failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let decoded = nbt::read(&std::fs::read(java_path)?)?;
    let Some(Tag::List { values, .. }) = decoded.value.get("pieces") else {
        anyhow::bail!("Java pieces missing")
    };
    ensure!(values.len() == pieces.len(), "Java piece count differs");
    for (tag, piece) in values.iter().zip(pieces) {
        assert_eq!(SavedRuinedPortalPiece::from_tag(tag, [7, 4, 5])?, piece);
    }
    Ok(())
}
