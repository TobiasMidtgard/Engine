//! CPU-side texture atlases for glyphs, icons and pictures. The render feature uploads the
//! changed region each frame.

/// A shelf-packed atlas page. Pixels live on the CPU; `dirty` is the region to upload.
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    /// Bytes per pixel: 1 (coverage masks) or 4 (RGBA8, premultiplied sRGB).
    pub bpp: u32,
    pub data: Vec<u8>,
    shelves: Vec<Shelf>,
    next_y: u32,
    /// Region changed since the last upload: x0, y0, x1, y1.
    pub dirty: Option<[u32; 4]>,
    /// Bumps when the atlas is cleared (everything cached in it is gone) or resized.
    pub generation: u32,
    /// Bumps when the GPU texture must be recreated (size changed).
    pub resized: bool,
    max_size: u32,
}

struct Shelf {
    y: u32,
    height: u32,
    x: u32,
}

/// Padding around each entry so bilinear filtering never bleeds neighbors in.
const PAD: u32 = 1;

impl Atlas {
    pub fn new(width: u32, height: u32, bpp: u32, max_size: u32) -> Self {
        Self {
            width,
            height,
            bpp,
            data: vec![0; (width * height * bpp) as usize],
            shelves: Vec::new(),
            next_y: 0,
            dirty: None,
            generation: 0,
            resized: true,
            max_size,
        }
    }

    /// Reserve a `w`×`h` region. `None` when full (call [`Atlas::clear`] or [`Atlas::grow`]).
    pub fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let (pw, ph) = (w + 2 * PAD, h + 2 * PAD);
        if pw > self.width || ph > self.height {
            return None;
        }
        // Best-fitting shelf that is tall enough and has room.
        let mut best: Option<usize> = None;
        for (i, s) in self.shelves.iter().enumerate() {
            if s.height >= ph && s.height <= ph + ph / 2 + 4 && s.x + pw <= self.width {
                match best {
                    Some(b) if self.shelves[b].height <= s.height => {}
                    _ => best = Some(i),
                }
            }
        }
        let i = match best {
            Some(i) => i,
            None => {
                if self.next_y + ph > self.height {
                    return None;
                }
                self.shelves.push(Shelf {
                    y: self.next_y,
                    height: ph,
                    x: 0,
                });
                self.next_y += ph;
                self.shelves.len() - 1
            }
        };
        let s = &mut self.shelves[i];
        let pos = (s.x + PAD, s.y + PAD);
        s.x += pw;
        Some(pos)
    }

    /// Copy `pixels` (tightly packed, `bpp` bytes each) into the region at `(x, y)`.
    pub fn write(&mut self, x: u32, y: u32, w: u32, h: u32, pixels: &[u8]) {
        let bpp = self.bpp as usize;
        let row = w as usize * bpp;
        debug_assert!(pixels.len() >= row * h as usize);
        for r in 0..h as usize {
            let dst = ((y as usize + r) * self.width as usize + x as usize) * bpp;
            self.data[dst..dst + row].copy_from_slice(&pixels[r * row..r * row + row]);
        }
        let (x0, y0, x1, y1) = (x - PAD.min(x), y - PAD.min(y), x + w + PAD, y + h + PAD);
        let (x1, y1) = (x1.min(self.width), y1.min(self.height));
        self.dirty = Some(match self.dirty {
            None => [x0, y0, x1, y1],
            Some([a, b, c, d]) => [a.min(x0), b.min(y0), c.max(x1), d.max(y1)],
        });
    }

    /// Forget every entry (callers drop their caches when `generation` changes).
    pub fn clear(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
        self.data.fill(0);
        self.dirty = Some([0, 0, self.width, self.height]);
        self.generation += 1;
    }

    /// Double the page (up to the maximum) and clear it. Returns false at the maximum.
    pub fn grow(&mut self) -> bool {
        if self.width >= self.max_size && self.height >= self.max_size {
            return false;
        }
        self.width = (self.width * 2).min(self.max_size);
        self.height = (self.height * 2).min(self.max_size);
        self.data = vec![0; (self.width * self.height * self.bpp) as usize];
        self.resized = true;
        self.clear();
        true
    }

    /// Fraction of rows in use.
    pub fn usage(&self) -> f32 {
        self.next_y as f32 / self.height as f32
    }

    /// UV rectangle (u0, v0, u1, v1) of a region.
    pub fn uv(&self, x: u32, y: u32, w: u32, h: u32) -> [f32; 4] {
        let (fw, fh) = (self.width as f32, self.height as f32);
        [
            x as f32 / fw,
            y as f32 / fh,
            (x + w) as f32 / fw,
            (y + h) as f32 / fh,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_without_overlap_and_reports_full() {
        let mut a = Atlas::new(64, 64, 1, 128);
        let mut rects = Vec::new();
        while let Some((x, y)) = a.alloc(10, 7) {
            rects.push((x, y));
        }
        assert!(rects.len() >= 20, "{}", rects.len());
        for (i, &(ax, ay)) in rects.iter().enumerate() {
            for &(bx, by) in &rects[i + 1..] {
                let overlap = ax < bx + 10 && bx < ax + 10 && ay < by + 7 && by < ay + 7;
                assert!(!overlap);
            }
        }
        let g = a.generation;
        assert!(a.grow());
        assert_eq!(a.width, 128);
        assert!(a.generation > g);
        assert!(!a.grow());
    }

    #[test]
    fn write_marks_dirty_region() {
        let mut a = Atlas::new(32, 32, 4, 32);
        a.dirty = None;
        let (x, y) = a.alloc(2, 2).unwrap();
        a.write(x, y, 2, 2, &[255; 16]);
        let [x0, y0, x1, y1] = a.dirty.unwrap();
        assert!(x0 <= x && y0 <= y && x1 >= x + 2 && y1 >= y + 2);
        assert_eq!(a.data[((y * 32 + x) * 4) as usize], 255);
    }
}
