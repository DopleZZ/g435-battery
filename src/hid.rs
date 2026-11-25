//! The Win32 HID layer this crate needs: enumerate the collections, open the G435 dongle's
//! protocol collection, exchange reports. Adapted from BatteryHub's hid.rs (GPL-3.0), which
//! follows the usual SetupAPI / HidD_* patterns.
//!
//! Report buffers follow hidapi conventions: write's data[0] is the report ID (0 when the
//! device does not number its reports), the buffer is zero-padded to the report length; read
//! strips a leading 0 report ID, so data[0] is the first payload byte for unnumbered reports
//! and the report ID for numbered ones.
use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid, HidD_GetPreparsedData, HidP_GetCaps,
    PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{HSTRING, PCWSTR};

/// One HID top-level collection, as far as this crate cares about it.
struct Coll {
    path: String,
    vid: u16,
    pid: u16,
    usage_page: u16,
    usage_id: u16,
    input_len: u16,
    output_len: u16,
    feature_len: u16,
}

/// Which dongle was found, with its protocol collection already open.
pub(crate) enum G435 {
    /// The older dongle (046D:0ACB): vendor collection FFC0, report 6, feature reports.
    Acb(HidDev),
    /// The newer dongle (046D:0ADF): vendor collection FF00:0001, unnumbered 65-byte reports.
    Adf(HidDev),
    Missing,
}

/// Looks for a G435 dongle among the present HID collections and opens its protocol collection.
pub(crate) fn find_dongle() -> G435 {
    for c in enumerate() {
        if c.vid != 0x046D {
            continue;
        }
        let known = match c.pid {
            0x0ACB => c.usage_page == 0xFFC0 && c.feature_len >= 64,
            0x0ADF => c.usage_page == 0xFF00 && c.usage_id == 0x0001 && c.input_len == 65 && c.output_len == 65,
            _ => continue,
        };
        if !known {
            continue;
        }
        if let Some(d) = HidDev::open(&c.path) {
            return if c.pid == 0x0ACB { G435::Acb(d) } else { G435::Adf(d) };
        }
    }
    G435::Missing
}

/// All present HID collections. Opening with zero access is enough to read the caps and works
/// on devices another app holds exclusively.
fn enumerate() -> Vec<Coll> {
    let mut out = Vec::new();
    unsafe {
        let guid = HidD_GetHidGuid();
        let Ok(di) = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else {
            return out;
        };
        let mut ifd = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
        let mut i = 0;
        while SetupDiEnumDeviceInterfaces(di, None, &guid, i, &mut ifd).is_ok() {
            i += 1;
            let mut need = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(di, &ifd, None, 0, Some(&mut need), None);
            if need == 0 {
                continue;
            }
            // u32-aligned buffer for the variable-length detail struct.
            let mut buf = vec![0u32; (need as usize).div_ceil(4)];
            let det = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*det).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(di, &ifd, Some(det), need, None, None).is_err() {
                continue;
            }
            let path = PCWSTR((*det).DevicePath.as_ptr()).to_string().unwrap_or_default();
            let Some(h) = open_raw(&path, 0, false) else { continue };
            let mut c = Coll { path, vid: 0, pid: 0, usage_page: 0, usage_id: 0, input_len: 0, output_len: 0, feature_len: 0 };
            let mut a = HIDD_ATTRIBUTES { Size: size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
            if HidD_GetAttributes(h, &mut a) {
                c.vid = a.VendorID;
                c.pid = a.ProductID;
            }
            let mut pp = PHIDP_PREPARSED_DATA::default();
            if HidD_GetPreparsedData(h, &mut pp) {
                let mut caps = HIDP_CAPS::default();
                let _ = HidP_GetCaps(pp, &mut caps);
                c.usage_page = caps.UsagePage;
                c.usage_id = caps.Usage;
                c.input_len = caps.InputReportByteLength;
                c.output_len = caps.OutputReportByteLength;
                c.feature_len = caps.FeatureReportByteLength;
                let _ = HidD_FreePreparsedData(pp);
            }
            let _ = CloseHandle(h);
            out.push(c);
        }
        let _ = SetupDiDestroyDeviceInfoList(di);
    }
    out
}

fn open_raw(path: &str, access: u32, overlapped: bool) -> Option<HANDLE> {
    let flags = if overlapped { FILE_FLAG_OVERLAPPED } else { FILE_FLAGS_AND_ATTRIBUTES(0) };
    unsafe { CreateFileW(&HSTRING::from(path), access, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, flags, None).ok().filter(|h| !h.is_invalid()) }
}

/// An open HID collection. Closed on drop; used from one thread at a time.
pub(crate) struct HidDev {
    h: HANDLE,
    path: String,
    in_len: usize,
    out_len: usize,
    feat_len: usize,
}

unsafe impl Send for HidDev {}

struct Event(HANDLE);
impl Event {
    fn new() -> Option<Self> {
        unsafe { CreateEventW(None, true, false, PCWSTR::null()).ok().map(Event) }
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

impl HidDev {
    pub(crate) fn open(path: &str) -> Option<Self> {
        let h = open_raw(path, GENERIC_READ.0 | GENERIC_WRITE.0, true)?;
        let (mut in_len, mut out_len, mut feat_len) = (0usize, 0usize, 0usize);
        unsafe {
            let mut pp = PHIDP_PREPARSED_DATA::default();
            if HidD_GetPreparsedData(h, &mut pp) {
                let mut caps = HIDP_CAPS::default();
                let _ = HidP_GetCaps(pp, &mut caps);
                in_len = caps.InputReportByteLength as usize;
                out_len = caps.OutputReportByteLength as usize;
                feat_len = caps.FeatureReportByteLength as usize;
                let _ = HidD_FreePreparsedData(pp);
            }
        }
        if in_len == 0 || out_len == 0 {
            unsafe {
                let _ = CloseHandle(h);
            }
            return None;
        }
        Some(Self { h, path: path.to_string(), in_len, out_len, feat_len })
    }

    /// A second handle to the same collection, for a retry.
    pub(crate) fn reopen(&self) -> Option<HidDev> {
        HidDev::open(&self.path)
    }

    pub(crate) fn out_len(&self) -> usize {
        self.out_len
    }

    /// Waits for one overlapped operation; on timeout exactly this operation is cancelled.
    /// The wait for completion that follows is required: the buffer and the OVERLAPPED belong
    /// to the caller's stack frame, so the kernel must be done with them before returning.
    fn wait_io(&self, ov: &mut OVERLAPPED, ev: &Event, timeout_ms: u32) -> Option<u32> {
        let mut n = 0u32;
        unsafe {
            if timeout_ms > 0 && WaitForSingleObject(ev.0, timeout_ms) == WAIT_OBJECT_0 {
                return GetOverlappedResult(self.h, ov, &mut n, false).ok().map(|_| n);
            }
            let _ = CancelIoEx(self.h, Some(ov as *const OVERLAPPED));
            match GetOverlappedResult(self.h, ov, &mut n, true) {
                Ok(()) => Some(n),
                Err(_) => None,
            }
        }
    }

    /// Writes one output report (data[0] = report ID), zero-padded, 500 ms timeout.
    pub(crate) fn write(&self, data: &[u8]) -> bool {
        let len = self.out_len;
        if len == 0 || data.is_empty() {
            return false;
        }
        let mut buf = vec![0u8; len];
        let n = data.len().min(len);
        buf[..n].copy_from_slice(&data[..n]);
        let Some(ev) = Event::new() else { return false };
        let mut ov = OVERLAPPED { hEvent: ev.0, ..Default::default() };
        unsafe {
            let r = WriteFile(self.h, Some(&buf), None, Some(&mut ov));
            if r.is_err() && GetLastError() != ERROR_IO_PENDING {
                return false;
            }
        }
        self.wait_io(&mut ov, &ev, 500).is_some()
    }

    /// Reads one input report (a leading 0 report ID is stripped). None on timeout or error.
    pub(crate) fn read(&self, timeout_ms: u32) -> Option<Vec<u8>> {
        let len = self.in_len;
        if len == 0 {
            return None;
        }
        let mut tmp = vec![0u8; len];
        let ev = Event::new()?;
        let mut ov = OVERLAPPED { hEvent: ev.0, ..Default::default() };
        unsafe {
            let r = ReadFile(self.h, Some(&mut tmp), None, Some(&mut ov));
            if r.is_err() && GetLastError() != ERROR_IO_PENDING {
                return None;
            }
        }
        let got = self.wait_io(&mut ov, &ev, timeout_ms)?;
        if got == 0 {
            return None;
        }
        let off = if tmp[0] == 0 { 1 } else { 0 };
        Some(tmp[off..got as usize].to_vec())
    }

    /// One feature report over the control pipe; out[0] = report ID.
    pub(crate) fn get_feature(&self, report_id: u8) -> Option<Vec<u8>> {
        let len = self.feat_len;
        if len == 0 {
            return None;
        }
        let mut out = vec![0u8; len];
        out[0] = report_id;
        unsafe { HidD_GetFeature(self.h, out.as_mut_ptr() as *mut c_void, len as u32) }.then_some(out)
    }
}

impl Drop for HidDev {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.h);
        }
    }
}
