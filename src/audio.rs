//! The silence gate for the older dongle: its bootloader bridge stops the dongle's own audio,
//! so a session is safe only while the dongle's playback *and* microphone are silent. The peak
//! meters of the matching endpoints are read twice, 120 ms apart; anything above ~-48 dB
//! counts as flowing. Endpoints are matched by FriendlyName substring: the device part of the
//! name is the USB product string ("G435 Wireless Gaming Headset"), only the prefix before it
//! is localized.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::util::{dbg, sleep_ms};

/// True while any active audio endpoint whose FriendlyName contains `name` (case-insensitive) -
/// playback or microphone - carries a signal. True as well when the meters cannot be read:
/// never cut audio unchecked.
pub fn busy(name: &str) -> bool {
    // Meters need COM. CoInitializeEx keeps this correct on any thread (S_FALSE for an
    // apartment already up still balances one CoUninitialize).
    unsafe {
        let com = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED).is_ok();
        let busy = busy_com(name);
        if com {
            windows::Win32::System::Com::CoUninitialize();
        }
        busy
    }
}

unsafe fn busy_com(name: &str) -> bool {
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eRender};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            dbg("gate: no device enumerator");
            return true;
        };
        for flow in [eRender, eCapture] {
            let Ok(coll) = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else {
                dbg("gate: no endpoints");
                return true;
            };
            let Ok(n) = coll.GetCount() else { continue };
            for i in 0..n {
                let Ok(dev) = coll.Item(i) else { continue };
                if !endpoint_name(&dev).to_lowercase().contains(&name.to_lowercase()) {
                    continue;
                }
                let Ok(meter) = dev.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None) else {
                    dbg("gate: no meter");
                    return true;
                };
                let Ok(a) = meter.GetPeakValue() else {
                    dbg("gate: meter unreadable");
                    return true;
                };
                sleep_ms(120);
                let Ok(b) = meter.GetPeakValue() else {
                    dbg("gate: meter unreadable");
                    return true;
                };
                if a > 0.004 || b > 0.004 {
                    dbg("gate: audio flowing, a read would cut it");
                    return true;
                }
            }
        }
        false
    }
}

/// The endpoint's FriendlyName, cached by endpoint id. Endpoint ids are stable; names change
/// only when the user renames a device.
unsafe fn endpoint_name(dev: &windows::Win32::Media::Audio::IMMDevice) -> String {
    unsafe {
        let Ok(pw) = dev.GetId() else { return String::new() };
        let id = pw.to_string().unwrap_or_default();
        windows::Win32::System::Com::CoTaskMemFree(Some(pw.0 as *const core::ffi::c_void));
        let cache = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = cache.get(&id) {
            return known.clone();
        }
        let name = friendly_name(dev);
        cache.insert(id, name.clone());
        name
    }
}

unsafe fn friendly_name(dev: &windows::Win32::Media::Audio::IMMDevice) -> String {
    use windows::Win32::System::Com::STGM_READ;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Variant::VT_LPWSTR;
    unsafe {
        let Ok(store) = dev.OpenPropertyStore(STGM_READ) else { return String::new() };
        let Ok(mut pv) = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME) else { return String::new() };
        let name = if pv.Anonymous.Anonymous.vt == VT_LPWSTR {
            windows::core::PCWSTR::from_raw(pv.Anonymous.Anonymous.Anonymous.pwszVal.0).to_string().unwrap_or_default()
        } else {
            String::new()
        };
        let _ = PropVariantClear(&mut pv);
        name
    }
}

static NAMES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// PKEY_Device_FriendlyName (a45c254e-df1c-4efd-8020-67d146a850e0, pid 14).
const PKEY_DEVICE_FRIENDLY_NAME: windows::Win32::Foundation::PROPERTYKEY =
    windows::Win32::Foundation::PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
