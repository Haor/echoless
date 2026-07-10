use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
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

#[derive(Debug, PartialEq, Eq)]
enum RemoveClientResult {
    Inactive,
    Removed(usize),
    Retained { client: usize, status: i32 },
}

fn try_remove_client(
    client: &mut Option<usize>,
    remove: impl FnOnce(usize) -> i32,
) -> RemoveClientResult {
    let Some(raw) = *client else {
        return RemoveClientResult::Inactive;
    };
    let status = remove(raw);
    if status == 0 {
        RemoveClientResult::Removed(client.take().expect("registered client must exist"))
    } else {
        RemoveClientResult::Retained {
            client: raw,
            status,
        }
    }
}

fn run_callback_boundary(body: impl FnOnce()) -> bool {
    if catch_unwind(AssertUnwindSafe(body)).is_ok() {
        return true;
    }

    // The diagnostic path is also contained so no secondary logging panic can
    // escape through CoreAudio's C callback boundary.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        crate::logging::log(
            "error",
            "device-watch",
            "panic caught in CoreAudio device-change callback",
        );
    }));
    false
}

// HAL 通知线程回调:只透传「变了」,枚举仍由前端调 list_devices 完成。
extern "C" fn on_devices_changed(
    _object_id: u32,
    _num_addresses: u32,
    _addresses: *const AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> i32 {
    run_callback_boundary(|| {
        // SAFETY: `start` registers a non-null `Box<AppHandle>` as client_data and
        // keeps it alive until CoreAudio confirms listener removal.
        let app = unsafe { &*(client_data as *const tauri::AppHandle) };
        let _ = app.emit("echoless://devices-changed", ());
    });
    0
}

pub fn start(app: &tauri::AppHandle, state: &DeviceWatchState) {
    if !stop(state) {
        let _ = app.emit(
            "echoless://log",
            "device watch: previous CoreAudio listener is still registered; restart skipped",
        );
        return;
    }
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
        let status = unsafe {
            AudioObjectRemovePropertyListener(
                SYSTEM_OBJECT,
                &DEVICES_ADDRESS,
                on_devices_changed,
                client as *mut c_void,
            )
        };
        if status == 0 {
            unsafe {
                drop(Box::from_raw(client));
            }
        } else {
            let message = format!(
                "device watch: AudioObjectRemovePropertyListener failed ({status}); retaining callback context"
            );
            crate::logging::log("error", "device-watch", &message);
            let _ = app.emit("echoless://log", message);
        }
    }
}

pub fn stop(state: &DeviceWatchState) -> bool {
    let Ok(mut guard) = state.client.lock() else {
        crate::logging::log(
            "error",
            "device-watch",
            "failed to lock CoreAudio listener state; callback context retained",
        );
        return false;
    };
    let result = try_remove_client(&mut guard, |client| unsafe {
        AudioObjectRemovePropertyListener(
            SYSTEM_OBJECT,
            &DEVICES_ADDRESS,
            on_devices_changed,
            client as *mut c_void,
        )
    });
    drop(guard);

    match result {
        RemoveClientResult::Inactive => true,
        RemoveClientResult::Removed(client) => {
            unsafe {
                drop(Box::from_raw(client as *mut tauri::AppHandle));
            }
            true
        }
        RemoveClientResult::Retained { status, .. } => {
            crate::logging::log(
                "error",
                "device-watch",
                &format!(
                    "AudioObjectRemovePropertyListener failed ({status}); callback context retained"
                ),
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_boundary_catches_injected_panic_before_ffi_boundary() {
        assert!(!run_callback_boundary(|| panic!("injected callback panic")));
        assert!(run_callback_boundary(|| {}));
    }

    #[test]
    fn successful_remove_releases_the_registered_client() {
        let mut client = Some(41);

        let result = try_remove_client(&mut client, |raw| {
            assert_eq!(raw, 41);
            0
        });

        assert_eq!(result, RemoveClientResult::Removed(41));
        assert_eq!(client, None);
    }

    #[test]
    fn failed_remove_retains_the_registered_client_and_status() {
        let mut client = Some(42);

        let result = try_remove_client(&mut client, |raw| {
            assert_eq!(raw, 42);
            -50
        });

        assert_eq!(
            result,
            RemoveClientResult::Retained {
                client: 42,
                status: -50,
            }
        );
        assert_eq!(client, Some(42));
    }

    #[test]
    fn retained_client_can_be_removed_by_a_later_attempt() {
        let mut client = Some(43);

        assert!(matches!(
            try_remove_client(&mut client, |_| -1),
            RemoveClientResult::Retained { .. }
        ));
        assert_eq!(
            try_remove_client(&mut client, |_| 0),
            RemoveClientResult::Removed(43)
        );
        assert_eq!(client, None);
    }
}
