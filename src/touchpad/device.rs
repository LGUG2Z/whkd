//! Reading a Precision Touchpad on Windows.
//!
//! A Precision Touchpad is a HID device with a top level collection on usage page 0x0D (digitizers),
//! usage 0x05 (touch pad). Registered for with `RIDEV_INPUTSINK`, Windows sends its reports to a
//! window that is not even in the foreground, and from them the fingers can be followed.
//!
//! Which numbers in a report are what (where the fingers are, whether they touch, the clock) is
//! found out from the device's own description of its reports, the "preparsed data", and not
//! assumed. This is the only part that talks to Windows, and it has not been tried on a real
//! touchpad: what comes out of it is the input to everything in `report` onwards, which is tested.

use super::dispatch::Dispatcher;
use super::protocol::Pad;
use super::recogniser::Output;
use super::recogniser::Recogniser;
use super::recogniser::Thresholds;
use super::report::Assembler;
use super::report::Axis;
use super::report::Calibration;
use super::report::RawContact;
use super::report::RawReport;
use crate::mouse::Modifiers;
use color_eyre::eyre::eyre;
use color_eyre::eyre::Result;
use parking_lot::Mutex;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use win_hotkeys::state::KeyboardState;
use win_hotkeys::PauseHandle;
use windows::core::w;
use windows::Win32::Devices::HumanInterfaceDevice::HidP_GetCaps;
use windows::Win32::Devices::HumanInterfaceDevice::HidP_GetUsageValue;
use windows::Win32::Devices::HumanInterfaceDevice::HidP_GetUsages;
use windows::Win32::Devices::HumanInterfaceDevice::HidP_GetValueCaps;
use windows::Win32::Devices::HumanInterfaceDevice::HidP_Input;
use windows::Win32::Devices::HumanInterfaceDevice::HIDP_CAPS;
use windows::Win32::Devices::HumanInterfaceDevice::HIDP_STATUS_SUCCESS;
use windows::Win32::Devices::HumanInterfaceDevice::HIDP_VALUE_CAPS;
use windows::Win32::Devices::HumanInterfaceDevice::PHIDP_PREPARSED_DATA;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::GetRawInputData;
use windows::Win32::UI::Input::GetRawInputDeviceInfoW;
use windows::Win32::UI::Input::GetRawInputDeviceList;
use windows::Win32::UI::Input::RegisterRawInputDevices;
use windows::Win32::UI::Input::HRAWINPUT;
use windows::Win32::UI::Input::RAWINPUT;
use windows::Win32::UI::Input::RAWINPUTDEVICE;
use windows::Win32::UI::Input::RAWINPUTDEVICELIST;
use windows::Win32::UI::Input::RAWINPUTHEADER;
use windows::Win32::UI::Input::RIDEV_DEVNOTIFY;
use windows::Win32::UI::Input::RIDEV_INPUTSINK;
use windows::Win32::UI::Input::RIDI_DEVICEINFO;
use windows::Win32::UI::Input::RIDI_DEVICENAME;
use windows::Win32::UI::Input::RIDI_PREPARSEDDATA;
use windows::Win32::UI::Input::RID_DEVICE_INFO;
use windows::Win32::UI::Input::RID_INPUT;
use windows::Win32::UI::Input::RIM_TYPEHID;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::RegisterClassW;
use windows::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows::Win32::UI::WindowsAndMessaging::HWND_MESSAGE;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WM_INPUT;
use windows::Win32::UI::WindowsAndMessaging::WM_INPUT_DEVICE_CHANGE;
use windows::Win32::UI::WindowsAndMessaging::WNDCLASSW;

/// The usages of a Precision Touchpad's top level collection
const DIGITIZER_PAGE: u16 = 0x0D;
const TOUCH_PAD: u16 = 0x05;

/// The usages of the numbers in its reports
const GENERIC_DESKTOP_PAGE: u16 = 0x01;
const X: u16 = 0x30;
const Y: u16 = 0x31;
const TIP_SWITCH: u16 = 0x42;
const CONFIDENCE: u16 = 0x47;
const CONTACT_ID: u16 = 0x51;
const CONTACT_COUNT: u16 = 0x54;
const SCAN_TIME: u16 = 0x56;

/// `wParam` of `WM_INPUT_DEVICE_CHANGE`
const GIDC_REMOVAL: usize = 2;

/// A Precision Touchpad that is connected
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchpadInfo {
    /// The path Windows knows the device by
    pub name: String,
    handle: isize,
}

fn handle(raw: isize) -> HANDLE {
    HANDLE(raw as *mut c_void)
}

/// What kind of HID device this is: `(usage page, usage)`, if it is one
fn usage_of(device: HANDLE) -> Option<(u16, u16)> {
    let mut info = RID_DEVICE_INFO {
        cbSize: size_of::<RID_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    let mut size = info.cbSize;

    let copied = unsafe {
        GetRawInputDeviceInfoW(
            Some(device),
            RIDI_DEVICEINFO,
            Some(std::ptr::from_mut(&mut info).cast::<c_void>()),
            &mut size,
        )
    };

    if copied == u32::MAX || info.dwType != RIM_TYPEHID {
        return None;
    }

    let hid = unsafe { info.Anonymous.hid };
    Some((hid.usUsagePage, hid.usUsage))
}

fn device_name(device: HANDLE) -> String {
    let mut length = 0u32;
    unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_DEVICENAME, None, &mut length) };

    let mut name = vec![0u16; length as usize];
    let copied = unsafe {
        GetRawInputDeviceInfoW(
            Some(device),
            RIDI_DEVICENAME,
            Some(name.as_mut_ptr().cast::<c_void>()),
            &mut length,
        )
    };

    if copied == u32::MAX {
        return String::new();
    }

    String::from_utf16_lossy(&name)
        .trim_end_matches('\0')
        .to_string()
}

/// The Precision Touchpads that are connected
#[must_use]
pub fn find_touchpads() -> Vec<TouchpadInfo> {
    let size = size_of::<RAWINPUTDEVICELIST>() as u32;
    let mut count = 0u32;
    unsafe { GetRawInputDeviceList(None, &mut count, size) };

    let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize];
    let copied = unsafe { GetRawInputDeviceList(Some(devices.as_mut_ptr()), &mut count, size) };
    if copied == u32::MAX {
        return vec![];
    }

    devices
        .iter()
        .take(copied as usize)
        .filter(|device| usage_of(device.hDevice) == Some((DIGITIZER_PAGE, TOUCH_PAD)))
        .map(|device| TouchpadInfo {
            name: device_name(device.hDevice),
            handle: device.hDevice.0 as isize,
        })
        .collect()
}

/// What a touchpad's description of its reports says: where in a report each number is
#[derive(Debug)]
pub struct Description {
    /// The device's description of its reports, which the HID functions read the numbers with
    preparsed: Vec<u8>,
    /// The link collection of each finger slot in a report
    fingers: Vec<u16>,
    pub calibration: Calibration,
}

fn value_range(caps: &HIDP_VALUE_CAPS) -> Axis {
    Axis {
        logical_min: i64::from(caps.LogicalMin),
        logical_max: i64::from(caps.LogicalMax),
        physical_min: i64::from(caps.PhysicalMin),
        physical_max: i64::from(caps.PhysicalMax),
        units: caps.Units,
        units_exp: caps.UnitsExp,
    }
}

/// Which usage a value is for, if it is for one and not for a range of them
fn usage_of_value(caps: &HIDP_VALUE_CAPS) -> Option<u16> {
    if caps.IsRange {
        None
    } else {
        Some(unsafe { caps.Anonymous.NotRange.Usage })
    }
}

impl Description {
    fn preparsed(&self) -> PHIDP_PREPARSED_DATA {
        PHIDP_PREPARSED_DATA(self.preparsed.as_ptr() as isize)
    }

    /// Read what the device says about its reports. The error says what was missing, which is
    /// worth saying when a touchpad turns out not to be one this can use
    pub fn read(device: HANDLE) -> Result<Self, String> {
        let mut size = 0u32;
        unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_PREPARSEDDATA, None, &mut size) };
        if size == 0 {
            return Err(String::from("the device has no description of its reports"));
        }

        let mut preparsed = vec![0u8; size as usize];
        let copied = unsafe {
            GetRawInputDeviceInfoW(
                Some(device),
                RIDI_PREPARSEDDATA,
                Some(preparsed.as_mut_ptr().cast::<c_void>()),
                &mut size,
            )
        };
        if copied == u32::MAX {
            return Err(String::from(
                "could not read the description of its reports",
            ));
        }

        let pre = PHIDP_PREPARSED_DATA(preparsed.as_ptr() as isize);

        let mut caps = HIDP_CAPS::default();
        if unsafe { HidP_GetCaps(pre, &mut caps) } != HIDP_STATUS_SUCCESS {
            return Err(String::from(
                "could not read the capabilities of the device",
            ));
        }

        let mut count = caps.NumberInputValueCaps;
        let mut values = vec![HIDP_VALUE_CAPS::default(); usize::from(count)];
        if unsafe { HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut count, pre) }
            != HIDP_STATUS_SUCCESS
        {
            return Err(String::from("could not read the values of its reports"));
        }
        values.truncate(usize::from(count));

        let find = |page: u16, usage: u16| {
            values
                .iter()
                .filter(move |caps| caps.UsagePage == page && usage_of_value(caps) == Some(usage))
        };

        let mut fingers: Vec<u16> = find(GENERIC_DESKTOP_PAGE, X)
            .map(|caps| caps.LinkCollection)
            .collect();
        fingers.sort_unstable();
        fingers.dedup();

        let Some(&first) = fingers.first() else {
            return Err(String::from("its reports have no fingers in them"));
        };

        let on_first = |usage: u16| {
            find(GENERIC_DESKTOP_PAGE, usage).find(|caps| caps.LinkCollection == first)
        };
        let (Some(x), Some(y)) = (on_first(X), on_first(Y)) else {
            return Err(String::from("its fingers have no position"));
        };

        let scan_time_max = find(DIGITIZER_PAGE, SCAN_TIME)
            .next()
            .map_or(0, |caps| u32::try_from(caps.LogicalMax).unwrap_or(0));

        let calibration = Calibration::new(value_range(x), value_range(y), scan_time_max)
            .ok_or_else(|| {
                String::from("it does not say how big it is, in a unit that can be used")
            })?;

        Ok(Self {
            preparsed,
            fingers,
            calibration,
        })
    }

    fn value(&self, link: u16, usage: u16, report: &[u8], page: u16) -> Option<u32> {
        let mut value = 0u32;
        let status = unsafe {
            HidP_GetUsageValue(
                HidP_Input,
                page,
                Some(link),
                usage,
                &mut value,
                self.preparsed(),
                report,
            )
        };
        (status == HIDP_STATUS_SUCCESS).then_some(value)
    }

    /// Whether the button of `usage` is down in the finger slot `link`
    fn button(&self, link: u16, usage: u16, report: &mut [u8]) -> bool {
        let mut usages = [0u16; 16];
        let mut length = usages.len() as u32;
        let status = unsafe {
            HidP_GetUsages(
                HidP_Input,
                DIGITIZER_PAGE,
                Some(link),
                usages.as_mut_ptr(),
                &mut length,
                self.preparsed(),
                report,
            )
        };

        status == HIDP_STATUS_SUCCESS && usages[..length as usize].contains(&usage)
    }

    /// The fingers in one report
    fn report(&self, report: &[u8]) -> Option<RawReport> {
        // reading the buttons wants the report to be writable
        let copy = report.to_vec();
        let scan_time = self.value(0, SCAN_TIME, report, DIGITIZER_PAGE)?;
        let contact_count = self
            .value(0, CONTACT_COUNT, report, DIGITIZER_PAGE)
            .unwrap_or(0);

        let contacts = self
            .fingers
            .iter()
            .filter_map(|&link| {
                Some(RawContact {
                    id: self.value(link, CONTACT_ID, report, DIGITIZER_PAGE)?,
                    x: i64::from(self.value(link, X, report, GENERIC_DESKTOP_PAGE)?),
                    y: i64::from(self.value(link, Y, report, GENERIC_DESKTOP_PAGE)?),
                    tip: self.button(link, TIP_SWITCH, &mut copy.clone()),
                    confident: self.button(link, CONFIDENCE, &mut copy.clone()),
                })
            })
            .collect();

        Some(RawReport {
            scan_time,
            contact_count,
            contacts,
        })
    }
}

/// Everything one touchpad needs to get from reports to what the fingers did
struct Pipeline {
    description: Description,
    assembler: Assembler,
    recogniser: Recogniser,
}

/// What the thread that reads the touchpad hands to the one that acts on it
struct Message {
    output: Output,
    modifiers: Modifiers,
    pad: Option<Pad>,
}

struct Shared {
    pipelines: Mutex<HashMap<isize, Pipeline>>,
    /// Touchpads that this cannot use, so that it is only said once
    unusable: Mutex<HashSet<isize>>,
    thresholds: Thresholds,
    queue: mpsc::Sender<Message>,
    paused: PauseHandle,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

fn modifiers_held() -> Modifiers {
    Modifiers::from_pressed(|key| KeyboardState::get_async_key_state(key.to_vk_code()))
}

fn send(shared: &Shared, output: Output, pad: Option<Pad>) {
    let _ = shared.queue.send(Message {
        output,
        modifiers: modifiers_held(),
        pad,
    });
}

/// A touchpad's reports, which arrive as `WM_INPUT`
fn input(lparam: LPARAM) {
    let Some(shared) = SHARED.get() else {
        return;
    };

    // the data is a RAWINPUT, which wants more alignment than bytes have
    let mut size = 0u32;
    let header = size_of::<RAWINPUTHEADER>() as u32;
    let raw_input = HRAWINPUT(lparam.0 as *mut c_void);
    unsafe { GetRawInputData(raw_input, RID_INPUT, None, &mut size, header) };
    if size == 0 {
        return;
    }

    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    let copied = unsafe {
        GetRawInputData(
            raw_input,
            RID_INPUT,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            &mut size,
            header,
        )
    };
    if copied == u32::MAX {
        return;
    }

    let raw = unsafe { &*buffer.as_ptr().cast::<RAWINPUT>() };
    if raw.header.dwType != RIM_TYPEHID.0 {
        return;
    }

    let device = raw.header.hDevice;
    let key = device.0 as isize;
    let hid = unsafe { &raw.data.hid };
    let (each, count) = (hid.dwSizeHid as usize, hid.dwCount as usize);
    let bytes = unsafe { std::slice::from_raw_parts(hid.bRawData.as_ptr(), each * count) };

    if shared.unusable.lock().contains(&key) {
        return;
    }

    let mut pipelines = shared.pipelines.lock();
    let pipeline = match pipelines.entry(key) {
        Entry::Occupied(pipeline) => pipeline.into_mut(),
        Entry::Vacant(slot) => match Description::read(device) {
            Ok(description) => {
                let assembler = Assembler::new(description.calibration);
                slot.insert(Pipeline {
                    description,
                    assembler,
                    recogniser: Recogniser::new(shared.thresholds),
                })
            }
            Err(reason) => {
                eprintln!(
                    "Unable to use the touchpad {} for gestures: {reason}",
                    device_name(device)
                );
                shared.unusable.lock().insert(key);
                return;
            }
        },
    };
    let pad = Some(pipeline.description.calibration.pad());
    let paused = shared.paused.is_paused();

    for report in bytes.chunks(each.max(1)).take(count) {
        if paused {
            // nothing starts while paused, and what had is abandoned
            if let Some(output) = pipeline.recogniser.reset() {
                send(shared, output, pad);
            }
            continue;
        }

        let Some(report) = pipeline.description.report(report) else {
            continue;
        };
        let Some(frame) = pipeline.assembler.push(&report) else {
            continue;
        };

        for output in pipeline.recogniser.frame(&frame) {
            send(shared, output, pad);
        }
    }
}

/// A touchpad that went away takes its gesture with it
fn removed(lparam: LPARAM) {
    let Some(shared) = SHARED.get() else {
        return;
    };

    if let Some(mut pipeline) = shared.pipelines.lock().remove(&(lparam.0)) {
        let pad = Some(pipeline.description.calibration.pad());
        if let Some(output) = pipeline.recogniser.reset() {
            send(shared, output, pad);
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_INPUT => input(lparam),
        WM_INPUT_DEVICE_CHANGE if wparam.0 == GIDC_REMOVAL => removed(lparam),
        _ => {}
    }

    // the system cleans up after a WM_INPUT in the default procedure
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Listen for touchpads, and act on what the fingers do with `dispatcher`, on a thread of its own.
/// Meant to be started once, and only if there are gesture bindings.
pub fn start(
    mut dispatcher: Dispatcher,
    paused: PauseHandle,
    thresholds: Thresholds,
) -> Result<()> {
    let (queue, messages) = mpsc::channel::<Message>();

    SHARED
        .set(Shared {
            pipelines: Mutex::new(HashMap::new()),
            unusable: Mutex::new(HashSet::new()),
            thresholds,
            queue,
            paused,
        })
        .map_err(|_| eyre!("the touchpad is already being read"))?;

    thread::spawn(move || {
        for message in messages {
            dispatcher.handle(message.output, message.modifiers, message.pad);
        }
    });

    // the window that the touchpad's reports are sent to has to be made, and its messages pumped,
    // on the thread that will receive them
    thread::spawn(|| unsafe {
        let class = w!("whkd-touchpad");

        let Ok(module) = GetModuleHandleW(None) else {
            eprintln!("Unable to listen for touchpad gestures (error: no module handle)");
            return;
        };

        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&window_class) == 0 {
            eprintln!(
                "Unable to listen for touchpad gestures (error: could not register a window)"
            );
            return;
        }

        // a window that only receives messages: it is never shown
        let window = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("whkd touchpad"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(module.into()),
            None,
        ) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("Unable to listen for touchpad gestures (error: {error})");
                return;
            }
        };

        let touchpad = [RAWINPUTDEVICE {
            usUsagePage: DIGITIZER_PAGE,
            usUsage: TOUCH_PAD,
            // reports are wanted whatever has the focus, and so is knowing when one goes away
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: window,
        }];

        if let Err(error) = RegisterRawInputDevices(&touchpad, size_of::<RAWINPUTDEVICE>() as u32) {
            eprintln!("Unable to listen for touchpad gestures (error: {error})");
            return;
        }

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });

    Ok(())
}

/// Used by tools that read the touchpad without acting on it
#[allow(dead_code)]
pub fn handle_of(info: &TouchpadInfo) -> HANDLE {
    handle(info.handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Says what touchpads this machine has, and what each reports about itself. Needs a touchpad,
    /// so it is not run by default: `cargo test -p whkd -- --ignored --nocapture touchpads_here`
    #[test]
    #[ignore = "needs a touchpad, and says what it finds"]
    fn touchpads_here() {
        let found = find_touchpads();
        println!("{} precision touchpad(s)", found.len());

        for pad in &found {
            println!("{}", pad.name);
            match Description::read(handle(pad.handle)) {
                Ok(description) => println!(
                    "  {} finger slots, {:.1} x {:.1} cm",
                    description.fingers.len(),
                    description.calibration.pad().width_cm,
                    description.calibration.pad().height_cm
                ),
                Err(reason) => println!("  cannot be used for gestures: {reason}"),
            }
        }
    }
}
