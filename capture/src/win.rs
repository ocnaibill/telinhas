//! Windows backend: DXGI for GPUs, GDI for monitors, user32 and DWM for
//! windows, and WinRT metadata to learn what Graphics Capture supports.
//!
//! Every `unsafe` block here is a call into the Win32 API with pointers that
//! point at locals which outlive the call.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::mem::size_of;

use windows::Foundation::Metadata::ApiInformation;
use windows::Graphics::Capture::GraphicsCaptureSession;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR};

use crate::{CaptureSupport, Error, Gpu, GpuVendor, Monitor, Result, Window};

impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Error::Os(error.to_string())
    }
}

fn utf16_to_string(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

pub fn init_dpi_awareness() {
    // Fails harmlessly if the process already has an awareness level.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

// ----- monitors ------------------------------------------------------------

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    list.push(monitor);
    BOOL(1)
}

pub fn list_monitors() -> Result<Vec<Monitor>> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut handles as *mut Vec<HMONITOR> as isize),
        )
        .ok()?;
    }

    let mut monitors = Vec::new();
    for (index, handle) in handles.into_iter().enumerate() {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        unsafe { GetMonitorInfoW(handle, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }
            .ok()?;

        // `rcMonitor` can be scaled by the display scaling; the current
        // display mode always holds the real pixel size and refresh rate.
        let mut mode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let have_mode = unsafe {
            EnumDisplaySettingsW(
                PCWSTR(info.szDevice.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
            )
        }
        .as_bool();

        let rect = info.monitorInfo.rcMonitor;
        let (width, height, x, y, refresh_hz) = if have_mode {
            let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
            (
                mode.dmPelsWidth,
                mode.dmPelsHeight,
                position.x,
                position.y,
                mode.dmDisplayFrequency,
            )
        } else {
            (
                (rect.right - rect.left) as u32,
                (rect.bottom - rect.top) as u32,
                rect.left,
                rect.top,
                0,
            )
        };
        monitors.push(Monitor {
            index,
            device: utf16_to_string(&info.szDevice),
            x,
            y,
            width,
            height,
            refresh_hz,
            primary: info.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
        });
    }
    Ok(monitors)
}

// ----- windows ---------------------------------------------------------------

unsafe extern "system" fn collect_window(window: HWND, data: LPARAM) -> BOOL {
    let list = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
    list.push(window);
    BOOL(1)
}

fn process_name(window: HWND) -> String {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    if pid == 0 {
        return String::new();
    }
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return String::new();
    };
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    if result.is_err() {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    path.rsplit('\\').next().unwrap_or_default().to_owned()
}

fn window_size(window: HWND) -> (u32, u32) {
    // The visible frame, without the invisible resize borders of Windows 10+.
    let mut frame = RECT::default();
    let have_frame = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
    }
    .is_ok();
    if !have_frame {
        let _ = unsafe { GetWindowRect(window, &mut frame) };
    }
    (
        (frame.right - frame.left).max(0) as u32,
        (frame.bottom - frame.top).max(0) as u32,
    )
}

pub fn list_windows() -> Result<Vec<Window>> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        EnumWindows(
            Some(collect_window),
            LPARAM(&mut handles as *mut Vec<HWND> as isize),
        )?;
    }

    let mut windows = Vec::new();
    for handle in handles {
        if !unsafe { IsWindowVisible(handle) }.as_bool() {
            continue;
        }
        let extended_style = unsafe { GetWindowLongW(handle, GWL_EXSTYLE) } as u32;
        if extended_style & WS_EX_TOOLWINDOW.0 != 0 {
            continue;
        }
        // Windows on other virtual desktops and suspended apps are "cloaked".
        let mut cloaked = 0u32;
        let _ = unsafe {
            DwmGetWindowAttribute(
                handle,
                DWMWA_CLOAKED,
                &mut cloaked as *mut u32 as *mut c_void,
                size_of::<u32>() as u32,
            )
        };
        if cloaked != 0 {
            continue;
        }
        let title_length = unsafe { GetWindowTextLengthW(handle) };
        if title_length <= 0 {
            continue;
        }
        let mut title = vec![0u16; title_length as usize + 1];
        let copied = unsafe { GetWindowTextW(handle, &mut title) };
        if copied <= 0 {
            continue;
        }

        let (width, height) = window_size(handle);
        windows.push(Window {
            handle: handle.0 as isize,
            title: utf16_to_string(&title),
            process: process_name(handle),
            width,
            height,
            minimized: unsafe { IsIconic(handle) }.as_bool(),
        });
    }
    Ok(windows)
}

// ----- GPUs --------------------------------------------------------------------

pub fn list_gpus() -> Result<Vec<Gpu>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut gpus = Vec::new();
    let mut index = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
        index += 1;
        let desc = unsafe { adapter.GetDesc1()? };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        gpus.push(Gpu {
            name: utf16_to_string(&desc.Description),
            vendor: GpuVendor::from_pci(desc.VendorId),
            vram_mb: (desc.DedicatedVideoMemory / (1024 * 1024)) as u64,
        });
    }
    Ok(gpus)
}

// ----- capture support ---------------------------------------------------------

pub fn capture_support() -> Result<CaptureSupport> {
    let graphics_capture = GraphicsCaptureSession::IsSupported()?;
    let session = HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureSession");
    let has_property = |name: &str| {
        ApiInformation::IsPropertyPresent(&session, &HSTRING::from(name)).unwrap_or(false)
    };
    Ok(CaptureSupport {
        graphics_capture,
        border_toggle: has_property("IsBorderRequired"),
        cursor_toggle: has_property("IsCursorCaptureEnabled"),
    })
}
