//! Windows backend: DXGI for GPUs, GDI for monitors, user32 and DWM for
//! windows, and WinRT metadata to learn what Graphics Capture supports.
//!
//! Every `unsafe` block here is a call into the Win32 API with pointers that
//! point at locals which outlive the call.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::Foundation::Metadata::ApiInformation;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind,
    GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HMODULE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetWindowLongW, GetWindowPlacement, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR};
use windows::core::{IInspectable, Interface};

use crate::{
    CaptureSupport, Error, FrameRef, Gpu, GpuVendor, Monitor, Options, Pacer, Result, Source,
    Window,
};

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

fn monitor_handles() -> Result<Vec<HMONITOR>> {
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
    Ok(handles)
}

pub fn list_monitors() -> Result<Vec<Monitor>> {
    let handles = monitor_handles()?;

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

fn process_id(window: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    pid
}

fn process_name(window: HWND) -> String {
    let pid = process_id(window);
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
    if unsafe { IsIconic(window) }.as_bool() {
        // A minimized window is a small title bar; report the size it has
        // when restored instead.
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if unsafe { GetWindowPlacement(window, &mut placement) }.is_ok() {
            let normal = placement.rcNormalPosition;
            return (
                (normal.right - normal.left).max(0) as u32,
                (normal.bottom - normal.top).max(0) as u32,
            );
        }
    }
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
            pid: process_id(handle),
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

// ----- capture session ---------------------------------------------------------

/// Buffers in the capture pool. More slack means a slow consumer drops
/// fewer frames.
const BUFFERS: i32 = 4;
const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

type FrameCallback = Box<dyn FnMut(FrameRef<'_>) + Send>;

struct Pipeline {
    context: ID3D11DeviceContext,
    device: IDirect3DDevice,
    size: SizeInt32,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    on_frame: FrameCallback,
    pacer: Option<Pacer>,
    paced_out: Arc<AtomicU64>,
}

// The D3D11 objects are free-threaded, and the mutex around `Pipeline` makes
// sure only one frame is handled at a time.
unsafe impl Send for Pipeline {}

impl Pipeline {
    fn handle(&mut self, pool: &Direct3D11CaptureFramePool) -> windows::core::Result<()> {
        let frame = pool.TryGetNextFrame()?;
        let content = frame.ContentSize()?;
        if content.Width != self.size.Width || content.Height != self.size.Height {
            // The source changed size: rebuild the pool for what follows and
            // skip this frame, which still has the old size.
            pool.Recreate(&self.device, PIXEL_FORMAT, BUFFERS, content)?;
            self.size = content;
            return Ok(());
        }

        // Decide before touching the GPU: a frame the pacer leaves out costs
        // almost nothing.
        let system_micros = frame.SystemRelativeTime()?.Duration / 10;
        if let Some(pacer) = &mut self.pacer
            && !pacer.accept(system_micros)
        {
            self.paced_out.fetch_add(1, Ordering::Relaxed);
            let _ = frame.Close();
            return Ok(());
        }

        let surface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };

        let started = Instant::now();
        let need_new =
            !matches!(&self.staging, Some((_, w, h)) if *w == desc.Width && *h == desc.Height);
        if need_new {
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: desc.Width,
                Height: desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut created = None;
            let d3d: ID3D11Device = unsafe { self.context.GetDevice()? };
            unsafe { d3d.CreateTexture2D(&staging_desc, None, Some(&mut created))? };
            let created = created
                .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            self.staging = Some((created, desc.Width, desc.Height));
        }
        let Some((staging, _, _)) = &self.staging else {
            return Ok(());
        };

        unsafe { self.context.CopyResource(staging, &texture) };
        // The copy is queued on the GPU, so the frame can go back to the pool
        // now instead of after the (slow) readback below.
        let _ = frame.Close();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?
        };
        let stride = mapped.RowPitch as usize;
        let length = stride * desc.Height as usize;
        let bytes = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, length) };
        let copy_micros = started.elapsed().as_micros() as u32;
        (self.on_frame)(FrameRef {
            width: desc.Width,
            height: desc.Height,
            stride,
            bgra: bytes,
            system_micros,
            copy_micros,
        });
        unsafe { self.context.Unmap(staging, 0) };
        Ok(())
    }
}

pub struct Session {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    size: (u32, u32),
    border_status: String,
    max_fps_status: String,
    paced_out: Arc<AtomicU64>,
    cursor_applied: bool,
    _pipeline: Arc<Mutex<Pipeline>>,
}

impl Session {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn border_status(&self) -> String {
        self.border_status.clone()
    }

    pub fn max_fps_status(&self) -> String {
        self.max_fps_status.clone()
    }

    pub fn frames_paced_out(&self) -> u64 {
        self.paced_out.load(Ordering::Relaxed)
    }

    pub fn cursor_applied(&self) -> bool {
        self.cursor_applied
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

fn create_devices() -> windows::core::Result<(ID3D11DeviceContext, IDirect3DDevice)> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    let failed = || windows::core::Error::from(windows::Win32::Foundation::E_FAIL);
    let device = device.ok_or_else(failed)?;
    let context = context.ok_or_else(failed)?;
    let dxgi: IDXGIDevice = device.cast()?;
    let inspectable: IInspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? };
    Ok((context, inspectable.cast()?))
}

fn create_item(source: Source) -> Result<GraphicsCaptureItem> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item = match source {
        Source::Monitor(index) => {
            let handles = monitor_handles()?;
            let handle = *handles
                .get(index)
                .ok_or_else(|| Error::Os(format!("there is no monitor #{index}")))?;
            unsafe { interop.CreateForMonitor(handle)? }
        }
        Source::Window(handle) => unsafe { interop.CreateForWindow(HWND(handle as *mut c_void))? },
        // The portal is a Linux thing: Windows lists its own screens and windows.
        Source::Portal => return Err(Error::Unsupported),
    };
    Ok(item)
}

/// Asks Windows to allow capture without the border, and says how it went.
fn turn_border_off(session: &GraphicsCaptureSession) -> String {
    let access = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
        .and_then(|operation| operation.join());
    let access_note = match access {
        Ok(status) => match status.0 {
            0 => "access denied by the system".to_owned(),
            1 => "access not declared by the app".to_owned(),
            2 => "access denied by the user".to_owned(),
            3 => "access needs a user prompt".to_owned(),
            4 => "access allowed".to_owned(),
            other => format!("access status {other}"),
        },
        Err(error) => format!("access request failed: {error}"),
    };
    match session.SetIsBorderRequired(false) {
        Ok(()) => format!("asked to hide ({access_note})"),
        Err(error) => format!("could not hide ({access_note}; {error})"),
    }
}

pub fn start(source: Source, options: Options, on_frame: FrameCallback) -> Result<Session> {
    let item = create_item(source)?;
    let size = item.Size()?;
    let (context, device) = create_devices()?;

    let pool =
        Direct3D11CaptureFramePool::CreateFreeThreaded(&device, PIXEL_FORMAT, BUFFERS, size)?;
    let session = pool.CreateCaptureSession(&item)?;

    let cursor_applied = session.SetIsCursorCaptureEnabled(options.cursor).is_ok();
    let border_status = if options.border {
        "left on (as requested)".to_owned()
    } else {
        turn_border_off(&session)
    };

    // Always ask for the monitor's full rate: left alone, Windows behaves as
    // if capped near 60 fps and rounds any interval up to whole refreshes.
    // A smaller rate is chosen in software by the pacer instead.
    let full_rate = TimeSpan { Duration: 10_000 }; // 1 ms
    let full_rate_note = match session.SetMinUpdateInterval(full_rate) {
        Ok(()) => "capturing at the full source rate".to_owned(),
        Err(error) => format!("could not ask for the full rate ({error}), expect up to 60 fps"),
    };
    let (pacer, max_fps_status) = match options.max_fps {
        Some(fps) if fps > 0 => (
            Some(Pacer::new(fps)),
            format!("{full_rate_note}; picking about {fps} fps in software"),
        ),
        _ => (None, full_rate_note),
    };
    let paced_out = Arc::new(AtomicU64::new(0));

    let pipeline = Arc::new(Mutex::new(Pipeline {
        context,
        device,
        size,
        staging: None,
        on_frame,
        pacer,
        paced_out: Arc::clone(&paced_out),
    }));
    let handler_pipeline = Arc::clone(&pipeline);
    pool.FrameArrived(
        &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
            if let Some(pool) = pool.as_ref() {
                let mut pipeline = handler_pipeline
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // A frame that cannot be read is dropped; the next one tries again.
                let _ = pipeline.handle(pool);
            }
            Ok(())
        }),
    )?;
    session.StartCapture()?;

    Ok(Session {
        pool,
        session,
        size: (size.Width.max(0) as u32, size.Height.max(0) as u32),
        border_status,
        max_fps_status,
        paced_out,
        cursor_applied,
        _pipeline: pipeline,
    })
}

pub fn process_cpu_seconds() -> Option<f64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Some((ticks(kernel) + ticks(user)) as f64 * 100e-9)
}
