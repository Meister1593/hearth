use std::sync::Arc;

use rend3::{InstanceAdapterDevice, Renderer};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use winit::window::{Window as WinitWindow, WindowBuilder};

fn vertex(pos: [f32; 3]) -> glam::Vec3 {
    glam::Vec3::from(pos)
}

fn create_mesh() -> rend3::types::Mesh {
    let vertex_positions = [
        // far side (0.0, 0.0, 1.0)
        vertex([-1.0, -1.0, 1.0]),
        vertex([1.0, -1.0, 1.0]),
        vertex([1.0, 1.0, 1.0]),
        vertex([-1.0, 1.0, 1.0]),
        // near side (0.0, 0.0, -1.0)
        vertex([-1.0, 1.0, -1.0]),
        vertex([1.0, 1.0, -1.0]),
        vertex([1.0, -1.0, -1.0]),
        vertex([-1.0, -1.0, -1.0]),
        // right side (1.0, 0.0, 0.0)
        vertex([1.0, -1.0, -1.0]),
        vertex([1.0, 1.0, -1.0]),
        vertex([1.0, 1.0, 1.0]),
        vertex([1.0, -1.0, 1.0]),
        // left side (-1.0, 0.0, 0.0)
        vertex([-1.0, -1.0, 1.0]),
        vertex([-1.0, 1.0, 1.0]),
        vertex([-1.0, 1.0, -1.0]),
        vertex([-1.0, -1.0, -1.0]),
        // top (0.0, 1.0, 0.0)
        vertex([1.0, 1.0, -1.0]),
        vertex([-1.0, 1.0, -1.0]),
        vertex([-1.0, 1.0, 1.0]),
        vertex([1.0, 1.0, 1.0]),
        // bottom (0.0, -1.0, 0.0)
        vertex([1.0, -1.0, 1.0]),
        vertex([-1.0, -1.0, 1.0]),
        vertex([-1.0, -1.0, -1.0]),
        vertex([1.0, -1.0, -1.0]),
    ];

    let index_data = &[
        0, 1, 2, 2, 3, 0, // far
        4, 5, 6, 6, 7, 4, // near
        8, 9, 10, 10, 11, 8, // right
        12, 13, 14, 14, 15, 12, // left
        16, 17, 18, 18, 19, 16, // top
        20, 21, 22, 22, 23, 20, // bottom
    ];

    rend3::types::MeshBuilder::new(vertex_positions.to_vec(), rend3::types::Handedness::Left)
        .with_indices(index_data.to_vec())
        .build()
        .unwrap()
}

/// A message sent from the rest of the program to a window.
#[derive(Clone, Debug)]
pub enum WindowRxMessage {
    /// The window is requested to quit.
    Quit,
}

/// A message sent from a window to the rest of the program.
#[derive(Clone, Debug)]
pub enum WindowTxMessage {
    /// The window has been requested to quit.
    Quit,
}

/// Message sent from the window on initialization.
#[derive(Debug)]
pub struct WindowOffer {
    pub event_rx: EventLoopProxy<WindowRxMessage>,
    pub event_tx: mpsc::UnboundedReceiver<WindowTxMessage>,
}

pub struct Window {
    event_tx: mpsc::UnboundedSender<WindowTxMessage>,
    window: WinitWindow,
    iad: InstanceAdapterDevice,
    surface: Arc<wgpu::Surface>,
    config: wgpu::SurfaceConfiguration,
    renderer: Arc<Renderer>,
    pbr_routine: rend3_routine::pbr::PbrRoutine,
    tonemapping_routine: rend3_routine::tonemapping::TonemappingRoutine,
    base_rendergraph: rend3_routine::base::BaseRenderGraph,
    _object_handle: rend3::types::ResourceHandle<rend3::types::Object>,
    _directional_handle: rend3::types::ResourceHandle<rend3::types::DirectionalLight>,
}

impl Window {
    pub async fn new(event_loop: &EventLoop<WindowRxMessage>) -> (Self, WindowOffer) {
        let window = WindowBuilder::new()
            .with_title("Hearth Client")
            .with_inner_size(winit::dpi::LogicalSize::new(128.0, 128.0))
            .build(&event_loop)
            .unwrap();

        let size = window.inner_size();
        let swapchain_format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let iad = rend3::create_iad(None, None, None, None).await.unwrap();
        let surface = unsafe { iad.instance.create_surface(&window) };
        let surface = Arc::new(surface);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: swapchain_format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Immediate,
        };

        surface.configure(&iad.device, &config);
        let (event_rx, event_tx) = mpsc::unbounded_channel();

        let renderer =
            rend3::Renderer::new(iad.to_owned(), rend3::types::Handedness::Left, None).unwrap();

        let base_rendergraph = rend3_routine::base::BaseRenderGraph::new(&renderer);

        let mut data_core = renderer.data_core.lock();
        let pbr_routine = rend3_routine::pbr::PbrRoutine::new(
            &renderer,
            &mut data_core,
            &base_rendergraph.interfaces,
        );

        drop(data_core);

        let tonemapping_routine = rend3_routine::tonemapping::TonemappingRoutine::new(
            &renderer,
            &base_rendergraph.interfaces,
            swapchain_format,
        );

        let mesh = create_mesh();
        let mesh_handle = renderer.add_mesh(mesh);

        let material = rend3_routine::pbr::PbrMaterial {
            albedo: rend3_routine::pbr::AlbedoComponent::Value(glam::Vec4::new(0.0, 0.5, 0.5, 1.0)),
            ..Default::default()
        };

        let material_handle = renderer.add_material(material);

        let object = rend3::types::Object {
            mesh_kind: rend3::types::ObjectMeshKind::Static(mesh_handle),
            material: material_handle,
            transform: glam::Mat4::IDENTITY,
        };

        let object_handle = renderer.add_object(object);

        let view_location = glam::Vec3::new(3.0, 3.0, -5.0);
        let view = glam::Mat4::from_euler(glam::EulerRot::XYZ, -0.55, 0.5, 0.0);
        let view = view * glam::Mat4::from_translation(-view_location);

        renderer.set_camera_data(rend3::types::Camera {
            projection: rend3::types::CameraProjection::Perspective {
                vfov: 60.0,
                near: 0.1,
            },
            view,
        });

        let directional_handle = renderer.add_directional_light(rend3::types::DirectionalLight {
            color: glam::Vec3::ONE,
            intensity: 10.0,
            direction: glam::Vec3::new(-1.0, -4.0, 2.0),
            distance: 400.0,
        });

        let window = Self {
            event_tx: event_rx,
            window,
            iad,
            surface,
            config,
            renderer,
            base_rendergraph,
            pbr_routine,
            tonemapping_routine,
            _object_handle: object_handle,
            _directional_handle: directional_handle,
        };

        let offer = WindowOffer {
            event_rx: event_loop.create_proxy(),
            event_tx,
        };

        (window, offer)
    }

    pub fn on_resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.iad.device, &self.config);
        self.window.request_redraw();
        self.renderer
            .set_aspect_ratio(size.width as f32 / size.height as f32);
    }

    pub fn on_draw(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated) => {
                let size = self.window.inner_size();
                self.on_resize(size);
                return;
            }
            Err(err) => {
                tracing::error!("Surface error: {:?}", err);
                return;
            }
        };

        let frame = rend3::util::output::OutputFrame::SurfaceAcquired {
            view: frame.texture.create_view(&Default::default()),
            surface_tex: frame,
        };

        let (cmd_bufs, ready) = self.renderer.ready();
        let mut graph = rend3::graph::RenderGraph::new();

        let size = self.window.inner_size();
        let resolution = glam::UVec2::new(size.width, size.height);

        self.base_rendergraph.add_to_graph(
            &mut graph,
            &ready,
            &self.pbr_routine,
            None,
            &self.tonemapping_routine,
            resolution,
            rend3::types::SampleCount::One,
            glam::Vec4::ZERO,
        );

        graph.execute(&self.renderer, frame, cmd_bufs, &ready);
    }
}

pub struct WindowCtx {
    event_loop: EventLoop<WindowRxMessage>,
    inner: Window,
}

impl WindowCtx {
    pub fn new(runtime: &Runtime, offer_sender: oneshot::Sender<WindowOffer>) -> Self {
        let event_loop = EventLoopBuilder::with_user_event().build();
        let (inner, offer) = runtime.block_on(async { Window::new(&event_loop).await });
        offer_sender.send(offer).unwrap();
        Self { event_loop, inner }
    }

    pub fn run(self) -> ! {
        let Self { event_loop, inner } = self;
        let mut window = inner;
        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;

            match &event {
                Event::WindowEvent { ref event, .. } => match event {
                    WindowEvent::Resized(size) => {
                        window.on_resize(*size);
                    }
                    WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                        window.on_resize(**new_inner_size);
                    }
                    WindowEvent::CloseRequested => {
                        *control_flow = ControlFlow::Exit;
                        window.event_tx.send(WindowTxMessage::Quit).unwrap();
                    }
                    _ => {}
                },
                Event::MainEventsCleared => {
                    window.window.request_redraw();
                }
                Event::RedrawRequested(_) => {
                    window.on_draw();
                }
                Event::UserEvent(WindowRxMessage::Quit) => {
                    *control_flow = ControlFlow::Exit;
                }
                _ => (),
            }
        });
    }
}
