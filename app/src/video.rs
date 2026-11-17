//! Video tiles on the GPU: planar YUV frames uploaded as three textures and
//! converted to RGB by a shader, drawn inside an egui paint callback.

use std::borrow::Cow;
use std::sync::Arc;

use eframe::egui;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;

/// One decoded picture, planar I420.
pub struct Frame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

const SHADER: &str = r#"
struct Uniforms { size_px: vec2<f32>, radius_px: f32, _pad: f32 };
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@group(0) @binding(0) var tex_y: texture_2d<f32>;
@group(0) @binding(1) var tex_u: texture_2d<f32>;
@group(0) @binding(2) var tex_v: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var<uniform> u: Uniforms;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var o: VsOut;
    o.pos = vec4(p[i], 0.0, 1.0);
    o.uv = vec2((p[i].x + 1.0) * 0.5, 1.0 - (p[i].y + 1.0) * 0.5);
    return o;
}

fn rounded_alpha(uv: vec2<f32>) -> f32 {
    let half = u.size_px * 0.5;
    let q = abs(uv * u.size_px - half) - (half - vec2(u.radius_px));
    let d = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - u.radius_px;
    return clamp(0.5 - d, 0.0, 1.0);
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let y = textureSample(tex_y, samp, in.uv).r;
    let cb = textureSample(tex_u, samp, in.uv).r - 0.5;
    let cr = textureSample(tex_v, samp, in.uv).r - 0.5;
    let yy = 1.1643836 * (y - 0.0627451);
    let rgb = vec3(
        yy + 1.5960268 * cr,
        yy - 0.3917623 * cb - 0.8129676 * cr,
        yy + 2.0172321 * cb,
    );
    return vec4(clamp(rgb, vec3(0.0), vec3(1.0)), rounded_alpha(in.uv));
}
"#;

struct TileGpu {
    textures: [wgpu::Texture; 3],
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
    last_seq: u64,
}

/// Shared GPU state, kept in egui's callback resources.
pub struct VideoGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    tiles: Vec<Option<TileGpu>>,
}

impl VideoGpu {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("video yuv"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
            tiles: Vec::new(),
        }
    }

    fn ensure_tile(&mut self, device: &wgpu::Device, id: usize, width: u32, height: u32) {
        if self.tiles.len() <= id {
            self.tiles.resize_with(id + 1, || None);
        }
        if matches!(&self.tiles[id], Some(tile) if tile.size == (width, height)) {
            return;
        }
        let make = |w: u32, h: u32| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video plane"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let textures = [
            make(width, height),
            make(width / 2, height / 2),
            make(width / 2, height / 2),
        ];
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("video uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let views: Vec<wgpu::TextureView> = textures
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        self.tiles[id] = Some(TileGpu {
            textures,
            uniform,
            bind_group,
            size: (width, height),
            last_seq: u64::MAX,
        });
    }
}

/// Draws tile `id` with `frame` into the rectangle it was created for.
pub struct VideoCallback {
    pub id: usize,
    pub frame: Arc<Frame>,
    pub rect: egui::Rect,
    /// Corner radius in points.
    pub radius: f32,
}

impl CallbackTrait for VideoCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<VideoGpu>() else {
            return Vec::new();
        };
        let frame = &self.frame;
        gpu.ensure_tile(device, self.id, frame.width, frame.height);
        if let Some(tile) = gpu.tiles[self.id].as_mut() {
            if tile.last_seq != frame.seq {
                tile.last_seq = frame.seq;
                let planes: [(&Vec<u8>, u32, u32); 3] = [
                    (&frame.y, frame.width, frame.height),
                    (&frame.u, frame.width / 2, frame.height / 2),
                    (&frame.v, frame.width / 2, frame.height / 2),
                ];
                for (texture, (data, w, h)) in tile.textures.iter().zip(planes) {
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        data,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(w),
                            rows_per_image: Some(h),
                        },
                        wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
            let ppp = screen.pixels_per_point;
            let mut bytes = [0u8; 16];
            bytes[0..4].copy_from_slice(&(self.rect.width() * ppp).to_ne_bytes());
            bytes[4..8].copy_from_slice(&(self.rect.height() * ppp).to_ne_bytes());
            bytes[8..12].copy_from_slice(&(self.radius * ppp).to_ne_bytes());
            queue.write_buffer(&tile.uniform, 0, &bytes);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(gpu) = resources.get::<VideoGpu>() else {
            return;
        };
        let Some(Some(tile)) = gpu.tiles.get(self.id) else {
            return;
        };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &tile.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Turns a callback into an egui shape.
pub fn shape(callback: VideoCallback) -> egui::Shape {
    egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(
        callback.rect,
        callback,
    ))
}
