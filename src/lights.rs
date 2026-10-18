//! Output thread: merges the captured scene with game effects and fans the
//! result out to one thread per device (keyboard, fans, Govee strip), so a
//! slow or unplugged device never holds back the others.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::aura::{FanConfig, Fans};
use crate::color::{self, Rgb};
use crate::effects::{Effect, Spot};
use crate::fanout::{self, Publisher};
use crate::govee::{self, GoveeConfig, Strip};
use crate::hid::{self, HidWriter, Target};
use crate::layout;
use crate::lol::Event;
use crate::protocol::{self, EffectParams};

/// Frame period while an effect animates.
const EFFECT_FRAME: Duration = Duration::from_millis(33);
/// Key animations skip channel changes smaller than this.
const ANIMATION_THRESHOLD: u8 = 10;
const IDLE_WAIT: Duration = Duration::from_millis(100);

pub struct Outputs {
    pub target: Target,
    pub fans: Option<FanConfig>,
    pub govee: Option<GoveeConfig>,
    /// Keyboard ignores the screen and shows this color between effects.
    pub keyboard_idle: Option<Rgb>,
    pub dry_run: bool,
}

/// One keyboard frame: per-key colors (a single slot in ambient mode) and the
/// effect drawn over them, if any.
struct Frame {
    keys: Vec<Rgb>,
    /// Effect color painted over the whole board.
    overlay: Option<Rgb>,
    /// Per-key effect colors in `layout::ISO_UK` order, drawn instead of
    /// `overlay`, and the color the board starts from.
    animation: Option<(Rgb, Vec<Rgb>)>,
}

/// A device driven by its own thread from the newest published frame.
trait Sink: Send + 'static {
    type Frame: Send + 'static;
    fn show(&mut self, frame: Self::Frame);
    /// Called when no frame arrived for `IDLE_WAIT`.
    fn idle(&mut self) {}
}

fn spawn<S: Sink>(name: &str, mut sink: S, threads: &mut Vec<JoinHandle<()>>) -> Publisher<S::Frame> {
    let (tx, rx) = fanout::mailbox();
    let handle = thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            while let Ok(frame) = rx.recv_timeout(IDLE_WAIT) {
                match frame {
                    Some(f) => sink.show(f),
                    None => sink.idle(),
                }
            }
        })
        .expect("spawn device thread");
    threads.push(handle);
    tx
}

pub fn run(scenes: Receiver<Vec<Rgb>>, events: Receiver<Event>, outputs: Outputs) {
    let per_key = matches!(outputs.target, Target::PerKey { .. });
    let live = !outputs.dry_run;
    let mut threads = Vec::new();
    let keyboard = live.then(|| spawn("keyboard", KeyboardOut::new(outputs.target), &mut threads));
    let fans = outputs.fans.filter(|_| live).map(|c| spawn("fans", FansOut::new(c), &mut threads));
    let strip = outputs.govee.filter(|_| live).map(|c| spawn("govee", GoveeOut::new(c), &mut threads));
    let mut dry = outputs.dry_run.then(|| DryOut {
        per_key,
        fans: outputs.fans.is_some(),
        strip: outputs.govee.is_some(),
        drawn: false,
        last: None,
    });
    let idle = outputs.keyboard_idle;
    let fan_leds = outputs.fans.map_or(0, |c| c.leds);
    let key_spots: Vec<Spot> = layout::ISO_UK
        .iter()
        .map(|k| Spot::Key { x: k.x + k.w / 2.0, y: k.y + k.h / 2.0 })
        .collect();

    let mut scene: Option<Vec<Rgb>> = None;
    let mut effect: Option<Effect> = None;
    let mut dead = false;

    loop {
        let wait = if effect.is_some() { EFFECT_FRAME } else { IDLE_WAIT };
        match scenes.recv_timeout(wait) {
            Ok(colors) => scene = Some(colors),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        while let Ok(event) = events.try_recv() {
            if let Event::Dead(d) = event {
                dead = d;
                continue;
            }
            let Some(new) = Effect::for_event(&event, now) else { continue };
            if effect.is_none_or(|e| e.finished(now) || new.priority >= e.priority) {
                effect = Some(new);
            }
        }

        let Some(colors) = scene.as_ref() else { continue };
        // The capture thread always appends the dominant border color.
        let (&ambient, keys) = colors.split_last().expect("scene has an ambient slot");
        let dim = |c: Rgb| if dead { color::ashen(c) } else { c };
        if effect.is_some_and(|e| e.finished(now)) {
            effect = None;
        }
        let sample = effect.and_then(|e| e.sample(now));
        let paint = |base: Rgb| sample.map(|(c, amount)| color::mix(base, c, amount));
        let spatial = effect.filter(|e| e.is_spatial());

        let board_keys: Vec<Rgb> = match idle {
            Some(c) => vec![dim(c)],
            None if per_key => keys.iter().map(|&c| dim(c)).collect(),
            None => vec![dim(ambient)],
        };
        let board_base = idle.map_or(dim(ambient), dim);
        let animation = spatial.map(|e| {
            let keys = key_spots
                .iter()
                .enumerate()
                .map(|(i, &spot)| {
                    let base = if board_keys.len() > 1 { board_keys[i] } else { board_base };
                    e.at(now, spot).map_or(base, |(c, amount)| color::mix(base, c, amount))
                })
                .collect();
            (board_base, keys)
        });
        let overlay = if animation.is_some() { None } else { paint(board_base) };
        let frame = Frame { keys: board_keys, overlay, animation };
        // Fans and strip sit behind glass or a wall and blend with the scene, so
        // they keep the effect color longer: the square root slows the fade-out.
        let glow_at = |spot| {
            effect
                .and_then(|e| e.at(now, spot))
                .map_or(dim(ambient), |(c, amount)| color::mix(dim(ambient), c, amount.sqrt()))
        };
        let glow = glow_at(Spot::Whole);

        if let Some(d) = dry.as_mut() {
            d.show(&frame, glow);
        }
        if let Some(tx) = &fans {
            tx.publish((0..fan_leds).map(|i| glow_at(Spot::Led(i))).collect());
        }
        if let Some(tx) = &strip {
            tx.publish(glow);
        }
        if let Some(tx) = &keyboard {
            tx.publish(frame);
        }
    }
    // Closing the mailboxes ends the device threads.
    drop((keyboard, fans, strip));
    for t in threads {
        let _ = t.join();
    }
}

/// Retries a device at most this often after a failure.
struct Backoff {
    retry_at: Instant,
}

impl Backoff {
    fn ready(&self) -> bool {
        Instant::now() >= self.retry_at
    }

    fn wait(&mut self, secs: u64) {
        self.retry_at = Instant::now() + Duration::from_secs(secs);
    }
}

struct KeyboardOut {
    target: Target,
    device: Option<HidWriter>,
    backoff: Backoff,
    /// What each key shows; `None` slots are unknown and always rewritten.
    written: Vec<Option<Rgb>>,
    /// Whole-board color currently painted by an effect.
    board: Option<Rgb>,
    /// What each `layout::ISO_UK` key shows while an animation runs.
    animated: Option<Vec<Option<Rgb>>>,
    /// Start, frames and reports of the running animation, logged at its end
    /// to show how fast the board keeps up.
    stats: (Instant, u32, u32),
}

impl KeyboardOut {
    fn new(target: Target) -> Self {
        KeyboardOut {
            target,
            device: None,
            backoff: Backoff { retry_at: Instant::now() },
            written: Vec::new(),
            board: None,
            animated: None,
            stats: (Instant::now(), 0, 0),
        }
    }

    fn connect(&mut self) -> Option<&HidWriter> {
        if self.device.is_none() && self.backoff.ready() {
            match hid::open_keyboard() {
                Ok(device) => {
                    log!("keyboard connected");
                    // Per-key colors are scaled by the profile brightness, so set it
                    // once with a black static base; unmapped keys (Fn) stay dark.
                    if let Target::PerKey { params, .. } = self.target {
                        let base = EffectParams { mode: protocol::MODE_STATIC, color_flags: 0, ..params };
                        if let Err(e) = device.write(&protocol::light_effect(&base, Rgb::BLACK)) {
                            log!("keyboard write failed: {}", e.message());
                            self.backoff.wait(1);
                            return None;
                        }
                    }
                    self.device = Some(device);
                    // A replugged board may show anything; rewrite every slot.
                    self.written.fill(None);
                    self.board = None;
                    self.animated = None;
                }
                Err(e) => {
                    log!("{e}");
                    self.backoff.wait(3);
                }
            }
        }
        self.device.as_ref()
    }

}

impl KeyboardOut {
    /// Draws one animation frame key by key, starting from a static `base`
    /// board so keys without a report yet blend in.
    fn animate(&mut self, base: Rgb, keys: &[Rgb]) {
        let params = self.target.params();
        let mut reports = Vec::new();
        let animated = self.animated.get_or_insert_with(|| {
            reports.push((self.target.whole_board(base), Vec::new()));
            self.stats = (Instant::now(), 0, 0);
            vec![Some(base); keys.len()]
        });
        reports.extend(hid::key_reports(&params, keys, animated, ANIMATION_THRESHOLD, layout::ISO_UK.len()));
        self.stats.1 += 1;
        self.stats.2 += reports.len() as u32;
        let device = self.device.as_ref().expect("connected");
        for (report, slots) in reports {
            if let Err(e) = device.write(&report) {
                log!("keyboard write failed: {}", e.message());
                self.device = None;
                self.animated = None;
                self.backoff.wait(1);
                return;
            }
            for i in slots {
                animated[i] = Some(keys[i]);
            }
        }
    }
}

impl Sink for KeyboardOut {
    type Frame = Frame;

    fn show(&mut self, frame: Frame) {
        self.written.resize(frame.keys.len(), None);
        if self.connect().is_none() {
            return;
        }
        if let Some((base, keys)) = &frame.animation {
            self.animate(*base, keys);
            return;
        }
        if self.animated.take().is_some() {
            let (start, frames, reports) = self.stats;
            log!("key animation: {frames} frames, {reports} reports in {} ms", start.elapsed().as_millis());
            // The animation covered the base colors; repaint them.
            self.written.fill(None);
            self.board = None;
        }
        let reports: Vec<([u8; protocol::REPORT_LEN], Vec<usize>)> = match frame.overlay {
            Some(color) if self.board == Some(color) => Vec::new(),
            Some(color) => {
                // One packet paints the whole board; the keys must be rewritten afterwards.
                self.written.fill(None);
                self.board = Some(color);
                vec![(self.target.whole_board(color), Vec::new())]
            }
            None => {
                self.board = None;
                // Changes beyond the per-update budget are picked up on the next pass.
                self.target.reports(&frame.keys, &self.written)
            }
        };
        let device = self.device.as_ref().expect("connected");
        for (report, slots) in reports {
            if let Err(e) = device.write(&report) {
                log!("keyboard write failed: {}", e.message());
                self.device = None;
                self.backoff.wait(1);
                return;
            }
            for i in slots {
                self.written[i] = Some(frame.keys[i]);
            }
        }
    }
}

struct FansOut {
    config: FanConfig,
    device: Option<Fans>,
    backoff: Backoff,
    shown: Option<Vec<Rgb>>,
}

impl FansOut {
    fn new(config: FanConfig) -> Self {
        FansOut { config, device: None, backoff: Backoff { retry_at: Instant::now() }, shown: None }
    }

}

impl Sink for FansOut {
    /// One color per LED.
    type Frame = Vec<Rgb>;

    fn show(&mut self, colors: Vec<Rgb>) {
        if self.device.is_none() && self.backoff.ready() {
            match Fans::open(self.config) {
                Ok(f) => {
                    log!("fans connected (header {}, {} LEDs)", self.config.header, self.config.leds);
                    self.device = Some(f);
                    self.shown = None;
                }
                Err(e) => {
                    log!("{e}");
                    self.backoff.wait(3);
                }
            }
        }
        let Some(fans) = self.device.as_ref() else { return };
        if self.shown.as_ref() == Some(&colors) {
            return;
        }
        match fans.show(&colors) {
            Ok(()) => self.shown = Some(colors),
            Err(e) => {
                log!("fan write failed: {e}");
                self.device = None;
                self.backoff.wait(1);
            }
        }
    }
}

/// Sends the newest color to the Govee strip at most every
/// `govee::MIN_INTERVAL`, and resends it every `govee::REFRESH` since UDP
/// datagrams may be lost.
struct GoveeOut {
    config: GoveeConfig,
    strip: Option<Strip>,
    backoff: Backoff,
    pending: Option<Rgb>,
    sent: Option<(Rgb, Instant)>,
}

impl GoveeOut {
    fn new(config: GoveeConfig) -> Self {
        GoveeOut { config, strip: None, backoff: Backoff { retry_at: Instant::now() }, pending: None, sent: None }
    }

    fn flush(&mut self) {
        let Some(color) = self.pending else { return };
        let now = Instant::now();
        if let Some((last, at)) = self.sent {
            let since = now - at;
            if since < govee::MIN_INTERVAL || (last == color && since < govee::REFRESH) {
                return;
            }
        }
        if self.strip.is_none() && self.backoff.ready() {
            match Strip::open(self.config) {
                Ok(s) => {
                    log!("Govee strip {} connected", self.config.ip);
                    self.strip = Some(s);
                }
                Err(e) => {
                    log!("Govee strip {}: {e}", self.config.ip);
                    self.backoff.wait(5);
                }
            }
        }
        let Some(strip) = self.strip.as_ref() else { return };
        match strip.show(color) {
            Ok(()) => self.sent = Some((color, now)),
            Err(e) => {
                log!("Govee write failed: {e}");
                self.strip = None;
                self.backoff.wait(5);
            }
        }
    }
}

impl Sink for GoveeOut {
    type Frame = Rgb;

    fn show(&mut self, color: Rgb) {
        self.pending = Some(color);
        self.flush();
    }

    fn idle(&mut self) {
        self.flush();
    }
}

/// Prints frames instead of touching any device. Per-key frames are drawn as
/// a keyboard of 24-bit colored blocks, redrawn in place.
struct DryOut {
    per_key: bool,
    fans: bool,
    strip: bool,
    drawn: bool,
    last: Option<(Rgb, Rgb)>,
}

impl DryOut {
    fn show(&mut self, frame: &Frame, fan: Rgb) {
        if !self.per_key && frame.animation.is_none() {
            if self.last == Some((frame.keys[0], fan)) {
                return;
            }
            self.last = Some((frame.keys[0], fan));
            let board = frame.overlay.unwrap_or(frame.keys[0]);
            match (self.fans, self.strip) {
                (false, false) => log!("{}", hex(board)),
                (true, false) => log!("keyboard {}  fans {}", hex(board), hex(fan)),
                (false, true) => log!("keyboard {}  strip {}", hex(board), hex(fan)),
                (true, true) => log!("keyboard {}  fans/strip {}", hex(board), hex(fan)),
            }
            return;
        }
        let mut out = String::new();
        if self.drawn {
            out.push_str("\x1b[7A");
        }
        let mut row_y = layout::ISO_UK[0].y;
        let mut col = 0.0;
        for (i, key) in layout::ISO_UK.iter().enumerate() {
            let c = match &frame.animation {
                Some((_, keys)) => keys[i],
                None => frame.overlay.unwrap_or(*frame.keys.get(i).unwrap_or(&frame.keys[0])),
            };
            if key.y != row_y {
                out.push_str("\x1b[0m\x1b[K\n");
                row_y = key.y;
                col = 0.0;
            }
            // Two terminal columns per key unit.
            let pad = ((key.x - col) * 2.0).round() as usize;
            let cells = ((key.w * 2.0).round() as usize).max(1);
            out.push_str("\x1b[0m");
            out.push_str(&" ".repeat(pad));
            out.push_str(&format!("\x1b[38;2;{};{};{}m{}", c.r, c.g, c.b, "\u{2588}".repeat(cells)));
            col = key.x + key.w;
        }
        out.push_str(&format!(
            "\x1b[0m\x1b[K\nfans \x1b[38;2;{};{};{}m{}\x1b[0m {}\x1b[K",
            fan.r,
            fan.g,
            fan.b,
            "\u{2588}".repeat(8),
            hex(fan)
        ));
        println!("{out}");
        self.drawn = true;
    }
}

fn hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}
