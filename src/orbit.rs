//! Orbit — Duck.glb rendered by our own wgpu pipeline on the platform
//! renderer's SHARED device (obtained via `Window::gpu_context_info`), then
//! composited into the GPUI scene with `Window::paint_surface`. This is the
//! GPU throughput / stability probe for the fork's Android surface path.

use gpui::prelude::*;
use gpui::{
    div, px, App, Bounds, Context, DevicePixels, Element, ElementId, GlobalElementId, IntoElement,
    InspectorElementId, LayoutId, ParentElement, Pixels, Render, Size, Styled, Style, Window,
};
use gpui_wgpu::WgpuContextHandle;
use std::panic;
use std::sync::Arc;
use wgpu::util::DeviceExt;

const TEX: u32 = 1024;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    light: [f32; 4],
    base_color: [f32; 4],
}

const SHADER: &str = r#"
struct Uniforms {
    mvp: mat4x4<f32>,
    light: vec4<f32>,
    base_color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var u_tex: texture_2d<f32>;
@group(0) @binding(2) var u_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) nrm: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) in_pos: vec3<f32>, @location(1) in_nrm: vec3<f32>, @location(2) in_uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.pos = u.mvp * vec4<f32>(in_pos, 1.0);
    out.nrm = in_nrm;
    out.uv = in_uv;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.nrm);
    let l = normalize(u.light.xyz);
    // Soft studio-ish shading: ambient + key + hemisphere sky fill.
    let key = max(dot(n, l), 0.0);
    let sky = clamp(0.5 + 0.5 * n.y, 0.0, 1.0);
    let amt = 0.35 + 0.55 * key + 0.25 * sky;
    let tex = textureSample(u_tex, u_samp, in.uv);
    return vec4<f32>(u.base_color.rgb * tex.rgb * amt, u.base_color.a * tex.a);
}
"#;

struct DuckPrim {
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    start: u32,
    count: u32,
    color: [f32; 4],
}

struct DuckRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    prims: Vec<DuckPrim>,
    texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
}

impl DuckRenderer {
    fn new(handle: &WgpuContextHandle, model_path: &std::path::Path) -> anyhow::Result<Self> {
        let device = handle.device().clone();
        let queue = handle.queue().clone();

        let (doc, buffers, images) = gltf::import(model_path)?;
        let mut verts: Vec<Vertex> = Vec::new();
        let mut prim_meta: Vec<(u32, u32, [f32; 4])> = Vec::new();
        'meshes: for mesh in doc.meshes() {
            for prim in mesh.primitives() {
                let reader = prim.reader(|b| buffers.get(b.index()).map(|s| s.0.as_slice()));
                let Some(pos_iter) = reader.read_positions() else {
                    continue 'meshes;
                };
                let pos: Vec<[f32; 3]> = pos_iter.collect();
                let nrm: Vec<[f32; 3]> =
                    reader.read_normals().map(|it| it.collect()).unwrap_or_default();
                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|it| it.into_f32().collect())
                    .unwrap_or_default();
                // Meshes are usually indexed; sequential position order is
                // NOT triangle order. Expand through the index buffer.
                let idx: Vec<u32> = reader
                    .read_indices()
                    .map(|it| it.into_u32().collect())
                    .unwrap_or_else(|| (0..pos.len() as u32).collect());
                let color = prim
                    .material()
                    .pbr_metallic_roughness()
                    .base_color_factor();
                let start = verts.len() as u32;
                for i in idx {
                    let Some(p) = pos.get(i as usize) else { continue };
                    let n = nrm
                        .get(i as usize)
                        .copied()
                        .unwrap_or([0., 1., 0.]);
                    let uv = uvs.get(i as usize).copied().unwrap_or([0., 0.]);
                    verts.push(Vertex { pos: *p, nrm: n, uv });
                }
                prim_meta.push((start, verts.len() as u32 - start, color));
            }
        }
        anyhow::ensure!(!verts.is_empty(), "no vertices in {}", model_path.display());
        let mut bmin = [f32::MAX; 3];
        let mut bmax = [f32::MIN; 3];
        for v in &verts {
            for i in 0..3 {
                bmin[i] = bmin[i].min(v.pos[i]);
                bmax[i] = bmax[i].max(v.pos[i]);
            }
        }
        // Node transforms are ignored above, so raw positions can be in any
        // unit/scale (this Duck.glb is ~165 units tall). Normalize: center
        // XZ, uniform-fit the largest dimension to 1.5, rest on y=0.
        let center = [
            (bmin[0] + bmax[0]) * 0.5,
            (bmin[1] + bmax[1]) * 0.5,
            (bmin[2] + bmax[2]) * 0.5,
        ];
        let extent = (bmax[0] - bmin[0])
            .max(bmax[1] - bmin[1])
            .max(bmax[2] - bmin[2]);
        let s = 1.5 / extent;
        for v in &mut verts {
            v.pos = [
                (v.pos[0] - center[0]) * s,
                (v.pos[1] - bmin[1]) * s,
                (v.pos[2] - center[2]) * s,
            ];
        }

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("duck-vb"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("duck-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        // Base-color texture: prefer the first material's PBR texture (the
        // Khronos duck paints its yellow body, black eyes and orange beak in
        // it); fall back to 1x1 white so sampling stays valid.
        let (tex_data, tex_w, tex_h) = doc
            .materials()
            .next()
            .and_then(|m| m.pbr_metallic_roughness().base_color_texture())
            .map(|t| t.texture().source().index())
            .and_then(|i| images.get(i))
            .map(|img| {
                // Decoded pixels may be RGB8; convert to RGBA8 for upload.
                let px = match img.format {
                    gltf::image::Format::R8G8B8A8 => img.pixels.clone(),
                    gltf::image::Format::R8G8B8 => {
                        let mut rgba = Vec::with_capacity(img.pixels.len() / 3 * 4);
                        for c in img.pixels.chunks_exact(3) {
                            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
                        }
                        rgba
                    }
                    _ => vec![255u8; 4],
                };
                (px, img.width, img.height)
            })
            .unwrap_or_else(|| (vec![255u8; 4], 1, 1));
        let model_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("duck-albedo"),
            size: wgpu::Extent3d {
                width: tex_w,
                height: tex_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &model_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &tex_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * tex_w),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: tex_w,
                height: tex_h,
                depth_or_array_layers: 1,
            },
        );
        let model_view = model_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("duck-samp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("duck-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("duck-pl"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("duck-tex"),
            size: wgpu::Extent3d {
                width: TEX,
                height: TEX,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let depth_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("duck-depth"),
                size: wgpu::Extent3d {
                    width: TEX,
                    height: TEX,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth24Plus,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("duck-pipe"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 24, shader_location: 2 },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let prims: Vec<DuckPrim> = prim_meta
            .iter()
            .map(|&(start, count, color)| {
                let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("duck-ub"),
                    size: std::mem::size_of::<Uniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("duck-bg"),
                    layout: &bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&model_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                });
                DuckPrim { uniform_buf, bind_group, start, count, color }
            })
            .collect();

        Ok(Self {
            device: device.into(),
            queue: queue.into(),
            pipeline,
            vertex_buf,
            prims,
            texture,
            depth_view,
        })
    }

    fn render_frame(&self, yaw: f32, pitch: f32, dist: f32) {
        let proj = perspective(50f32.to_radians(), 1.0, 0.1, 100.0);
        let eye = [
            dist * pitch.cos() * yaw.sin(),
            dist * pitch.sin(),
            dist * pitch.cos() * yaw.cos(),
        ];
        let view = look_at(eye, [0.0, 0.1, 0.0], [0.0, 1.0, 0.0]);
        let mvp = mul(proj, view);
        let light = [-0.5, 0.8, 0.6, 0.];
        for p in &self.prims {
            let u = Uniforms { mvp, light, base_color: p.color };
            self.queue.write_buffer(&p.uniform_buf, 0, bytemuck::bytes_of(&u));
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("duck-enc") });
        let view = self.texture.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("duck-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.07,
                            g: 0.09,
                            b: 0.15,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            for p in &self.prims {
                pass.set_bind_group(0, &p.bind_group, &[]);
                pass.draw(p.start..p.start + p.count, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }
}

fn identity() -> [[f32; 4]; 4] {
    [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ]
}

fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    // wgpu/WebGPU depth range is 0..1 (not OpenGL's -1..1).
    let f = 1.0 / (fovy / 2.0).tan();
    let nf = 1.0 / (near - far);
    [
        [f / aspect, 0., 0., 0.],
        [0., f, 0., 0.],
        [0., 0., far * nf, -1.],
        [0., 0., far * near * nf, 0.],
    ]
}

fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let norm = |a: [f32; 3]| {
        let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let z = norm(sub(eye, center));
    let x = norm(cross(up, z));
    let y = cross(z, x);
    [
        [x[0], y[0], z[0], 0.],
        [x[1], y[1], z[1], 0.],
        [x[2], y[2], z[2], 0.],
        [-dot(x, eye), -dot(y, eye), -dot(z, eye), 1.],
    ]
}

fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0f32; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            o[c][r] = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    o
}

pub struct OrbitScreen {
    renderer: Option<DuckRenderer>,
    yaw: f32,
    pitch: f32,
    dist: f32,
    auto: bool,
    prev_drag: Option<gpui::Point<gpui::Pixels>>,
    err: Option<String>,
}

impl OrbitScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(33))
                .await;
            if this
                .update(cx, |s, cx| {
                    if s.auto {
                        s.yaw += 0.03;
                        cx.notify();
                    }
                })
                .is_err()
            {
                return;
            }
        })
        .detach();

        Self {
            renderer: None,
            yaw: 0.6,
            pitch: 0.35,
            dist: 3.5,
            auto: true,
            prev_drag: None,
            err: None,
        }
    }
}

impl Render for OrbitScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.renderer.is_none() {
            if let Some(any_info) = window.gpu_context_info() {
                match any_info.downcast_ref::<WgpuContextHandle>() {
                    Some(handle) => {
                        match DuckRenderer::new(handle, &crate::gallery::asset_path("Duck.glb")) {
                            Ok(r) => self.renderer = Some(r),
                            Err(e) => self.err = Some(format!("duck init: {e}")),
                        }
                    }
                    None => self.err = Some("gpu context is not a WgpuContextHandle".into()),
                }
            } else {
                self.err = Some("waiting for gpu context…".into());
            }
        }

        if let Some(r) = self.renderer.as_ref() {
            r.render_frame(self.yaw, self.pitch, self.dist);
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .p_2()
                    .text_size(px(13.))
                    .child(
                        div()
                            .id("auto")
                            .px(px(3.))
                            .py(px(1.5))
                            .mr_2()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .bg(if self.auto {
                                gpui::rgb(0x1d4ed8)
                            } else {
                                gpui::rgb(0x1f2937)
                            })
                            .child("auto-rotate")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|s, _: &gpui::MouseDownEvent, _, cx| {
                                    s.auto = !s.auto;
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_color(gpui::rgb(0x6b6b74))
                            .child(
                                self.err
                                    .clone()
                                    .unwrap_or_else(|| {
                                        format!("yaw {:.2} pitch {:.2}", self.yaw, self.pitch)
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("orbit-canvas")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(gpui::rgb(0x0a0a0d))
                    .on_mouse_move(cx.listener(|s, e: &gpui::MouseMoveEvent, _, cx| {
                        // Orbit on any pressed move. Android touch drags emit
                        // MouseMove(pressed) WITHOUT a MouseDown (gpui-mobile
                        // reserves Down/Up for taps), so don't gate on down.
                        if e.pressed_button == Some(gpui::MouseButton::Left) {
                            if let Some(prev) = s.prev_drag {
                                // Drag right should spin the duck's face to the
                                // right (grab metaphor) → camera orbits the
                                // other way, so yaw sign is inverted.
                                s.yaw -= f32::from(e.position.x - prev.x) * 0.01;
                                s.pitch = (s.pitch + f32::from(e.position.y - prev.y) * 0.01)
                                    .clamp(-1.4, 1.4);
                                cx.notify();
                            }
                            s.prev_drag = Some(e.position);
                        } else if s.prev_drag.is_some() {
                            s.prev_drag = None;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|s, e: &gpui::MouseDownEvent, _, cx| {
                            s.prev_drag = Some(e.position);
                            s.auto = false;
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|s, _: &gpui::MouseUpEvent, _, cx| {
                            s.prev_drag = None;
                            cx.notify();
                        }),
                    )
                    .when_some(self.renderer.as_ref().map(|r| r.texture.clone()), |d, t| {
                        d.child(OrbitCanvas { texture: t })
                    }),
            )
    }
}

/// Full-bleed element painting the duck texture via paint_surface.
struct OrbitCanvas {
    texture: wgpu::Texture,
}

impl IntoElement for OrbitCanvas {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for OrbitCanvas {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size = gpui::size(
            gpui::DefiniteLength::from(gpui::relative(1.)).into(),
            gpui::DefiniteLength::from(gpui::relative(1.)).into(),
        );
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        eprintln!("mdrv-lab: OrbitCanvas::paint bounds={bounds:?}");
        window.paint_surface(
            bounds,
            gpui::SurfaceSource::Texture {
                texture: Arc::new(self.texture.clone()),
                size: Size {
                    width: DevicePixels(TEX as i32),
                    height: DevicePixels(TEX as i32),
                },
            },
        );
    }
}
