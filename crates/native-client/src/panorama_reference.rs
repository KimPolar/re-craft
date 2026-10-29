//! Optional differential test against the installed 26.3 panorama fragment shader.
use super::*;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::Duration,
};

const WIDTH: u32 = 128;
const HEIGHT: u32 = 64;
const FACE_SIZE: u32 = 32;
const YAWS: [f32; 4] = [0.0, 0.37, 1.25, 2.58];

// The fragment shader is loaded from the verified local JAR. The synthetic
// vertex shader supplies the same camera rays used by the native renderer.
const JAVA: &str = r##"
import java.nio.*;
import java.nio.file.*;
import java.util.zip.*;
import org.lwjgl.BufferUtils;
import org.lwjgl.opengl.GL;
import static org.lwjgl.opengl.GL33C.*;
import static org.lwjgl.sdl.SDLInit.*;
import static org.lwjgl.sdl.SDLVideo.*;
import static org.lwjgl.sdl.SDLError.*;

public class PanoramaGlOracle {
  static int compile(int type, String source) {
    int shader=glCreateShader(type);
    glShaderSource(shader,source);
    glCompileShader(shader);
    if(glGetShaderi(shader,GL_COMPILE_STATUS)==0) throw new AssertionError(glGetShaderInfoLog(shader));
    return shader;
  }
  public static void main(String[] args) throws Exception {
    if(!SDL_Init(SDL_INIT_VIDEO)) throw new AssertionError(SDL_GetError());
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MAJOR_VERSION,3);
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MINOR_VERSION,3);
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_PROFILE_MASK,SDL_GL_CONTEXT_PROFILE_CORE);
    long window=SDL_CreateWindow("Panorama parity",128,64,SDL_WINDOW_OPENGL|SDL_WINDOW_HIDDEN);
    if(window==0) throw new AssertionError(SDL_GetError());
    long context=SDL_GL_CreateContext(window);
    if(context==0 || !SDL_GL_MakeCurrent(window,context)) throw new AssertionError(SDL_GetError());
    GL.createCapabilities();
    String fragment;
    try(var jar=new ZipFile(args[0])) {
      fragment=new String(jar.getInputStream(jar.getEntry("assets/minecraft/shaders/core/panorama.fsh")).readAllBytes(),java.nio.charset.StandardCharsets.UTF_8);
    }
    String vertex="""
      #version 330
      #extension GL_ARB_separate_shader_objects : require
      layout(location=0) out vec3 texCoord0;
      uniform float yaw;
      void main() {
        vec2 p[3]=vec2[3](vec2(-1,1),vec2(-1,-3),vec2(3,1));
        vec2 t[3]=vec2[3](vec2(0,0),vec2(0,2),vec2(2,0));
        vec2 uv=t[gl_VertexID];
        float u=(2.0*uv.x-1.0)*2.0*0.9163312;
        float v=(1.0-2.0*uv.y)*0.9163312;
        float py=v*0.98480773-0.17364818;
        float pz=v*0.17364818+0.98480773;
        texCoord0=vec3(u*cos(yaw)+pz*sin(yaw),-py,-u*sin(yaw)+pz*cos(yaw));
        gl_Position=vec4(p[gl_VertexID],0,1);
      }
      """;
    int program=glCreateProgram();
    glAttachShader(program,compile(GL_VERTEX_SHADER,vertex));
    glAttachShader(program,compile(GL_FRAGMENT_SHADER,fragment));
    glLinkProgram(program);
    if(glGetProgrami(program,GL_LINK_STATUS)==0) throw new AssertionError(glGetProgramInfoLog(program));
    glUseProgram(program);
    glBindVertexArray(glGenVertexArrays());
    byte[] source=Files.readAllBytes(Path.of(args[1]));
    int[] order={1,3,5,4,0,2};
    int cube=glGenTextures();
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_CUBE_MAP,cube);
    glTexParameteri(GL_TEXTURE_CUBE_MAP,GL_TEXTURE_MIN_FILTER,GL_LINEAR);
    glTexParameteri(GL_TEXTURE_CUBE_MAP,GL_TEXTURE_MAG_FILTER,GL_LINEAR);
    glTexParameteri(GL_TEXTURE_CUBE_MAP,GL_TEXTURE_WRAP_S,GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_CUBE_MAP,GL_TEXTURE_WRAP_T,GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_CUBE_MAP,GL_TEXTURE_WRAP_R,GL_CLAMP_TO_EDGE);
    glEnable(GL_TEXTURE_CUBE_MAP_SEAMLESS);
    for(int layer=0;layer<6;layer++) {
      ByteBuffer pixels=BufferUtils.createByteBuffer(32*32*4);
      for(int y=0;y<32;y++) {
        int offset=((order[layer]*32+(31-y))*32)*4;
        pixels.put(source,offset,32*4);
      }
      pixels.flip();
      glTexImage2D(GL_TEXTURE_CUBE_MAP_POSITIVE_X+layer,0,GL_RGBA8,32,32,0,GL_RGBA,GL_UNSIGNED_BYTE,pixels);
    }
    glUniform1i(glGetUniformLocation(program,"Sampler0"),0);
    int color=glGenTextures();
    glBindTexture(GL_TEXTURE_2D,color);
    glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,128,64,0,GL_RGBA,GL_UNSIGNED_BYTE,(ByteBuffer)null);
    int fbo=glGenFramebuffers();
    glBindFramebuffer(GL_FRAMEBUFFER,fbo);
    glFramebufferTexture2D(GL_FRAMEBUFFER,GL_COLOR_ATTACHMENT0,GL_TEXTURE_2D,color,0);
    if(glCheckFramebufferStatus(GL_FRAMEBUFFER)!=GL_FRAMEBUFFER_COMPLETE) throw new AssertionError("bad framebuffer");
    glViewport(0,0,128,64);
    glDisable(GL_DITHER);
    var out=new java.io.ByteArrayOutputStream();
    for(float yaw:new float[]{0.0f,0.37f,1.25f,2.58f}) {
      glUniform1f(glGetUniformLocation(program,"yaw"),yaw);
      glDrawArrays(GL_TRIANGLES,0,3);
      ByteBuffer pixels=BufferUtils.createByteBuffer(128*64*4);
      glReadPixels(0,0,128,64,GL_RGBA,GL_UNSIGNED_BYTE,pixels);
      byte[] bytes=new byte[pixels.capacity()]; pixels.get(bytes); out.write(bytes);
    }
    Files.write(Path.of(args[2]),out.toByteArray());
    SDL_GL_DestroyContext(context); SDL_DestroyWindow(window); SDL_Quit();
  }
}
"##;

fn java_bin() -> PathBuf {
    std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
            )
        })
        .join("bin")
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in metadata["libraries"].as_array().unwrap() {
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
        "panorama GL oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3, Java 25, an OpenGL 3.3 desktop and a wgpu adapter"]
fn panorama_cube_matches_installed_java_shader() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let java_source = temp.path().join("PanoramaGlOracle.java");
    let faces_path = temp.path().join("faces.bin");
    let output_path = temp.path().join("java.bin");
    std::fs::write(&java_source, JAVA).unwrap();
    let images: Vec<_> = (0..6)
        .map(|face| {
            RgbaImage::from_fn(FACE_SIZE, FACE_SIZE, |x, y| {
                image::Rgba([
                    (face * 37 + x * 7 + y * 3) as u8,
                    (face * 19 + x * 2 + y * 11) as u8,
                    (face * 53 + x * 13 + y * 5) as u8,
                    255,
                ])
            })
        })
        .collect();
    let source_pixels: Vec<_> = images
        .iter()
        .flat_map(|image| image.as_raw())
        .copied()
        .collect();
    std::fs::write(&faces_path, source_pixels).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&java_source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("PanoramaGlOracle")
        .arg(game.join("versions/26.3/26.3.jar"))
        .arg(&faces_path)
        .arg(&output_path));
    let reference = std::fs::read(output_path).unwrap();
    assert_eq!(reference.len(), YAWS.len() * (WIDTH * HEIGHT * 4) as usize);
    let transparent = RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 0]));
    let mut worst_software = 0_u8;
    for (case, yaw) in YAWS.into_iter().enumerate() {
        let mut canvas = crate::menu_draw::Canvas::new(WIDTH, HEIGHT);
        canvas.panorama(&images, &transparent, yaw / (std::f32::consts::PI / 90.0));
        for y in 0..HEIGHT as usize {
            for x in 0..WIDTH as usize {
                let pixel = canvas.pixels[y * WIDTH as usize + x];
                for (channel, actual) in [(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8]
                    .into_iter()
                    .enumerate()
                {
                    let offset =
                        ((case * HEIGHT as usize + (HEIGHT as usize - 1 - y)) * WIDTH as usize + x)
                            * 4
                            + channel;
                    let expected = reference[offset];
                    worst_software = worst_software.max(actual.abs_diff(expected));
                    assert!(
                        actual.abs_diff(expected) <= 3,
                        "software yaw={yaw} ({x},{y}) channel {channel}: {actual} != Java GL {expected}"
                    );
                }
            }
        }
    }
    println!("Software panorama largest channel difference = {worst_software}");

    pollster::block_on(async {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = if std::env::var("NATIVE_TEST_GPU_BACKEND").as_deref() == Ok("dx12") {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::VULKAN
        };
        let instance = wgpu::Instance::new(descriptor);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();
        let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = panorama_layout(&device);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("panorama comparison shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("menu_panorama.wgsl").into()),
        });
        let pipeline = pipeline(
            &device,
            &layout,
            &shader,
            wgpu::TextureFormat::Rgba8Unorm,
            None,
            "panorama comparison",
        );
        let cube =
            upload_panorama(&device, &queue, &images, wgpu::TextureFormat::Rgba8Unorm).unwrap();
        let view = cube_view(&cube);
        let sampler = panorama_sampler(&device);
        let overlay = upload_rgba(
            &device,
            &queue,
            &RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 0])),
            wgpu::TextureFormat::Rgba8Unorm,
            "transparent",
        )
        .unwrap();
        let overlay_view = overlay.create_view(&wgpu::TextureViewDescriptor::default());
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("panorama comparison parameters"),
            contents: bytemuck::cast_slice(&[2.0_f32, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind = panorama_binding(&device, &layout, &view, &sampler, &params, &overlay_view);
        let output = texture(
            &device,
            (WIDTH, HEIGHT),
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            "panorama output",
        )
        .unwrap();
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("panorama readback"),
            size: u64::from(WIDTH * HEIGHT * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut worst = 0_u8;
        for (case, yaw) in YAWS.into_iter().enumerate() {
            queue.write_buffer(&params, 0, bytemuck::cast_slice(&[2.0_f32, yaw, 0.0, 0.0]));
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("panorama comparison"),
                    color_attachments: &[Some(color_attachment(
                        &output_view,
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    ))],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
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
                        bytes_per_row: Some(WIDTH * 4),
                        rows_per_image: Some(HEIGHT),
                    },
                },
                wgpu::Extent3d {
                    width: WIDTH,
                    height: HEIGHT,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
            let (sender, receiver) = mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |value| {
                    let _ = sender.send(value);
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
            for y in 0..HEIGHT as usize {
                for x in 0..WIDTH as usize {
                    for channel in 0..4 {
                        let actual = data[(y * WIDTH as usize + x) * 4 + channel];
                        let offset = ((case * HEIGHT as usize + (HEIGHT as usize - 1 - y))
                            * WIDTH as usize
                            + x)
                            * 4
                            + channel;
                        let expected = reference[offset];
                        worst = worst.max(actual.abs_diff(expected));
                        assert!(
                            actual.abs_diff(expected) <= 3,
                            "yaw={yaw} ({x},{y}) channel {channel}: GPU {actual} != Java GL {expected}"
                        );
                    }
                }
            }
            drop(data);
            buffer.unmap();
        }
        if let Some(error) = errors.pop().await {
            panic!("GPU validation failed: {error}");
        }
        println!(
            "Compared {} panorama pixels against Java GL; largest channel difference = {worst}",
            YAWS.len() * (WIDTH * HEIGHT) as usize
        );
    });
}
