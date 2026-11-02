use super::*;

pub struct UnlitPlugin;

impl Plugin for UnlitPlugin {
    fn build(&self, app: &mut App) {
        use bevy::asset::embedded_asset;
        embedded_asset!(app, "unlit.wgsl");
        app.add_plugins(MaterialPlugin::<Unlit>::default());
    }
}

/// the textures of a mesh's materials as layers, which layer a vertex uses is in its second uvs
#[derive(bevy::render::render_resource::AsBindGroup, Asset, Reflect, Clone)]
pub struct Unlit {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    pub texture: Handle<Image>,
}

impl Material for Unlit {
    fn fragment_shader() -> bevy::render::render_resource::ShaderRef {
        "embedded://stove/unlit.wgsl".into()
    }
}

/// a texture's size and bgra pixels
pub type Layer = (u32, u32, Vec<u8>);

/// the grid shown on surfaces without a texture
pub fn grid() -> Layer {
    let mut bgra = include_bytes!("../assets/proto.rgba").to_vec();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    (128, 128, bgra)
}

/// puts textures together as an array, layers have to be the same size so smaller ones are stretched
pub fn layered(layers: Vec<Layer>) -> Image {
    use bevy::render::{render_resource as res, texture as tex};
    let width = layers.iter().map(|layer| layer.0).max().unwrap_or(1).max(1);
    let height = layers.iter().map(|layer| layer.1).max().unwrap_or(1).max(1);
    let count = layers.len().max(1) as u32;
    let mut data = Vec::with_capacity((width * height * 4 * count) as usize);
    for (w, h, pixels) in layers {
        if (w, h) == (width, height) {
            data.extend_from_slice(&pixels);
            continue;
        }
        for y in 0..height {
            let from_y = (y * h / height) as usize;
            for x in 0..width {
                let from = (from_y * w as usize + (x * w / width) as usize) * 4;
                data.extend_from_slice(pixels.get(from..from + 4).unwrap_or(&[0; 4]));
            }
        }
    }
    data.resize((width * height * 4 * count) as usize, 0);
    Image {
        data,
        texture_descriptor: res::TextureDescriptor {
            label: None,
            size: res::Extent3d {
                width,
                height,
                depth_or_array_layers: count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: res::TextureDimension::D2,
            format: res::TextureFormat::Bgra8UnormSrgb,
            usage: res::TextureUsages::TEXTURE_BINDING,
            view_formats: &[res::TextureFormat::Bgra8UnormSrgb],
        },
        sampler: tex::ImageSampler::Descriptor(tex::ImageSamplerDescriptor {
            address_mode_u: tex::ImageAddressMode::Repeat,
            address_mode_v: tex::ImageAddressMode::Repeat,
            address_mode_w: tex::ImageAddressMode::Repeat,
            // the textures are smaller than in game so they'd look blocky otherwise
            mag_filter: tex::ImageFilterMode::Linear,
            min_filter: tex::ImageFilterMode::Linear,
            ..default()
        }),
        // a single layer would otherwise be viewed as a plain texture
        texture_view_descriptor: Some(res::TextureViewDescriptor {
            dimension: Some(res::TextureViewDimension::D2Array),
            ..default()
        }),
        ..default()
    }
}
