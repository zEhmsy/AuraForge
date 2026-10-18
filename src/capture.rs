//! Rate-limited screen capture through DXGI Desktop Duplication.
//!
//! This is the same OS service used by OBS, Discord and Teams: the compositor
//! hands out a read-only copy of the desktop image. Nothing is injected into
//! or hooked inside any other process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{Error, Interface, Result};
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};

use crate::color::{self, Analyzer, FrameView, Rgb, Smoother};
use crate::layout;

/// Hard ceiling regardless of configuration.
pub const MAX_FPS: u32 = 20;

#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub fps: u32,
    pub monitor: u32,
    pub border_percent: u32,
    pub stride: usize,
    pub smoothing: f32,
    pub threshold: u8,
    /// Sample the whole screen once per key instead of the border once.
    pub per_key: bool,
    /// Gamma applied to brightness by `color::boost`; 1.0 disables it.
    pub boost: f32,
}

struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    border: u32,
}

struct Duplicator {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<Staging>,
}

impl Duplicator {
    fn new(monitor: u32) -> Result<Self> {
        let (adapter, output) = find_output(monitor)?;
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device = device.ok_or_else(|| Error::from(E_FAIL))?;
        let context = context.ok_or_else(|| Error::from(E_FAIL))?;
        let output: IDXGIOutput1 = output.cast()?;
        let duplication = unsafe { output.DuplicateOutput(&device)? };
        Ok(Duplicator { device, context, duplication, staging: None })
    }

    /// Copies the next desktop image into the staging texture: the whole frame
    /// when `full_frame` is set, only the border bands otherwise.
    /// Returns `false` when the desktop has not changed.
    fn grab(&mut self, border_percent: u32, full_frame: bool) -> Result<bool> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        if let Err(e) = unsafe { self.duplication.AcquireNextFrame(0, &mut info, &mut resource) } {
            return if e.code() == DXGI_ERROR_WAIT_TIMEOUT { Ok(false) } else { Err(e) };
        }
        let copied = match resource {
            // A zero present time means only the mouse pointer moved.
            Some(resource) if info.LastPresentTime != 0 => {
                resource.cast().and_then(|t| self.copy(&t, border_percent, full_frame)).map(|()| true)
            }
            _ => Ok(false),
        };
        unsafe { self.duplication.ReleaseFrame()? };
        copied
    }

    fn copy(&mut self, frame: &ID3D11Texture2D, border_percent: u32, full_frame: bool) -> Result<()> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { frame.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(Error::new(E_FAIL, "unsupported desktop format (expected BGRA8)"));
        }
        let (w, h) = (desc.Width, desc.Height);
        if self.staging.as_ref().is_none_or(|s| s.width != w || s.height != h) {
            self.staging = Some(self.create_staging(&desc, border_percent)?);
        }
        let staging = self.staging.as_ref().unwrap();
        if full_frame {
            unsafe { self.context.CopyResource(&staging.texture, frame) };
            return Ok(());
        }
        let t = staging.border;
        // Only the four bands are copied; the centre of the staging texture stays stale.
        let bands = [(0, 0, w, t), (0, h - t, w, h), (0, t, t, h - t), (w - t, t, w, h - t)];
        for (left, top, right, bottom) in bands {
            let region = D3D11_BOX { left, top, front: 0, right, bottom, back: 1 };
            unsafe {
                self.context.CopySubresourceRegion(
                    &staging.texture,
                    0,
                    left,
                    top,
                    0,
                    frame,
                    0,
                    Some(&region),
                );
            }
        }
        Ok(())
    }

    fn create_staging(&self, frame: &D3D11_TEXTURE2D_DESC, border_percent: u32) -> Result<Staging> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: frame.Width,
            Height: frame.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: frame.Format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut texture))? };
        let border = (frame.Width.min(frame.Height) * border_percent / 100).max(1);
        Ok(Staging {
            texture: texture.ok_or_else(|| Error::from(E_FAIL))?,
            width: frame.Width,
            height: frame.Height,
            border,
        })
    }

    /// Maps the staging texture and runs `f` on it.
    fn with_view<R>(&self, f: impl FnOnce(&FrameView) -> R) -> Result<R> {
        let staging = self.staging.as_ref().ok_or_else(|| Error::from(E_FAIL))?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { self.context.Map(&staging.texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };
        let pitch = mapped.RowPitch as usize;
        let len = pitch * staging.height as usize;
        // SAFETY: the mapping covers `RowPitch * Height` bytes until `Unmap`.
        let data = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, len) };
        let view = FrameView {
            data,
            pitch,
            width: staging.width as usize,
            height: staging.height as usize,
            border: staging.border as usize,
        };
        let result = f(&view);
        unsafe { self.context.Unmap(&staging.texture, 0) };
        Ok(result)
    }
}

/// Monitors are numbered across all adapters in DXGI enumeration order.
fn find_output(monitor: u32) -> Result<(IDXGIAdapter1, IDXGIOutput)> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut index = 0;
    for a in 0.. {
        let adapter = unsafe { factory.EnumAdapters1(a) }
            .map_err(|_| Error::new(E_FAIL, format!("monitor {monitor} not found")))?;
        for o in 0.. {
            let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else { break };
            if index == monitor {
                return Ok((adapter, output));
            }
            index += 1;
        }
    }
    unreachable!()
}

/// Describes every output, in the order of the indices accepted by `--monitor`.
pub fn monitors() -> Result<Vec<String>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut list = Vec::new();
    for a in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(a) }) else { break };
        for o in 0.. {
            let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else { break };
            let desc = unsafe { output.GetDesc()? };
            let name = String::from_utf16_lossy(&desc.DeviceName);
            let r = desc.DesktopCoordinates;
            list.push(format!(
                "{}  {}x{} at ({}, {})",
                name.trim_end_matches('\0'),
                r.right - r.left,
                r.bottom - r.top,
                r.left,
                r.top
            ));
        }
    }
    Ok(list)
}

/// Sleeps so that consecutive ticks are never closer than `period`.
struct RateLimiter {
    period: Duration,
    next: Instant,
}

impl RateLimiter {
    fn new(fps: u32) -> Self {
        RateLimiter { period: Duration::from_secs(1) / fps, next: Instant::now() }
    }

    fn wait(&mut self) {
        let now = Instant::now();
        if now < self.next {
            thread::sleep(self.next - now);
        }
        // Measured from the actual wake-up, so a late tick never causes a catch-up burst.
        self.next = Instant::now() + self.period;
    }
}

/// Per-key zones: each key samples the screen area under its position on the board.
fn key_colors(view: &FrameView, stride: usize) -> Vec<Rgb> {
    let sx = view.width as f32 / layout::WIDTH;
    let sy = view.height as f32 / layout::HEIGHT;
    layout::ISO_UK
        .iter()
        .map(|k| {
            let x0 = (k.x * sx) as usize;
            let y0 = (k.y * sy) as usize;
            let x1 = (((k.x + k.w) * sx) as usize).min(view.width);
            let y1 = (((k.y + k.h) * sy) as usize).min(view.height);
            color::average(view, x0, y0, x1, y1, stride)
        })
        .collect()
}

/// Sends one color per `layout::ISO_UK` key (per-key only) followed by the
/// dominant border color, which is always the last slot.
pub fn run(config: CaptureConfig, tx: SyncSender<Vec<Rgb>>, stop: &AtomicBool) {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let mut limiter = RateLimiter::new(config.fps.clamp(1, MAX_FPS));
    let mut analyzer = Analyzer::new();
    let mut smoothers: Vec<Smoother> = Vec::new();
    let mut duplicator: Option<Duplicator> = None;
    let mut retry_at = Instant::now();
    let mut target: Option<Vec<Rgb>> = None;
    let mut last_sent: Option<Vec<Rgb>> = None;

    while !stop.load(Ordering::Relaxed) {
        limiter.wait();

        if duplicator.is_none() && Instant::now() >= retry_at {
            match Duplicator::new(config.monitor) {
                Ok(d) => {
                    log!("desktop duplication started on monitor {}", config.monitor);
                    duplicator = Some(d);
                }
                Err(e) => {
                    log!("desktop duplication unavailable: {}", e.message());
                    retry_at = Instant::now() + Duration::from_secs(2);
                }
            }
        }

        if let Some(d) = duplicator.as_mut() {
            let result = d.grab(config.border_percent, config.per_key).and_then(|fresh| {
                if !fresh {
                    return Ok(None);
                }
                d.with_view(|view| {
                    let mut colors =
                        if config.per_key { key_colors(view, config.stride) } else { Vec::new() };
                    colors.push(analyzer.dominant(view, config.stride));
                    colors
                })
                .map(Some)
            });
            match result {
                Ok(Some(colors)) => {
                    target = Some(colors.into_iter().map(|c| color::boost(c, config.boost)).collect())
                }
                Ok(None) => {}
                // Mode changes, exclusive fullscreen and the secure desktop
                // invalidate the duplication; rebuild it after a short pause.
                Err(e) => {
                    log!("desktop duplication lost: {}", e.message());
                    duplicator = None;
                    retry_at = Instant::now() + Duration::from_millis(500);
                }
            }
        }

        // Keep stepping the smoothers even on static frames so fades complete.
        let Some(target) = target.as_ref() else { continue };
        smoothers.resize_with(target.len(), || Smoother::new(config.smoothing));
        let colors: Vec<Rgb> = smoothers.iter_mut().zip(target).map(|(s, &t)| s.update(t)).collect();
        let changed = last_sent.as_ref().is_none_or(|last| {
            last.iter().zip(&colors).any(|(a, &b)| a.distance(b) >= config.threshold)
        });
        if !changed {
            continue;
        }
        match tx.try_send(colors.clone()) {
            Ok(()) => last_sent = Some(colors),
            // USB thread is still busy; retry with fresher colors next tick.
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => break,
        }
    }
}
