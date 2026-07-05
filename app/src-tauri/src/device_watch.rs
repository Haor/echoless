use std::ffi::c_void;
use std::sync::Mutex;

use tauri::Emitter;

// CoreAudio/AudioHardware.h
#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

const SYSTEM_OBJECT: u32 = 1; // kAudioObjectSystemObject
const DEVICES_ADDRESS: AudioObjectPropertyAddress = AudioObjectPropertyAddress {
    selector: u32::from_be_bytes(*b"dev#"), // kAudioHardwarePropertyDevices
    scope: u32::from_be_bytes(*b"glob"),    // kAudioObjectPropertyScopeGlobal
    element: 0,                             // kAudioObjectPropertyElementMain
};

type Listener = extern "C" fn(u32, u32, *const AudioObjectPropertyAddress, *mut c_void) -> i32;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectAddPropertyListener(
        object_id: u32,
        address: *const AudioObjectPropertyAddress,
        listener: Listener,
        client_data: *mut c_void,
    ) -> i32;
    fn AudioObjectRemovePropertyListener(
        object_id: u32,
        address: *const AudioObjectPropertyAddress,
        listener: Listener,
        client_data: *mut c_void,
    ) -> i32;
}

#[derive(Default)]
pub struct DeviceWatchState {
    client: Mutex<Option<usize>>,
}

// HAL 通知线程回调:只透传「变了」,枚举仍由前端调 list_devices 完成。
extern "C" fn on_devices_changed(
    _object_id: u32,
    _num_addresses: u32,
    _addresses: *const AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> i32 {
    let app = unsafe { &*(client_data as *const tauri::AppHandle) };
    let _ = app.emit("echoless://devices-changed", ());
    0
}

pub fn start(app: &tauri::AppHandle, state: &DeviceWatchState) {
    stop(state);
    let client = Box::into_raw(Box::new(app.clone()));
    let status = unsafe {
        AudioObjectAddPropertyListener(
            SYSTEM_OBJECT,
            &DEVICES_ADDRESS,
            on_devices_changed,
            client as *mut c_void,
        )
    };
    if status != 0 {
        let _ = app.emit(
            "echoless://log",
            format!("device watch: AudioObjectAddPropertyListener failed ({status})"),
        );
        unsafe {
            drop(Box::from_raw(client));
        }
        return;
    }
    if let Ok(mut guard) = state.client.lock() {
        *guard = Some(client as usize);
    } else {
        let _ = app.emit(
            "echoless://log",
            "device watch: failed to store CoreAudio listener state",
        );
        unsafe {
            let _ = AudioObjectRemovePropertyListener(
                SYSTEM_OBJECT,
                &DEVICES_ADDRESS,
                on_devices_changed,
                client as *mut c_void,
            );
            drop(Box::from_raw(client));
        }
    }
}

pub fn stop(state: &DeviceWatchState) {
    let Some(client) = state.client.lock().ok().and_then(|mut guard| guard.take()) else {
        return;
    };
    unsafe {
        let _ = AudioObjectRemovePropertyListener(
            SYSTEM_OBJECT,
            &DEVICES_ADDRESS,
            on_devices_changed,
            client as *mut c_void,
        );
        drop(Box::from_raw(client as *mut tauri::AppHandle));
    }
}
