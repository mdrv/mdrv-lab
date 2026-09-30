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

const TEX: u32 = 1024;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    nrm: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    light: [f32; 4],
}

const SHADER: &str = r#"
struct Uniforms {
    mvp: mat4x4<f32>,
    light: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) nrm: vec3<f32>,
};

@vertex
fn vs(@location(0) in_pos: vec3<f32>, @location(1) in_nrm: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.pos = u.mvp * vec4<f32>(in_pos, 1.0);
    out.nrm = in_nrm;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.nrm);
    let l = normalize(u.light.xyz);
    let diff = max(dot(n, l), 0.15);
    return vec4<f32>(vec3<f32>(0.98, 0.78, 0.05) * diff, 1.0);
}
"#;

struct DuckRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    vertex_count: u32,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    texture: wgpu::Texture,
}

impl DuckRenderer {
    fn new(handle: &WgpuContextHandle, model_path: &std::path::Path) -> anyhow::Result<Self> {
        let device = handle.device().clone();
        let queue = handle.queue().clone();

        let (doc, buffers, _images) = gltf::import(model_path)?;
        let mut verts: Vec<Vertex> = Vec::new();
        'meshes: for mesh in doc.meshes() {
            for prim in mesh.primitives() {
                let reader = prim.reader(|b| buffers.get(b.index()).map(|s| s.0.as_slice()));
                let Some(pos_iter) = reader.read_positions() else {
                    continue 'meshes;
                };
                let nrm_iter = reader.read_normals();
                for (i, p) in pos_iter.enumerate() {
                    let n = nrm_iter
                        .clone()
                        .and_then(|mut n| n.next())
                        .unwrap_or([0., 1., 0.]);
                    verts.push(Vertex { pos: p, nrm: n });
                }
                break 'meshes; // first primitive of the first mesh only
            }
        }
        anyhow::ensure!(!verts.is_empty(), "no vertices in {}", model_path.display());
        let vertex_count = verts.len() as u32;

        let vertex_buf = device.create_buffer_init(&wgpu::util::DeviceExt::BufferInitDescriptor {
            label: Some("duck-vb"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("duck-shader"),
            source: wgpu::ShaderSource::WGSL(SHADER.into()),
        });

        let uniform_buf = device.create_buffer_init(&wgpu::util::DeviceExt::BufferInitDescriptor {
            label: Some("duck-ub"),
            contents: bytemuck::bytes_of(&Uniforms {
                mvp: identity(),
                light: [-0.5, 0.8, 0.6, 0.],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("duck-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("duck-pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
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
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("duck-bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            vertex_buf,
            vertex_count,
            uniform_buf,
            bind_group,
            texture,
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
        let u = Uniforms {
            mvp: mul(proj, view),
            light: [-0.5, 0.8, 0.6, 0.],
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));

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
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            pass.draw(0..self.vertex_count, 0..1);
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
    let f = 1.0 / (fovy / 2.0).tan();
    let nf = 1.0 / (near - far);
    [
        [f / aspect, 0., 0., 0.],
        [0., f, 0., 0.],
        [0., 0., (far + near) * nf, -1.],
        [0., 0., 2. * far * near * nf, 0.],
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
    dragging: Option<gpui::Point<f32>>,
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
            dragging: None,
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
                            .px(3)
                            .py(1.5)
                            .mr_2()
                            .rounded(6.)
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
                        if let Some(start) = s.dragging {
                            s.yaw += (e.position.x - start.x) * 0.01;
                            s.pitch = (s.pitch + (e.position.y - start.y) * 0.01).clamp(-1.4, 1.4);
                            s.dragging = Some(e.position);
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|s, e: &gpui::MouseDownEvent, _, cx| {
                            s.dragging = Some(e.position);
                            s.auto = false;
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|s, _: &gpui::MouseUpEvent, _, cx| {
                            s.dragging = None;
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
        (
            window.request_layout(Style::default(), [], cx),
            (),
        )
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
        window.paint_surface(
            bounds,
            gpui::SurfaceSource::Texture {
                texture: Arc::new(self.texture.clone()),
                size: Size {
                    width: DevicePixels(TEX),
                    height: DevicePixels(TEX),
                },
            },
        );
    }
}

// Pixels import used by Bounds<Pixels> in the Element impl above.
#[allow(unused)]
fn _pixels_typecheck(_: Bounds<Pixels>) {}
