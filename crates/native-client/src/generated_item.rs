//! Extrude 26.3's generated item sprites, retaining transparent boundaries.

use std::sync::Arc;

use anyhow::{Result, ensure};
use image::RgbaImage;
use native_resources::geometry::{self, Direction};

use crate::held_items::TexturedQuad;

// The asset loader runs off-thread; bound pathological high-resolution masks.
const MAX_SIDE_FACES: usize = 65_536;

pub fn bake(texture: Arc<RgbaImage>, tint: [f32; 3], layer: u8) -> Result<Vec<TexturedQuad>> {
    let (width, height) = texture.dimensions();
    ensure!(width > 0 && height > 0, "empty generated item sprite");
    let mut quads = Vec::new();
    let mut add = |direction, from, to, uv| {
        quads.push(TexturedQuad {
            vertices: geometry::face_vertices(direction, from, to, uv),
            normal: direction.normal(),
            texture: Arc::clone(&texture),
            tint,
            layer,
        });
    };
    add(
        Direction::South,
        [0.0, 0.0, 7.5],
        [16.0, 16.0, 8.5],
        [0.0, 0.0, 16.0, 16.0],
    );
    add(
        Direction::North,
        [0.0, 0.0, 7.5],
        [16.0, 16.0, 8.5],
        [16.0, 0.0, 0.0, 16.0],
    );
    let sx = 16.0 / width as f32;
    let sy = 16.0 / height as f32;
    let transparent = |x: i64, y: i64| {
        x < 0
            || y < 0
            || x >= i64::from(width)
            || y >= i64::from(height)
            || texture.get_pixel(x as u32, y as u32).0[3] == 0
    };
    let mut side_count = 0;
    for y in 0..height {
        for x in 0..width {
            if transparent(i64::from(x), i64::from(y)) {
                continue;
            }
            let (xf, yf) = (x as f32, y as f32);
            let x0 = xf * sx;
            let x1 = (xf + 1.0) * sx;
            let top = 16.0 - yf * sy;
            let bottom = 16.0 - (yf + 1.0) * sy;
            let u0 = (xf + 0.1) * sx;
            let u1 = (xf + 1.0 - 0.1) * sx;
            let v0 = (yf + 0.1) * sy;
            let v1 = (yf + 1.0 - 0.1) * sy;
            for (direction, adjacent_x, adjacent_y, from, to, uv) in [
                (
                    Direction::Up,
                    i64::from(x),
                    i64::from(y) - 1,
                    [x0, top, 7.5],
                    [x1, top, 8.5],
                    [u0, v0, u1, v1],
                ),
                (
                    Direction::Down,
                    i64::from(x),
                    i64::from(y) + 1,
                    [x0, bottom, 7.5],
                    [x1, bottom, 8.5],
                    [u0, v0, u1, v1],
                ),
                (
                    Direction::East,
                    i64::from(x) - 1,
                    i64::from(y),
                    [x0, top, 7.5],
                    [x0, bottom, 8.5],
                    [u0, v1, u1, v0],
                ),
                (
                    Direction::West,
                    i64::from(x) + 1,
                    i64::from(y),
                    [x1, top, 7.5],
                    [x1, bottom, 8.5],
                    [u0, v1, u1, v0],
                ),
            ] {
                if transparent(adjacent_x, adjacent_y) {
                    side_count += 1;
                    ensure!(
                        side_count <= MAX_SIDE_FACES,
                        "generated item sprite has too many boundary faces"
                    );
                    add(direction, from, to, uv);
                }
            }
        }
    }
    Ok(quads)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn alpha_edges_and_non_square_sprites_make_the_expected_faces() {
        let mut image = RgbaImage::from_pixel(3, 2, Rgba([255, 255, 255, 0]));
        image.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        image.put_pixel(1, 0, Rgba([255, 255, 255, 1]));
        let quads = bake(Arc::new(image), [1.0; 3], 0).unwrap();
        // Two neighboring opaque pixels have six exposed sides and two broad faces.
        assert_eq!(quads.len(), 8);
        assert_eq!(quads[0].vertices[0].position[2], 8.5 / 16.0);
        assert_eq!(quads[1].vertices[0].position[2], 7.5 / 16.0);
        assert!(quads.iter().all(|q| q.layer == 0));
    }

    #[test]
    #[ignore = "requires a GPU and locally installed Minecraft 26.3; renders offscreen"]
    fn gpu_displays_local_diamond_sword_in_first_person() {
        use crate::{
            first_person::{FirstPersonInput, FirstPersonRenderer},
            held_items::{ItemModelCache, Loader},
            player_skin::PlayerSkin,
        };
        use std::{
            sync::{Mutex, mpsc},
            time::Duration,
        };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let stack = native_resources::packs::ResourceStack::open(&game).unwrap();
        let mut cache = ItemModelCache::new(Arc::new(Mutex::new(Loader::from_stack(stack))));
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let model = loop {
            cache.poll();
            if let Some(model) = cache.get("minecraft:diamond_sword") {
                break model;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "sword model did not load"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let backend = match std::env::var("NATIVE_TEST_GPU_BACKEND").ok().as_deref() {
            Some("dx12") => wgpu::Backends::DX12,
            Some("vulkan") => wgpu::Backends::VULKAN,
            _ => wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
        };
        pollster::block_on(async {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = backend;
            let instance = wgpu::Instance::new(descriptor);
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                })
                .await
                .unwrap();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut renderer =
                FirstPersonRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None);
            renderer.update(
                &device,
                &queue,
                FirstPersonInput {
                    skin: Arc::new(PlayerSkin {
                        image: RgbaImage::from_pixel(64, 64, Rgba([255, 0, 0, 255])),
                        slim: false,
                    }),
                    main_left: false,
                    main_present: true,
                    main: Some(Arc::clone(&model)),
                    off: None,
                    visible: true,
                    main_equip: 0.0,
                    off_equip: 0.0,
                    swing_hand: None,
                    swing_progress: 0.0,
                    item_use: None,
                },
            );
            let size = wgpu::Extent3d {
                width: 256,
                height: 144,
                depth_or_array_layers: 1,
            };
            let texture = |format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let output = texture(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = texture(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let view = output.create_view(&wgpu::TextureViewDescriptor::default());
            let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 144 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            renderer.draw(
                &queue,
                &mut encoder,
                &view,
                &depth_view,
                glam::Mat4::perspective_rh(70_f32.to_radians(), 256.0 / 144.0, 0.05, 512.0),
            );
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &output,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(1024),
                        rows_per_image: Some(144),
                    },
                },
                size,
            );
            queue.submit([encoder.finish()]);
            let (sender, receiver) = mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(15)),
                })
                .unwrap();
            receiver
                .recv_timeout(Duration::from_secs(15))
                .unwrap()
                .unwrap();
            let data = buffer.slice(..).get_mapped_range().unwrap();
            let cyan = data
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[1] > 40 && p[2] > 40 && p[1] > p[0].saturating_add(15))
                .count();
            if let Some(path) = std::env::var_os("NATIVE_GENERATED_HAND_PREVIEW") {
                RgbaImage::from_raw(256, 144, data.to_vec())
                    .unwrap()
                    .save(path)
                    .unwrap();
            }
            assert!(
                cyan > 20,
                "the generated sword was not rendered: {cyan} cyan pixels"
            );
            drop(data);
            buffer.unmap();
            // Exercise the same installed model in the world item pipeline as well.
            use wgpu::util::DeviceExt;
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
            let mut camera_values = [0.0_f32; 20];
            for index in [0, 5, 10, 15] {
                camera_values[index] = 1.0;
            }
            camera_values[16] = 256.0;
            camera_values[17] = 144.0;
            let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&camera_values),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let glint_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let glint_view = glint_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
            let glint_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[0.0_f32; 4]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let mut ground = crate::ground_items::GroundItemRenderer::new(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
                &layout,
                &camera,
                (&glint_view, &sampler, &glint_uniform),
            );
            let item = native_session::items::DroppedItem {
                position: glam::DVec3::new(0.0, -0.5, 0.5),
                previous_position: glam::DVec3::new(0.0, -0.5, 0.5),
                stack: native_session::inventory::ItemStack::new(
                    "minecraft:diamond_sword".into(),
                    1,
                    1,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
                age: 0,
                bob_offset: 0.0,
            };
            ground
                .update(
                    &device,
                    &queue,
                    &[item],
                    &[Some(model)],
                    glam::IVec3::ZERO,
                    0.0,
                )
                .unwrap();
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                ground.draw(&mut pass);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &output,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(1024),
                        rows_per_image: Some(144),
                    },
                },
                size,
            );
            queue.submit([encoder.finish()]);
            let (sender, receiver) = mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(15)),
                })
                .unwrap();
            receiver
                .recv_timeout(Duration::from_secs(15))
                .unwrap()
                .unwrap();
            let data = buffer.slice(..).get_mapped_range().unwrap();
            let cyan = data
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[1] > 40 && p[2] > 40 && p[1] > p[0].saturating_add(15))
                .count();
            assert!(
                cyan > 20,
                "the installed sword was not rendered as a ground item: {cyan} cyan pixels"
            );
            drop(data);
            buffer.unmap();
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn generated_sprite_geometry_matches_vanilla() {
        use serde_json::Value;
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import sun.misc.Unsafe;
import org.joml.*;
import it.unimi.dsi.fastutil.ints.*;
import net.minecraft.resources.Identifier;
import net.minecraft.client.renderer.texture.SpriteContents;
import net.minecraft.client.renderer.texture.TextureAtlasSprite;
import net.minecraft.client.renderer.block.dispatch.BlockModelRotation;
import net.minecraft.client.renderer.block.dispatch.ModelState;
import net.minecraft.client.resources.model.ModelBaker;
import net.minecraft.client.resources.model.cuboid.ItemModelGenerator;
import net.minecraft.client.resources.model.geometry.BakedQuad;
import net.minecraft.client.resources.model.geometry.QuadCollection;
import net.minecraft.client.model.geom.builders.UVPair;
public class GeneratedOracle {
  static class Contents extends SpriteContents {
    int w,h,mask;
    Contents() { super(null,null,null); }
    public int width() { return w; }
    public int height() { return h; }
    public IntList getUniqueFrames() { return IntLists.singleton(0); }
    public boolean isTransparent(int frame,int x,int y) { return (mask & (1 << (y*w+x))) == 0; }
  }
  static class Sprite extends TextureAtlasSprite {
    Sprite(Contents c) { super(Identifier.withDefaultNamespace("test"),c,c.w,c.h,0,0,0); }
  }
  public static void main(String[] args) throws Exception {
    var field=Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(Unsafe)field.get(null);
    var method=ItemModelGenerator.class.getDeclaredMethod("bakeExtrudedSprite", QuadCollection.Builder.class,ModelBaker.Interner.class,ModelState.class,BakedQuad.MaterialInfo.class);
    method.setAccessible(true);
    var interner=new ModelBaker.Interner() {
      public Vector3fc vector(Vector3fc p) { return new Vector3f(p); }
      public BakedQuad.MaterialInfo materialInfo(BakedQuad.MaterialInfo m) { return m; }
    };
    var result=new JsonArray();
    for(int[] dimensions:new int[][]{{2,2},{3,2},{3,3}}) {
      int w=dimensions[0],h=dimensions[1];
      for(int mask=0;mask<(1 << (w*h));mask++) {
        var contents=(Contents)unsafe.allocateInstance(Contents.class);
        contents.w=w; contents.h=h; contents.mask=mask;
        var sprite=new Sprite(contents);
        var info=new BakedQuad.MaterialInfo(sprite,null,null,null,null,0,null,0);
        var builder=new QuadCollection.Builder();
        method.invoke(null,builder,interner,BlockModelRotation.IDENTITY,info);
        var row=new JsonArray();
        for(var quad:builder.build().getAll()) {
          var vertices=new JsonArray();
          for(int v=0;v<4;v++) {
            var p=quad.position(v); long uv=quad.packedUV(v);
            var value=new JsonArray(); value.add(p.x());value.add(p.y());value.add(p.z());
            value.add(UVPair.unpackU(uv));value.add(UVPair.unpackV(uv)); vertices.add(value);
          }
          row.add(vertices);
        }
        result.add(row);
      }
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
                "generated geometry oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("GeneratedOracle.java");
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
            .arg("GeneratedOracle")
            .arg(&output));
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let mut index = 0;
        for (width, height) in [(2, 2), (3, 2), (3, 3)] {
            for mask in 0..1_u32 << (width * height) {
                let image = RgbaImage::from_fn(width, height, |x, y| {
                    Rgba([
                        255,
                        255,
                        255,
                        if mask & (1 << (y * width + x)) == 0 {
                            0
                        } else {
                            255
                        },
                    ])
                });
                let actual = bake(Arc::new(image), [1.0; 3], 0).unwrap();
                let expected = rows[index].as_array().unwrap();
                index += 1;
                assert_eq!(actual.len(), expected.len(), "{width}x{height} mask={mask}");
                let mut used = vec![false; actual.len()];
                for java_quad in expected {
                    let found = actual.iter().enumerate().find(|(i, quad)| {
                        if used[*i] {
                            return false;
                        }
                        java_quad.as_array().unwrap().iter().all(|vertex| {
                            quad.vertices.iter().any(|v| {
                                let values = [
                                    v.position[0],
                                    v.position[1],
                                    v.position[2],
                                    v.uv[0],
                                    v.uv[1],
                                ];
                                values.into_iter().enumerate().all(|(axis, value)| {
                                    (value - vertex[axis].as_f64().unwrap() as f32).abs() < 0.00001
                                })
                            })
                        })
                    });
                    let Some((i, _)) = found else {
                        panic!("unmatched Java quad {java_quad} for {width}x{height} mask={mask}")
                    };
                    used[i] = true;
                }
            }
        }
        assert_eq!(index, rows.len());
    }
}
