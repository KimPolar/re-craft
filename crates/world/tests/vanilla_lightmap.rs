//! Compare our light map with the installed 26.3 fragment shader on an actual GPU.
//! The shader itself is read from the user's JAR and is never stored in this project.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_world::lighting::LightmapSettings;

const ORACLE: &str = r##"
import java.io.*;
import java.nio.*;
import java.nio.file.*;
import java.util.zip.*;
import org.lwjgl.BufferUtils;
import org.lwjgl.opengl.GL;
import static org.lwjgl.opengl.GL33C.*;
import static org.lwjgl.sdl.SDLInit.*;
import static org.lwjgl.sdl.SDLVideo.*;
import static org.lwjgl.sdl.SDLError.*;

public class LightmapOracle {
    static int compile(int type, String source) {
        int shader = glCreateShader(type);
        glShaderSource(shader, source);
        glCompileShader(shader);
        if (glGetShaderi(shader, GL_COMPILE_STATUS) == 0) throw new AssertionError(glGetShaderInfoLog(shader));
        return shader;
    }
    public static void main(String[] args) throws Exception {
        if (!SDL_Init(SDL_INIT_VIDEO)) throw new AssertionError(SDL_GetError());
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_MAJOR_VERSION, 3);
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_MINOR_VERSION, 3);
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_PROFILE_MASK, SDL_GL_CONTEXT_PROFILE_CORE);
        long window = SDL_CreateWindow("Lightmap parity", 16, 16, SDL_WINDOW_OPENGL | SDL_WINDOW_HIDDEN);
        if (window == 0) throw new AssertionError(SDL_GetError());
        long context = SDL_GL_CreateContext(window);
        if (context == 0 || !SDL_GL_MakeCurrent(window, context)) throw new AssertionError(SDL_GetError());
        GL.createCapabilities();
        String fragment;
        try (var zip = new ZipFile(args[0])) {
            fragment = new String(zip.getInputStream(zip.getEntry("assets/minecraft/shaders/core/lightmap.fsh")).readAllBytes(), java.nio.charset.StandardCharsets.UTF_8);
        }
        String vertex = "#version 330\nlayout(location=0) out vec2 texCoord; void main() { vec2 p=vec2(float((gl_VertexID<<1)&2),float(gl_VertexID&2)); texCoord=p; gl_Position=vec4(p*2.0-1.0,0,1); }";
        int program = glCreateProgram();
        glAttachShader(program, compile(GL_VERTEX_SHADER, vertex));
        glAttachShader(program, compile(GL_FRAGMENT_SHADER, fragment));
        glLinkProgram(program);
        if (glGetProgrami(program, GL_LINK_STATUS) == 0) throw new AssertionError(glGetProgramInfoLog(program));
        glUseProgram(program);
        glBindVertexArray(glGenVertexArrays());
        int texture = glGenTextures();
        glBindTexture(GL_TEXTURE_2D, texture);
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA32F, 16, 16, 0, GL_RGBA, GL_FLOAT, (ByteBuffer)null);
        int framebuffer = glGenFramebuffers();
        glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, texture, 0);
        if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) throw new AssertionError("framebuffer incomplete");
        glViewport(0, 0, 16, 16);
        glDisable(GL_DITHER);
        int buffer = glGenBuffers();
        glBindBuffer(GL_UNIFORM_BUFFER, buffer);
        glBufferData(GL_UNIFORM_BUFFER, 96, GL_DYNAMIC_DRAW);
        glBindBufferBase(GL_UNIFORM_BUFFER, 0, buffer);
        glUniformBlockBinding(program, glGetUniformBlockIndex(program, "LightmapInfo"), 0);
        try (var in = new DataInputStream(Files.newInputStream(Path.of(args[1])));
             var out = new DataOutputStream(Files.newOutputStream(Path.of(args[2])))) {
            int count = in.readInt();
            for (int test = 0; test < count; test++) {
                ByteBuffer uniform = BufferUtils.createByteBuffer(96);
                for (int i = 0; i < 6; i++) uniform.putFloat(i * 4, in.readFloat());
                for (int i = 0; i < 4; i++) for (int j = 0; j < 3; j++) uniform.putFloat(32 + i * 16 + j * 4, in.readFloat());
                glBufferSubData(GL_UNIFORM_BUFFER, 0, uniform);
                glDrawArrays(GL_TRIANGLES, 0, 3);
                FloatBuffer pixels = BufferUtils.createFloatBuffer(16 * 16 * 4);
                glReadPixels(0, 0, 16, 16, GL_RGBA, GL_FLOAT, pixels);
                for (int i = 0; i < pixels.capacity(); i++) out.writeFloat(pixels.get(i));
            }
        }
        SDL_GL_DestroyContext(context);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}
"##;

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to Java 25 JDK"))
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
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

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "light map oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3, Java 25 JDK, an OpenGL 3.3 device and a desktop"]
fn lightmap_matches_the_local_vanilla_shader() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified_client = native_resources::packs::ResourceStack::open(&game)
        .expect("Minecraft 26.3 client must pass version and checksum checks");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("LightmapOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let base = LightmapSettings::default();
    let cases = [
        base.clone(),
        LightmapSettings {
            sky_factor: 0.24,
            sky_color: [0.48, 0.48, 1.0],
            brightness: 0.5,
            ..base.clone()
        },
        LightmapSettings {
            sky_factor: 0.0,
            ambient_color: [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0],
            ..base.clone()
        },
        LightmapSettings {
            sky_factor: 0.0,
            ambient_color: [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0],
            ..base.clone()
        },
        LightmapSettings {
            block_factor: 1.2,
            night_vision_factor: 0.3,
            darkness_scale: 0.01,
            boss_darkening: 0.35,
            brightness: 0.8,
            ambient_color: [0.1, 0.15, 0.2],
            ..base
        },
    ];
    let mut input = (cases.len() as i32).to_be_bytes().to_vec();
    for case in &cases {
        for value in [
            case.sky_factor,
            case.block_factor,
            case.night_vision_factor,
            case.darkness_scale,
            case.boss_darkening,
            case.brightness,
        ]
        .into_iter()
        .chain(case.block_tint)
        .chain(case.sky_color)
        .chain(case.ambient_color)
        .chain(case.night_vision_color)
        {
            input.extend_from_slice(&value.to_be_bytes());
        }
    }
    let input_path = temp.path().join("input.bin");
    let output_path = temp.path().join("output.bin");
    std::fs::write(&input_path, input).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("LightmapOracle")
        .arg(game.join("versions/26.3/26.3.jar"))
        .arg(input_path)
        .arg(&output_path));
    let bytes = std::fs::read(output_path).unwrap();
    assert_eq!(bytes.len(), cases.len() * 16 * 16 * 4 * 4);
    for (scenario, case) in cases.iter().enumerate() {
        for sky in 0..16usize {
            for block in 0..16usize {
                let expected = case.sample(block as u8, sky as u8);
                for (channel, expected) in expected.into_iter().enumerate() {
                    let offset = ((scenario * 256 + sky * 16 + block) * 4 + channel) * 4;
                    let actual = f32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
                    assert!(
                        (actual - expected).abs() < 0.000_01,
                        "case {scenario}, block {block}, sky {sky}, channel {channel}: GPU {actual} != Rust {expected}"
                    );
                }
            }
        }
    }
}
