// Copyright (c) 2023 the Hearth contributors.
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// This file is part of Hearth.
//
// Hearth is free software: you can redistribute it and/or modify it under the
// terms of the GNU Affero General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option)
// any later version.
//
// Hearth is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with Hearth. If not, see <https://www.gnu.org/licenses/>.

use std::sync::Arc;

use glam::Mat4;
use hearth_core::flue::CapabilityRef;
use hearth_core::process::ProcessMetadata;
use hearth_core::runtime::{Plugin, RuntimeBuilder};
use hearth_core::utils::{MessageInfo, ServiceRunner, SinkProcess};
use hearth_core::{async_trait, cargo_process_metadata};
use hearth_rend3::rend3::types::{Camera, CameraProjection};
use hearth_rend3::{rend3, wgpu, FrameRequest, Rend3Plugin};
use hearth_types::window::winit::window::CursorGrabMode;
use hearth_types::window::*;
use rend3::InstanceAdapterDevice;
use tokio::sync::{mpsc, oneshot};
use tracing::warn;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use winit::window::{Window as WinitWindow, WindowBuilder};

/// A message sent from the rest of the program to a window.
#[derive(Clone, Debug)]
pub enum WindowRxMessage {
    /// Update the title.
    SetTitle(String),

    /// Set the cursor grab mode.
    SetCursorGrab(CursorGrabMode),

    /// Set the cursor visibility.
    SetCursorVisible(bool),

    /// Update the renderer camera.
    SetCamera {
        /// Vertical field of view in degrees.
        vfov: f32,

        /// Near plane distance. All projection uses an infinite far plane.
        near: f32,

        /// The camera's view matrix.
        view: Mat4,
    },

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
pub struct WindowOffer {
    /// A sender of [WindowRxMessage] to this window.
    pub incoming: EventLoopProxy<WindowRxMessage>,

    /// A receiver for [WindowTxMessage] from the window.
    pub outgoing: mpsc::UnboundedReceiver<WindowTxMessage>,

    /// A [Rend3Plugin] compatible with this window.
    pub rend3_plugin: Rend3Plugin,

    /// The [WindowPlugin] for this window.
    pub window_plugin: WindowPlugin,
}

struct Window {
    outgoing_tx: mpsc::UnboundedSender<WindowTxMessage>,
    window: WinitWindow,
    iad: InstanceAdapterDevice,
    surface: Arc<wgpu::Surface>,
    config: wgpu::SurfaceConfiguration,
    frame_request_tx: mpsc::UnboundedSender<FrameRequest>,
    camera: Camera,
    _directional_handle: rend3::types::ResourceHandle<rend3::types::DirectionalLight>,
}

impl Window {
    async fn new(event_loop: &EventLoop<WindowRxMessage>) -> (Self, WindowOffer) {
        let window = WindowBuilder::new()
            .with_title("Hearth Client")
            .with_inner_size(winit::dpi::LogicalSize::new(128.0, 128.0))
            .build(event_loop)
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
        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel();
        let rend3_plugin = Rend3Plugin::new(iad.to_owned(), swapchain_format);
        let renderer = rend3_plugin.renderer.to_owned();
        let frame_request_tx = rend3_plugin.frame_request_tx.clone();

        let directional_handle = renderer.add_directional_light(rend3::types::DirectionalLight {
            color: glam::Vec3::ONE,
            intensity: 10.0,
            direction: glam::Vec3::new(-1.0, -4.0, 2.0),
            distance: 400.0,
        });

        let window = Self {
            outgoing_tx,
            window,
            iad,
            surface,
            config,
            camera: Camera::default(),
            frame_request_tx,
            _directional_handle: directional_handle,
        };

        let window_plugin = WindowPlugin {
            incoming: event_loop.create_proxy(),
        };

        let offer = WindowOffer {
            incoming: event_loop.create_proxy(),
            outgoing: outgoing_rx,
            rend3_plugin,
            window_plugin,
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

        let output_frame = rend3::util::output::OutputFrame::SurfaceAcquired {
            view: frame.texture.create_view(&Default::default()),
            surface_tex: frame,
        };

        let resolution = glam::UVec2::new(self.config.width, self.config.height);

        let (on_complete, on_complete_rx) = oneshot::channel();

        let request = FrameRequest {
            output_frame,
            camera: self.camera,
            resolution,
            on_complete,
        };

        if self.frame_request_tx.send(request).is_err() {
            tracing::warn!("failed to request frame");
        } else {
            let _ = on_complete_rx.blocking_recv();
        }

        self.window.request_redraw();
    }
}

pub struct WindowCtx {
    event_loop: EventLoop<WindowRxMessage>,
    window: Window,
}

impl WindowCtx {
    pub async fn new() -> (Self, WindowOffer) {
        let event_loop = EventLoopBuilder::with_user_event().build();
        let (window, offer) = Window::new(&event_loop).await;
        (Self { event_loop, window }, offer)
    }

    pub fn run(self) -> ! {
        let Self {
            event_loop,
            mut window,
        } = self;

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;

            match event {
                Event::WindowEvent { ref event, .. } => match event {
                    WindowEvent::Resized(size) => {
                        window.on_resize(*size);
                    }
                    WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                        window.on_resize(**new_inner_size);
                    }
                    WindowEvent::CloseRequested => {
                        *control_flow = ControlFlow::Exit;
                        window.outgoing_tx.send(WindowTxMessage::Quit).unwrap();
                    }
                    _ => {}
                },
                Event::MainEventsCleared => {
                    window.window.request_redraw();
                }
                Event::RedrawRequested(_) => {
                    window.on_draw();
                }
                Event::UserEvent(event) => match event {
                    WindowRxMessage::SetTitle(title) => window.window.set_title(&title),
                    WindowRxMessage::SetCursorGrab(mode) => {
                        if let Err(err) = window.window.set_cursor_grab(mode) {
                            warn!("set cursor grab error: {err:?}");
                        }
                    }
                    WindowRxMessage::SetCursorVisible(visible) => {
                        window.window.set_cursor_visible(visible)
                    }
                    WindowRxMessage::SetCamera { vfov, near, view } => {
                        window.camera = Camera {
                            projection: CameraProjection::Perspective { vfov, near },
                            view,
                        }
                    }
                    WindowRxMessage::Quit => control_flow.set_exit(),
                },
                _ => (),
            }
        });
    }
}

/// A plugin that provides native window access to guests.
pub struct WindowPlugin {
    incoming: EventLoopProxy<WindowRxMessage>,
}

impl Plugin for WindowPlugin {
    fn finalize(self, builder: &mut RuntimeBuilder) {
        builder.add_plugin(WindowService {
            incoming: self.incoming,
        });
    }
}

/// A service that implements the windowing protocol using winit.
pub struct WindowService {
    incoming: EventLoopProxy<WindowRxMessage>,
}

#[async_trait]
impl SinkProcess for WindowService {
    type Message = WindowCommand;

    async fn on_message<'a>(&'a mut self, message: MessageInfo<'a, WindowCommand>) {
        let send = |event| {
            self.incoming.send_event(event).unwrap();
        };

        use WindowCommand::*;
        match message.data {
            Subscribe => todo!(), // pubsub subscribe goes here
            SetTitle(title) => send(WindowRxMessage::SetTitle(title)),
            SetCursorGrab(grab) => send(WindowRxMessage::SetCursorGrab(grab)),
            SetCursorVisible(visible) => send(WindowRxMessage::SetCursorVisible(visible)),
            SetCamera { vfov, near, view } => send(WindowRxMessage::SetCamera { vfov, near, view }),
        }
    }

    async fn on_down<'a>(&'a mut self, _cap: CapabilityRef<'a>) {
        // pubsub unsubscribe goes here
    }
}

impl ServiceRunner for WindowService {
    const NAME: &'static str = SERVICE_NAME;

    fn get_process_metadata() -> ProcessMetadata {
        let mut meta = cargo_process_metadata!();
        meta.description = Some("The native window service. Accepts WindowRequest.".to_string());
        meta
    }
}
