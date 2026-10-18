//! Short light animations for game events, drawn over the ambilight scene.

use std::f32::consts::{PI, TAU};
use std::time::{Duration, Instant};

use crate::color::{self, Rgb};
use crate::layout;
use crate::lol::Event;

const GOLD: Rgb = Rgb { r: 255, g: 170, b: 0 };
const ORANGE: Rgb = Rgb { r: 255, g: 90, b: 0 };
const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
const CRIMSON: Rgb = Rgb { r: 220, g: 0, b: 40 };
const CYAN: Rgb = Rgb { r: 0, g: 200, b: 255 };
const WHITE: Rgb = Rgb { r: 255, g: 255, b: 255 };
const BLUE: Rgb = Rgb { r: 0, g: 90, b: 255 };
const PURPLE: Rgb = Rgb { r: 150, g: 0, b: 255 };
const MAGENTA: Rgb = Rgb { r: 220, g: 0, b: 200 };
const DEEP_RED: Rgb = Rgb { r: 255, g: 40, b: 0 };
const BLOOD: Rgb = Rgb { r: 255, g: 0, b: 70 };
const YELLOW: Rgb = Rgb { r: 255, g: 220, b: 40 };

/// Every animation runs this many times slower than its nominal timing.
const SLOWDOWN: f32 = 1.4;
/// Fraction of each flash spent at full color before fading.
const FLASH_HOLD: f32 = 0.35;
/// Shockwave ring half-thickness and reach, in key units.
const RING_WIDTH: f32 = 0.9;
const RING_REACH: f32 = 11.0;
/// Fan LEDs per bright segment of a spinning chase.
const CHASE_PERIOD: f32 = 8.0;

#[derive(Clone, Copy, Debug)]
enum Pattern {
    /// `count` flashes that hold full color, then decay back to the scene.
    Flash { count: u32 },
    /// `count` smooth swells.
    Pulse { count: u32 },
    /// Cycling hues, fading out over the last fifth; a hue wave on the keys.
    Rainbow,
    /// `count` white-hot rings expanding across the keys with sparks in
    /// their wake; a spinning chase on the fans.
    Shockwave { count: u32 },
    /// A band crossing the keyboard from left to right.
    Sweep,
    /// Streams running down the keys at uneven speeds.
    Drip,
    /// Flickering flames rising from the bottom row (Infernal drake).
    Flames,
    /// Rolling waves with white crests (Ocean drake).
    Waves,
    /// Rock filling the board from the bottom with a rumble (Mountain drake).
    Rise,
    /// Light streaks blowing across the rows (Cloud drake).
    Gust,
    /// Electric arcs jumping between random keys (Hextech drake).
    Sparks,
    /// Bubbles rising up the columns (Chemtech drake).
    Bubbles,
    /// Spiral arms turning around the center (Baron, Elder drake).
    Vortex { arms: u32 },
    /// `count` accelerating rams that hit the right edge (Rift Herald).
    Charge { count: u32 },
}

/// Where an effect is sampled.
#[derive(Clone, Copy, Debug)]
pub enum Spot {
    /// The whole device in one color (Govee strip, whole-board keyboard).
    Whole,
    /// Key center, in `layout` key units.
    Key { x: f32, y: f32 },
    /// LED index along the fan stream.
    Led(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Effect {
    start: Instant,
    duration: Duration,
    pattern: Pattern,
    color: Rgb,
    /// Peak coverage of the scene, 0..=1.
    strength: f32,
    /// A running effect is only replaced by one of equal or higher priority.
    pub priority: u8,
}

impl Effect {
    /// `ms` is the nominal length, stretched by `SLOWDOWN`.
    fn new(now: Instant, ms: u64, pattern: Pattern, color: Rgb, strength: f32, priority: u8) -> Self {
        let duration = Duration::from_millis(ms).mul_f32(SLOWDOWN);
        Effect { start: now, duration, pattern, color, strength, priority }
    }

    /// Effect for `event`, or `None` for state changes such as `Event::Dead`.
    /// Objectives taken by the enemy get a single dim pulse.
    pub fn for_event(event: &Event, now: Instant) -> Option<Self> {
        use Pattern::*;
        let e = |ms, pattern, color, priority| Some(Effect::new(now, ms, pattern, color, 1.0, priority));
        let enemy = |color| Some(Effect::new(now, 900, Pulse { count: 1 }, color, 0.5, 1));
        match *event {
            Event::Kill => e(1000, Shockwave { count: 1 }, GOLD, 2),
            Event::Assist => Some(Effect::new(now, 800, Sweep, CYAN, 0.8, 1)),
            Event::Death => e(1600, Pulse { count: 2 }, RED, 3),
            Event::Multikill(5) => e(3500, Rainbow, WHITE, 9),
            Event::Multikill(n) => {
                let color = match n {
                    2 => ORANGE,
                    3 => DEEP_RED,
                    _ => BLOOD,
                };
                e(550 * u64::from(n) + 300, Shockwave { count: u32::from(n) }, color, 2 + n)
            }
            Event::FirstBlood => e(1600, Drip, CRIMSON, 4),
            Event::Ace { ally: true } => e(1800, Shockwave { count: 3 }, WHITE, 7),
            Event::Ace { ally: false } => enemy(RED),
            Event::Dragon { ref element, ally } => {
                let (pattern, color) = dragon(element);
                if ally {
                    let ms = if matches!(pattern, Vortex { .. }) { 3000 } else { 2400 };
                    e(ms, pattern, color, 5)
                } else {
                    enemy(color)
                }
            }
            Event::Herald { ally: true } => e(2400, Charge { count: 3 }, MAGENTA, 5),
            Event::Herald { ally: false } => enemy(MAGENTA),
            Event::Baron { ally: true } => e(3500, Vortex { arms: 3 }, PURPLE, 6),
            Event::Baron { ally: false } => enemy(PURPLE),
            Event::Turret { ally: true } => e(700, Flash { count: 1 }, BLUE, 1),
            Event::Turret { ally: false } => enemy(ORANGE),
            Event::Inhibitor { ally: true } => e(1100, Flash { count: 2 }, BLUE, 3),
            Event::Inhibitor { ally: false } => enemy(RED),
            Event::Victory => e(5000, Rainbow, WHITE, 10),
            Event::Defeat => e(3000, Pulse { count: 3 }, RED, 10),
            Event::Dead(_) => None,
        }
    }

    pub fn finished(&self, now: Instant) -> bool {
        now >= self.start + self.duration
    }

    /// Whether keys and fan LEDs get different colors, so devices that can
    /// address them individually should.
    pub fn is_spatial(&self) -> bool {
        !matches!(self.pattern, Pattern::Flash { .. } | Pattern::Pulse { .. })
    }

    /// Whole-device overlay; see `at`.
    pub fn sample(&self, now: Instant) -> Option<(Rgb, f32)> {
        self.at(now, Spot::Whole)
    }

    /// Overlay color at `now` and `spot`, and how much of the scene it covers
    /// (0..=1); `None` once the effect is over.
    pub fn at(&self, now: Instant, spot: Spot) -> Option<(Rgb, f32)> {
        if self.finished(now) {
            return None;
        }
        let real = (now - self.start).as_secs_f32();
        let t = real / self.duration.as_secs_f32();
        // Animation clock: speeds below are tuned for nominal timing.
        let elapsed = real / SLOWDOWN;
        let env = envelope(t);
        let (color, amount) = match (self.pattern, spot) {
            (Pattern::Flash { count }, _) => (self.color, flash(t, count)),
            (Pattern::Pulse { count }, _) => (self.color, pulse(t, count)),
            (Pattern::Rainbow, spot) => {
                let offset = match spot {
                    Spot::Whole => 0.0,
                    Spot::Key { x, .. } => x / layout::WIDTH,
                    Spot::Led(i) => i as f32 / (2.0 * CHASE_PERIOD),
                };
                (color::hue(elapsed * 0.8 - offset), ((1.0 - t) * 5.0).min(1.0))
            }
            (Pattern::Shockwave { count }, spot) => self.shockwave(elapsed, t, count, spot),
            (Pattern::Sweep, Spot::Key { x, .. }) => {
                let front = -2.0 + t * (layout::WIDTH + 4.0);
                let d = x - front;
                // Bright head with a short tail behind it.
                let amount = if d > 0.0 { (-(d / 0.8).powi(2)).exp() } else { (d / 2.5).exp() };
                (color::mix(self.color, WHITE, 0.4 * (-(d / 0.5).powi(2)).exp()), amount)
            }
            (Pattern::Sweep, Spot::Led(i)) => (self.color, chase(i, elapsed * 1.5) * (1.0 - t)),
            (Pattern::Sweep, Spot::Whole) => (self.color, flash(t, 1)),
            (Pattern::Drip, Spot::Key { x, y }) => {
                let column = x.floor() as u32;
                let speed = 0.6 + 0.8 * noise(column, 7);
                let head = t * speed * 1.8 * layout::HEIGHT - 0.5;
                let behind = head - y;
                let amount = if behind < 0.0 { (-(behind / 0.4).powi(2)).exp() } else { (-behind / 2.0).exp() };
                let hot = (-(behind / 0.5).powi(2)).exp();
                (color::mix(self.color, Rgb { r: 255, g: 120, b: 120 }, 0.5 * hot), amount * fade_out(t))
            }
            (Pattern::Drip, Spot::Led(i)) => (self.color, chase(i, -elapsed) * fade_out(t)),
            (Pattern::Drip, Spot::Whole) => (self.color, flash(t, 2)),
            (Pattern::Flames, spot) => {
                let slot = (elapsed * 10.0) as u32;
                match spot {
                    Spot::Key { x, y } => {
                        // Height above the bottom row, 0..1.
                        let h = (layout::HEIGHT - y) / layout::HEIGHT;
                        let column = (x * 2.0) as u32;
                        let top = (0.3 + 0.55 * smooth_noise(column, elapsed * 8.0)) * env;
                        let amount = if h < top { 1.0 } else { (-((h - top) / 0.08).powi(2)).exp() };
                        let heat = (h / top.max(0.05)).min(1.0);
                        (color::mix(YELLOW, DEEP_RED, heat), amount)
                    }
                    Spot::Led(i) => {
                        let flicker = 0.45 + 0.55 * noise(i as u32, slot);
                        (color::mix(DEEP_RED, YELLOW, noise(i as u32 + 99, slot)), flicker * env)
                    }
                    Spot::Whole => (ORANGE, (0.7 + 0.3 * noise(1, slot)) * env),
                }
            }
            (Pattern::Waves, spot) => match spot {
                Spot::Key { x, y } => {
                    let phase = x * 0.55 - elapsed * 5.0 + (y * 0.9).sin();
                    let wave = 0.5 + 0.5 * phase.sin();
                    let crest = wave.powi(6);
                    (color::mix(self.color, WHITE, 0.7 * crest), (0.25 + 0.75 * wave) * env)
                }
                Spot::Led(i) => (self.color, chase(i, elapsed * 0.8) * env),
                Spot::Whole => (self.color, (0.6 + 0.4 * (elapsed * TAU * 0.8).sin().abs()) * env),
            },
            (Pattern::Rise, spot) => {
                let slot = (elapsed * 14.0) as u32;
                // The fill reaches the top past mid-effect and then holds.
                let level = (t * 2.2).min(1.0) * (layout::HEIGHT + 0.5);
                let rumble = 0.75 + 0.25 * noise(9, slot);
                match spot {
                    Spot::Key { x, y } => {
                        let depth = level - (layout::HEIGHT - y);
                        let amount = if depth > 0.0 { rumble } else { (-(depth / 0.4).powi(2)).exp() * 0.6 };
                        let grain = noise((x * 3.0) as u32 * 31 + (y * 3.0) as u32, 3);
                        (color::mix(self.color, YELLOW, 0.35 * grain), amount * fade_out(t))
                    }
                    Spot::Led(i) => (self.color, rumble * (0.6 + 0.4 * noise(i as u32 / 4, slot)) * env),
                    Spot::Whole => (self.color, rumble * env),
                }
            }
            (Pattern::Gust, spot) => match spot {
                Spot::Key { x, y } => {
                    let row = (y + 0.25) as u32;
                    let speed = 9.0 + 6.0 * noise(row, 5);
                    let span = layout::WIDTH + 8.0;
                    let front = (elapsed * speed + noise(row, 6) * span).rem_euclid(span) - 4.0;
                    let d = x - front;
                    let streak = if d > 0.0 { (-(d / 0.6).powi(2)).exp() } else { (d / 3.0).exp() };
                    (color::mix(self.color, WHITE, 0.5 * streak), (0.15 + 0.85 * streak) * env)
                }
                Spot::Led(i) => (self.color, chase(i, elapsed * 2.0) * env),
                Spot::Whole => (self.color, (0.6 + 0.4 * smooth_noise(2, elapsed * 4.0)) * env),
            },
            (Pattern::Sparks, spot) => {
                let slot = (elapsed * 12.0) as u32;
                let glow = 0.25 + 0.15 * (elapsed * TAU * 2.0).sin();
                let arc_color = color::mix(self.color, WHITE, 0.6);
                let (arc, base) = match spot {
                    Spot::Key { x, y } => (noise((x * 4.0) as u32 * 64 + (y * 4.0) as u32, slot) > 0.86, glow),
                    Spot::Led(i) => (noise(i as u32, slot) > 0.7, glow + 0.2),
                    Spot::Whole => (noise(3, slot) > 0.6, 0.5),
                };
                if arc { (arc_color, env) } else { (self.color, base * env) }
            }
            (Pattern::Bubbles, spot) => match spot {
                Spot::Key { x, y } => {
                    let column = (x * 1.2) as u32;
                    let speed = 2.5 + 2.5 * noise(column, 11);
                    let span = layout::HEIGHT + 2.0;
                    let pos = layout::HEIGHT + 1.0 - (elapsed * speed + noise(column, 12) * span).rem_euclid(span);
                    let bubble = (-((y - pos) / 0.55).powi(2)).exp();
                    (color::mix(self.color, YELLOW, 0.4 * bubble), (0.2 + 0.8 * bubble) * env)
                }
                Spot::Led(i) => (self.color, chase(i, -elapsed * 0.9) * env),
                Spot::Whole => (self.color, (0.6 + 0.4 * smooth_noise(4, elapsed * 3.0)) * env),
            },
            (Pattern::Vortex { arms }, spot) => match spot {
                Spot::Key { x, y } => {
                    let (dx, dy) = (x - layout::WIDTH / 2.0, (y - layout::HEIGHT / 2.0) * 1.8);
                    let r = dx.hypot(dy);
                    let arm = 0.5 + 0.5 * (arms as f32 * dy.atan2(dx) - elapsed * 4.0 + r * 0.45).cos();
                    let core = (-(r / 1.6).powi(2)).exp();
                    let c = color::mix(self.color, WHITE, 0.6 * core + 0.3 * arm.powi(8));
                    (c, arm.powi(2).max(core) * env)
                }
                Spot::Led(i) => {
                    let c = color::mix(self.color, WHITE, 0.4 * chase(i, elapsed * 1.2 + 0.5));
                    (c, chase(i, elapsed * 1.2) * env)
                }
                Spot::Whole => (self.color, (0.65 + 0.35 * (elapsed * TAU * 0.7).sin()) * env),
            },
            (Pattern::Charge { count }, spot) => {
                let local = (t * count as f32).fract();
                // Accelerating run, then the impact at the right edge.
                let front = local.powi(2) * (layout::WIDTH + 3.0) - 2.0;
                let hit = (PI * ((local - 0.8) / 0.2).clamp(0.0, 1.0)).sin();
                match spot {
                    Spot::Key { x, .. } => {
                        let d = x - front;
                        let head = if d > 0.0 { (-(d / 0.7).powi(2)).exp() } else { (d / 4.0).exp() * 0.7 };
                        let blast = hit * (-(layout::WIDTH - x) / 4.0).exp();
                        let c = color::mix(self.color, WHITE, 0.6 * blast.max(head.powi(4)));
                        (c, head.max(blast))
                    }
                    Spot::Led(i) => (self.color, (chase(i, local * 3.0) * local).max(hit)),
                    Spot::Whole => (color::mix(self.color, WHITE, 0.4 * hit), (0.25 + 0.5 * local).max(hit)),
                }
            }
        };
        Some((color, (amount * self.strength).clamp(0.0, 1.0)))
    }

    fn shockwave(&self, elapsed: f32, t: f32, count: u32, spot: Spot) -> (Rgb, f32) {
        let progress = t * count as f32;
        let wave = progress.floor() as u32;
        let local = progress.fract();
        // White-hot at the blast, settling into the effect color.
        let heat = (1.0 - local).powi(3);
        let hot = color::mix(self.color, WHITE, 0.6 * heat);
        match spot {
            Spot::Whole => {
                // Flash with a shimmer the strip's update rate can still show.
                let shimmer = 0.85 + 0.15 * (elapsed * TAU * 3.0).sin();
                (hot, flash(local, 1) * shimmer)
            }
            Spot::Led(i) => {
                let burst = (1.0 - local / 0.12).max(0.0);
                let spin = chase(i, elapsed * (2.0 + count as f32)) * (1.0 - local).powf(0.7);
                (hot, burst.max(spin))
            }
            Spot::Key { x, y } => {
                // Single kills burst from the middle, streaks alternate sides.
                let cx = match (count, wave % 2) {
                    (1, _) => layout::WIDTH / 2.0,
                    (_, 0) => layout::WIDTH * 0.3,
                    _ => layout::WIDTH * 0.7,
                };
                let dist = (x - cx).hypot(y - layout::HEIGHT / 2.0);
                let radius = RING_REACH * (1.0 - (1.0 - local).powi(2));
                let d = dist - radius;
                let ring = (-(d / RING_WIDTH).powi(2)).exp();
                // Embers glowing behind the front, with random sparks.
                let wake = if d < 0.0 { 0.45 * (d / 3.0).exp() } else { 0.0 };
                let key = (x * 4.0) as u32 * 64 + (y * 4.0) as u32;
                let slot = (elapsed * 18.0) as u32;
                let spark = if d < 0.0 && noise(key, slot) > 0.75 + 0.25 * local { 1.0 } else { 0.0 };
                let fade = 1.0 - local.powi(2);
                let color = color::mix(hot, WHITE, 0.5 * ring.max(spark) * heat);
                (color, ring.max(wake).max(spark * 0.9) * fade)
            }
        }
    }
}

fn flash(t: f32, count: u32) -> f32 {
    let phase = (t * count as f32).fract();
    let decay = ((phase - FLASH_HOLD).max(0.0) / (1.0 - FLASH_HOLD)).min(1.0);
    (1.0 - decay).powi(2)
}

fn pulse(t: f32, count: u32) -> f32 {
    (PI * (t * count as f32).fract()).sin().powi(2)
}

/// Fades over the last quarter of an effect.
fn fade_out(t: f32) -> f32 {
    ((1.0 - t) * 4.0).min(1.0)
}

/// Quick fade-in, hold, then `fade_out`.
fn envelope(t: f32) -> f32 {
    (t / 0.12).min(1.0) * fade_out(t)
}

/// Bright segments every `CHASE_PERIOD` LEDs, rotated by `turns` periods.
fn chase(led: usize, turns: f32) -> f32 {
    let phase = led as f32 / CHASE_PERIOD - turns;
    let wave = 0.5 + 0.5 * (phase * TAU).cos();
    0.15 + 0.85 * wave.powi(4)
}

/// Deterministic pseudo-random value in 0..1 for a pair of integers.
fn noise(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65536.0
}

/// `noise` interpolated along a continuous `time`, for flicker without jumps.
fn smooth_noise(seed: u32, time: f32) -> f32 {
    let i = time.floor();
    let f = time - i;
    let f = f * f * (3.0 - 2.0 * f);
    let (a, b) = (noise(seed, i as u32), noise(seed, i as u32 + 1));
    a + (b - a) * f
}

/// Animation and color of each elemental drake; names from the API's `DragonType`.
fn dragon(element: &str) -> (Pattern, Rgb) {
    match element {
        "Fire" | "Infernal" => (Pattern::Flames, Rgb { r: 255, g: 50, b: 0 }),
        "Water" | "Ocean" => (Pattern::Waves, Rgb { r: 0, g: 140, b: 255 }),
        "Earth" | "Mountain" => (Pattern::Rise, Rgb { r: 200, g: 100, b: 20 }),
        "Air" | "Cloud" => (Pattern::Gust, Rgb { r: 190, g: 230, b: 255 }),
        "Hextech" => (Pattern::Sparks, Rgb { r: 0, g: 230, b: 230 }),
        "Chemtech" => (Pattern::Bubbles, Rgb { r: 90, g: 255, b: 0 }),
        "Elder" => (Pattern::Vortex { arms: 2 }, Rgb { r: 255, g: 230, b: 180 }),
        _ => (Pattern::Pulse { count: 2 }, GOLD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_fraction(effect: &Effect, now: Instant, f: f32) -> Instant {
        now + effect.duration.mul_f32(f)
    }

    #[test]
    fn flash_starts_full_and_ends() {
        let now = Instant::now();
        let effect = Effect::for_event(&Event::Turret { ally: true }, now).unwrap();
        assert_eq!(effect.duration, Duration::from_millis(700).mul_f32(SLOWDOWN));
        assert_eq!(effect.sample(now), Some((BLUE, 1.0)));
        assert_eq!(effect.sample(at_fraction(&effect, now, 0.3)), Some((BLUE, 1.0)));
        let (_, late) = effect.sample(at_fraction(&effect, now, 0.97)).unwrap();
        assert!(late < 0.05);
        assert_eq!(effect.sample(now + effect.duration), None);
    }

    #[test]
    fn kill_ring_crosses_the_board() {
        let now = Instant::now();
        let kill = Effect::for_event(&Event::Kill, now).unwrap();
        assert!(kill.is_spatial());
        let center = Spot::Key { x: layout::WIDTH / 2.0, y: layout::HEIGHT / 2.0 };
        let edge = Spot::Key { x: 0.5, y: layout::HEIGHT / 2.0 };
        let amount = |percent: u32, spot| kill.at(at_fraction(&kill, now, percent as f32 / 100.0), spot).unwrap().1;
        // The blast starts in the middle and reaches the edge later.
        assert!(amount(0, center) > 0.9);
        assert!(amount(0, edge) < 0.1);
        let peak = (0..100).max_by(|&a, &b| amount(a, edge).total_cmp(&amount(b, edge))).unwrap();
        assert!((15..60).contains(&peak), "ring reached the edge at {peak}%");
        // The whole-device version opens with a white-hot flash.
        let (color, full) = kill.sample(now).unwrap();
        assert!(full > 0.8 && color.b > 100);
    }

    #[test]
    fn penta_outranks_kill_and_enemy_objectives_are_dim() {
        let now = Instant::now();
        let penta = Effect::for_event(&Event::Multikill(5), now).unwrap();
        let kill = Effect::for_event(&Event::Kill, now).unwrap();
        assert!(penta.priority > kill.priority);
        let baron = Effect::for_event(&Event::Baron { ally: false }, now).unwrap();
        let peak = baron.sample(at_fraction(&baron, now, 0.5)).unwrap().1;
        assert!((peak - 0.5).abs() < 0.01);
        assert!(Effect::for_event(&Event::Dead(true), now).is_none());
    }

    #[test]
    fn objective_animations_stay_in_range_and_move() {
        let now = Instant::now();
        let mut events: Vec<Event> = ["Fire", "Water", "Earth", "Air", "Hextech", "Chemtech", "Elder"]
            .iter()
            .map(|e| Event::Dragon { element: (*e).into(), ally: true })
            .collect();
        events.extend([Event::Baron { ally: true }, Event::Herald { ally: true }]);
        let mut spots: Vec<Spot> =
            layout::ISO_UK.iter().map(|k| Spot::Key { x: k.x + k.w / 2.0, y: k.y + k.h / 2.0 }).collect();
        spots.extend((0..40).map(Spot::Led));
        spots.push(Spot::Whole);
        for event in &events {
            let effect = Effect::for_event(event, now).unwrap();
            assert!(effect.is_spatial(), "{event:?}");
            let frame = |f: f32| -> Vec<f32> {
                let when = at_fraction(&effect, now, f);
                spots.iter().map(|&s| effect.at(when, s).unwrap().1).collect()
            };
            for f in [0.0, 0.2, 0.5, 0.8, 0.99] {
                assert!(frame(f).iter().all(|v| (0.0..=1.0).contains(v)), "{event:?} at {f}");
            }
            let (a, b) = (frame(0.4), frame(0.45));
            assert!(a.iter().zip(&b).any(|(x, y)| (x - y).abs() > 0.05), "{event:?} does not animate");
        }
    }
}
