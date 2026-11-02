#import bevy_pbr::forward_io::VertexOutput

@group(2) @binding(0) var texture: texture_2d_array<f32>;
@group(2) @binding(1) var tsampler: sampler;

@fragment
fn fragment(
    mesh: VertexOutput,
) -> @location(0) vec4<f32> {
    // the texture of the vertex's material
#ifdef VERTEX_UVS_B
    let layer = i32(mesh.uv_b.x + 0.5);
#else
    let layer = 0;
#endif
    let colour = textureSample(texture, tsampler, mesh.uv, layer);
    // meshes without normals are left flat
    if dot(mesh.world_normal, mesh.world_normal) < 0.0001 {
        return colour;
    }
    // a light from above and to the side so walls facing different ways differ
    let light = dot(normalize(mesh.world_normal), normalize(vec3(0.5, 0.8, 0.3)));
    return vec4(colour.rgb * clamp(0.6 + 0.4 * light, 0.3, 1.0), colour.a);
}
