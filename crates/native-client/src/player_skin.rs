//! Resolve vanilla's offline default player skin from the active resource stack.

use std::io::Cursor;

use anyhow::{Context, Result, ensure};
use image::{ImageFormat, ImageReader, RgbaImage};
use native_resources::packs::ResourceProvider;

const NAMES: [&str; 9] = [
    "alex", "ari", "efe", "kai", "makena", "noor", "steve", "sunny", "zuri",
];

pub struct PlayerSkin {
    pub image: RgbaImage,
    pub slim: bool,
}

/// The UUID hash and floor modulus used by 26.3's DefaultPlayerSkin.get.
fn skin_index(uuid: Option<&str>) -> usize {
    let Some(uuid) = uuid else { return 6 };
    let mut digits = String::with_capacity(32);
    for byte in uuid.bytes() {
        if byte != b'-' {
            digits.push(char::from(byte));
        }
    }
    if digits.len() != 32 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return 6;
    }
    let mut hash = 0_u32;
    for start in [0, 8, 16, 24] {
        let Ok(value) = u32::from_str_radix(&digits[start..start + 8], 16) else {
            return 6;
        };
        hash ^= value;
    }
    (hash as i32).rem_euclid(18) as usize
}

impl PlayerSkin {
    pub fn load(provider: &mut impl ResourceProvider, uuid: Option<&str>) -> Result<Self> {
        let index = skin_index(uuid);
        let slim = index < 9;
        let folder = if slim { "slim" } else { "wide" };
        let path = format!(
            "assets/minecraft/textures/entity/player/{folder}/{}.png",
            NAMES[index % 9]
        );
        let bytes = provider
            .read_asset(&path)?
            .with_context(|| format!("missing player skin {path}"))?;
        let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let image = reader
            .decode()
            .with_context(|| format!("invalid player skin {path}"))?
            .to_rgba8();
        ensure!(
            image.width() >= 64 && image.width() == image.height() && image.width() % 64 == 0,
            "invalid player skin dimensions in {path}"
        );
        Ok(Self { image, slim })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_uses_signed_java_hash_and_default_fallback() {
        assert_eq!(skin_index(None), 6);
        assert_eq!(skin_index(Some("00000000-0000-0000-0000-000000000000")), 0);
        assert_eq!(skin_index(Some("ffffffff-0000-0000-0000-000000000000")), 17);
        assert_eq!(skin_index(Some("not-a-uuid")), 6);
    }

    #[test]
    fn skin_reads_the_selected_resource_and_rejects_invalid_dimensions() {
        struct Source {
            bytes: Vec<u8>,
            requested: String,
        }
        impl ResourceProvider for Source {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.requested = path.to_owned();
                Ok(Some(self.bytes.clone()))
            }
        }
        let encode = |width, height| {
            let image = image::RgbaImage::from_pixel(width, height, image::Rgba([12, 34, 56, 255]));
            let mut bytes = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut bytes, ImageFormat::Png)
                .unwrap();
            bytes.into_inner()
        };
        let mut source = Source {
            bytes: encode(64, 64),
            requested: String::new(),
        };
        let skin =
            PlayerSkin::load(&mut source, Some("00000000-0000-0000-0000-000000000000")).unwrap();
        assert!(skin.slim);
        assert!(source.requested.ends_with("entity/player/slim/alex.png"));
        assert_eq!(skin.image.get_pixel(0, 0).0, [12, 34, 56, 255]);
        source.bytes = encode(64, 32);
        assert!(PlayerSkin::load(&mut source, None).is_err());
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn selected_resource_pack_overrides_the_default_skin() {
        use native_resources::packs::ResourceStack;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .join("assets/minecraft/textures/entity/player/slim/steve.png");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbaImage::from_pixel(64, 64, image::Rgba([7, 33, 99, 255]))
            .save(&path)
            .unwrap();
        let mut provider = ResourceStack::open(&game).unwrap();
        provider.push_pack(temp.path()).unwrap();
        let skin = PlayerSkin::load(&mut provider, None).unwrap();
        assert_eq!(skin.image.get_pixel(0, 0).0, [7, 33, 99, 255]);
        assert!(skin.slim);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn default_skin_selection_matches_java_uuid_hash() {
        use serde_json::Value;
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import java.util.UUID;
import net.minecraft.client.resources.DefaultPlayerSkin;
public class DefaultSkinOracle {
  public static void main(String[] args) throws Exception {
    var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var output=new JsonArray();
    for(var item:input) {
      var skin=item.isJsonNull()?DefaultPlayerSkin.getDefaultSkin():DefaultPlayerSkin.get(UUID.fromString(item.getAsString()));
      output.add(skin.body().texturePath().toString());
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(output));
  }
}
"#;
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
        fn run(command: &mut Command) {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "skin oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("DefaultSkinOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        let mut ids: Vec<Option<String>> = vec![None];
        for index in 0..256_u32 {
            let word = index.wrapping_mul(2654435761);
            ids.push(Some(format!(
                "{word:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
                index.wrapping_mul(17) % 65536,
                index.wrapping_mul(31) % 65536,
                index.wrapping_mul(127) % 65536,
                u64::from(word) * 17
            )));
        }
        std::fs::write(&source, JAVA).unwrap();
        std::fs::write(&input, serde_json::to_vec(&ids).unwrap()).unwrap();
        let cp = classpath(&game, temp.path());
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("DefaultSkinOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<String> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(ids.len(), expected.len());
        for (id, expected) in ids.iter().zip(expected) {
            let index = skin_index(id.as_deref());
            let folder = if index < 9 { "slim" } else { "wide" };
            assert_eq!(
                format!(
                    "minecraft:textures/entity/player/{folder}/{}.png",
                    NAMES[index % 9]
                ),
                expected,
                "{id:?}"
            );
        }
    }
}
