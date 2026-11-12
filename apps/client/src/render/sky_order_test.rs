//! Proof on a real GPU of the sky drawn after the opaque terrain (`RUSTCRAFT_OPT_SKY_AFTER_OPAQUE`).
//!
//! Each view is drawn twice in one launch by the real sky pipelines, shaders and draw sequence
//! ([`encode_sky`], [`encode_sky_behind_terrain`], [`Sky`], [`plain_sky_pipeline`]): sky first with
//! neither depth test nor write, then the terrain; or terrain first, then the sky at the farthest
//! depth behind it. The terrain stands in as quads with the depth state of the real terrain
//! pipelines ([`terrain_depth_state`]): opaque ground and mountains, leaves whose fragments are
//! thrown away one block in two (their depth stays cleared), then translucent water blended after
//! the sky in both orders. The two images must be equal to the byte. Views: day (sun), dawn (glow),
//! night (moon, stars), End sky (box and flash, cut by the far plane of render distance 2), under
//! water (close sky fog), horizon (dark disc), render distance 2 with cloud range 2 (far plane 128:
//! the dome is cut), render distance 4 with cloud range 16 (far plane 256), leaves against the sky,
//! cave (everything hidden). Each view first proves it shows what it is named after (its image
//! changes without it). The title panorama is left out: it covers the whole screen after everything
//! else in both orders, with neither test nor write.
//!
//! Sabotages checked: a "less" test in place of "greater or equal" (the sky disappears), the sky
//! drawn after the terrain at its own depth instead of the farthest (it covers the far mountain).
//! `sky_order_cost` (ignored): GPU time of a frame in each order, 1600 × 900.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::super::pipelines::{plain_sky_pipeline, terrain_depth_state};
use super::super::{CameraUniform, DEPTH_CLEAR, perspective};
use super::{PassViewport, SkyParts, WorldSky, encode_sky, encode_sky_behind_terrain};
use crate::render::upload::Uploader;
use crate::render::upload::tests::{gpu, gpu_with_timestamps, read};
use crate::sky::{Sky, SkyAssets, SkyFrame, SkyOrder};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Far plane of render distance 2 with cloud range 2: max(64 × 2, 16 × 2).
const FAR_RENDER_DISTANCE_2: f32 = 128.0;
/// Far plane of render distance 4 with cloud range 16: max(64 × 4, 16 × 16).
const FAR_RENDER_DISTANCE_4: f32 = 256.0;
/// Far plane of a long render distance: nothing of the sky is cut.
const FAR_LONG: f32 = 2048.0;

/// Stand-in for the terrain: opaque blocks, leaves (cutout) or water (translucent).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Opaque,
    Leaves,
    Water,
}

/// A quad of the stand-in terrain, corners relative to the eye.
#[derive(Clone, Copy)]
struct Quad {
    corners: [Vec3; 4],
    color: [f32; 4],
    surface: Surface,
}

impl Quad {
    /// Upright quad facing the camera's start direction (−z), `x` and `y` ranges, `distance` ahead.
    fn upright(x: (f32, f32), y: (f32, f32), distance: f32, color: [f32; 4], surface: Surface) -> Quad {
        let z = -distance;
        let corners = [Vec3::new(x.0, y.0, z), Vec3::new(x.1, y.0, z), Vec3::new(x.1, y.1, z), Vec3::new(x.0, y.1, z)];
        Quad { corners, color, surface }
    }

    /// Ground: horizontal quad `below` the eye, `reach` blocks around it.
    fn ground(below: f32, reach: f32) -> Quad {
        let y = -below;
        let corners = [
            Vec3::new(-reach, y, -reach),
            Vec3::new(reach, y, -reach),
            Vec3::new(reach, y, reach),
            Vec3::new(-reach, y, reach),
        ];
        Quad { corners, color: [0.35, 0.55, 0.2, 1.0], surface: Surface::Opaque }
    }

    /// Turned by `yaw` degrees around the vertical (the views look in several directions).
    fn turned(self, yaw: f32) -> Quad {
        let m = Mat4::from_rotation_y(yaw.to_radians());
        Quad { corners: self.corners.map(|c| m.transform_point3(c)), ..self }
    }
}

/// One view of the proof: camera, sky of the frame, stand-in terrain.
#[derive(Clone)]
struct View {
    name: &'static str,
    yaw: f32,
    pitch: f32,
    far: f32,
    sky_color: [f32; 4],
    fog_color: [f32; 4],
    /// End of the sky fog (`fog_sky.x` of the camera).
    sky_fog_end: f32,
    frame: SkyFrame,
    parts: SkyParts,
    quads: Vec<Quad>,
}

fn overworld_frame(sun_deg: f32) -> SkyFrame {
    SkyFrame {
        end: None,
        sun_deg,
        moon_deg: sun_deg + 180.0,
        star_deg: 30.0,
        star_brightness: 0.0,
        rain: 0.0,
        moon_phase: 3,
        sunrise: [1.0, 0.6, 0.3, 0.0],
        clouds: None,
    }
}

const OVERWORLD: SkyParts = SkyParts { dome: true, dark_disc: true, end: false };

/// Mountain on the left, a far mountain behind the sky's own depth, leaves in the middle, water on
/// the right, ground: seen from `yaw`.
fn landscape(yaw: f32) -> Vec<Quad> {
    let quads = [
        Quad::ground(2.0, 60.0),
        Quad::upright((-60.0, -8.0), (-2.0, 14.0), 40.0, [0.45, 0.42, 0.4, 1.0], Surface::Opaque),
        Quad::upright((0.0, 200.0), (-2.0, 60.0), 250.0, [0.5, 0.5, 0.55, 1.0], Surface::Opaque),
        Quad::upright((-3.0, 3.0), (-1.0, 4.0), 7.0, [0.1, 0.5, 0.1, 1.0], Surface::Leaves),
        Quad::upright((4.0, 30.0), (-2.0, 6.0), 20.0, [0.1, 0.25, 0.8, 0.6], Surface::Water),
    ];
    quads.into_iter().map(|q| q.turned(yaw)).collect()
}

fn day() -> View {
    View {
        name: "day",
        yaw: 90.0,
        pitch: -25.0,
        far: FAR_LONG,
        sky_color: [0.47, 0.65, 1.0, 1.0],
        fog_color: [0.75, 0.85, 1.0, 1.0],
        sky_fog_end: 192.0,
        frame: overworld_frame(60.0),
        parts: OVERWORLD,
        quads: landscape(90.0),
    }
}

/// Each view and what it is named after: the same view without it.
fn views() -> Vec<(View, View)> {
    let mut list = Vec::new();
    let day = day();
    list.push((day.clone(), View { frame: SkyFrame { rain: 1.0, ..day.frame }, ..day.clone() }));
    let dawn = View {
        name: "dawn",
        pitch: -5.0,
        frame: SkyFrame { sunrise: [1.0, 0.6, 0.3, 0.8], ..overworld_frame(85.0) },
        ..day.clone()
    };
    list.push((dawn.clone(), View { frame: SkyFrame { sunrise: [0.0; 4], ..dawn.frame }, ..dawn.clone() }));
    let night = View {
        name: "night",
        pitch: -40.0,
        sky_color: [0.0, 0.0, 0.05, 1.0],
        fog_color: [0.03, 0.03, 0.08, 1.0],
        frame: SkyFrame { star_brightness: 0.9, ..overworld_frame(240.0) },
        ..day.clone()
    };
    // Without the stars, and the moon put out by the rain.
    let dark = SkyFrame { star_brightness: 0.0, rain: 1.0, ..night.frame };
    list.push((night.clone(), View { frame: dark, ..night.clone() }));
    let end = View {
        name: "End sky at render distance 2",
        yaw: 45.0,
        pitch: -35.0,
        far: FAR_RENDER_DISTANCE_2,
        sky_color: [0.0; 4],
        fog_color: [0.06, 0.05, 0.08, 1.0],
        frame: SkyFrame { end: Some((0.8, 10.0, 20.0)), ..overworld_frame(0.0) },
        parts: SkyParts { dome: false, dark_disc: false, end: true },
        quads: landscape(45.0),
        ..day.clone()
    };
    list.push((end.clone(), View { frame: SkyFrame { end: Some((0.0, 10.0, 20.0)), ..end.frame }, ..end.clone() }));
    let underwater =
        View { name: "under water", pitch: -60.0, fog_color: [0.02, 0.1, 0.4, 1.0], sky_fog_end: 24.0, ..day.clone() };
    list.push((underwater.clone(), View { sky_fog_end: 192.0, ..underwater.clone() }));
    let horizon = View { name: "horizon", yaw: 200.0, pitch: 12.0, quads: Vec::new(), ..day.clone() };
    let without_disc = SkyParts { dark_disc: false, ..OVERWORLD };
    list.push((horizon.clone(), View { parts: without_disc, ..horizon.clone() }));
    // The far plane cuts the dome a few degrees above the horizon (the plane at 16 blocks of height
    // is farther than the far plane there); a thin sky fog keeps the cut part unlike the fog colour.
    let short = View {
        name: "render distance 2, cloud range 2",
        pitch: -8.0,
        far: FAR_RENDER_DISTANCE_2,
        sky_fog_end: 1000.0,
        ..day.clone()
    };
    list.push((short.clone(), View { far: FAR_LONG, ..short.clone() }));
    let medium = View {
        name: "render distance 4, cloud range 16",
        pitch: -3.0,
        far: FAR_RENDER_DISTANCE_4,
        sky_fog_end: 1000.0,
        ..day.clone()
    };
    list.push((medium.clone(), View { far: FAR_LONG, ..medium.clone() }));
    let leaves = View {
        name: "leaves against the sky",
        pitch: -20.0,
        quads: vec![Quad::upright((-6.0, 6.0), (-4.0, 8.0), 5.0, [0.1, 0.5, 0.1, 1.0], Surface::Leaves).turned(90.0)],
        ..day.clone()
    };
    list.push((leaves.clone(), View { quads: Vec::new(), ..leaves.clone() }));
    // Cave: stone all around the camera, the sky is wholly hidden (its image equals a frame
    // without sky, checked apart).
    let wall = |yaw| Quad::upright((-3.0, 3.0), (-3.0, 3.0), 3.0, [0.3, 0.3, 0.3, 1.0], Surface::Opaque).turned(yaw);
    let cave_quads = vec![wall(0.0), wall(90.0), wall(180.0), wall(270.0), Quad::ground(3.0, 3.0), ceiling(3.0)];
    let cave = View { name: "cave", pitch: -30.0, quads: cave_quads, ..day.clone() };
    list.push((cave.clone(), View { quads: Vec::new(), ..cave.clone() }));
    list
}

fn ceiling(height: f32) -> Quad {
    let mut q = Quad::ground(-height, height);
    q.color = [0.25, 0.25, 0.25, 1.0];
    q
}

/// Sun, moon phases and End flash with see-through texels (the bodies' discard), End sky texture.
fn sky_assets() -> SkyAssets {
    let image = |seed: u8, side: u32| {
        let rgba = (0..side * side)
            .flat_map(|i| {
                let (x, y) = (i % side, i / side);
                let alpha = if (x + y + u32::from(seed)) % 3 == 0 { 0 } else { 255 };
                [200u8.wrapping_add(seed * 7), 180u8.wrapping_sub(seed * 11), 120 + seed, alpha]
            })
            .collect();
        rc_assets::texture::Image { width: side, height: side, rgba }
    };
    SkyAssets { celestial: (0..10).map(|n| image(n, 8)).collect(), clouds: None, end_sky: Some(image(17, 16)) }
}

const OCCLUDER_SHADER: &str = r"
struct Occluder {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
    cutout: vec4<f32>,
};
@group(0) @binding(0) var<uniform> o: Occluder;

@vertex
fn vs(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
    return o.mvp * vec4<f32>(p, 1.0);
}

@fragment
fn fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    // Leaves: one block of 3 x 3 pixels in two thrown away, like the see-through texels.
    let cell = vec2<u32>(frag.xy) / 3u;
    if o.cutout.x > 0.0 && (cell.x + cell.y) % 2u == 0u {
        discard;
    }
    return o.color;
}
";

/// The sky pipelines of one order: dome and disc pipeline, bodies.
struct SkyDraw {
    plain_sky: wgpu::RenderPipeline,
    sky: Sky,
    order: SkyOrder,
    /// Sabotage: drawn after the terrain at its own depth instead of the farthest.
    own_depth: bool,
}

/// GPU objects shared by every image: camera binding, stand-in terrain pipelines, targets.
struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: (u32, u32),
    camera_layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    camera_bind: wgpu::BindGroup,
    occluder_layout: wgpu::BindGroupLayout,
    /// Opaque and leaves; water.
    solid: wgpu::RenderPipeline,
    water: wgpu::RenderPipeline,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    readback: wgpu::Buffer,
}

impl Rig {
    fn new(device: wgpu::Device, queue: wgpu::Queue, size: (u32, u32)) -> Rig {
        let camera_layout = crate::render::camera_layout(&device, false);
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind = camera_bind(&device, &camera_layout, &camera_buffer);
        let occluder_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let solid = occluder_pipeline(&device, &occluder_layout, None);
        let water = occluder_pipeline(&device, &occluder_layout, Some(wgpu::BlendState::ALPHA_BLENDING));
        let texture = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
                view_formats: &[],
            })
        };
        let color = texture(FORMAT, wgpu::TextureUsages::COPY_SRC);
        let color_view = color.create_view(&Default::default());
        let depth =
            texture(wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::empty()).create_view(&Default::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(size.0 * size.1 * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Rig {
            device,
            queue,
            size,
            camera_layout,
            camera_buffer,
            camera_bind,
            occluder_layout,
            solid,
            water,
            color,
            color_view,
            depth,
            readback,
        }
    }

    /// Real sky pipelines of `order`.
    fn sky_draw(&self, order: SkyOrder) -> SkyDraw {
        self.sky_draw_with(order, order.depth_compare(), false)
    }

    /// Sky pipelines of `order` with the dome's depth test `dome_test`; `own_depth`: drawn after the
    /// terrain at its own depth instead of the farthest (sabotages).
    fn sky_draw_with(&self, order: SkyOrder, dome_test: wgpu::CompareFunction, own_depth: bool) -> SkyDraw {
        let source = crate::terrain_shader::terrain(crate::terrain_shader::ShaderOpts::NEW);
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let plain_sky = plain_sky_pipeline(&self.device, &module, &self.camera_layout, FORMAT, dome_test);
        let sky = Sky::with_order(&self.device, &self.queue, FORMAT, &sky_assets(), order);
        SkyDraw { plain_sky, sky, order, own_depth }
    }

    /// Image (RGBA) of `view` with the sky of `draw`.
    fn render(&self, view: &View, draw: &mut SkyDraw) -> Vec<u8> {
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode_frame(&mut enc, view, draw, None);
        enc.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.size.0 * 4),
                    rows_per_image: Some(self.size.1),
                },
            },
            wgpu::Extent3d { width: self.size.0, height: self.size.1, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        read(&self.device, &self.queue, &self.readback)
    }

    /// Camera and sky uniforms of `view`, written before the frame.
    fn prepare(&self, view: &View, draw: &mut SkyDraw) {
        let proj = perspective(70.0, self.size.0 as f32 / self.size.1 as f32, 0.05, view.far);
        let sky_vp = proj * crate::camera::view_rotation(view.yaw, view.pitch);
        let camera = CameraUniform {
            view_proj: sky_vp.to_cols_array_2d(),
            fog_color: view.fog_color,
            fog_sky: [view.sky_fog_end, 0.0],
            sky_color: view.sky_color,
            sky_view_proj: sky_vp.to_cols_array_2d(),
            ..bytemuck::Zeroable::zeroed()
        };
        self.queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
        let mut up = Uploader::new(&self.device, true);
        draw.sky.prepare(&self.device, &self.queue, &mut up, sky_vp, &view.frame, view.parts.dome, [0.0, 64.0, 0.0]);
        up.submit(&self.queue, self.device.create_command_encoder(&Default::default()).finish());
    }

    /// The world pass of `view`: clear to the fog colour, then sky and terrain in the order of
    /// `draw` (sky before the terrain, or after its opaque and cutout part), then the water.
    fn encode_frame(
        &self,
        enc: &mut wgpu::CommandEncoder,
        view: &View,
        draw: &mut SkyDraw,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        self.prepare(view, draw);
        let proj = perspective(70.0, self.size.0 as f32 / self.size.1 as f32, 0.05, view.far);
        let vp = proj * crate::camera::view_rotation(view.yaw, view.pitch);
        let quads: Vec<(Quad, wgpu::Buffer, wgpu::BindGroup)> = view.quads.iter().map(|q| self.quad(vp, q)).collect();
        let c = view.fog_color.map(f64::from);
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: c[0], g: c[1], b: c[2], a: 0.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: timestamps,
            ..Default::default()
        });
        pass.set_bind_group(0, &self.camera_bind, &[]);
        if draw.order == SkyOrder::BeforeTerrain {
            encode_sky(&mut pass, &draw.plain_sky, &draw.sky, &self.camera_bind, view.parts);
        }
        pass.set_pipeline(&self.solid);
        for (_, vertices, bind) in quads.iter().filter(|(q, ..)| q.surface != Surface::Water) {
            draw_quad(&mut pass, vertices, bind);
        }
        if draw.order == SkyOrder::AfterOpaque {
            pass.set_bind_group(0, &self.camera_bind, &[]);
            if draw.own_depth {
                encode_sky(&mut pass, &draw.plain_sky, &draw.sky, &self.camera_bind, view.parts);
            } else {
                let sky = WorldSky { parts: view.parts, world_viewport: PassViewport::full(self.size) };
                encode_sky_behind_terrain(&mut pass, &draw.plain_sky, &draw.sky, &self.camera_bind, sky);
            }
        }
        pass.set_pipeline(&self.water);
        for (_, vertices, bind) in quads.iter().filter(|(q, ..)| q.surface == Surface::Water) {
            draw_quad(&mut pass, vertices, bind);
        }
    }

    /// Vertices and uniform of a stand-in quad seen through `vp`.
    fn quad(&self, vp: Mat4, q: &Quad) -> (Quad, wgpu::Buffer, wgpu::BindGroup) {
        let c = q.corners;
        let vertices: Vec<[f32; 3]> = [c[0], c[1], c[2], c[2], c[3], c[0]].iter().map(|v| v.to_array()).collect();
        let vertices = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mut uniform = [0f32; 24];
        uniform[..16].copy_from_slice(&vp.to_cols_array());
        uniform[16..20].copy_from_slice(&q.color);
        uniform[20] = if q.surface == Surface::Leaves { 1.0 } else { 0.0 };
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.occluder_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        (*q, vertices, bind)
    }
}

fn draw_quad(pass: &mut wgpu::RenderPass<'_>, vertices: &wgpu::Buffer, bind: &wgpu::BindGroup) {
    pass.set_bind_group(0, bind, &[]);
    pass.set_vertex_buffer(0, vertices.slice(..));
    pass.draw(0..6, 0..1);
}

/// Stand-in terrain pipeline with the depth state of the real terrain layers; `blend`: water.
fn occluder_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("stand-in terrain"),
        source: wgpu::ShaderSource::Wgsl(OCCLUDER_SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(layout)],
        ..Default::default()
    });
    const ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("stand-in terrain"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        // Every terrain layer writes its depth, the translucent one included.
        depth_stencil: Some(terrain_depth_state(true)),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Camera binding of the real layout: the uniform, and small stand-ins for the terrain resources
/// the sky never reads.
fn camera_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, camera: &wgpu::Buffer) -> wgpu::BindGroup {
    let storage = || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        })
    };
    let texture = |dimension| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor { dimension: Some(dimension), ..Default::default() })
    };
    let (buffers, array, flat) = (
        [storage(), storage(), storage(), storage()],
        texture(wgpu::TextureViewDimension::D2Array),
        texture(wgpu::TextureViewDimension::D2),
    );
    let sampler = device.create_sampler(&Default::default());
    use wgpu::BindingResource as R;
    let entry = |binding, resource| wgpu::BindGroupEntry { binding, resource };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("camera"),
        layout,
        entries: &[
            entry(0, camera.as_entire_binding()),
            entry(1, buffers[0].as_entire_binding()),
            entry(2, R::TextureView(&array)),
            entry(3, R::Sampler(&sampler)),
            entry(4, buffers[1].as_entire_binding()),
            entry(5, R::TextureView(&array)),
            entry(6, buffers[2].as_entire_binding()),
            entry(7, R::TextureView(&flat)),
            entry(8, R::Sampler(&sampler)),
            entry(9, buffers[3].as_entire_binding()),
        ],
    })
}

fn differing_pixels(a: &[u8], b: &[u8]) -> usize {
    a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0).filter(|(x, y)| x != y).count()
}

/// Every view drawn with the sky after the opaque terrain equals, to the byte, the same view
/// with the sky first; each view shows what it is named after; the sabotages change an image.
#[test]
fn sky_after_the_opaque_terrain_renders_identically_on_gpu() {
    let Some((device, queue)) = gpu() else { return };
    let rig = Rig::new(device, queue, (256, 144));
    let mut before = rig.sky_draw(SkyOrder::BeforeTerrain);
    let mut after = rig.sky_draw(SkyOrder::AfterOpaque);
    let views = views();
    let mut failures = Vec::new();
    for (view, without) in &views {
        let old = rig.render(view, &mut before);
        let shown = differing_pixels(&old, &rig.render(without, &mut before));
        assert!(shown > 20, "{}: what the view is named after does not show ({shown} pixels)", view.name);
        let differing = differing_pixels(&old, &rig.render(view, &mut after));
        eprintln!("{}: {shown} pixels show it, {differing} pixels differ between the orders", view.name);
        if differing != 0 {
            failures.push((view.name, differing));
        }
    }
    assert!(failures.is_empty(), "the sky after the opaque terrain changes the image: {failures:?}");
    // Cave: the sky is wholly hidden (same image as a frame without sky).
    let find = |name: &str| views.iter().find(|(v, _)| v.name == name).map(|(v, _)| v.clone()).expect("view");
    let cave = find("cave");
    let no_sky = View { parts: SkyParts { dome: false, dark_disc: false, end: false }, ..cave.clone() };
    assert!(rig.render(&cave, &mut after) == rig.render(&no_sky, &mut after), "cave: some sky shows");
    let day = find("day");
    // Sabotage: "less" in place of "greater or equal": the dome is never drawn.
    let mut less = rig.sky_draw_with(SkyOrder::AfterOpaque, wgpu::CompareFunction::Less, false);
    let lost = differing_pixels(&rig.render(&day, &mut before), &rig.render(&day, &mut less));
    assert!(lost > 1000, "sabotage \"less\" not seen: {lost} pixels");
    // Sabotage: after the terrain at its own depth, the dome covers the far mountain.
    let mut own_depth = rig.sky_draw_with(SkyOrder::AfterOpaque, SkyOrder::AfterOpaque.depth_compare(), true);
    let over = differing_pixels(&rig.render(&day, &mut before), &rig.render(&day, &mut own_depth));
    assert!(over > 100, "sky drawn at its own depth not seen: {over} pixels");
}

/// GPU time of the world pass of a frame in each sky order, 1600 × 900, cave, landscape and open
/// sky (`cargo test --release -p rc-app sky_order_cost -- --ignored --nocapture`). Timestamps of
/// the pass, so the CPU load of the machine does not enter the figure; orders alternate over
/// rounds; median and best.
#[test]
#[ignore]
fn sky_order_cost() {
    const FRAMES: u32 = 200;
    const ROUNDS: usize = 4;
    let Some((device, queue)) = gpu_with_timestamps() else {
        eprintln!("no hardware GPU with timestamp queries");
        return;
    };
    let rig = Rig::new(device, queue, (1600, 900));
    let mut draws = [
        ("sky before the terrain", rig.sky_draw(SkyOrder::BeforeTerrain)),
        ("sky after the opaque terrain", rig.sky_draw(SkyOrder::AfterOpaque)),
    ];
    let all = views();
    let pick = |name: &str| all.iter().find(|(v, _)| v.name == name).map(|(v, _)| v.clone()).expect("view");
    let open = View { name: "open sky", quads: Vec::new(), ..pick("day") };
    for view in [pick("cave"), pick("day"), open, pick("night")] {
        let mut times = vec![Vec::new(); draws.len()];
        for _ in 0..ROUNDS {
            for (t, (_, draw)) in times.iter_mut().zip(&mut draws) {
                t.extend(pass_times(&rig, &view, draw, FRAMES));
            }
        }
        println!("{} ({} frames per order):", view.name, FRAMES as usize * ROUNDS);
        for ((label, _), t) in draws.iter().zip(&mut times) {
            t.sort_by(f64::total_cmp);
            println!("    {label}: median {:.4} ms, best {:.4} ms", t[t.len() / 2], t[0]);
        }
    }
}

/// GPU time (ms) of the world pass of `view` over `frames` frames.
fn pass_times(rig: &Rig, view: &View, draw: &mut SkyDraw, frames: u32) -> Vec<f64> {
    let queries = rig.device.create_query_set(&wgpu::QuerySetDescriptor {
        label: None,
        ty: wgpu::QueryType::Timestamp,
        count: 2 * frames,
    });
    for f in 0..frames {
        let mut enc = rig.device.create_command_encoder(&Default::default());
        let stamps = wgpu::RenderPassTimestampWrites {
            query_set: &queries,
            beginning_of_pass_write_index: Some(2 * f),
            end_of_pass_write_index: Some(2 * f + 1),
        };
        rig.encode_frame(&mut enc, view, draw, Some(stamps));
        rig.queue.submit([enc.finish()]);
    }
    let resolved = rig.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16 * u64::from(frames),
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let mut enc = rig.device.create_command_encoder(&Default::default());
    enc.resolve_query_set(&queries, 0..2 * frames, &resolved, 0);
    rig.queue.submit([enc.finish()]);
    let ticks: Vec<u64> = bytemuck::cast_slice(&read(&rig.device, &rig.queue, &resolved)).to_vec();
    let period_ns = f64::from(rig.queue.get_timestamp_period());
    ticks.chunks(2).map(|t| t[1].saturating_sub(t[0]) as f64 * period_ns / 1e6).collect()
}
