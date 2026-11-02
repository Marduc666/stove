#import bevy_pbr::forward_io::VertexOutput

@group(2) @binding(0) var texture: texture_2d<f32>;
@group(2) @binding(1) var tsampler: sampler;
@group(2) @binding(2) var<uniform> uv_scale: vec4<f32>;

@fragment
fn fragment(
    mesh: VertexOutput,
) -> @location(0) vec4<f32> {
    let colour = textureSample(texture, tsampler, mesh.uv * uv_scale.xy);
    // meshes without normals are left flat
    if dot(mesh.world_normal, mesh.world_normal) < 0.0001 {
        return colour;
    }
    // a light from above and to the side so walls facing different ways differ
    let light = dot(normalize(mesh.world_normal), normalize(vec3(0.5, 0.8, 0.3)));
    return vec4(colour.rgb * clamp(0.6 + 0.4 * light, 0.3, 1.0), colour.a);
}