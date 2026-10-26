//! Entity texture sheet loaded on demand: the PNGs stay compressed in memory,
//! a texture is decoded and placed (1024-pixel shelves) only on its first use. The
//! sheet doubles in height when full (contents copied on the GPU). Rectangles in
//! pixels: the shader normalizes them by the texture size, so a growth does not
//! invalidate them.

use hashbrown::HashMap;
use std::sync::Arc;

use rc_assets::source::Assets;

use rc_assets::texture::Image;

const W: u32 = 1024;

/// Pending entity texture: path in the resources (PNG read and decoded on first use),
/// compressed PNG, or decoded image (item sprites, downloaded skins).
pub enum Source {
    Asset(String),
    #[allow(dead_code)]
    Png(Vec<u8>),
    Image(Image),
}

pub struct EntityAtlas {
    sources: HashMap<String, Source>,
    /// Resources read on demand (`Source::Asset`).
    assets: Option<Arc<Assets>>,
    /// Rectangle in pixels (x, y, width, height), `None`: texture not found or unreadable.
    rects: HashMap<String, Option<[f32; 4]>>,
    /// Shelf cursor: x, y, shelf height.
    cursor: (u32, u32, u32),
    /// Wanted height (the texture follows on upload).
    height: u32,
    pending: Vec<(u32, u32, Image)>,
    tex: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    /// Changes with every new texture (binding to redo).
    pub generation: u32,
}

impl EntityAtlas {
    pub fn new(sources: HashMap<String, Source>, assets: Option<Arc<Assets>>) -> Self {
        EntityAtlas {
            sources,
            assets,
            rects: HashMap::new(),
            cursor: (0, 0, 0),
            height: 256,
            pending: Vec::new(),
            tex: None,
            view: None,
            generation: 0,
        }
    }

    /// Adds an image (downloaded skin…) under a name, if it is not already placed.
    pub fn insert(&mut self, name: &str, img: Image) {
        if !self.rects.contains_key(name) {
            self.sources.insert(name.to_owned(), Source::Image(img));
        }
    }

    /// Rewrites an already placed area (modified map texture), uploaded at the next `flush`.
    pub fn write(&mut self, x: u32, y: u32, img: Image) {
        self.pending.push((x, y, img));
    }

    /// Rectangle of an already placed texture (without loading it otherwise).
    pub fn placed(&self, name: &str) -> Option<[f32; 4]> {
        self.rects.get(name).copied().flatten()
    }

    /// Known texture (without loading it).
    pub fn contains(&self, name: &str) -> bool {
        self.sources.contains_key(name) || self.rects.get(name).is_some_and(Option::is_some)
    }

    /// Rectangle of a texture, loaded and placed on the first call.
    pub fn rect(&mut self, name: &str) -> Option<[f32; 4]> {
        if let Some(r) = self.rects.get(name) {
            return *r;
        }
        let img = match self.sources.remove(name)? {
            Source::Asset(path) => self.assets.as_ref()?.image(&path).map(|i| i.first_frame()),
            Source::Png(bytes) => Image::decode_png(&bytes).map(|i| i.first_frame()),
            Source::Image(i) => Some(i),
        };
        let r = img.map(|img| self.place(img));
        self.rects.insert(name.to_owned(), r);
        r
    }

    fn place(&mut self, img: Image) -> [f32; 4] {
        let (w, h) = (img.width.min(W), img.height);
        let (mut x, mut y, mut row) = self.cursor;
        if x + w > W {
            x = 0;
            y += row;
            row = 0;
        }
        while y + h > self.height {
            self.height *= 2;
        }
        self.cursor = (x + w, y, row.max(h));
        let r = [x as f32, y as f32, w as f32, h as f32];
        self.pending.push((x, y, img));
        r
    }

    /// Upload of the textures placed since the last call; new texture (contents copied) if
    /// the sheet grew. Returns true if the view changed.
    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let old_h = self.tex.as_ref().map_or(0, wgpu::Texture::height);
        let mut changed = false;
        if old_h < self.height {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("entity textures"),
                size: wgpu::Extent3d { width: W, height: self.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            if let Some(old) = &self.tex {
                let mut enc = device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("entity textures") });
                enc.copy_texture_to_texture(
                    old.as_image_copy(),
                    tex.as_image_copy(),
                    wgpu::Extent3d { width: W, height: old_h, depth_or_array_layers: 1 },
                );
                queue.submit([enc.finish()]);
            }
            self.view = Some(tex.create_view(&Default::default()));
            self.tex = Some(tex);
            self.generation += 1;
            changed = true;
        }
        let tex = self.tex.as_ref().expect("created above");
        for (x, y, img) in self.pending.drain(..) {
            let w = img.width.min(W);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &img.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(img.width * 4),
                    rows_per_image: Some(img.height),
                },
                wgpu::Extent3d { width: w, height: img.height, depth_or_array_layers: 1 },
            );
        }
        changed
    }

    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.view.as_ref()
    }
}
