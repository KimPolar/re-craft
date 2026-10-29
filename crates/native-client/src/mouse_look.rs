//! Default Java 26.3 mouse sensitivity for ordinary first-person movement.

const DEFAULT_SENSITIVITY: f64 = 0.5;

fn degrees(delta: f64) -> f32 {
    // MouseHandler promotes these float literals to double before computing
    // the gain; Entity.turn then converts the result to float and multiplies.
    let value = DEFAULT_SENSITIVITY * f64::from(0.6_f32) + f64::from(0.2_f32);
    let gain = value * value * value * 8.0;
    (delta * gain) as f32 * 0.15_f32
}

pub(crate) fn look_delta(delta: (f64, f64)) -> (f32, f32) {
    (
        degrees(delta.0).to_radians(),
        -degrees(delta.1).to_radians(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mouse_motion_has_expected_signs_and_scale() {
        let (yaw, pitch) = look_delta((10.0, 10.0));
        assert!((yaw.to_degrees() - 1.5).abs() < 1e-5);
        assert!((pitch.to_degrees() + 1.5).abs() < 1e-5);
        assert_eq!(look_delta((0.0, 0.0)), (0.0, -0.0));
    }

    #[test]
    #[ignore = "requires the installed Minecraft 26.3 and Java 25 JDK"]
    fn default_mouse_turn_matches_installed_java() {
        use std::{ffi::OsString, path::PathBuf, process::Command};
        const JAVA: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import net.minecraft.client.*;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.client.tutorial.Tutorial;
import sun.misc.Unsafe;

public class MouseOracle {
    static class TestPlayer extends LocalPlayer {
        TestPlayer() { super(null, null, null, null, null, null, false, null, null); }
        @Override public boolean isScoping() { return false; }
    }
    static Unsafe unsafe;
    static void set(Class<?> type, Object obj, String name, Object value) throws Exception {
        var field = type.getDeclaredField(name); field.setAccessible(true); field.set(obj, value);
    }
    static OptionInstance<?> option(Object value) throws Exception {
        var option = (OptionInstance<?>) unsafe.allocateInstance(OptionInstance.class);
        set(OptionInstance.class, option, "value", value); return option;
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f = Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); unsafe = (Unsafe)f.get(null);
        var client = (Minecraft) unsafe.allocateInstance(Minecraft.class);
        var options = (Options) unsafe.allocateInstance(Options.class);
        options.setCameraType(CameraType.FIRST_PERSON);
        set(Options.class, options, "sensitivity", option(Double.valueOf(0.5)));
        set(Options.class, options, "invertXMouse", option(Boolean.FALSE));
        set(Options.class, options, "invertYMouse", option(Boolean.FALSE));
        set(Minecraft.class, client, "options", options);
        set(Minecraft.class, client, "tutorial", unsafe.allocateInstance(Tutorial.class));
        var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        client.player = player;
        var mouse = new MouseHandler(client);
        var turn = MouseHandler.class.getDeclaredMethod("turnPlayer", double.class); turn.setAccessible(true);
        double[] values = {-128, -10, -0.01, 0, 0.125, 1, 3.5, 17.5, 100, 2048};
        var out = new StringBuilder();
        for (double x : values) for (double y : values) {
            player.setYRot(0); player.setXRot(0);
            set(MouseHandler.class, mouse, "accumulatedDX", x);
            set(MouseHandler.class, mouse, "accumulatedDY", y);
            turn.invoke(mouse, 0.05);
            out.append(Float.floatToIntBits(player.getYRot())).append(' ')
               .append(Float.floatToIntBits(player.getXRot())).append('\n');
        }
        Files.writeString(Path.of(args[0]), out);
    }
}
"#;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("MouseOracle.java");
        std::fs::write(&source, JAVA).unwrap();
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath: OsString = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        let output = temp.path().join("results.txt");
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("MouseOracle")
            .arg(&output));
        let data = std::fs::read_to_string(output).unwrap();
        let values = [
            -128.0, -10.0, -0.01, 0.0, 0.125, 1.0, 3.5, 17.5, 100.0, 2048.0,
        ];
        let mut lines = data.lines();
        for x in values {
            for y in values {
                let bits: Vec<i32> = lines
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .map(|v| v.parse().unwrap())
                    .collect();
                assert_eq!(degrees(x).to_bits(), bits[0] as u32, "yaw {x}");
                assert_eq!(
                    degrees(y).clamp(-90.0, 90.0).to_bits(),
                    bits[1] as u32,
                    "pitch {y}"
                );
                let (yaw, pitch) = look_delta((x, y));
                assert_eq!(yaw, f32::from_bits(bits[0] as u32).to_radians());
                assert_eq!(
                    pitch.clamp(-native_session::MAX_PITCH, native_session::MAX_PITCH),
                    -f32::from_bits(bits[1] as u32).to_radians()
                );
            }
        }
        assert!(lines.next().is_none());
    }
}
