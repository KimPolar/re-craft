//! Build cached 16×16 GUI icons from the resolved item model on the loader thread.

use glam::{Mat3, Quat, Vec2, Vec3};
use image::{Rgba, RgbaImage};

use crate::{
    held_items::{DisplayTransform, HeldModel},
    menu_draw::{Canvas, Rect},
    player_preview::{Projected, raster, raster_with_fragment},
};

// A deliberately pathological resource pack must not monopolize the item worker.
const MAX_ICON_QUADS: usize = 8192;
const ICON_SIZE: u32 = 16;

struct GuiTransform {
    display: DisplayTransform,
    rotation: Quat,
    inverse_scale: Vec3,
}

impl GuiTransform {
    fn new(display: DisplayTransform) -> Self {
        let inverse = display
            .scale
            .to_array()
            .map(|v| if v == 0.0 { 0.0 } else { 1.0 / v });
        Self {
            display,
            rotation: display.rotation(false),
            inverse_scale: Vec3::from_array(inverse),
        }
    }

    fn project(&self, position: Vec3, uv: Vec2) -> Projected {
        let position = self.display.translation
            + self.rotation * (self.display.scale * (position - Vec3::splat(0.5)));
        Projected {
            screen: Vec2::new(8.0 + position.x * 16.0, 8.0 - position.y * 16.0),
            depth: position.z * 16.0,
            uv,
        }
    }

    fn normal(&self, normal: Vec3) -> Vec3 {
        (Vec3::new(1.0, -1.0, 1.0) * (self.rotation * (normal * self.inverse_scale)))
            .normalize_or_zero()
    }
}

fn lights(side: bool) -> (Vec3, Vec3) {
    // Lighting.Entry.ITEMS_FLAT / ITEMS_3D in the installed Java renderer.
    let flat =
        Mat3::from_rotation_y(-std::f32::consts::FRAC_PI_8) * Mat3::from_rotation_x(2.3561945);
    let matrix = if side {
        Mat3::from_diagonal(Vec3::new(1.0, -1.0, 1.0))
            * Mat3::from_rotation_y(1.0821041)
            * Mat3::from_rotation_x(3.2375858)
            * flat
    } else {
        flat
    };
    (
        matrix * Vec3::new(0.2, 1.0, -0.7).normalize(),
        matrix * Vec3::new(-0.2, 1.0, 0.7).normalize(),
    )
}

fn brightness(normal: Vec3, lights: (Vec3, Vec3)) -> f32 {
    ((normal.dot(lights.0).max(0.0) + normal.dot(lights.1).max(0.0)) * 0.6 + 0.4).min(1.0)
}

fn as_image(canvas: Canvas) -> RgbaImage {
    RgbaImage::from_fn(ICON_SIZE, ICON_SIZE, |x, y| {
        let pixel = canvas.pixels[(y * ICON_SIZE + x) as usize];
        Rgba([
            (pixel >> 16) as u8,
            (pixel >> 8) as u8,
            pixel as u8,
            (pixel >> 24) as u8,
        ])
    })
}

type GlintPixels = Box<[[u16; 3]; 256]>;

fn record_glint(
    pixels: &mut [[u16; 3]; 256],
    x: i32,
    y: i32,
    local: Vec2,
    bounds: Option<[f32; 4]>,
    alpha: u8,
    previous_alpha: u8,
) {
    if alpha == 0 {
        return;
    }
    let pixel = &mut pixels[(y as usize) * ICON_SIZE as usize + x as usize];
    // A single UV cannot represent two translucent layers. Keep that pixel on
    // the existing fallback path until both contributions can be rendered.
    let Some([u0, v0, u1, v1]) = bounds.filter(|_| alpha == 255 || previous_alpha == 0) else {
        *pixel = [0; 3];
        return;
    };
    let encode = |value: f32| (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
    *pixel = [
        encode(u0 + (u1 - u0) * local.x),
        encode(v0 + (v1 - v0) * local.y),
        1,
    ];
}

pub fn render(model: &HeldModel) -> Option<RgbaImage> {
    render_inner(model, false).map(|(icon, _)| icon)
}

pub fn render_with_glint(model: &HeldModel) -> Option<(RgbaImage, Option<GlintPixels>)> {
    render_inner(model, true)
}

fn render_inner(
    model: &HeldModel,
    capture_glint: bool,
) -> Option<(RgbaImage, Option<GlintPixels>)> {
    let mut canvas = Canvas::new(ICON_SIZE, ICON_SIZE);
    let mut glint =
        (capture_glint && model.gui_glint_uv.is_none()).then(|| Box::new([[0; 3]; 256]));
    let display = model.gui;
    if !model.sprite_layers.is_empty()
        && !model.gui_side_light
        && display.rotation == Vec3::ZERO
        && display.translation == Vec3::ZERO
        && display.scale == Vec3::ONE
    {
        // Avoid rasterizing thousands of sprite-edge quads for the common
        // untransformed, front-lit case. Preserve layers and alpha in order.
        for layer in &model.sprite_layers {
            let image = &layer.texture;
            for y in 0..ICON_SIZE {
                for x in 0..ICON_SIZE {
                    let p = image
                        .get_pixel(
                            x * image.width() / ICON_SIZE,
                            y * image.height() / ICON_SIZE,
                        )
                        .0;
                    if let Some(glint) = glint.as_mut() {
                        let previous_alpha =
                            (canvas.pixels[(y * ICON_SIZE + x) as usize] >> 24) as u8;
                        record_glint(
                            glint,
                            x as i32,
                            y as i32,
                            Vec2::new(
                                (x as f32 + 0.5) / ICON_SIZE as f32,
                                (y as f32 + 0.5) / ICON_SIZE as f32,
                            ),
                            layer.glint_uv,
                            p[3],
                            previous_alpha,
                        );
                    }
                    canvas.pixel(
                        x as i32,
                        y as i32,
                        [
                            (f32::from(p[0]) * layer.tint[0]) as u8,
                            (f32::from(p[1]) * layer.tint[1]) as u8,
                            (f32::from(p[2]) * layer.tint[2]) as u8,
                            p[3],
                        ],
                    );
                }
            }
        }
        return Some((
            as_image(canvas),
            glint.filter(|pixels| pixels.iter().any(|pixel| pixel[2] != 0)),
        ));
    }
    if model.quads.is_empty() || model.quads.len() > MAX_ICON_QUADS {
        return None;
    }
    let mut depth = [f32::NEG_INFINITY; (ICON_SIZE * ICON_SIZE) as usize];
    let clip = Rect {
        x: 0,
        y: 0,
        w: ICON_SIZE as i32,
        h: ICON_SIZE as i32,
    };
    let lights = lights(model.gui_side_light);
    let transform = GuiTransform::new(display);
    for (quad_index, quad) in model.quads.iter().enumerate() {
        let shade = brightness(transform.normal(Vec3::from_array(quad.normal)), lights);
        let bounds = model
            .gui_glint_quads
            .as_ref()
            .and_then(|bounds| bounds.get(quad_index))
            .copied()
            .flatten();
        let vertices = quad
            .vertices
            .map(|v| transform.project(Vec3::from_array(v.position), Vec2::from_array(v.uv)));
        for indices in [[0, 1, 2], [0, 2, 3]] {
            let triangle = [
                vertices[indices[0]],
                vertices[indices[1]],
                vertices[indices[2]],
            ];
            if let Some(glint) = glint.as_mut() {
                raster_with_fragment(
                    &mut canvas,
                    &quad.texture,
                    &mut depth,
                    clip,
                    triangle,
                    shade,
                    quad.tint,
                    |x, y, uv, alpha, previous| {
                        record_glint(glint, x, y, uv, bounds, alpha, previous)
                    },
                );
            } else {
                raster(
                    &mut canvas,
                    &quad.texture,
                    &mut depth,
                    clip,
                    triangle,
                    shade,
                    quad.tint,
                );
            }
        }
    }
    Some((
        as_image(canvas),
        glint.filter(|pixels| pixels.iter().any(|pixel| pixel[2] != 0)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::geometry::{self, Direction};
    use std::sync::Arc;

    #[test]
    fn rotated_cube_preserves_face_textures_and_transparent_background() {
        let display = DisplayTransform {
            translation: Vec3::ZERO,
            rotation: Vec3::new(30.0, 225.0, 0.0),
            scale: Vec3::splat(0.625),
        };
        let quads = Direction::ALL
            .into_iter()
            .map(|direction| {
                let color = match direction {
                    Direction::Up => [0, 255, 0, 255],
                    Direction::North => [0, 0, 255, 255],
                    Direction::East => [255, 0, 0, 255],
                    _ => [255, 255, 255, 255],
                };
                crate::held_items::TexturedQuad {
                    vertices: geometry::face_vertices(
                        direction,
                        [0.0; 3],
                        [16.0; 3],
                        [0.0, 0.0, 16.0, 16.0],
                    ),
                    normal: direction.normal(),
                    texture: Arc::new(RgbaImage::from_pixel(2, 2, Rgba(color))),
                    tint: [1.0; 3],
                    layer: 0,
                }
            })
            .collect();
        let model = HeldModel {
            particle_texture: None,
            numeric_item_id: None,
            quads,
            glint: false,
            glint_materials: Vec::new(),
            right: display,
            left: display,
            first_right: display,
            first_left: display,
            ground: display,
            gui: display,
            gui_side_light: true,
            gui_icon: None,
            gui_glint_uv: None,
            gui_glint_pixels: None,
            gui_glint_quads: None,
            sprite_layers: Vec::new(),
        };
        let icon = render(&model).unwrap();
        assert_eq!(icon.get_pixel(0, 0).0[3], 0);
        let visible = icon.pixels().filter(|p| p.0[3] == 255).count();
        assert!((80..200).contains(&visible), "visible pixels: {visible}");
        assert!(icon.pixels().any(|p| p.0[0] > 0));
        assert!(icon.pixels().any(|p| p.0[1] > 0));
        assert!(icon.pixels().any(|p| p.0[2] > 0));
    }

    #[test]
    fn layered_items_use_the_visible_layer_atlas_coordinates() {
        use crate::held_items::SpriteLayer;
        let display = DisplayTransform {
            translation: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
        };
        let base = Arc::new(RgbaImage::from_pixel(16, 16, Rgba([255, 255, 255, 255])));
        let overlay = Arc::new(RgbaImage::from_fn(16, 16, |x, _| {
            Rgba([
                255,
                255,
                255,
                if x < 8 {
                    0
                } else if x == 8 {
                    128
                } else {
                    255
                },
            ])
        }));
        let model = HeldModel {
            particle_texture: None,
            numeric_item_id: None,
            quads: Vec::new(),
            glint: false,
            glint_materials: Vec::new(),
            right: display,
            left: display,
            first_right: display,
            first_left: display,
            ground: display,
            gui: display,
            gui_side_light: false,
            gui_icon: None,
            gui_glint_uv: None,
            gui_glint_pixels: None,
            gui_glint_quads: None,
            sprite_layers: vec![
                SpriteLayer {
                    texture: base,
                    tint: [1.0; 3],
                    glint_uv: Some([0.1, 0.2, 0.2, 0.3]),
                },
                SpriteLayer {
                    texture: overlay,
                    tint: [1.0; 3],
                    glint_uv: Some([0.6, 0.7, 0.7, 0.8]),
                },
            ],
        };
        let (icon, pixels) = render_with_glint(&model).unwrap();
        let pixels = pixels.unwrap();
        let at = |x: usize| pixels[4 * 16 + x];
        assert_eq!(at(2)[2], 1);
        assert!((f32::from(at(2)[0]) / 65535.0 - (0.1 + 0.1 * 2.5 / 16.0)).abs() < 0.00001);
        assert_eq!(
            at(8)[2],
            0,
            "overlapping translucent layers use the fallback"
        );
        assert_eq!(at(12)[2], 1);
        assert!((f32::from(at(12)[0]) / 65535.0 - (0.6 + 0.1 * 12.5 / 16.0)).abs() < 0.00001);
        assert_eq!(icon.get_pixel(8, 4).0[3], 255);
    }

    #[test]
    fn rotated_cuboid_records_the_frontmost_faces_texture_coordinates() {
        let display = DisplayTransform {
            translation: Vec3::ZERO,
            rotation: Vec3::new(30.0, 225.0, 0.0),
            scale: Vec3::splat(0.625),
        };
        let quads = Direction::ALL
            .into_iter()
            .map(|direction| crate::held_items::TexturedQuad {
                vertices: geometry::face_vertices(
                    direction,
                    [0.0; 3],
                    [16.0; 3],
                    [0.0, 0.0, 16.0, 16.0],
                ),
                normal: direction.normal(),
                texture: Arc::new(RgbaImage::from_pixel(2, 2, Rgba([255; 4]))),
                tint: [1.0; 3],
                layer: 0,
            })
            .collect();
        let model = HeldModel {
            particle_texture: None,
            numeric_item_id: None,
            quads,
            glint: false,
            glint_materials: Vec::new(),
            right: display,
            left: display,
            first_right: display,
            first_left: display,
            ground: display,
            gui: display,
            gui_side_light: true,
            gui_icon: None,
            gui_glint_uv: None,
            gui_glint_pixels: None,
            gui_glint_quads: Some(
                (0..6)
                    .map(|i| {
                        let u = 0.1 * i as f32;
                        Some([u, 0.0, u + 0.05, 0.1])
                    })
                    .collect(),
            ),
            sprite_layers: Vec::new(),
        };
        let (icon, pixels) = render_with_glint(&model).unwrap();
        let pixels = pixels.unwrap();
        let visible: Vec<_> = pixels
            .iter()
            .enumerate()
            .filter(|(_, p)| p[2] == 1)
            .collect();
        assert!((80..200).contains(&visible.len()));
        assert!(
            visible
                .iter()
                .all(|(index, _)| icon.as_raw()[index * 4 + 3] == 255)
        );
        let distinct = visible
            .iter()
            .map(|(_, p)| p[0] / 6554)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            distinct.len() >= 2,
            "rotated cube must use more than one face"
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn gui_projection_and_light_match_java_26_3() {
        use serde_json::Value;
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.resources.model.cuboid.ItemTransform;
import org.joml.Matrix4f;
import org.joml.Vector3f;
public class GuiItemOracle {
 public static void main(String[] args) throws Exception {
  float[][] r={{0,0,0},{30,225,0},{20,-50,15},{-23,93,7}};
  float[][] t={{0,0,0},{0,0,0},{0.2f,-0.1f,0.3f},{-0.17f,0.25f,0.04f}};
  float[][] s={{1,1,1},{0.625f,0.625f,0.625f},{-1.25f,0.5f,0.75f},{0.9f,0.6f,0.5f}};
  float[][] points={{0,0,0},{1,1,1},{0.25f,0.4f,0.8f}};
  float[][] normals={{0,-1,0},{0,1,0},{0,0,-1},{0,0,1},{-1,0,0},{1,0,0}};
  var flat=new Matrix4f().rotationY(-0.3926991f).rotateX(2.3561945f);
  var side=new Matrix4f().scaling(1,-1,1).rotateYXZ(1.0821041f,3.2375858f,0).rotateYXZ(-0.3926991f,2.3561945f,0);
  var result=new JsonArray();
  for(boolean useSide:new boolean[]{false,true}) for(int i=0;i<r.length;i++) {
   var pose=new PoseStack(); pose.translate(8,8,0); pose.scale(16,-16,16);
   new ItemTransform(new Vector3f(r[i]),new Vector3f(t[i]),new Vector3f(s[i])).apply(false,pose.last());
   var matrix=useSide?side:flat;
   var a=matrix.transformDirection(new Vector3f(0.2f,1,-0.7f).normalize());
   var b=matrix.transformDirection(new Vector3f(-0.2f,1,0.7f).normalize());
   var row=new JsonObject(); var positions=new JsonArray(); var shades=new JsonArray();
   for(float[] point:points) { var p=pose.last().pose().transformPosition(new Vector3f(point)); var v=new JsonArray(); v.add(p.x);v.add(p.y);v.add(p.z);positions.add(v); }
   for(float[] n:normals) { var v=pose.last().transformNormal(new Vector3f(n),new Vector3f()); float light=Math.min(1, (Math.max(0,v.dot(a))+Math.max(0,v.dot(b)))*0.6f+0.4f); var values=new JsonArray();values.add(v.x);values.add(v.y);values.add(v.z);values.add(light);shades.add(values); }
   row.add("points",positions); row.add("normals",shades); result.add(row);
  }
  Files.writeString(Path.of(args[0]),new Gson().toJson(result));
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
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "GUI item oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("GuiItemOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, JAVA).unwrap();
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
            .arg("GuiItemOracle")
            .arg(&output));
        let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let displays = [
            DisplayTransform {
                rotation: Vec3::ZERO,
                translation: Vec3::ZERO,
                scale: Vec3::ONE,
            },
            DisplayTransform {
                rotation: Vec3::new(30.0, 225.0, 0.0),
                translation: Vec3::ZERO,
                scale: Vec3::splat(0.625),
            },
            DisplayTransform {
                rotation: Vec3::new(20.0, -50.0, 15.0),
                translation: Vec3::new(0.2, -0.1, 0.3),
                scale: Vec3::new(-1.25, 0.5, 0.75),
            },
            DisplayTransform {
                rotation: Vec3::new(-23.0, 93.0, 7.0),
                translation: Vec3::new(-0.17, 0.25, 0.04),
                scale: Vec3::new(0.9, 0.6, 0.5),
            },
        ];
        let points = [Vec3::ZERO, Vec3::ONE, Vec3::new(0.25, 0.4, 0.8)];
        let normals = [
            Vec3::NEG_Y,
            Vec3::Y,
            Vec3::NEG_Z,
            Vec3::Z,
            Vec3::NEG_X,
            Vec3::X,
        ];
        let mut row = 0;
        for side in [false, true] {
            for display in displays {
                let transform = GuiTransform::new(display);
                for (index, p) in points.into_iter().enumerate() {
                    let p = transform.project(p, Vec2::ZERO);
                    for (axis, actual) in [p.screen.x, p.screen.y, p.depth].into_iter().enumerate()
                    {
                        let java = expected[row]["points"][index][axis].as_f64().unwrap() as f32;
                        assert!(
                            (actual - java).abs() < 0.0001,
                            "side={side} row={row} point={index} axis={axis}: {actual} != {java}"
                        );
                    }
                }
                for (index, normal) in normals.into_iter().enumerate() {
                    let normal = transform.normal(normal);
                    for (axis, actual) in [
                        normal.x,
                        normal.y,
                        normal.z,
                        brightness(normal, lights(side)),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let java = expected[row]["normals"][index][axis].as_f64().unwrap() as f32;
                        assert!(
                            (actual - java).abs() < 0.0001,
                            "side={side} row={row} normal={index} axis={axis}: {actual} != {java}"
                        );
                    }
                }
                row += 1;
            }
        }
    }
}
