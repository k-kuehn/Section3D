//! wgpu renderer for the 3D viewport. The scene is drawn into an offscreen
//! MSAA colour+depth target in `prepare`, then blitted into egui's pass.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use glam::{Mat4, Vec3, Vec4};
use s3d_core::mesh::Mesh;
use wgpu::util::DeviceExt;

const MSAA: u32 = 4;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

pub struct MeshRef {
    pub id: u64,
    pub rev: u64,
    pub mesh: Arc<Mesh>,
}

#[derive(Clone, Debug)]
pub struct DrawItem {
    pub mesh: u64,
    pub model: Mat4,
    pub print: Mat4,
    pub color: [f32; 4],
    pub highlight: [f32; 4],
    pub flags: u32,
    /// Cut planes (normal, offset) in the mesh's local frame, for highlighting.
    pub planes: Vec<Vec4>,
}

pub const FLAG_SUPPORT: u32 = 1;
pub const FLAG_SELECTED: u32 = 2;
pub const FLAG_GHOST: u32 = 4;
pub const FLAG_HOVER: u32 = 8;

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
pub struct LineSeg {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub color: [f32; 4],
    pub width: f32,
}

impl LineSeg {
    pub fn new(a: Vec3, b: Vec3, color: [f32; 4], width: f32) -> Self {
        LineSeg { a: a.into(), b: b.into(), color, width }
    }
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
pub struct TriVert {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

#[derive(Default)]
pub struct SceneFrame {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub clip: Option<Vec4>,
    pub overhang_sin: f32,
    pub cap_color: [f32; 4],
    pub support_color: [f32; 4],
    pub bg: [f32; 3],
    pub items: Vec<DrawItem>,
    pub lines: Vec<LineSeg>,
    pub overlay_lines: Vec<LineSeg>,
    pub tris: Vec<TriVert>,
    /// Every mesh the document currently owns; others are evicted from the GPU.
    pub meshes: Vec<MeshRef>,
    pub size_px: [u32; 2],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    clip: [f32; 4],
    params: [f32; 4],
    cap_color: [f32; 4],
    support_color: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct ObjectGpu {
    model: [[f32; 4]; 4],
    print: [[f32; 4]; 4],
    color: [f32; 4],
    highlight: [f32; 4],
    flags: [u32; 4],
}

struct GpuMesh {
    rev: u64,
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
}

struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolved: wgpu::TextureView,
    blit_bg: wgpu::BindGroup,
}

struct GrowBuffer {
    buf: wgpu::Buffer,
    cap: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl GrowBuffer {
    fn new(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages, cap: u64) -> Self {
        let usage = usage | wgpu::BufferUsages::COPY_DST;
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: cap, usage, mapped_at_creation: false });
        GrowBuffer { buf, cap, usage, label }
    }
    /// Writes `data`; returns true if the buffer was reallocated.
    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) -> bool {
        let need = (data.len() as u64).max(16).next_multiple_of(16);
        let mut grown = false;
        if need > self.cap {
            self.cap = need.next_power_of_two();
            self.buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some(self.label), size: self.cap, usage: self.usage, mapped_at_creation: false });
            grown = true;
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buf, 0, data);
        }
        grown
    }
}

pub struct Renderer3d {
    format: wgpu::TextureFormat,
    mesh_opaque: wgpu::RenderPipeline,
    mesh_ghost: wgpu::RenderPipeline,
    line_depth: wgpu::RenderPipeline,
    line_overlay: wgpu::RenderPipeline,
    tri: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    scene_bgl: wgpu::BindGroupLayout,
    blit_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    globals: wgpu::Buffer,
    objects: GrowBuffer,
    planes: GrowBuffer,
    lines: GrowBuffer,
    tris: GrowBuffer,
    scene_bg: wgpu::BindGroup,
    meshes: HashMap<u64, GpuMesh>,
    targets: Option<Targets>,
    // Draw lists recorded by prepare.
    n_lines: u32,
    n_overlay: u32,
    n_tris: u32,
}

fn uniform_entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty, count: None }
}

impl Renderer3d {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("scene"), source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()) });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("blit"), source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()) });
        let storage = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None };
        let scene_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene"),
            entries: &[
                uniform_entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                uniform_entry(1, storage),
                uniform_entry(2, storage),
            ],
        });
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                uniform_entry(0, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
                uniform_entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("scene"), bind_group_layouts: &[Some(&scene_bgl)], immediate_size: 0 });
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("blit"), bind_group_layouts: &[Some(&blit_bgl)], immediate_size: 0 });

        let ms = wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false };
        let depth = |write: bool, compare: wgpu::CompareFunction| {
            Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(write), depth_compare: Some(compare), stencil: Default::default(), bias: Default::default() })
        };
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let mesh_vb = [wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3] }];
        let line_vb = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineSeg>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32],
        }];
        let tri_vb = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TriVert>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
        }];
        let pipe = |label: &str,
                    vs: &str,
                    fs: &str,
                    vb: &[wgpu::VertexBufferLayout],
                    blend: Option<wgpu::BlendState>,
                    depth: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&scene_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers: vb },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: depth,
                multisample: ms,
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let mesh_opaque = pipe("mesh", "vs_mesh", "fs_mesh", &mesh_vb, None, depth(true, wgpu::CompareFunction::Less));
        let mesh_ghost = pipe("mesh-ghost", "vs_mesh", "fs_mesh", &mesh_vb, blend, depth(false, wgpu::CompareFunction::Less));
        let line_depth = pipe("lines", "vs_line", "fs_line", &line_vb, blend, depth(false, wgpu::CompareFunction::LessEqual));
        let line_overlay = pipe("lines-overlay", "vs_line", "fs_line", &line_vb, blend, depth(false, wgpu::CompareFunction::Always));
        let tri = pipe("tris", "vs_tri", "fs_tri", &tri_vb, blend, depth(false, wgpu::CompareFunction::LessEqual));
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState { module: &blit_shader, entry_point: Some("vs_blit"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_blit"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globals"),
            contents: bytemuck::bytes_of(&Globals::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let objects = GrowBuffer::new(device, "objects", wgpu::BufferUsages::STORAGE, 64 * std::mem::size_of::<ObjectGpu>() as u64);
        let planes = GrowBuffer::new(device, "planes", wgpu::BufferUsages::STORAGE, 4096);
        let lines = GrowBuffer::new(device, "lines", wgpu::BufferUsages::VERTEX, 64 * 1024);
        let tris = GrowBuffer::new(device, "tris", wgpu::BufferUsages::VERTEX, 64 * 1024);
        let scene_bg = Self::make_scene_bg(device, &scene_bgl, &globals, &objects.buf, &planes.buf);
        Renderer3d {
            format,
            mesh_opaque,
            mesh_ghost,
            line_depth,
            line_overlay,
            tri,
            blit,
            scene_bgl,
            blit_bgl,
            sampler,
            globals,
            objects,
            planes,
            lines,
            tris,
            scene_bg,
            meshes: HashMap::new(),
            targets: None,
            n_lines: 0,
            n_overlay: 0,
            n_tris: 0,
        }
    }

    fn make_scene_bg(device: &wgpu::Device, bgl: &wgpu::BindGroupLayout, globals: &wgpu::Buffer, objects: &wgpu::Buffer, planes: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: objects.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: planes.as_entire_binding() },
            ],
        })
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let tex = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let msaa = tex("vp-msaa", self.format, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = tex("vp-depth", DEPTH, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolved = tex("vp-resolved", self.format, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        let blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&resolved) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.targets = Some(Targets { size, msaa, depth, resolved, blit_bg });
    }

    fn sync_meshes(&mut self, device: &wgpu::Device, frame: &SceneFrame) {
        let live: std::collections::HashSet<u64> = frame.meshes.iter().map(|m| m.id).collect();
        self.meshes.retain(|id, _| live.contains(id));
        for m in &frame.meshes {
            if self.meshes.get(&m.id).is_some_and(|g| g.rev == m.rev) {
                continue;
            }
            let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-vb"),
                contents: bytemuck::cast_slice(&m.mesh.verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-ib"),
                contents: bytemuck::cast_slice(&m.mesh.tris),
                usage: wgpu::BufferUsages::INDEX,
            });
            self.meshes.insert(m.id, GpuMesh { rev: m.rev, vb, ib, count: m.mesh.tris.len() as u32 * 3 });
        }
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, frame: &SceneFrame) {
        self.ensure_targets(device, frame.size_px);
        self.sync_meshes(device, frame);

        let clip = frame.clip.unwrap_or(Vec4::ZERO);
        let g = Globals {
            view_proj: frame.view_proj.to_cols_array_2d(),
            eye: frame.eye.extend(1.0).into(),
            clip: clip.into(),
            params: [frame.overhang_sin, if frame.clip.is_some() { 1.0 } else { 0.0 }, frame.size_px[0] as f32, frame.size_px[1] as f32],
            cap_color: frame.cap_color,
            support_color: frame.support_color,
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));

        let mut objs = Vec::with_capacity(frame.items.len());
        let mut planes: Vec<[f32; 4]> = Vec::new();
        for it in &frame.items {
            let first = planes.len() as u32;
            planes.extend(it.planes.iter().map(|p| p.to_array()));
            objs.push(ObjectGpu {
                model: it.model.to_cols_array_2d(),
                print: it.print.to_cols_array_2d(),
                color: it.color,
                highlight: it.highlight,
                flags: [it.flags, first, it.planes.len() as u32, 0],
            });
        }
        if planes.is_empty() {
            planes.push([0.0; 4]);
        }
        if objs.is_empty() {
            objs.push(ObjectGpu::zeroed());
        }
        let g1 = self.objects.write(device, queue, bytemuck::cast_slice(&objs));
        let g2 = self.planes.write(device, queue, bytemuck::cast_slice(&planes));
        if g1 || g2 {
            self.scene_bg = Self::make_scene_bg(device, &self.scene_bgl, &self.globals, &self.objects.buf, &self.planes.buf);
        }
        let mut all_lines = frame.lines.clone();
        all_lines.extend_from_slice(&frame.overlay_lines);
        self.lines.write(device, queue, bytemuck::cast_slice(&all_lines));
        self.n_lines = frame.lines.len() as u32;
        self.n_overlay = frame.overlay_lines.len() as u32;
        self.tris.write(device, queue, bytemuck::cast_slice(&frame.tris));
        self.n_tris = frame.tris.len() as u32;

        let t = self.targets.as_ref().unwrap();
        let bg = frame.bg;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.msaa,
                depth_slice: None,
                resolve_target: Some(&t.resolved),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: bg[0] as f64, g: bg[1] as f64, b: bg[2] as f64, a: 1.0 }),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.scene_bg, &[]);
        for ghost in [false, true] {
            pass.set_pipeline(if ghost { &self.mesh_ghost } else { &self.mesh_opaque });
            for (i, it) in frame.items.iter().enumerate() {
                if (it.flags & FLAG_GHOST != 0) != ghost {
                    continue;
                }
                let Some(m) = self.meshes.get(&it.mesh) else { continue };
                pass.set_vertex_buffer(0, m.vb.slice(..));
                pass.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..m.count, 0, i as u32..i as u32 + 1);
            }
            if !ghost && self.n_tris > 0 {
                // Translucent planes after opaque geometry, before ghosts.
                pass.set_pipeline(&self.tri);
                pass.set_vertex_buffer(0, self.tris.buf.slice(..));
                pass.draw(0..self.n_tris, 0..1);
            }
        }
        if self.n_lines > 0 {
            pass.set_pipeline(&self.line_depth);
            pass.set_vertex_buffer(0, self.lines.buf.slice(..));
            pass.draw(0..6, 0..self.n_lines);
        }
        if self.n_overlay > 0 {
            pass.set_pipeline(&self.line_overlay);
            pass.set_vertex_buffer(0, self.lines.buf.slice(..));
            pass.draw(0..6, self.n_lines..self.n_lines + self.n_overlay);
        }
    }

    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>) {
        if let Some(t) = &self.targets {
            pass.set_pipeline(&self.blit);
            pass.set_bind_group(0, &t.blit_bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// egui paint callback carrying one frame of the scene.
pub struct ViewportCallback {
    pub frame: Arc<SceneFrame>,
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(r) = resources.get_mut::<Renderer3d>() {
            r.prepare(device, queue, encoder, &self.frame);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        if let Some(r) = resources.get::<Renderer3d>() {
            r.paint(pass);
        }
    }
}
