use rend3_routine::base::BaseRenderGraphIntermediateState;
use winit::event::{Event, WindowEvent};
use winit::event_loop::ControlFlow;

use std::sync::Arc;

const SAMPLE_COUNT: rend3::types::SampleCount = rend3::types::SampleCount::One;

pub struct DemoInner {
    alacritty_routine: rend3_alacritty::AlacrittyRoutine,
}

#[derive(Default)]
pub struct Demo {
    inner: Option<DemoInner>,
}

impl rend3_framework::App for Demo {
    const HANDEDNESS: rend3::types::Handedness = rend3::types::Handedness::Right;

    fn sample_count(&self) -> rend3::types::SampleCount {
        SAMPLE_COUNT
    }

    fn setup(
        &mut self,
        _window: &winit::window::Window,
        renderer: &Arc<rend3::Renderer>,
        _routines: &Arc<rend3_framework::DefaultRoutines>,
        surface_format: rend3::types::TextureFormat,
    ) {
        let ttf_src = include_bytes!("../../../resources/mononoki/mononoki-Regular.ttf");
        let face = ttf_parser::Face::parse(ttf_src, 0).unwrap();
        let (glyph_atlas, _errors) = font_mud::glyph_atlas::GlyphAtlas::new(&face).unwrap();
        let alacritty_routine =
            rend3_alacritty::AlacrittyRoutine::new(glyph_atlas, &renderer, surface_format);
        self.inner = Some(DemoInner { alacritty_routine });
    }

    fn handle_event(
        &mut self,
        window: &winit::window::Window,
        renderer: &Arc<rend3::Renderer>,
        routines: &Arc<rend3_framework::DefaultRoutines>,
        base_rendergraph: &rend3_routine::base::BaseRenderGraph,
        surface: Option<&Arc<rend3::types::Surface>>,
        resolution: glam::UVec2,
        event: rend3_framework::Event<'_, ()>,
        control_flow: impl FnOnce(winit::event_loop::ControlFlow),
    ) {
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                control_flow(ControlFlow::Exit);
            }
            Event::MainEventsCleared => {
                window.request_redraw();
            }
            Event::RedrawRequested(_) => {
                let frame = rend3::util::output::OutputFrame::Surface {
                    surface: Arc::clone(surface.unwrap()),
                };

                let (cmd_bufs, ready) = renderer.ready();

                let pbr_routine = rend3_framework::lock(&routines.pbr);
                let tonemapping_routine = rend3_framework::lock(&routines.tonemapping);
                let mut graph = rend3::graph::RenderGraph::new();

                base_rendergraph.add_to_graph(
                    &mut graph,
                    &ready,
                    &pbr_routine,
                    None,
                    &tonemapping_routine,
                    resolution,
                    SAMPLE_COUNT,
                    glam::Vec4::ZERO,
                );

                let state = BaseRenderGraphIntermediateState::new(
                    &mut graph,
                    &ready,
                    resolution,
                    SAMPLE_COUNT,
                );

                let depth = state.depth;
                let output = graph.add_surface_texture();
                self.inner
                    .as_mut()
                    .unwrap()
                    .alacritty_routine
                    .add_to_graph(&mut graph, output, depth);

                graph.execute(renderer, frame, cmd_bufs, &ready);
            }
            _ => {}
        }
    }
}

fn main() {
    let app = Demo::default();
    rend3_framework::start(
        app,
        winit::window::WindowBuilder::new().with_title("rend3-alacritty demo"),
    );
}
