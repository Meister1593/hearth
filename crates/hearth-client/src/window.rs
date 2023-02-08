use std::sync::Arc;

use rend3::InstanceAdapterDevice;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use winit::window::{Window as WinitWindow, WindowBuilder};

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

        let window = Self {
            event_tx: event_rx,
            window,
            iad,
            surface,
            config,
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

        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.iad.device.create_command_encoder(&Default::default());
        {
            let _rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: true,
                    },
                }],
                depth_stencil_attachment: None,
            });
        }

        self.iad.queue.submit(Some(encoder.finish()));
        frame.present();
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
