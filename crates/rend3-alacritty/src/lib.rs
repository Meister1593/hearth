use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::{Arc, RwLock, RwLockReadGuard, Weak};

use alacritty_terminal::ansi::{Color, CursorShape, NamedColor};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::color::{Colors, Rgb};
use alacritty_terminal::Term;
use bytemuck::{Pod, Zeroable};
use font_mud::glyph_atlas::GlyphAtlas;
use owned_ttf_parser::{AsFaceRef, OwnedFace};
use rend3::graph::{
    RenderGraph, RenderPassDepthTarget, RenderPassTarget, RenderPassTargets, RenderTargetHandle,
};
use rend3::Renderer;
use wgpu::util::DeviceExt;
use wgpu::*;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct CameraUniform {
    pub mvp: glam::Mat4,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SolidVertex {
    pub position: glam::Vec2,
    pub color: u32,
}

impl SolidVertex {
    pub const LAYOUT: VertexBufferLayout<'static> = VertexBufferLayout {
        array_stride: std::mem::size_of::<Self>() as BufferAddress,
        step_mode: VertexStepMode::Vertex,
        attributes: &[
            VertexAttribute {
                offset: 0,
                format: VertexFormat::Float32x2,
                shader_location: 0,
            },
            VertexAttribute {
                offset: std::mem::size_of::<[f32; 2]>() as BufferAddress,
                format: VertexFormat::Unorm8x4,
                shader_location: 1,
            },
        ],
    };
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GlyphVertex {
    pub position: glam::Vec2,
    pub tex_coords: glam::Vec2,
    pub color: u32,
}

impl GlyphVertex {
    pub const LAYOUT: VertexBufferLayout<'static> = VertexBufferLayout {
        array_stride: std::mem::size_of::<Self>() as BufferAddress,
        step_mode: VertexStepMode::Vertex,
        attributes: &[
            VertexAttribute {
                offset: 0,
                format: VertexFormat::Float32x2,
                shader_location: 0,
            },
            VertexAttribute {
                offset: std::mem::size_of::<[f32; 2]>() as BufferAddress,
                format: VertexFormat::Float32x2,
                shader_location: 1,
            },
            VertexAttribute {
                offset: std::mem::size_of::<[f32; 4]>() as BufferAddress,
                format: VertexFormat::Unorm8x4,
                shader_location: 2,
            },
        ],
    };
}

struct DynamicMesh<T> {
    vertex_buffer: Buffer,
    index_buffer: Buffer,
    index_num: u32,
    _data: PhantomData<T>,
}

impl<T: Pod> DynamicMesh<T> {
    pub fn new(device: &Device) -> Self {
        Self {
            vertex_buffer: device.create_buffer(&BufferDescriptor {
                label: Some("AlacrittyRoutine vertex buffer"),
                size: 0,
                mapped_at_creation: false,
                usage: BufferUsages::VERTEX,
            }),
            index_buffer: device.create_buffer(&BufferDescriptor {
                label: Some("AlacrittyRoutine vertex buffer"),
                size: 0,
                mapped_at_creation: false,
                usage: BufferUsages::INDEX,
            }),
            index_num: 0,
            _data: PhantomData,
        }
    }

    pub fn update(&mut self, device: &Device, vertices: &[T], indices: &[u32]) {
        self.vertex_buffer = device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("AlacrittyRoutine vertex buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: BufferUsages::VERTEX,
        });

        self.index_buffer = device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("AlacrittyRoutine index buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: BufferUsages::INDEX,
        });

        self.index_num = indices.len() as u32;
    }

    pub fn draw<'a>(&'a self, rpass: &mut RenderPass<'a>) {
        rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        rpass.set_index_buffer(self.index_buffer.slice(..), IndexFormat::Uint32);
        rpass.draw_indexed(0..self.index_num, 0, 0..1);
    }
}

/// A font face and its MSDF glyph atlas.
pub struct FaceAtlas {
    pub face: OwnedFace,
    pub atlas: GlyphAtlas,
    pub texture: Texture,
}

impl FaceAtlas {
    /// Create a new atlas from a face. Note that this takes time to complete.
    pub fn new(face: OwnedFace, device: &Device, queue: &Queue) -> Self {
        let (atlas, _errors) = GlyphAtlas::new(face.as_face_ref()).unwrap();

        let width = atlas.bitmap.width as u32;
        let height = atlas.bitmap.height as u32;

        let size = Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&TextureDescriptor {
            label: Some("AlacrittyRoutine::glyph_texture"),
            size: size.clone(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        });

        queue.write_texture(
            ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            atlas.bitmap.data_bytes(),
            ImageDataLayout {
                offset: 0,
                bytes_per_row: std::num::NonZeroU32::new(width * 4),
                rows_per_image: std::num::NonZeroU32::new(height),
            },
            size,
        );

        Self {
            face,
            atlas,
            texture,
        }
    }
}

/// CPU-side terminal rendering options.
pub struct TerminalConfig {
    pub normal_font: Arc<FaceAtlas>,
}

/// A single render-able terminal. Paired with an [AlacrittyRoutine].
pub struct Terminal {
    device: Arc<Device>,
    config: Arc<TerminalConfig>,
    bg_mesh: DynamicMesh<SolidVertex>,
    glyph_mesh: DynamicMesh<GlyphVertex>,
    overlay_mesh: DynamicMesh<SolidVertex>,
}

impl Terminal {
    pub fn update<T: alacritty_terminal::event::EventListener>(
        &mut self,
        term: &Term<T>,
        colors: &Colors,
    ) {
        let mut cells: Vec<(glam::Vec2, usize, u32)> = Vec::new();

        let color_to_rgb = |color| -> u32 {
            let rgb = match color {
                Color::Named(name) => colors[name].unwrap(),
                Color::Spec(rgb) => rgb,
                Color::Indexed(index) => colors[index as usize].unwrap_or(Rgb {
                    r: 255,
                    g: 0,
                    b: 255,
                }),
            };

            0xff000000 | ((rgb.b as u32) << 16) | ((rgb.g as u32) << 8) | (rgb.r as u32)
        };

        let scale = 1.0 / 30.0;
        let grid_to_pos = |x: i32, y: i32| -> glam::Vec2 {
            let col = x as f32 / 40.0 - 1.0;
            let row = (y as f32 + 1.0) / -30.0 + 1.0;
            glam::Vec2::new(col, row)
        };

        let mut bg_vertices = Vec::new();
        let mut bg_indices = Vec::new();
        let mut overlay_vertices = Vec::new();
        let mut overlay_indices = Vec::new();

        let content = term.renderable_content();
        for cell in content.display_iter.into_iter() {
            if cell.flags.contains(CellFlags::HIDDEN) {
                continue;
            }

            let col = cell.point.column.0 as i32;
            let row = cell.point.line.0;
            let pos = grid_to_pos(col, row);
            let mut fg = cell.fg;
            let mut bg = cell.bg;

            if cell.flags.contains(CellFlags::INVERSE) {
                let temp = fg;
                fg = bg;
                bg = temp;
            }

            let face = self.config.normal_font.face.as_face_ref();
            if let Some(glyph) = face.glyph_index(cell.c) {
                cells.push((pos, glyph.0 as usize, color_to_rgb(fg)));
            }

            if bg == Color::Named(NamedColor::Background) {
                continue;
            }

            let bg = color_to_rgb(bg);
            let index = bg_vertices.len() as u32;

            bg_vertices.extend_from_slice(&[
                SolidVertex {
                    position: grid_to_pos(col, row - 1),
                    color: bg,
                },
                SolidVertex {
                    position: grid_to_pos(col + 1, row - 1),
                    color: bg,
                },
                SolidVertex {
                    position: grid_to_pos(col, row),
                    color: bg,
                },
                SolidVertex {
                    position: grid_to_pos(col + 1, row),
                    color: bg,
                },
            ]);

            bg_indices.extend_from_slice(&[
                index,
                index + 1,
                index + 2,
                index + 2,
                index + 1,
                index + 3,
            ]);
        }

        let cursor_color = Color::Named(NamedColor::Foreground);
        let cursor_color = color_to_rgb(cursor_color);
        match content.cursor.shape {
            CursorShape::Hidden => {}
            _ => {
                let index = overlay_vertices.len() as u32;
                let col = content.cursor.point.column.0 as i32;
                let row = content.cursor.point.line.0;
                overlay_vertices.extend_from_slice(&[
                    SolidVertex {
                        position: grid_to_pos(col, row - 1),
                        color: cursor_color,
                    },
                    SolidVertex {
                        position: grid_to_pos(col + 1, row - 1),
                        color: cursor_color,
                    },
                    SolidVertex {
                        position: grid_to_pos(col, row),
                        color: cursor_color,
                    },
                    SolidVertex {
                        position: grid_to_pos(col + 1, row),
                        color: cursor_color,
                    },
                ]);

                overlay_indices.extend_from_slice(&[
                    index,
                    index + 1,
                    index + 2,
                    index + 2,
                    index + 1,
                    index + 3,
                ]);
            }
        }

        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        for (offset, glyph, color) in cells {
            let index = vertices.len() as u32;
            let atlas = &self.config.normal_font.atlas;
            let bitmap = match atlas.glyphs[glyph].as_ref() {
                Some(b) => b,
                None => continue,
            };

            vertices.extend(bitmap.vertices.iter().map(|v| GlyphVertex {
                position: v.position * scale + offset,
                tex_coords: v.tex_coords,
                color,
            }));

            indices.extend_from_slice(&[
                index,
                index + 1,
                index + 2,
                index + 2,
                index + 1,
                index + 3,
            ]);
        }

        self.bg_mesh.update(&self.device, &bg_vertices, &bg_indices);
        self.glyph_mesh.update(&self.device, &vertices, &indices);
        self.overlay_mesh
            .update(&self.device, &overlay_vertices, &overlay_indices);
    }
}

/// Generates a pipeline for either a glyph shader or a solid shader.
fn make_pipeline(
    device: &Device,
    label: Option<&str>,
    shader_module: &ShaderModule,
    vertex_layout: VertexBufferLayout,
    layout: &PipelineLayout,
    output_format: TextureFormat,
) -> RenderPipeline {
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label,
        layout: Some(layout),
        vertex: VertexState {
            module: shader_module,
            entry_point: "vs_main",
            buffers: &[vertex_layout],
        },
        depth_stencil: Some(DepthStencilState {
            format: TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: CompareFunction::GreaterEqual,
            stencil: StencilState::default(),
            bias: DepthBiasState::default(),
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: PolygonMode::Fill,
            conservative: false,
        },
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            module: shader_module,
            entry_point: "fs_main",
            targets: &[ColorTargetState {
                format: output_format,
                blend: Some(BlendState::ALPHA_BLENDING),
                write_mask: ColorWrites::COLOR,
            }],
        }),
        multiview: None,
    })
}

/// Persistent terminal rendering configuration and handles to active terminals.
pub struct TerminalStore {
    device: Arc<Device>,
    config: Arc<TerminalConfig>,
    camera_buffer: Buffer,
    bind_group: BindGroup,
    solid_pipeline: RenderPipeline,
    glyph_pipeline: RenderPipeline,
    terminals: Vec<Weak<RwLock<Terminal>>>,
    owned_terminals: Vec<Arc<RwLock<Terminal>>>,
}

impl TerminalStore {
    /// This routine runs after tonemapping, so `format` is the format of the
    /// final swapchain image format.
    pub fn new(config: Arc<TerminalConfig>, renderer: &Renderer, format: TextureFormat) -> Self {
        let solid_shader = renderer
            .device
            .create_shader_module(&include_wgsl!("solid.wgsl"));

        let glyph_shader = renderer
            .device
            .create_shader_module(&include_wgsl!("glyph.wgsl"));

        let bgl = renderer
            .device
            .create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("AlacrittyRoutine bind group layout"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            multisampled: false,
                            view_dimension: TextureViewDimension::D2,
                            sample_type: TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::VERTEX,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let layout = renderer
            .device
            .create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("AlacrittyRoutine pipeline layout"),
                bind_group_layouts: &[&bgl],
                push_constant_ranges: &[],
            });

        let solid_pipeline = make_pipeline(
            &renderer.device,
            Some("AlacrittyRoutine solid pipeline"),
            &solid_shader,
            SolidVertex::LAYOUT,
            &layout,
            format,
        );

        let glyph_pipeline = make_pipeline(
            &renderer.device,
            Some("AlacrittyRoutine glyph pipeline"),
            &glyph_shader,
            GlyphVertex::LAYOUT,
            &layout,
            format,
        );

        let atlas_view = config.normal_font.texture.create_view(&Default::default());
        let atlas_sampler = renderer.device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Nearest,
            ..Default::default()
        });

        let camera_buffer = renderer
            .device
            .create_buffer_init(&util::BufferInitDescriptor {
                label: Some("AlacrittyRoutine camera uniform"),
                contents: bytemuck::cast_slice(&[CameraUniform {
                    mvp: Default::default(),
                }]),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            });

        let bind_group = renderer.device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&atlas_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&atlas_sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: camera_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            device: renderer.device.to_owned(),
            config,
            camera_buffer,
            bind_group,
            solid_pipeline,
            glyph_pipeline,
            terminals: vec![],
            owned_terminals: vec![],
        }
    }

    /// Creates a new terminal associated with this store.
    ///
    /// When the last `Arc` is dropped, this terminal will stop being rendered.
    pub fn create_terminal(&mut self) -> Arc<RwLock<Terminal>> {
        let terminal = Arc::new(RwLock::new(Terminal {
            device: self.device.clone(),
            config: self.config.clone(),
            bg_mesh: DynamicMesh::new(&self.device),
            glyph_mesh: DynamicMesh::new(&self.device),
            overlay_mesh: DynamicMesh::new(&self.device),
        }));

        self.terminals.push(Arc::downgrade(&terminal));

        terminal
    }

    /// Creates a new render routine for this frame.
    ///
    /// Note that this locks the mutexes on all owned terminals for as long as
    /// this routine is owned. Create and then drop as quickly as possible!
    pub fn create_routine(&mut self) -> TerminalRenderRoutine<'_> {
        self.owned_terminals.clear();

        let mut index = 0;
        while let Some(terminal) = self.terminals.get(index) {
            if let Some(terminal) = terminal.upgrade() {
                self.owned_terminals.push(terminal);
                index += 1;
            } else {
                self.terminals.remove(index);
            }
        }

        let terminals = self
            .owned_terminals
            .iter()
            .map(|term| term.read().unwrap())
            .collect();

        TerminalRenderRoutine {
            store: self,
            terminals,
        }
    }

    /// Draws a single terminal to a render pass.
    pub fn draw_terminal<'a>(&'a self, rpass: &mut RenderPass<'a>, terminal: &'a Terminal) {
        rpass.set_bind_group(0, &self.bind_group, &[]);
        rpass.set_pipeline(&self.solid_pipeline);
        terminal.bg_mesh.draw(rpass);

        rpass.set_pipeline(&self.glyph_pipeline);
        terminal.glyph_mesh.draw(rpass);

        rpass.set_pipeline(&self.solid_pipeline);
        terminal.overlay_mesh.draw(rpass);
    }
}

pub struct TerminalRenderRoutine<'a> {
    store: &'a TerminalStore,
    terminals: Vec<RwLockReadGuard<'a, Terminal>>,
}

impl<'a> TerminalRenderRoutine<'a> {
    pub fn add_to_graph<'node>(
        &'node self,
        graph: &mut RenderGraph<'node>,
        output: RenderTargetHandle,
        depth: RenderTargetHandle,
    ) {
        let mut builder = graph.add_node("alacritty");
        let output_handle = builder.add_render_target_output(output);
        let depth_handle = builder.add_render_target_input(depth);
        let rpass_handle = builder.add_renderpass(RenderPassTargets {
            targets: vec![RenderPassTarget {
                color: output_handle,
                clear: rend3::types::Color::BLACK,
                resolve: None,
            }],
            depth_stencil: Some(RenderPassDepthTarget {
                target: rend3::graph::DepthHandle::RenderTarget(depth_handle),
                depth_clear: None,
                stencil_clear: None,
            }),
        });

        let store = builder.passthrough_ref(self.store);

        let terminals: Vec<_> = self
            .terminals
            .iter()
            .map(|t| builder.passthrough_ref(t.deref()))
            .collect();

        builder.build(
            move |pt, renderer, encoder_or_pass, _temps, _ready, graph_data| {
                let store = pt.get(store);
                let rpass = encoder_or_pass.get_rpass(rpass_handle);
                let view_proj = graph_data.camera_manager.view_proj();
                let model = glam::Mat4::IDENTITY;

                renderer.queue.write_buffer(
                    &store.camera_buffer,
                    0,
                    bytemuck::cast_slice(&[CameraUniform {
                        mvp: view_proj * model,
                    }]),
                );

                for terminal in terminals {
                    store.draw_terminal(rpass, pt.get(terminal));
                }
            },
        );
    }
}
