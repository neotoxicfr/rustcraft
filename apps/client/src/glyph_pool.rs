//! Glyph pool shared by the 3D texts (name tags, sign text, text displays).
//!
//! The glyphs of each kept layout live in one storage buffer, appended at its tail and sent
//! only when the layout is made; per frame, a text sends a small reference (first glyph,
//! count, plus its own per-frame fields) drawn as an instance of `6 × n` vertices, `n` being
//! the longest text of the draw (the vertex shader turns the vertices past a reference's
//! count into degenerate triangles). A pass is cut into contiguous draws whose vertices stay
//! under twice those of their glyphs, so one long text does not make the short ones pay.
//!
//! Forgotten glyphs (layouts dropped or redone) are counted, and the pool is rebuilt at the
//! start of a frame when they exceed both the live ones and 4096: the pool holds at most
//! live + max(live, 4096) glyphs; the GPU buffer (power of 2) never shrinks.

use crate::nametag::Glyph;
use crate::render::upload::Uploader;

/// Glyph of the pool: the fields of [`Glyph`], reordered so that the vectors are aligned to
/// 16 bytes in a storage buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct PooledGlyph {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub skew: f32,
    pub color: u32,
    pub mode: u32,
    pad: u32,
}

impl PooledGlyph {
    pub fn new(g: &Glyph) -> Self {
        PooledGlyph { rect: g.rect, uv: g.uv, skew: g.skew, color: g.color, mode: g.mode, pad: 0 }
    }

    /// Back to the glyph it was made from (proofs read the GPU buffer copy through it).
    #[cfg(test)]
    pub fn glyph(&self) -> Glyph {
        Glyph { rect: self.rect, skew: self.skew, uv: self.uv, color: self.color, mode: self.mode }
    }
}

/// Glyphs of one layer kept in the pool: original order from `first`, and a copy with the
/// opaque glyphs first from `sorted` (equal to `first` when the original order already is), of
/// which `solids` are opaque. Improved transparency draws the opaque ones with the solids.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PooledRun {
    pub first: u32,
    pub count: u32,
    pub sorted: u32,
    pub solids: u32,
}

impl PooledRun {
    /// Glyphs occupied in the pool.
    pub fn pooled(&self) -> usize {
        let copy = if self.sorted != self.first { self.count } else { 0 };
        self.count as usize + copy as usize
    }
}

/// Forgotten glyphs tolerated before compaction (beyond, if they also exceed the live ones).
const COMPACT_MIN: usize = 4096;

/// CPU copy of the pool's storage buffer.
#[derive(Default)]
pub(crate) struct GlyphPool {
    pub glyphs: Vec<PooledGlyph>,
    /// First glyph not yet sent.
    unsent: usize,
    /// Glyphs no longer referenced by any kept layout.
    forgotten: usize,
    /// Capacity of the GPU buffer in glyphs (0 = not yet created).
    capacity: usize,
    /// Test sabotage: additions never sent.
    #[cfg(test)]
    pub sabotage: bool,
    #[cfg(test)]
    pub compactions: u32,
}

/// Upload of the pool for the frame: buffer recreated at `grow` glyphs, glyphs `from..` to write.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PoolUpload {
    pub grow: Option<usize>,
    pub from: usize,
}

impl GlyphPool {
    /// Appends glyphs at the tail (always past the sent ones, hence sent at the end of the
    /// frame); returns the index of the first one.
    pub fn push<'a>(&mut self, glyphs: impl IntoIterator<Item = &'a Glyph>) -> u32 {
        let at = self.glyphs.len() as u32;
        self.glyphs.extend(glyphs.into_iter().map(PooledGlyph::new));
        at
    }

    /// Appends a layer with its opaque-first copy (see [`PooledRun`]).
    pub fn push_run(&mut self, glyphs: &[Glyph]) -> PooledRun {
        let first = self.push(glyphs);
        let solids = glyphs.iter().filter(|g| g.solid()).count();
        let sorted = if glyphs[..solids].iter().all(Glyph::solid) {
            first
        } else {
            let at = self.push(glyphs.iter().filter(|g| g.solid()));
            self.push(glyphs.iter().filter(|g| !g.solid()));
            at
        };
        PooledRun { first, count: glyphs.len() as u32, sorted, solids: solids as u32 }
    }

    /// `glyphs` of the pool are no longer referenced by any kept layout.
    pub fn forget(&mut self, glyphs: usize) {
        self.forgotten += glyphs;
    }

    /// Start of a frame, before any reference of the frame exists: empties the pool when the
    /// forgotten glyphs outweigh the live ones (and 4096); the caller then pushes its kept
    /// layouts again. True if it did.
    pub fn compact_if_worth(&mut self) -> bool {
        if self.forgotten <= (self.glyphs.len() - self.forgotten).max(COMPACT_MIN) {
            return false;
        }
        self.glyphs.clear();
        self.forgotten = 0;
        self.unsent = 0;
        #[cfg(test)]
        {
            self.compactions += 1;
        }
        true
    }

    /// End of the frame: what to send (everything after a growth, otherwise the new tail).
    pub fn plan_upload(&mut self) -> PoolUpload {
        let len = self.glyphs.len();
        let grow = (self.capacity < len).then(|| {
            self.capacity = len.next_power_of_two().max(1024);
            self.unsent = 0;
            self.capacity
        });
        #[cfg(test)]
        if self.sabotage && grow.is_none() {
            self.unsent = len;
        }
        let from = self.unsent;
        self.unsent = len;
        PoolUpload { grow, from }
    }

    /// Carries out `plan` on the GPU buffer `slot` (created or recreated on growth); true if
    /// the buffer was recreated, so the bind groups that hold it must be remade.
    pub fn upload(
        &self,
        plan: PoolUpload,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &mut Uploader,
        slot: &mut Option<wgpu::Buffer>,
        label: &str,
    ) -> bool {
        if let Some(capacity) = plan.grow {
            *slot = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (capacity * size_of::<PooledGlyph>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(new) = self.glyphs.get(plan.from..).filter(|g| !g.is_empty()) {
            let buffer = slot.as_ref().expect("pool buffer created on the first upload");
            up.write(queue, buffer, (plan.from * size_of::<PooledGlyph>()) as u64, bytemuck::cast_slice(new));
        }
        plan.grow.is_some()
    }
}

/// Instanced draw of references: `count` references from `first`, each drawn with the
/// vertices of `glyphs_each` glyphs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RefDraw {
    pub first: u32,
    pub count: u32,
    pub glyphs_each: u32,
}

/// Cuts the references of one pass, taken in order, into contiguous draws.
pub(crate) struct DrawSplitter {
    open: RefDraw,
    /// Glyphs of the references of the open draw.
    useful: u64,
    split: bool,
}

impl DrawSplitter {
    /// Pass whose first reference has index `first`; `split = false` (test sabotage) keeps
    /// the pass in a single draw.
    pub fn new(first: u32, split: bool) -> Self {
        DrawSplitter { open: RefDraw { first, count: 0, glyphs_each: 0 }, useful: 0, split }
    }

    /// Next reference of the pass, of `glyphs` > 0 glyphs.
    pub fn add(&mut self, glyphs: u32, out: &mut Vec<RefDraw>) {
        // Every instance of a draw has the vertices of the longest reference: the draw is
        // closed before its vertices exceed twice those of its glyphs.
        let most = self.open.glyphs_each.max(glyphs);
        let too_many = u64::from(most) * u64::from(self.open.count + 1) > 2 * (self.useful + u64::from(glyphs));
        if self.split && self.open.count > 0 && too_many {
            out.push(self.open);
            self.open = RefDraw { first: self.open.first + self.open.count, count: 0, glyphs_each: 0 };
            self.useful = 0;
        }
        self.open.count += 1;
        self.open.glyphs_each = self.open.glyphs_each.max(glyphs);
        self.useful += u64::from(glyphs);
    }

    pub fn finish(self, out: &mut Vec<RefDraw>) {
        if self.open.count > 0 {
            out.push(self.open);
        }
    }
}

/// Distance from the font origin to the farthest corner of `glyphs` (font pixels).
pub(crate) fn glyph_radius(glyphs: &[Glyph]) -> f32 {
    let (mut x, mut y) = (0f32, 0f32);
    for g in glyphs {
        // Top corners at rect[0..2], bottom ones shifted by the italic skew.
        x = x.max(g.rect[0].abs()).max(g.rect[1].abs()).max((g.rect[0] + g.skew).abs()).max((g.rect[1] + g.skew).abs());
        y = y.max(g.rect[2].abs()).max(g.rect[3].abs());
    }
    x.hypot(y)
}

/// Per-frame writes identical to the GPU content skipped
/// (`RUSTCRAFT_OPT_TEXT_SKIP_UNCHANGED_WRITES`, default 1), read once.
pub(crate) fn skip_unchanged_writes_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_TEXT_SKIP_UNCHANGED_WRITES")
}

/// Old paths of the sign text and the text displays: writes `bytes` at the start of `slot`
/// (buffer, capacity), recreated a power of two larger when too small; true if recreated.
pub(crate) fn write_growing(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    slot: &mut Option<(wgpu::Buffer, u64)>,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
    label: &str,
) -> bool {
    let fresh = slot.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64);
    if fresh {
        let cap = (bytes.len() as u64).next_power_of_two().max(1024);
        *slot = Some((
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: cap,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            cap,
        ));
    }
    queue.write_buffer(&slot.as_ref().expect("created above").0, 0, bytes);
    fresh
}

/// Records the draws of a list of references.
pub(crate) fn draw_refs(pass: &mut wgpu::RenderPass<'_>, draws: &[RefDraw]) {
    for d in draws {
        pass.draw(0..6 * d.glyphs_each, d.first..d.first + d.count);
    }
}

/// What to do with a per-frame GPU buffer (references, matrices) this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BufferPlan {
    /// Buffer recreated with this many bytes (its content is lost).
    pub recreate: Option<u64>,
    pub write: bool,
}

/// CPU side of a per-frame GPU buffer: its capacity is decided here, so the proofs see
/// every recreation; with `skip_unchanged`, bytes identical to those the buffer already holds
/// are not sent again (a still camera re-sends nothing).
#[derive(Default)]
pub(crate) struct FrameBuffer {
    capacity: u64,
    /// Bytes the GPU buffer holds (kept only when skipping unchanged writes).
    on_gpu: Vec<u8>,
    /// Test sabotage: "unchanged" judged on the length alone.
    #[cfg(test)]
    pub sabotage: bool,
}

impl FrameBuffer {
    pub fn plan(&mut self, bytes: &[u8], skip_unchanged: bool) -> BufferPlan {
        let len = bytes.len() as u64;
        let recreate = (self.capacity < len).then(|| {
            self.capacity = len.next_power_of_two().max(1024);
            self.on_gpu.clear();
            self.capacity
        });
        #[cfg(test)]
        let same = if self.sabotage { self.on_gpu.len() >= bytes.len() } else { self.on_gpu.starts_with(bytes) };
        #[cfg(not(test))]
        let same = self.on_gpu.starts_with(bytes);
        let unchanged = skip_unchanged && recreate.is_none() && same;
        if !unchanged {
            self.on_gpu.clear();
        }
        if skip_unchanged && !unchanged {
            self.on_gpu.extend_from_slice(bytes);
        }
        BufferPlan { recreate, write: !bytes.is_empty() && !unchanged }
    }

    /// Carries out `plan` on `slot`; true if the buffer was recreated.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        plan: BufferPlan,
        bytes: &[u8],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &mut Uploader,
        slot: &mut Option<wgpu::Buffer>,
        usage: wgpu::BufferUsages,
        label: &str,
    ) -> bool {
        if let Some(size) = plan.recreate {
            *slot = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if plan.write {
            up.write(queue, slot.as_ref().expect("buffer created on the first upload"), 0, bytes);
        }
        plan.recreate.is_some()
    }
}

/// Camera of the frame, to leave out the text faces that turn their back to it: the GPU
/// culls back faces (counter-clockwise front, like the game), so sending them only costs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BackFaceTest {
    /// Eye in the frame of the face matrices (relative to the camera origin).
    pub eye: glam::Vec3,
    /// Screen pixels per radian of the view (height / (2 tan(vertical field / 2))).
    pub pixels_per_radian: f32,
}

/// Distance (blocks) the projection's eye may stray from the camera position: the view
/// bobbing is a view-space translation under 0.12 block.
const EYE_SLACK: f32 = 0.25;

/// Projected thickness (screen pixels) a glyph triangle must keep for the GPU's rounding not to
/// flip its facing: 4 steps of the usual 1/256-pixel vertex snapping.
const SAFE_THICKNESS: f32 = 1.0 / 64.0;

impl BackFaceTest {
    /// Camera at `eye` (world), vertical field of view `fov` (degrees) over `height` pixels;
    /// the eye is expressed in the camera frame of the face matrices (`camera::view_matrix`).
    pub fn for_camera(eye: [f64; 3], fov: f32, height: u32) -> Self {
        let local = crate::camera::view_matrix(eye, 0.0, 0.0).1;
        let pixels_per_radian = height as f32 / (2.0 * (fov.to_radians() / 2.0).tan());
        BackFaceTest { eye: local, pixels_per_radian }
    }

    /// True when the eye is surely behind the text plane of `face` (font pixels → camera
    /// frame), whose glyphs lie within `radius` font pixels of its origin.
    ///
    /// The front side is that of `c1 × c0` (font y points down, the corners go top-left,
    /// bottom-left, bottom-right). Near the plane (grazing view) the GPU's rounding decides, so
    /// the face is only left out when, besides the bobbing slack, the thinnest feature (half a
    /// font pixel, `h` blocks) seen at distance `d` from `s` blocks behind the plane keeps a
    /// projected thickness `h·s/d²` (radians) above [`SAFE_THICKNESS`].
    pub fn faces_away(&self, face: &glam::Mat4, radius: f32) -> bool {
        let (x, y) = (face.x_axis.truncate(), face.y_axis.truncate());
        let normal = y.cross(x);
        let to_eye = self.eye - face.w_axis.truncate();
        // Eye in front of the plane (or in it, or not a number): kept, before any square root.
        let side = normal.dot(to_eye);
        if side.is_nan() || side >= 0.0 {
            return false;
        }
        let (area, font_pixel) = (normal.length(), x.length().min(y.length()));
        if !(area > 0.0 && font_pixel > 0.0 && self.pixels_per_radian > 0.0) {
            return false;
        }
        let behind = -side / area;
        let farthest = to_eye.length() + radius * x.length().max(y.length());
        let thinnest = 0.5 * font_pixel;
        behind > EYE_SLACK + farthest * farthest * SAFE_THICKNESS / (thinnest * self.pixels_per_radian)
    }
}

#[cfg(test)]
pub(crate) mod tests;
