//! Write-only HID output, shared by the keyboard and the fan controller.
//!
//! `hidapi` is used only to enumerate devices and find the interface path;
//! enumeration opens each device with zero access rights. hidapi's own
//! `open_path` always requests `GENERIC_READ | GENERIC_WRITE`, so the handle
//! is opened here with `GENERIC_WRITE` alone. No input report is ever read.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::{Error, PCWSTR};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetPreparsedData, HidP_GetCaps, HIDP_CAPS,
    PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, E_FAIL, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::color::Rgb;
use crate::layout;
use crate::protocol::{self, EffectParams, KEYS_PER_REPORT, REPORT_LEN};

const WRITE_TIMEOUT_MS: u32 = 500;

pub struct HidWriter {
    handle: HANDLE,
    event: HANDLE,
}

// Kernel handles are not tied to the thread that opened them; the writer owns
// both exclusively and moves to its device thread whole.
unsafe impl Send for HidWriter {}

impl HidWriter {
    /// Opens the first interface accepted by `filter`, write-only, and checks
    /// that its output reports are `report_len` bytes long.
    pub fn open(
        what: &str,
        report_len: usize,
        filter: impl Fn(&hidapi::DeviceInfo) -> bool,
    ) -> Result<Self, String> {
        let path = find_path(what, filter)?;
        let wide: Vec<u16> = OsStr::new(&path).encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(|e| format!("cannot open {what} for writing: {}", e.message()))?;
        let event = match unsafe { CreateEventW(None, true, false, None) } {
            Ok(event) => event,
            Err(e) => {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(e.message());
            }
        };
        let writer = HidWriter { handle, event };
        writer.check_report_length(what, report_len)?;
        Ok(writer)
    }

    fn check_report_length(&self, what: &str, report_len: usize) -> Result<(), String> {
        let mut preparsed = PHIDP_PREPARSED_DATA::default();
        if !unsafe { HidD_GetPreparsedData(self.handle, &mut preparsed) } {
            return Err(format!("cannot query {what} HID capabilities"));
        }
        let mut caps = HIDP_CAPS::default();
        let status = unsafe { HidP_GetCaps(preparsed, &mut caps) };
        unsafe {
            let _ = HidD_FreePreparsedData(preparsed);
        }
        status.ok().map_err(|e| e.message())?;
        if usize::from(caps.OutputReportByteLength) != report_len {
            return Err(format!(
                "{what}: unexpected output report length {} (expected {report_len})",
                caps.OutputReportByteLength
            ));
        }
        Ok(())
    }

    pub fn write(&self, report: &[u8]) -> Result<(), Error> {
        let mut overlapped = OVERLAPPED { hEvent: self.event, ..Default::default() };
        unsafe {
            if let Err(e) = WriteFile(self.handle, Some(report), None, Some(&mut overlapped)) {
                if e.code() != ERROR_IO_PENDING.to_hresult() {
                    return Err(e);
                }
            }
            if WaitForSingleObject(self.event, WRITE_TIMEOUT_MS) != WAIT_OBJECT_0 {
                let _ = CancelIoEx(self.handle, Some(&overlapped));
                let mut written = 0;
                let _ = GetOverlappedResult(self.handle, &overlapped, &mut written, true);
                return Err(Error::new(E_FAIL, "HID write timed out"));
            }
            let mut written = 0;
            GetOverlappedResult(self.handle, &overlapped, &mut written, false)
        }
    }
}

impl Drop for HidWriter {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.event);
        }
    }
}

fn find_path(what: &str, filter: impl Fn(&hidapi::DeviceInfo) -> bool) -> Result<String, String> {
    let api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
    let path = api
        .device_list()
        .find(|d| filter(d))
        .map(|d| d.path().to_string_lossy().into_owned())
        .ok_or_else(|| format!("{what} not found"));
    path
}

/// Opens the keyboard's command collection.
pub fn open_keyboard() -> Result<HidWriter, String> {
    HidWriter::open("keyboard command interface", REPORT_LEN, |d| {
        d.vendor_id() == protocol::VENDOR_ID
            && d.product_id() == protocol::PRODUCT_ID
            && d.interface_number() == protocol::INTERFACE
            && d.usage_page() == protocol::USAGE_PAGE
            && d.usage() == protocol::USAGE
    })
}

#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// One color for the whole board through `KM_IO_SET_LIGHT_EFFECT`.
    Ambient(EffectParams),
    /// One color per `layout::ISO_UK` key through `KM_IO_SET_KEY_COLOR`.
    PerKey { params: EffectParams, threshold: u8, max_reports: usize },
}

impl Target {
    pub fn params(&self) -> EffectParams {
        let (Target::Ambient(params) | Target::PerKey { params, .. }) = *self;
        params
    }

    /// Report that paints the whole board one static color, used by effects.
    pub fn whole_board(&self, color: Rgb) -> [u8; REPORT_LEN] {
        let (Target::Ambient(params) | Target::PerKey { params, .. }) = *self;
        protocol::light_effect(&EffectParams { mode: protocol::MODE_STATIC, color_flags: 0, ..params }, color)
    }

    /// Reports needed to move the board from `written` towards `colors`,
    /// largest changes first, together with the slots each report updates.
    pub fn reports(&self, colors: &[Rgb], written: &[Option<Rgb>]) -> Vec<([u8; REPORT_LEN], Vec<usize>)> {
        match *self {
            Target::Ambient(params) => {
                if written[0] == Some(colors[0]) {
                    return Vec::new();
                }
                vec![(protocol::light_effect(&params, colors[0]), vec![0])]
            }
            Target::PerKey { params, threshold, max_reports } => {
                key_reports(&params, colors, written, threshold, max_reports)
            }
        }
    }
}

/// `KM_IO_SET_KEY_COLOR` reports moving `layout::ISO_UK` keys from `written`
/// towards `colors`, largest changes first, at most `max_reports` of them,
/// together with the key slots each report updates. Changes below
/// `threshold` are skipped.
pub fn key_reports(
    params: &EffectParams,
    colors: &[Rgb],
    written: &[Option<Rgb>],
    threshold: u8,
    max_reports: usize,
) -> Vec<([u8; REPORT_LEN], Vec<usize>)> {
    let mut dirty: Vec<(usize, u8)> = colors
        .iter()
        .zip(written)
        .enumerate()
        .filter_map(|(i, (&c, w))| match w {
            None => Some((i, u8::MAX)),
            Some(w) => Some((i, w.distance(c))).filter(|&(_, d)| d >= threshold.max(1)),
        })
        .collect();
    dirty.sort_by_key(|&(_, d)| std::cmp::Reverse(d));
    dirty
        .chunks(KEYS_PER_REPORT)
        .take(max_reports)
        .map(|chunk| {
            let keys: Vec<(u8, Rgb)> = chunk.iter().map(|&(i, _)| (layout::ISO_UK[i].code, colors[i])).collect();
            let slots = chunk.iter().map(|&(i, _)| i).collect();
            (protocol::key_colors(params, &keys), slots)
        })
        .collect()
}
