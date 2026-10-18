use crate::aura::FanConfig;
use crate::capture::{CaptureConfig, MAX_FPS};
use crate::color::Rgb;
use crate::govee::GoveeConfig;
use crate::hid::Target;
use crate::lights::Outputs;
use crate::protocol::{self, EffectParams, BRIGHTNESS_MAX, MODE_STATIC};

pub const USAGE: &str = "\
Usage: auraforge-cli [options]

  --per-key        color every key from the screen area above it
  --fps N          capture rate, 1-20 (default 15, 8 with --per-key)
  --monitor N      monitor index across all GPUs (default 0)
  --border PCT     thickness of the sampled screen border, 1-50% (default 12)
  --stride N       sample every N-th pixel in each direction (default 16)
  --boost G        brighten dark colors, gamma 0.2-1.0 (default 0.5, 1.0 = off)
  --smoothing A    fade factor per frame, 0.05-1.0 (default 0.35)
  --threshold N    minimum channel change before writing (default 6)
  --max-reports N  per-key: at most N packets (5 keys each) per update (default 9)
  --brightness N   keyboard brightness, 0-100 (default 100)
  --profile N      keyboard profile id (default 1)
  --mode N         ambient lighting mode byte (default 1 = static)
  --keyboard-idle RRGGBB  keyboard ignores the screen: shows this color and
                   only the League effects (fans keep following the screen)
  --keyboard-theme NAME  firmware animation behind the League effects, needs
                   --keyboard-idle (its color): static, breath, streamer, rain,
                   horse-race, stars, reactive, waterdrop, cross, ripple, aurora,
                   bomb, heatmap, spotlight, radar, snaking, convolution
  --theme-speed N  animation speed byte (default 2)
  --theme-dir N    animation direction: 0 right, 1 left, 2 down, 3 up
  --theme-color C  theme palette: fixed (default), rainbow, random
  --fans           also paint the ARGB fans (ASUS Aura header) with the scene color
  --fan-header N   0-based addressable header the fans hang off (default 0)
  --fan-leds N     LEDs painted per update, 1-120 (default 40)
  --govee IP       also paint a Govee LED strip (LAN API, enable LAN Control
                   in Govee Home) with the fan color
  --govee-brightness N  strip brightness set on connect, 1-100
  --govee-scan     list Govee strips answering on the LAN and exit
  --lol            flash effects for League of Legends kills, deaths and objectives
  --lol-demo       cycle through every League effect without a match
  --dry-run        print colors instead of writing to any device
  --list-monitors  print monitor indices and exit
  -h, --help       show this help
";

pub struct Config {
    pub capture: CaptureConfig,
    pub outputs: Outputs,
    /// Live League events from the game client.
    pub lol: bool,
    /// Cycle through every effect; combinable with `lol`.
    pub lol_demo: bool,
    pub list_monitors: bool,
    pub govee_scan: bool,
}

impl Config {
    /// Returns `Ok(None)` when help was requested.
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Result<Option<Self>, String> {
        let mut capture = CaptureConfig {
            fps: 0,
            monitor: 0,
            border_percent: 12,
            stride: 16,
            smoothing: 0.35,
            threshold: 6,
            per_key: false,
            boost: 0.5,
        };
        let mut effect = EffectParams {
            device_id: 1,
            profile_id: 1,
            mode: MODE_STATIC,
            speed: 2,
            brightness: BRIGHTNESS_MAX,
            direction: 0,
            color_flags: 0,
        };
        let mut dry_run = false;
        let mut list_monitors = false;
        let mut max_reports = 9;
        let mut fans = false;
        let mut fan = FanConfig { header: 0, leds: 40 };
        let mut lol = false;
        let mut lol_demo = false;
        let mut keyboard_idle = None;
        let mut govee_ip = None;
        let mut govee_brightness = None;
        let mut govee_scan = false;

        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("missing value for {flag}"));
            match flag.as_str() {
                "--fps" => capture.fps = parse(&flag, &value()?, 1, MAX_FPS)?,
                "--monitor" => capture.monitor = parse(&flag, &value()?, 0, 15)?,
                "--border" => capture.border_percent = parse(&flag, &value()?, 1, 50)?,
                "--stride" => capture.stride = parse(&flag, &value()?, 1, 512)?,
                "--boost" => capture.boost = parse(&flag, &value()?, 0.2, 1.0)?,
                "--smoothing" => capture.smoothing = parse(&flag, &value()?, 0.05, 1.0)?,
                "--threshold" => capture.threshold = parse(&flag, &value()?, 0, 255)?,
                "--brightness" => effect.brightness = parse(&flag, &value()?, 0, BRIGHTNESS_MAX)?,
                "--profile" => effect.profile_id = parse(&flag, &value()?, 0, 255)?,
                "--mode" => effect.mode = parse(&flag, &value()?, 0, 255)?,
                "--per-key" => capture.per_key = true,
                "--max-reports" => max_reports = parse(&flag, &value()?, 1, 18)?,
                "--keyboard-idle" => keyboard_idle = Some(parse_hex(&flag, &value()?)?),
                "--keyboard-theme" => effect.mode = parse_theme(&value()?)?,
                "--theme-speed" => effect.speed = parse(&flag, &value()?, 0, 255)?,
                "--theme-dir" => effect.direction = parse(&flag, &value()?, 0, 3)?,
                "--theme-color" => {
                    effect.color_flags = match value()?.as_str() {
                        "fixed" => 0,
                        "rainbow" => protocol::COLOR_HUE,
                        "random" => protocol::COLOR_RANDOM,
                        _ => return Err(format!("{flag} expects fixed, rainbow or random")),
                    }
                }
                "--fans" => fans = true,
                "--fan-header" => fan.header = parse(&flag, &value()?, 0, 2)?,
                "--fan-leds" => fan.leds = parse(&flag, &value()?, 1, 120)?,
                "--govee" => {
                    let v = value()?;
                    govee_ip = Some(v.parse().map_err(|_| format!("{flag} expects an IPv4 address, not {v}"))?);
                }
                "--govee-brightness" => govee_brightness = Some(parse(&flag, &value()?, 1, 100)?),
                "--govee-scan" => govee_scan = true,
                "--lol" => lol = true,
                "--lol-demo" => lol_demo = true,
                "--dry-run" => dry_run = true,
                "--list-monitors" => list_monitors = true,
                "-h" | "--help" => return Ok(None),
                _ => return Err(format!("unknown option {flag}")),
            }
        }
        if keyboard_idle.is_some() {
            // Per-key sampling would only feed the fans, which use the border color.
            capture.per_key = false;
        }
        if capture.fps == 0 {
            capture.fps = if capture.per_key { 8 } else { 15 };
        }
        let target = if capture.per_key {
            Target::PerKey { params: effect, threshold: capture.threshold, max_reports }
        } else {
            Target::Ambient(effect)
        };
        let govee = govee_ip.map(|ip| GoveeConfig { ip, brightness: govee_brightness });
        let outputs = Outputs { target, fans: fans.then_some(fan), govee, keyboard_idle, dry_run };
        Ok(Some(Config { capture, outputs, lol, lol_demo, list_monitors, govee_scan }))
    }
}

fn parse_theme(name: &str) -> Result<u8, String> {
    protocol::THEMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|&(_, mode)| mode)
        .ok_or_else(|| format!("unknown keyboard theme {name}"))
}

fn parse_hex(flag: &str, value: &str) -> Result<Rgb, String> {
    let v = value.trim_start_matches('#');
    match u32::from_str_radix(v, 16) {
        Ok(n) if v.len() == 6 => Ok(Rgb { r: (n >> 16) as u8, g: (n >> 8) as u8, b: n as u8 }),
        _ => Err(format!("{flag} expects a color like FF8800")),
    }
}

fn parse<T>(flag: &str, value: &str, min: T, max: T) -> Result<T, String>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display,
{
    match value.parse::<T>() {
        Ok(v) if v >= min && v <= max => Ok(v),
        _ => Err(format!("{flag} expects a value between {min} and {max}")),
    }
}
