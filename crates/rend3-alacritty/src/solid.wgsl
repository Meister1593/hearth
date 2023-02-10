struct VertexInput {
    [[location(0)]] position: vec2<f32>;
    [[location(1)]] color: vec4<f32>;
};

struct VertexOutput {
    [[builtin(position)]] clip_position: vec4<f32>;
    [[location(0)]] color: vec4<f32>;
};

struct CameraUniform {
    mvp: mat4x4<f32>;
};

[[group(0), binding(0)]] var<uniform> camera: CameraUniform;

[[stage(vertex)]]
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.mvp * vec4<f32>(in.position, 0.0, 1.0);
    out.color = in.color;
    return out;
}

[[stage(fragment)]]
fn fs_main(frag: VertexOutput) -> [[location(0)]] vec4<f32> {
    return frag.color;
}
