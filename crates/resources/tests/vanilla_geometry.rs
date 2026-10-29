//! Compares cuboid corners and UVs with classes from the locally installed 26.3 client.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_resources::{
    blockstates::{BlockstateDefinition, ModelChoice, positional_seed},
    default_minecraft_dir,
    geometry::{self, Direction},
    models::ResolvedModel,
    packs::ResourceId,
};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.util.Locale;
import java.util.ArrayList;
import org.joml.Vector3f;
import org.joml.Vector3fc;
import net.minecraft.core.Direction;
import net.minecraft.client.renderer.FaceInfo;
import net.minecraft.client.resources.model.cuboid.CuboidFace;
import net.minecraft.client.resources.model.cuboid.FaceBakery;
import net.minecraft.client.resources.model.cuboid.CuboidRotation;
import net.minecraft.client.renderer.block.dispatch.BlockModelRotation;
import com.mojang.math.Quadrant;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;
import net.minecraft.util.random.WeightedList;

public class GeometryOracle {
    public static void main(String[] args) throws Exception {
        var from = new Vector3f(1, 2, 3);
        var to = new Vector3f(13, 14, 15);
        if (args.length > 0 && args[0].equals("elements")) {
            var rotations = new ArrayList<CuboidRotation>();
            var origin = new Vector3f(5f / 16f, 10f / 16f, 7f / 16f);
            for (Direction.Axis axis : Direction.Axis.values())
                for (float angle : new float[]{-67.5f, -45f, -22.5f, 0f, 22.5f, 45f, 67.5f, 90f})
                    for (boolean scale : new boolean[]{false, true})
                        rotations.add(new CuboidRotation(origin, new CuboidRotation.SingleAxisRotation(axis, angle), scale));
            for (float[] angles : new float[][]{{180f, -67.5f, -180f}, {35f, -27f, 61f}, {0f, 0f, 0f}, {-23f, 90f, 15f}})
                for (boolean scale : new boolean[]{false, true})
                    rotations.add(new CuboidRotation(origin, new CuboidRotation.EulerXYZRotation(angles[0], angles[1], angles[2]), scale));
            var findFacing = FaceBakery.class.getDeclaredMethod("calculateFacing", Vector3fc[].class);
            findFacing.setAccessible(true);
            var uvs = new CuboidFace.UVs(-2, 3, 14, 20);
            var xs = new Quadrant[]{Quadrant.R0, Quadrant.R90, Quadrant.R0, Quadrant.R90};
            var ys = new Quadrant[]{Quadrant.R0, Quadrant.R0, Quadrant.R90, Quadrant.R270};
            for (int element = 0; element < rotations.size(); element++) for (int variant = 0; variant < xs.length; variant++) {
                var group = Quadrant.fromXYAngles(xs[variant], ys[variant]);
                var state = BlockModelRotation.get(group);
                for (boolean lock : new boolean[]{false, true}) {
                    var mapping = lock ? state.withUvLock() : state;
                    for (Direction direction : Direction.values()) {
                        Vector3fc[] points = new Vector3fc[4];
                        for (int vertex = 0; vertex < 4; vertex++) {
                            var p = FaceInfo.fromFacing(direction).getVertexInfo(vertex).select(from, to).div(16f).sub(origin);
                            rotations.get(element).transform().transformPosition(p);
                            p.add(origin).sub(0.5f, 0.5f, 0.5f);
                            state.transformation().getMatrix().transformPosition(p);
                            points[vertex] = p.add(0.5f, 0.5f, 0.5f);
                        }
                        var face = (Direction)findFacing.invoke(null, (Object)points);
                        if (face == null) face = Direction.UP;
                        for (int vertex = 0; vertex < 4; vertex++) {
                            float u = CuboidFace.getU(uvs, Quadrant.R90, vertex);
                            float v = CuboidFace.getV(uvs, Quadrant.R90, vertex);
                            var mapped = mapping.inverseFaceTransformation(direction).transformPosition(new Vector3f(u - 0.5f, v - 0.5f, 0));
                            var p = points[vertex];
                            System.out.printf(Locale.ROOT, "%d %d %d %s %s %d %.9f %.9f %.9f %.9f %.9f%n", element, variant,
                                lock ? 1 : 0, direction.getName(), face.getName(), vertex, p.x(), p.y(), p.z(), mapped.x + 0.5f, mapped.y + 0.5f);
                        }
                    }
                }
            }
            return;
        }
        if (args.length > 0 && args[0].equals("selection")) {
            var a = WeightedList.<Integer>builder().add(0, 1).add(1, 2).add(2, 3).build();
            var b = WeightedList.<Integer>builder().add(0, 1).add(1, 64).add(2, 13).build();
            for (int i = -128; i <= 128; i++) {
                int x = i * 100003, y = i * 37, z = i * -10000019;
                long seed = Mth.getSeed(x, y, z);
                var random = RandomSource.createThreadLocalInstance(seed);
                int variant = a.getRandomOrThrow(random);
                random.setSeed(seed);
                long multipartSeed = random.nextLong();
                random.setSeed(multipartSeed);
                int partA = a.getRandomOrThrow(random);
                random.setSeed(multipartSeed);
                int partB = b.getRandomOrThrow(random);
                System.out.printf(Locale.ROOT, "%d %d %d %d %d %d %d%n", x, y, z, seed, variant, partA, partB);
            }
            return;
        }
        if (args.length > 0 && args[0].equals("rotations")) {
            var uvs = new CuboidFace.UVs(-2, 3, 14, 20);
            for (Quadrant x : Quadrant.values()) for (Quadrant y : Quadrant.values()) {
                var group = Quadrant.fromXYAngles(x, y);
                var state = BlockModelRotation.get(group);
                for (boolean lock : new boolean[]{false, true}) {
                    var mapping = lock ? state.withUvLock() : state;
                    for (Direction direction : Direction.values()) for (int vertex = 0; vertex < 4; vertex++) {
                        var p = FaceInfo.fromFacing(direction).getVertexInfo(vertex).select(from, to).div(16f).sub(0.5f, 0.5f, 0.5f);
                        state.transformation().getMatrix().transformPosition(p);
                        p.add(0.5f, 0.5f, 0.5f);
                        float u = CuboidFace.getU(uvs, Quadrant.R90, vertex);
                        float v = CuboidFace.getV(uvs, Quadrant.R90, vertex);
                        var mapped = mapping.inverseFaceTransformation(direction).transformPosition(new Vector3f(u - 0.5f, v - 0.5f, 0));
                        System.out.printf(Locale.ROOT, "%d %d %d %s %s %d %.9f %.9f %.9f %.9f %.9f%n",
                            x.shift, y.shift, lock ? 1 : 0, direction.getName(), group.rotate(direction).getName(), vertex,
                            p.x, p.y, p.z, mapped.x + 0.5f, mapped.y + 0.5f);
                    }
                }
            }
            return;
        }
        var defaultUV = FaceBakery.class.getDeclaredMethod("defaultFaceUV", Vector3fc.class, Vector3fc.class, Direction.class);
        defaultUV.setAccessible(true);
        for (int mode = 0; mode < 2; mode++) {
            for (Quadrant rotation : Quadrant.values()) {
                for (Direction direction : Direction.values()) {
                    var uv = mode == 0 ? (CuboidFace.UVs)defaultUV.invoke(null, from, to, direction)
                        : new CuboidFace.UVs(-2, 3, 14, 20);
                    for (int vertex = 0; vertex < 4; vertex++) {
                        var p = FaceInfo.fromFacing(direction).getVertexInfo(vertex).select(from, to);
                        System.out.printf(Locale.ROOT, "%d %d %s %d %.9f %.9f %.9f %.9f %.9f%n",
                            mode, rotation.shift, direction.getName(), vertex, p.x / 16f, p.y / 16f, p.z / 16f,
                            CuboidFace.getU(uv, rotation, vertex), CuboidFace.getV(uv, rotation, vertex));
                    }
                }
            }
        }
    }
}
"#;

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to a Java 25 JDK"))
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn run(command: &mut Command) -> String {
    let output = command.output().expect("could not run Java 25");
    assert!(
        output.status.success(),
        "Java geometry oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn cuboid_corners_and_uvs_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_minecraft_dir().unwrap());
    let java = java_bin();
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("GeometryOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    let cp = classpath(&game, temporary.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeometryOracle"));

    let directions = [
        ("down", Direction::Down),
        ("up", Direction::Up),
        ("north", Direction::North),
        ("south", Direction::South),
        ("west", Direction::West),
        ("east", Direction::East),
    ];
    let mut baked = Vec::new();
    for mode in 0..2 {
        for rotation in 0..4 {
            let mut faces = serde_json::Map::new();
            for (name, _) in directions {
                let mut face = json!({"texture":"#all", "rotation":rotation * 90});
                if mode == 1 {
                    face["uv"] = json!([-2, 3, 14, 20]);
                }
                faces.insert(name.to_string(), face);
            }
            let input = json!({"textures":{"all":"block/stone"}, "elements":[{"from":[1,2,3],"to":[13,14,15],"faces":faces}]});
            let model = ResolvedModel {
                fields: input.as_object().unwrap().clone(),
                generated: false,
            };
            baked.push(geometry::bake_untransformed(&model).unwrap());
        }
    }

    let mut checked = 0;
    for line in output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 9, "unexpected oracle output: {line}");
        let mode: usize = parts[0].parse().unwrap();
        let rotation: usize = parts[1].parse().unwrap();
        let direction = directions
            .iter()
            .find(|(name, _)| *name == parts[2])
            .unwrap()
            .1;
        let vertex: usize = parts[3].parse().unwrap();
        let quad = baked[mode * 4 + rotation]
            .quads
            .iter()
            .find(|quad| quad.direction == direction)
            .unwrap();
        let actual = quad.vertices[vertex];
        for (left, expected) in actual
            .position
            .into_iter()
            .chain(actual.uv)
            .zip(&parts[4..])
        {
            let right: f32 = expected.parse().unwrap();
            assert!(
                (left - right).abs() < 1e-6,
                "{line}: Rust value {left} differs from Java {right}"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 2 * 4 * 6 * 4);

    let mut faces = serde_json::Map::new();
    for (name, _) in directions {
        faces.insert(
            name.to_string(),
            json!({"texture":"#all", "rotation":90, "uv":[-2,3,14,20], "cullface":name}),
        );
    }
    let input = json!({"textures":{"all":"block/stone"}, "elements":[{"from":[1,2,3],"to":[13,14,15],"faces":faces}]});
    let model = ResolvedModel {
        fields: input.as_object().unwrap().clone(),
        generated: false,
    };
    let mut rotations = Vec::new();
    for x in 0..4 {
        for y in 0..4 {
            for lock in [false, true] {
                rotations.push(
                    geometry::bake(
                        &model,
                        &ModelChoice {
                            model: ResourceId::parse("block/stone").unwrap(),
                            x_rotation: x * 90,
                            y_rotation: y * 90,
                            uv_lock: lock,
                            weight: 1,
                        },
                    )
                    .unwrap(),
                );
            }
        }
    }
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeometryOracle")
        .arg("rotations"));
    let mut checked = 0;
    for line in output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 11, "unexpected oracle output: {line}");
        let x: usize = parts[0].parse().unwrap();
        let y: usize = parts[1].parse().unwrap();
        let lock: usize = parts[2].parse().unwrap();
        let direction = directions
            .iter()
            .find(|(name, _)| *name == parts[4])
            .unwrap()
            .1;
        let vertex: usize = parts[5].parse().unwrap();
        let quad = rotations[(x * 4 + y) * 2 + lock]
            .quads
            .iter()
            .find(|quad| quad.direction == direction)
            .unwrap();
        assert_eq!(quad.cullface, Some(direction));
        let actual = quad.vertices[vertex];
        for (left, expected) in actual
            .position
            .into_iter()
            .chain(actual.uv)
            .zip(&parts[6..])
        {
            let right: f32 = expected.parse().unwrap();
            assert!(
                (left - right).abs() < 1e-6,
                "{line}: Rust value {left} differs from Java {right}"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 4 * 4 * 2 * 6 * 4);

    let mut elements = Vec::new();
    for axis in ["x", "y", "z"] {
        for angle in [-67.5, -45.0, -22.5, 0.0, 22.5, 45.0, 67.5, 90.0] {
            for rescale in [false, true] {
                elements.push(
                    json!({"origin":[5,10,7], "axis":axis, "angle":angle, "rescale":rescale}),
                );
            }
        }
    }
    for angles in [
        [180.0, -67.5, -180.0],
        [35.0, -27.0, 61.0],
        [0.0, 0.0, 0.0],
        [-23.0, 90.0, 15.0],
    ] {
        for rescale in [false, true] {
            elements.push(json!({"origin":[5,10,7], "x":angles[0], "y":angles[1], "z":angles[2], "rescale":rescale}));
        }
    }
    let mut all_models = Vec::new();
    for rotation in elements {
        let mut input = input.clone();
        input["elements"][0]["rotation"] = rotation;
        let model = ResolvedModel {
            fields: input.as_object().unwrap().clone(),
            generated: false,
        };
        for (x, y) in [(0, 0), (90, 0), (0, 90), (90, 270)] {
            for lock in [false, true] {
                all_models.push(
                    geometry::bake(
                        &model,
                        &ModelChoice {
                            model: ResourceId::parse("block/stone").unwrap(),
                            x_rotation: x,
                            y_rotation: y,
                            uv_lock: lock,
                            weight: 1,
                        },
                    )
                    .unwrap(),
                );
            }
        }
    }
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeometryOracle")
        .arg("elements"));
    let mut checked = 0;
    for line in output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 11, "unexpected oracle output: {line}");
        let element: usize = parts[0].parse().unwrap();
        let variant: usize = parts[1].parse().unwrap();
        let lock: usize = parts[2].parse().unwrap();
        let source = directions
            .iter()
            .find(|(name, _)| *name == parts[3])
            .unwrap()
            .1;
        let expected_face = directions
            .iter()
            .find(|(name, _)| *name == parts[4])
            .unwrap()
            .1;
        let vertex: usize = parts[5].parse().unwrap();
        let quad = all_models[(element * 4 + variant) * 2 + lock]
            .quads
            .iter()
            .find(|quad| quad.source_face == source)
            .unwrap();
        assert_eq!(
            quad.direction, expected_face,
            "direction mismatch for {line}"
        );
        let actual = quad.vertices[vertex];
        for (left, expected) in actual
            .position
            .into_iter()
            .chain(actual.uv)
            .zip(&parts[6..])
        {
            let right: f32 = expected.parse().unwrap();
            assert!(
                (left - right).abs() < 3e-6,
                "{line}: Rust value {left} differs from Java {right}"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, (3 * 8 * 2 + 4 * 2) * 4 * 2 * 6 * 4);

    let make_group = |weights: &[u32]| {
        weights
            .iter()
            .map(|weight| ModelChoice {
                model: ResourceId::parse("block/stone").unwrap(),
                x_rotation: 0,
                y_rotation: 0,
                uv_lock: false,
                weight: *weight,
            })
            .collect::<Vec<_>>()
    };
    let variant = BlockstateDefinition {
        multipart: false,
        groups: vec![make_group(&[1, 2, 3])],
    };
    let multipart = BlockstateDefinition {
        multipart: true,
        groups: vec![make_group(&[1, 2, 3]), make_group(&[1, 64, 13])],
    };
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeometryOracle")
        .arg("selection"));
    let mut checked = 0;
    for line in output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 7, "unexpected oracle output: {line}");
        let x: i32 = parts[0].parse().unwrap();
        let y: i32 = parts[1].parse().unwrap();
        let z: i32 = parts[2].parse().unwrap();
        let seed: i64 = parts[3].parse().unwrap();
        assert_eq!(positional_seed(x, y, z), seed);
        assert_eq!(
            variant.select_indices(seed).unwrap(),
            vec![parts[4].parse::<usize>().unwrap()]
        );
        assert_eq!(
            multipart.select_indices(seed).unwrap(),
            vec![
                parts[5].parse::<usize>().unwrap(),
                parts[6].parse::<usize>().unwrap()
            ]
        );
        checked += 1;
    }
    assert_eq!(checked, 257);
}
