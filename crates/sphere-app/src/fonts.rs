//! Bundled fonts (all SIL OFL-1.1, see assets/fonts) and a tiny CPU rasteriser for core labels.
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageSampler, ImageSamplerDescriptor, ImageFilterMode},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

pub const RAJDHANI_500: &[u8] = include_bytes!("../../../assets/fonts/Rajdhani-500.ttf");
pub const RAJDHANI_600: &[u8] = include_bytes!("../../../assets/fonts/Rajdhani-600.ttf");
pub const RAJDHANI_700: &[u8] = include_bytes!("../../../assets/fonts/Rajdhani-700.ttf");
pub const MICHROMA: &[u8] = include_bytes!("../../../assets/fonts/Michroma-400.ttf");
pub const MONO_400: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-400.ttf");
#[allow(dead_code)]
pub const MONO_500: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-500.ttf");
pub const MONO_700: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-700.ttf");

/// UI font handles used by the Bevy UI panels.
#[derive(Resource, Clone)]
pub struct UiFonts {
    pub ui500: Handle<Font>,
    pub ui600: Handle<Font>,
    pub ui700: Handle<Font>,
    pub display: Handle<Font>,
    pub mono: Handle<Font>,
    pub mono_bold: Handle<Font>,
}

pub fn load_ui_fonts(fonts: &mut Assets<Font>) -> UiFonts {
    UiFonts {
        ui500: fonts.add(Font::from_bytes(RAJDHANI_500.to_vec())),
        ui600: fonts.add(Font::from_bytes(RAJDHANI_600.to_vec())),
        ui700: fonts.add(Font::from_bytes(RAJDHANI_700.to_vec())),
        display: fonts.add(Font::from_bytes(MICHROMA.to_vec())),
        mono: fonts.add(Font::from_bytes(MONO_400.to_vec())),
        mono_bold: fonts.add(Font::from_bytes(MONO_700.to_vec())),
    }
}

pub struct Label {
    pub image: Image,
    /// size in world units (texture size / supersampling)
    pub size: Vec2,
}

/// Port of the prototype's `textMesh` canvas: tracked caps drawn glyph by glyph into an alpha texture
/// at 4x, with a box-filtered mip chain so small labels stay clean at any tilt.
pub fn raster_label(font_bytes: &[u8], text: &str, size: f32, track: f32) -> Label {
    const S: f32 = 4.0;
    let font = fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default()).expect("font");
    let fs = size * S;
    let sp = fs * track;
    let pad = (3.0 * S) as i32;
    let chars: Vec<char> = text.chars().collect();
    let glyphs: Vec<_> = chars.iter().map(|c| font.rasterize(*c, fs)).collect();
    let total: f32 = glyphs.iter().map(|(m, _)| m.advance_width).sum::<f32>() + sp * (chars.len().saturating_sub(1)) as f32;
    let w = (total + pad as f32 * 2.0).ceil() as usize;
    let h = (fs * 1.5).ceil() as usize;
    let lm = font.horizontal_line_metrics(fs).unwrap();
    let baseline = h as f32 / 2.0 + (lm.ascent + lm.descent) / 2.0 + fs * 0.04;
    let mut px = vec![0u8; w * h];
    let mut cx = pad as f32;
    for (m, bmp) in glyphs.iter() {
        let gx = (cx + m.xmin as f32).round() as i32;
        let gy = (baseline - m.ymin as f32 - m.height as f32).round() as i32;
        for yy in 0..m.height {
            for xx in 0..m.width {
                let (x, y) = (gx + xx as i32, gy + yy as i32);
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                    let i = y as usize * w + x as usize;
                    px[i] = px[i].max(bmp[yy * m.width + xx]);
                }
            }
        }
        cx += m.advance_width + sp;
    }
    Label { image: mip_r8(px, w, h), size: Vec2::new(w as f32 / S, h as f32 / S) }
}

pub fn mip_r8(base: Vec<u8>, w: usize, h: usize) -> Image {
    let mut data = base.clone();
    let (mut cw, mut ch) = (w, h);
    let mut prev = base;
    let mut levels = 1;
    while cw > 1 || ch > 1 {
        let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
        let mut next = vec![0u8; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let mut s = 0u32;
                let mut n = 0u32;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (sx, sy) = ((x * 2 + dx).min(cw - 1), (y * 2 + dy).min(ch - 1));
                        s += prev[sy * cw + sx] as u32;
                        n += 1;
                    }
                }
                next[y * nw + x] = (s / n) as u8;
            }
        }
        data.extend_from_slice(&next);
        prev = next;
        cw = nw;
        ch = nh;
        levels += 1;
    }
    let mut img = Image::new(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.mip_level_count = levels;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    img
}
