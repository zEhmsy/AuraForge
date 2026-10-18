//! Dominant-color extraction from sparse screen-border samples, plus the
//! small color helpers used by effects.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    /// Largest per-channel difference.
    pub fn distance(self, other: Rgb) -> u8 {
        self.r
            .abs_diff(other.r)
            .max(self.g.abs_diff(other.g))
            .max(self.b.abs_diff(other.b))
    }
}

/// Read-only view of a mapped BGRA frame in which only the border bands are valid.
pub struct FrameView<'a> {
    pub data: &'a [u8],
    pub pitch: usize,
    pub width: usize,
    pub height: usize,
    pub border: usize,
}

/// Pixels darker than this (max channel) are ignored so letterboxing and
/// black HUD bars do not win the vote.
const BLACK_LEVEL: u8 = 24;
/// 3 bits per channel: coarse enough that similar shades vote together.
const BITS: u32 = 3;
const BINS: usize = 1 << (BITS * 3);

#[derive(Clone, Copy, Default)]
struct Bin {
    score: u32,
    count: u32,
    r: u32,
    g: u32,
    b: u32,
}

/// Coarse color histogram; the winning bin is averaged to recover the exact shade.
pub struct Analyzer {
    bins: Vec<Bin>,
}

impl Analyzer {
    pub fn new() -> Self {
        Analyzer { bins: vec![Bin::default(); BINS] }
    }

    /// Samples the border bands of `view` every `stride` pixels and returns the dominant color.
    pub fn dominant(&mut self, view: &FrameView, stride: usize) -> Rgb {
        self.bins.fill(Bin::default());
        let (w, h, t) = (view.width, view.height, view.border);
        self.sample_rect(view, 0, 0, w, t, stride);
        self.sample_rect(view, 0, h - t, w, h, stride);
        self.sample_rect(view, 0, t, t, h - t, stride);
        self.sample_rect(view, w - t, t, w, h - t, stride);
        self.winner()
    }

    fn sample_rect(&mut self, view: &FrameView, x0: usize, y0: usize, x1: usize, y1: usize, stride: usize) {
        for_each_sample(view, x0, y0, x1, y1, stride, |r, g, b| self.push(r, g, b));
    }

    #[inline]
    fn push(&mut self, r: u8, g: u8, b: u8) {
        let max = r.max(g).max(b);
        if max < BLACK_LEVEL {
            return;
        }
        let min = r.min(g).min(b);
        // Saturated pixels weigh more, so a vivid scene is not washed out by greys.
        let weight = 8 + u32::from(max - min);
        let shift = 8 - BITS;
        let index = (usize::from(r >> shift) << (2 * BITS))
            | (usize::from(g >> shift) << BITS)
            | usize::from(b >> shift);
        let bin = &mut self.bins[index];
        bin.score += weight;
        bin.count += 1;
        bin.r += u32::from(r);
        bin.g += u32::from(g);
        bin.b += u32::from(b);
    }

    fn winner(&self) -> Rgb {
        let best = self.bins.iter().max_by_key(|bin| bin.score).copied().unwrap_or_default();
        if best.count == 0 {
            return Rgb::BLACK;
        }
        Rgb {
            r: (best.r / best.count) as u8,
            g: (best.g / best.count) as u8,
            b: (best.b / best.count) as u8,
        }
    }
}

/// Calls `f(r, g, b)` for every `stride`-th pixel of the rectangle `[x0, x1) × [y0, y1)`.
fn for_each_sample(
    view: &FrameView,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    stride: usize,
    mut f: impl FnMut(u8, u8, u8),
) {
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // Start half a stride in so thin bands still get sampled away from the edge.
    let offset = stride / 2;
    let xs = x0 + offset.min((x1 - x0) / 2);
    let ys = y0 + offset.min((y1 - y0) / 2);
    for y in (ys..y1).step_by(stride) {
        let row = &view.data[y * view.pitch..];
        for x in (xs..x1).step_by(stride) {
            let px = &row[x * 4..x * 4 + 3];
            f(px[2], px[1], px[0]);
        }
    }
}

/// Plain mean of the sampled pixels in `[x0, x1) × [y0, y1)`; used for per-key zones.
pub fn average(view: &FrameView, x0: usize, y0: usize, x1: usize, y1: usize, stride: usize) -> Rgb {
    let (mut n, mut r, mut g, mut b) = (0u32, 0u32, 0u32, 0u32);
    for_each_sample(view, x0, y0, x1, y1, stride, |pr, pg, pb| {
        n += 1;
        r += u32::from(pr);
        g += u32::from(pg);
        b += u32::from(pb);
    });
    if n == 0 {
        return Rgb::BLACK;
    }
    Rgb { r: (r / n) as u8, g: (g / n) as u8, b: (b / n) as u8 }
}

/// Colors whose brightest channel is below this are treated as black by
/// `boost`, so near-black noise does not light up as a full-hue key.
const BOOST_FLOOR: u8 = 10;

/// Brightens dark colors without shifting their hue: the brightest channel is
/// mapped through `255 * (v / 255) ^ gamma` and the others scaled by the same
/// factor. `gamma = 1.0` is the identity; smaller values brighten more.
pub fn boost(color: Rgb, gamma: f32) -> Rgb {
    let max = color.r.max(color.g).max(color.b);
    if max < BOOST_FLOOR {
        return Rgb::BLACK;
    }
    let v = f32::from(max) / 255.0;
    let k = v.powf(gamma) / v;
    let scale = |c: u8| (f32::from(c) * k).round().min(255.0) as u8;
    Rgb { r: scale(color.r), g: scale(color.g), b: scale(color.b) }
}

/// Linear blend from `a` (`t = 0`) to `b` (`t = 1`).
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Rgb { r: lerp(a.r, b.r), g: lerp(a.g, b.g), b: lerp(a.b, b.b) }
}

/// Dim desaturated version of `color`, used while the player is dead.
pub fn ashen(color: Rgb) -> Rgb {
    let luma = 0.3 * f32::from(color.r) + 0.59 * f32::from(color.g) + 0.11 * f32::from(color.b);
    let v = (luma * 0.5).max(12.0).round() as u8;
    Rgb { r: v, g: v, b: v }
}

/// Fully saturated color at `hue` turns around the color wheel (0 = red).
pub fn hue(hue: f32) -> Rgb {
    let h = hue.rem_euclid(1.0) * 6.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let byte = |v: f32| (v * 255.0).round() as u8;
    Rgb { r: byte(r), g: byte(g), b: byte(b) }
}

/// Exponential smoothing so scene cuts fade instead of flashing.
pub struct Smoother {
    alpha: f32,
    state: Option<[f32; 3]>,
}

impl Smoother {
    pub fn new(alpha: f32) -> Self {
        Smoother { alpha, state: None }
    }

    pub fn update(&mut self, target: Rgb) -> Rgb {
        let target = [f32::from(target.r), f32::from(target.g), f32::from(target.b)];
        let state = self.state.get_or_insert(target);
        for (s, t) in state.iter_mut().zip(target) {
            *s += self.alpha * (t - *s);
            if (*s - t).abs() < 1.0 {
                *s = t;
            }
        }
        Rgb {
            r: state[0].round() as u8,
            g: state[1].round() as u8,
            b: state[2].round() as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_frame(w: usize, h: usize, bgra: [u8; 4]) -> Vec<u8> {
        bgra.iter().copied().cycle().take(w * h * 4).collect()
    }

    #[test]
    fn dominant_picks_vivid_majority() {
        let (w, h) = (64, 32);
        let mut data = solid_frame(w, h, [0, 0, 200, 255]); // red
        // Paint the left band blue; it is the minority of border samples.
        for y in 0..h {
            for x in 0..4 {
                data[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[200, 0, 0]);
            }
        }
        let view = FrameView { data: &data, pitch: w * 4, width: w, height: h, border: 4 };
        assert_eq!(Analyzer::new().dominant(&view, 2), Rgb { r: 200, g: 0, b: 0 });
    }

    #[test]
    fn black_frame_is_black() {
        let (w, h) = (32, 32);
        let data = solid_frame(w, h, [5, 5, 5, 255]);
        let view = FrameView { data: &data, pitch: w * 4, width: w, height: h, border: 4 };
        assert_eq!(Analyzer::new().dominant(&view, 3), Rgb::BLACK);
    }

    #[test]
    fn average_of_region() {
        let (w, h) = (16, 16);
        let mut data = solid_frame(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 8..w {
                data[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[10, 20, 30]);
            }
        }
        let view = FrameView { data: &data, pitch: w * 4, width: w, height: h, border: 0 };
        assert_eq!(average(&view, 8, 0, 16, 16, 2), Rgb { r: 30, g: 20, b: 10 });
        assert_eq!(average(&view, 0, 0, 8, 16, 2), Rgb::BLACK);
    }

    #[test]
    fn boost_brightens_and_keeps_hue() {
        // Purple read from the screen during testing.
        let c = boost(Rgb { r: 0x4D, g: 0x2A, b: 0x6D }, 0.5);
        assert_eq!(c, Rgb { r: 118, g: 64, b: 167 });
        assert_eq!(boost(Rgb { r: 255, g: 128, b: 0 }, 0.5), Rgb { r: 255, g: 128, b: 0 });
        assert_eq!(boost(Rgb { r: 40, g: 20, b: 10 }, 1.0), Rgb { r: 40, g: 20, b: 10 });
        assert_eq!(boost(Rgb { r: 9, g: 3, b: 3 }, 0.5), Rgb::BLACK);
    }

    #[test]
    fn mix_and_hue() {
        let red = Rgb { r: 255, g: 0, b: 0 };
        assert_eq!(mix(Rgb::BLACK, red, 0.0), Rgb::BLACK);
        assert_eq!(mix(Rgb::BLACK, red, 0.5), Rgb { r: 128, g: 0, b: 0 });
        assert_eq!(mix(Rgb::BLACK, red, 2.0), red);
        assert_eq!(hue(0.0), red);
        assert_eq!(hue(1.0 / 3.0), Rgb { r: 0, g: 255, b: 0 });
        assert_eq!(hue(2.0 / 3.0), Rgb { r: 0, g: 0, b: 255 });
        assert_eq!(ashen(Rgb { r: 255, g: 255, b: 255 }), Rgb { r: 128, g: 128, b: 128 });
    }

    #[test]
    fn smoother_converges() {
        let mut s = Smoother::new(0.5);
        s.update(Rgb::BLACK);
        let target = Rgb { r: 255, g: 128, b: 0 };
        let mut c = Rgb::BLACK;
        for _ in 0..20 {
            c = s.update(target);
        }
        assert_eq!(c, target);
    }
}
