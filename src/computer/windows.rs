//! Windows desktop-window backend for the in-world computer.
//!
//! Capture uses Windows Graphics Capture (`GraphicsCaptureItem` for an HWND +
//! a free-threaded `Direct3D11CaptureFramePool`). Frames are delivered on a
//! system thread-pool thread through the `FrameArrived` event, converted from
//! BGRA to RGBA and published as a single "latest frame" slot, so the render
//! thread only ever takes a mutex for the duration of a pointer swap.
//!
//! Input is injected with `PostMessageW` straight into the captured window's
//! message queue (client coordinates mapped from frame pixels), so the game
//! keeps keyboard focus and the real cursor never moves. Consequences of that
//! design: `GetKeyState`/`GetAsyncKeyState` in the target do not observe the
//! injected input, and windows of elevated processes reject the messages
//! (UIPI) which surfaces as an error from the `send_*` calls.
//!
//! Compiled for `x86_64-pc-windows-gnu` only; not yet run on Windows.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{BOOL, HMODULE, HWND, LPARAM, POINT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{ClientToScreen, MapWindowPoints, ScreenToClient};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MAPVK_VK_TO_VSC, MapVirtualKeyW, VIRTUAL_KEY, VK_ADD, VK_BACK, VK_CAPITAL, VK_CONTROL,
    VK_DECIMAL, VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT,
    VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_MULTIPLY, VK_NEXT, VK_NUMLOCK,
    VK_NUMPAD0, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_102,
    VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RCONTROL, VK_RETURN,
    VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SCROLL, VK_SHIFT, VK_SPACE, VK_SUBTRACT, VK_TAB,
    VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, ChildWindowFromPointEx, EnumWindows,
    GA_ROOT, GUITHREADINFO, GW_OWNER, GWL_EXSTYLE, GetAncestor, GetClientRect, GetGUIThreadInfo,
    GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW, WM_CHAR, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::core::{IInspectable, Interface};

use super::{DesktopKey, DesktopSource, Frame, PointerButton, WindowInfo};

/// `WHEEL_DELTA`: one wheel notch.
const WHEEL_DELTA: f64 = 120.0;
const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const MK_SHIFT: usize = 0x0004;
const MK_CONTROL: usize = 0x0008;
const MK_MBUTTON: usize = 0x0010;

/// A window handle is an opaque OS identifier, not a pointer we dereference, so
/// it can move between threads.
#[derive(Clone, Copy, PartialEq)]
struct WinHandle(HWND);

// SAFETY: see the type documentation.
unsafe impl Send for WinHandle {}

fn hwnd_from_id(id: u64) -> HWND {
    HWND(id as usize as *mut c_void)
}

fn id_from_hwnd(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

fn win_err(context: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |error| format!("{context}: {error}")
}

// ---------------------------------------------------------------------------
// Window listing
// ---------------------------------------------------------------------------

/// Visible, titled, top-level application windows (excluding this process's
/// own windows, tool windows, owned popups and cloaked UWP shells).
pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    let mut windows: Vec<WindowInfo> = Vec::new();
    // SAFETY: the callback only dereferences `lparam` as the `Vec` above, which
    // outlives the synchronous `EnumWindows` call.
    unsafe {
        EnumWindows(
            Some(enum_window),
            LPARAM(&mut windows as *mut Vec<WindowInfo> as isize),
        )
        .map_err(win_err("EnumWindows"))?;
    }
    Ok(windows)
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: see `list_windows`.
    let windows = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };
    if let Some(title) = listable_title(hwnd) {
        windows.push(WindowInfo {
            id: id_from_hwnd(hwnd),
            title,
        });
    }
    TRUE
}

fn listable_title(hwnd: HWND) -> Option<String> {
    // SAFETY: plain Win32 queries on a handle supplied by EnumWindows; stale
    // handles make the calls fail, which is handled as "not listable".
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return None;
        }
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid());
        if owned && ex_style & WS_EX_APPWINDOW.0 == 0 {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return None;
        }
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        if copied <= 0 {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..copied as usize]))
    }
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// State shared between the capture callbacks (system threads) and the owner.
struct Shared {
    latest: Mutex<Option<Frame>>,
    error: Mutex<Option<String>>,
    closed: AtomicBool,
}

impl Shared {
    fn fail(&self, message: String) {
        let mut error = self.error.lock();
        if error.is_none() {
            *error = Some(message);
        }
    }
}

struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
}

/// GPU objects used only inside the `FrameArrived` handler (serialised by the
/// surrounding mutex, since the immediate context is not thread-safe).
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    staging: Option<Staging>,
    pool_size: SizeInt32,
    /// Conversion buffer swapped with the published frame's buffer, so a
    /// steady stream of frames allocates nothing.
    scratch: Vec<u8>,
}

// SAFETY: the D3D11 device is free-threaded, `IDirect3DDevice` is agile, and
// the immediate context (not thread-safe) is only touched while holding the
// `Mutex<Gpu>` that every user goes through.
unsafe impl Send for Gpu {}

struct Capture {
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    frame_token: i64,
    closed_token: i64,
}

fn start_capture(hwnd: HWND, shared: &Arc<Shared>) -> Result<Capture, String> {
    if !GraphicsCaptureSession::IsSupported()
        .map_err(win_err("GraphicsCaptureSession::IsSupported"))?
    {
        return Err("Windows Graphics Capture is not supported on this system".to_owned());
    }
    // WinRT needs an initialised apartment on this thread. winit already puts
    // the UI thread in an STA, in which case this reports "changed mode" and is
    // deliberately ignored; on a bare worker thread it joins the MTA.
    // SAFETY: no pointers involved.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    // SAFETY: standard interop call chain; every out-parameter is a local that
    // outlives the call.
    let (item, device, context, winrt_device) = unsafe {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(win_err("GraphicsCaptureItem factory"))?;
        let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd).map_err(win_err(
            "CreateForWindow (window may be elevated, minimised or gone)",
        ))?;

        // Hardware first; GPU-less machines (VMs, RDP sessions) fall back to the
        // WARP software rasteriser, which Graphics Capture also supports.
        let mut created = None;
        let mut failures = Vec::new();
        for (name, driver) in [
            ("hardware", D3D_DRIVER_TYPE_HARDWARE),
            ("WARP", D3D_DRIVER_TYPE_WARP),
        ] {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            match D3D11CreateDevice(
                None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            ) {
                Ok(()) => {
                    if let (Some(device), Some(context)) = (device, context) {
                        created = Some((device, context));
                        break;
                    }
                    failures.push(format!("{name}: no device returned"));
                }
                Err(error) => failures.push(format!("{name}: {error}")),
            }
        }
        let (device, context) =
            created.ok_or_else(|| format!("D3D11CreateDevice failed ({})", failures.join("; ")))?;
        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(win_err("ID3D11Device -> IDXGIDevice"))?;
        let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
            .map_err(win_err("CreateDirect3D11DeviceFromDXGIDevice"))?
            .cast()
            .map_err(win_err("IInspectable -> IDirect3DDevice"))?;
        (item, device, context, winrt_device)
    };

    let size = item.Size().map_err(win_err("GraphicsCaptureItem::Size"))?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )
    .map_err(win_err("Direct3D11CaptureFramePool::CreateFreeThreaded"))?;

    let gpu = Arc::new(Mutex::new(Gpu {
        device,
        context,
        winrt_device,
        staging: None,
        pool_size: size,
        scratch: Vec::new(),
    }));

    let handler_shared = Arc::clone(shared);
    let frame_token = pool
        .FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                if let Some(pool) = pool.as_ref()
                    && let Err(message) = on_frame(pool, &gpu, &handler_shared)
                {
                    handler_shared.fail(message);
                }
                Ok(())
            }),
        )
        .map_err(win_err("FrameArrived"))?;

    let closed_shared = Arc::clone(shared);
    let closed_token = item
        .Closed(
            &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                closed_shared.closed.store(true, Ordering::Release);
                Ok(())
            }),
        )
        .map_err(win_err("GraphicsCaptureItem::Closed"))?;

    let session = pool
        .CreateCaptureSession(&item)
        .map_err(win_err("CreateCaptureSession"))?;
    // The pointer shown in-world is the forwarded one; do not bake the user's
    // real cursor into the texture. Both setters need newer Windows builds and
    // are best-effort cosmetics (yellow capture border, cursor).
    let _ = session.SetIsCursorCaptureEnabled(false);
    let _ = session.SetIsBorderRequired(false);
    session.StartCapture().map_err(win_err("StartCapture"))?;

    Ok(Capture {
        item,
        pool,
        session,
        frame_token: frame_token.Value,
        closed_token: closed_token.Value,
    })
}

/// `FrameArrived` body: copy the new frame into a CPU-readable staging
/// texture, convert BGRA -> opaque RGBA and publish it as the latest frame.
fn on_frame(
    pool: &Direct3D11CaptureFramePool,
    gpu: &Mutex<Gpu>,
    shared: &Shared,
) -> Result<(), String> {
    let Ok(frame) = pool.TryGetNextFrame() else {
        return Ok(());
    };
    let content = frame.ContentSize().map_err(win_err("ContentSize"))?;
    let surface = frame.Surface().map_err(win_err("frame Surface"))?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(win_err("surface -> IDirect3DDxgiInterfaceAccess"))?;
    // SAFETY: `access` wraps a live D3D11 texture owned by the frame, which is
    // kept alive until the end of this function.
    let texture: ID3D11Texture2D =
        unsafe { access.GetInterface() }.map_err(win_err("GetInterface<ID3D11Texture2D>"))?;

    let mut gpu = gpu.lock();
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { texture.GetDesc(&mut desc) };

    // The window was resized: rebuild the pool at the new size; this frame is
    // still valid (possibly cropped) and is published below.
    if content.Width != gpu.pool_size.Width || content.Height != gpu.pool_size.Height {
        gpu.pool_size = content;
        let _ = pool.Recreate(
            &gpu.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            content,
        );
    }

    let stale = gpu.staging.as_ref().is_none_or(|staging| {
        staging.width != desc.Width
            || staging.height != desc.Height
            || staging.format != desc.Format
    });
    if stale {
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let mut texture_out: Option<ID3D11Texture2D> = None;
        // SAFETY: `staging_desc` and the out-pointer are valid for the call.
        unsafe {
            gpu.device
                .CreateTexture2D(&staging_desc, None, Some(&mut texture_out))
        }
        .map_err(win_err("CreateTexture2D(staging)"))?;
        let texture_out = texture_out.ok_or("CreateTexture2D returned no texture")?;
        gpu.staging = Some(Staging {
            texture: texture_out,
            width: desc.Width,
            height: desc.Height,
            format: desc.Format,
        });
    }

    let width = (content.Width.max(0) as u32).min(desc.Width);
    let height = (content.Height.max(0) as u32).min(desc.Height);
    if width == 0 || height == 0 {
        return Ok(());
    }

    let Gpu {
        context,
        staging,
        scratch,
        ..
    } = &mut *gpu;
    let staging = staging.as_ref().ok_or("staging texture missing")?;
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: both textures belong to the same device; `mapped` is a valid
    // out-pointer; the mapped memory is read only between Map and Unmap.
    unsafe {
        context.CopyResource(&staging.texture, &texture);
        context
            .Map(&staging.texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(win_err("Map(staging)"))?;
    }
    let row_bytes = width as usize * 4;
    scratch.clear();
    scratch.reserve(row_bytes * height as usize);
    for row in 0..height as usize {
        // SAFETY: the mapping covers `RowPitch * desc.Height` bytes and
        // `row < height <= desc.Height`, `row_bytes <= RowPitch`.
        let source = unsafe {
            std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                row_bytes,
            )
        };
        scratch.extend(
            source
                .chunks_exact(4)
                .flat_map(|px| [px[2], px[1], px[0], 255]),
        );
    }
    // SAFETY: matches the successful Map above.
    unsafe { context.Unmap(&staging.texture, 0) };
    let _ = frame.Close();

    let mut latest = shared.latest.lock();
    match latest.as_mut() {
        Some(previous) => {
            previous.width = width;
            previous.height = height;
            std::mem::swap(&mut previous.rgba, scratch);
        }
        None => {
            *latest = Some(Frame {
                width,
                height,
                rgba: std::mem::take(scratch),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Default)]
struct InputState {
    /// Last pointer position in the captured window's client coordinates.
    client: Option<POINT>,
    /// `MK_*` button bits currently held.
    buttons: usize,
    /// Window that received the last button-down; keeps receiving the drag.
    capture: Option<WinHandle>,
    shift: bool,
    ctrl: bool,
    alt: bool,
    wheel_x: f64,
    wheel_y: f64,
}

impl InputState {
    fn mk_flags(&self) -> usize {
        self.buttons
            | if self.shift { MK_SHIFT } else { 0 }
            | if self.ctrl { MK_CONTROL } else { 0 }
    }
}

fn make_lparam(x: i32, y: i32) -> LPARAM {
    LPARAM((((y as u16 as u32) << 16) | x as u16 as u32) as i32 as isize)
}

/// Windows-backed [`DesktopSource`] for one captured top-level window.
pub struct WindowsDesktopSource {
    hwnd: WinHandle,
    shared: Arc<Shared>,
    capture: Option<Capture>,
    input: InputState,
}

/// Start capturing the window picked from [`list_windows`].
pub fn open(id: u64) -> Result<Box<dyn DesktopSource>, String> {
    let hwnd = hwnd_from_id(id);
    // SAFETY: IsWindow accepts any handle value.
    if !unsafe { IsWindow(hwnd) }.as_bool() {
        return Err("the selected window no longer exists".to_owned());
    }
    let shared = Arc::new(Shared {
        latest: Mutex::new(None),
        error: Mutex::new(None),
        closed: AtomicBool::new(false),
    });
    let capture = start_capture(hwnd, &shared)?;
    Ok(Box::new(WindowsDesktopSource {
        hwnd: WinHandle(hwnd),
        shared,
        capture: Some(capture),
        input: InputState::default(),
    }))
}

impl WindowsDesktopSource {
    fn hwnd(&self) -> HWND {
        self.hwnd.0
    }

    fn ensure_alive(&self) -> Result<(), String> {
        // SAFETY: IsWindow accepts any handle value.
        if self.capture.is_none()
            || self.shared.closed.load(Ordering::Acquire)
            || !unsafe { IsWindow(self.hwnd()) }.as_bool()
        {
            return Err("the captured window was closed".to_owned());
        }
        Ok(())
    }

    /// Screen position of the capture frame's top-left pixel. WGC frames cover
    /// the DWM "extended frame bounds" (window without the invisible resize
    /// border and shadow); fall back to the plain window rectangle.
    fn frame_origin(&self) -> Option<POINT> {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a valid out-buffer of the declared size.
        unsafe {
            if DwmGetWindowAttribute(
                self.hwnd(),
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut RECT as *mut c_void,
                size_of::<RECT>() as u32,
            )
            .is_err()
            {
                GetWindowRect(self.hwnd(), &mut rect).ok()?;
            }
        }
        Some(POINT {
            x: rect.left,
            y: rect.top,
        })
    }

    fn client_origin(&self) -> Option<POINT> {
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: `origin` is a valid in/out point.
        unsafe { ClientToScreen(self.hwnd(), &mut origin) }
            .as_bool()
            .then_some(origin)
    }

    /// Map frame pixels to the top-level window's client coordinates.
    fn frame_to_client(&self, x: f64, y: f64) -> Option<POINT> {
        let frame = self.frame_origin()?;
        let client = self.client_origin()?;
        Some(POINT {
            x: x.floor() as i32 + frame.x - client.x,
            y: y.floor() as i32 + frame.y - client.y,
        })
    }

    fn in_client(&self, point: POINT) -> bool {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a valid out-buffer.
        unsafe { GetClientRect(self.hwnd(), &mut rect) }.is_ok()
            && point.x >= 0
            && point.y >= 0
            && point.x < rect.right
            && point.y < rect.bottom
    }

    /// Deepest child window under `point` (top-level client coordinates), with
    /// the point translated into that child's client coordinates.
    fn hit_test(&self, point: POINT) -> (HWND, POINT) {
        let mut target = self.hwnd();
        let mut local = point;
        loop {
            // SAFETY: handles come from the OS; failures fall through to the
            // current target.
            let child = unsafe {
                ChildWindowFromPointEx(
                    target,
                    local,
                    CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT,
                )
            };
            if child.is_invalid() || child == target {
                return (target, local);
            }
            let mut points = [local];
            // SAFETY: `points` is a valid one-element buffer.
            unsafe { MapWindowPoints(target, child, &mut points) };
            target = child;
            local = points[0];
        }
    }

    /// Target and target-local position for mouse messages: the drag-capturing
    /// window while a button is held, the window under the pointer otherwise.
    fn mouse_target(&self, point: POINT) -> (HWND, POINT) {
        if let Some(WinHandle(captured)) = self.input.capture {
            let mut screen = point;
            // SAFETY: valid in/out points; failures keep the unconverted value.
            unsafe {
                let _ = ClientToScreen(self.hwnd(), &mut screen);
                let _ = ScreenToClient(captured, &mut screen);
            }
            (captured, screen)
        } else {
            self.hit_test(point)
        }
    }

    fn post(
        &self,
        target: HWND,
        message: u32,
        wparam: usize,
        lparam: LPARAM,
    ) -> Result<(), String> {
        // SAFETY: PostMessageW only queues a message; no pointers are passed.
        unsafe { PostMessageW(target, message, WPARAM(wparam), lparam) }
            .map_err(win_err("PostMessageW (target may be elevated)"))
    }

    /// Keyboard messages go to the target thread's focus window when it has
    /// one (e.g. Chromium's inner widget), else to the top-level window.
    fn keyboard_target(&self) -> HWND {
        // SAFETY: `info` is a correctly sized, initialised GUITHREADINFO.
        unsafe {
            let thread = GetWindowThreadProcessId(self.hwnd(), None);
            let mut info = GUITHREADINFO {
                cbSize: size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if thread != 0
                && GetGUIThreadInfo(thread, &mut info).is_ok()
                && !info.hwndFocus.is_invalid()
                && GetAncestor(info.hwndFocus, GA_ROOT) == self.hwnd()
            {
                info.hwndFocus
            } else {
                self.hwnd()
            }
        }
    }
}

impl DesktopSource for WindowsDesktopSource {
    fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        if let Some(message) = self.shared.error.lock().take() {
            return Err(message);
        }
        let frame = self.shared.latest.lock().take();
        if frame.is_none() {
            self.ensure_alive()?;
        }
        Ok(frame)
    }

    fn send_pointer(&mut self, x: f64, y: f64) -> Result<(), String> {
        self.ensure_alive()?;
        let Some(point) = self.frame_to_client(x, y) else {
            return Ok(());
        };
        self.input.client = Some(point);
        // Title bar / borders are not client area; WM_MOUSEMOVE there would
        // hit-test wrongly. Drags that left the client area keep flowing.
        if self.input.capture.is_none() && !self.in_client(point) {
            return Ok(());
        }
        let (target, local) = self.mouse_target(point);
        self.post(
            target,
            WM_MOUSEMOVE,
            self.input.mk_flags(),
            make_lparam(local.x, local.y),
        )
    }

    fn send_button(&mut self, button: PointerButton, pressed: bool) -> Result<(), String> {
        self.ensure_alive()?;
        let Some(point) = self.input.client else {
            return Ok(());
        };
        if pressed && self.input.capture.is_none() && !self.in_client(point) {
            return Ok(());
        }
        let (mk, down, up) = match button {
            PointerButton::Left => (MK_LBUTTON, WM_LBUTTONDOWN, WM_LBUTTONUP),
            PointerButton::Right => (MK_RBUTTON, WM_RBUTTONDOWN, WM_RBUTTONUP),
            PointerButton::Middle => (MK_MBUTTON, WM_MBUTTONDOWN, WM_MBUTTONUP),
        };
        if pressed {
            let (target, local) = self.mouse_target(point);
            if self.input.buttons == 0 {
                self.input.capture = Some(WinHandle(target));
            }
            self.input.buttons |= mk;
            // WM_*BUTTONDOWN reports the state *including* the pressed button.
            self.post(
                target,
                down,
                self.input.mk_flags(),
                make_lparam(local.x, local.y),
            )
        } else {
            let (target, local) = self.mouse_target(point);
            // WM_*BUTTONUP reports the state *excluding* the released button.
            let flags = self.input.mk_flags() & !mk;
            self.input.buttons &= !mk;
            if self.input.buttons == 0 {
                self.input.capture = None;
            }
            self.post(target, up, flags, make_lparam(local.x, local.y))
        }
    }

    /// `dx`/`dy` are wheel notches; positive `dy` is wheel-forward (scroll up),
    /// positive `dx` scrolls right. Fractions accumulate until a whole
    /// `WHEEL_DELTA` unit is available.
    fn send_scroll(&mut self, dx: f64, dy: f64) -> Result<(), String> {
        self.ensure_alive()?;
        let Some(point) = self.input.client else {
            return Ok(());
        };
        if !self.in_client(point) {
            return Ok(());
        }
        self.input.wheel_x += dx * WHEEL_DELTA;
        self.input.wheel_y += dy * WHEEL_DELTA;
        let whole_x = self.input.wheel_x.trunc();
        let whole_y = self.input.wheel_y.trunc();
        self.input.wheel_x -= whole_x;
        self.input.wheel_y -= whole_y;

        let (target, _) = self.hit_test(point);
        let mut screen = point;
        // SAFETY: valid in/out point; failure keeps client coordinates.
        unsafe {
            let _ = ClientToScreen(self.hwnd(), &mut screen);
        }
        let lparam = make_lparam(screen.x, screen.y);
        let flags = self.input.mk_flags();
        for (delta, message) in [(whole_y, WM_MOUSEWHEEL), (whole_x, WM_MOUSEHWHEEL)] {
            if delta != 0.0 {
                let delta = delta.clamp(i16::MIN as f64, i16::MAX as f64) as i16;
                let wparam = ((delta as u16 as usize) << 16) | flags;
                self.post(target, message, wparam, lparam)?;
            }
        }
        Ok(())
    }

    /// `Code` is a Linux evdev key code (translated to a virtual key), `Char`
    /// is committed text delivered as `WM_CHAR` on press only.
    fn send_key(&mut self, key: DesktopKey, pressed: bool) -> Result<(), String> {
        self.ensure_alive()?;
        match key {
            DesktopKey::Char(ch) => {
                if !pressed {
                    return Ok(());
                }
                let target = self.keyboard_target();
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    self.post(target, WM_CHAR, *unit as usize, LPARAM(1))?;
                }
                Ok(())
            }
            DesktopKey::Code(code) => {
                let Some((vk, extended)) = evdev_to_vk(code) else {
                    return Ok(());
                };
                match vk {
                    VK_LSHIFT | VK_RSHIFT | VK_SHIFT => self.input.shift = pressed,
                    VK_LCONTROL | VK_RCONTROL | VK_CONTROL => self.input.ctrl = pressed,
                    VK_LMENU | VK_RMENU | VK_MENU => self.input.alt = pressed,
                    _ => {}
                }
                // SAFETY: pure lookup.
                let scancode = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } & 0xFF;
                let mut lparam = 1u32 | (scancode << 16);
                if extended {
                    lparam |= 1 << 24;
                }
                let system = self.input.alt || matches!(vk, VK_LMENU | VK_RMENU | VK_MENU);
                if self.input.alt {
                    lparam |= 1 << 29;
                }
                if !pressed {
                    lparam |= (1 << 30) | (1 << 31);
                }
                let message = match (pressed, system) {
                    (true, false) => WM_KEYDOWN,
                    (false, false) => WM_KEYUP,
                    (true, true) => WM_SYSKEYDOWN,
                    (false, true) => WM_SYSKEYUP,
                };
                self.post(
                    self.keyboard_target(),
                    message,
                    vk.0 as usize,
                    LPARAM(lparam as i32 as isize),
                )
            }
        }
    }

    fn close(&mut self) {
        let Some(capture) = self.capture.take() else {
            return;
        };
        // Teardown is best effort: the window or the device may already be gone.
        let _ = capture
            .pool
            .RemoveFrameArrived(windows::Foundation::EventRegistrationToken {
                Value: capture.frame_token,
            });
        let _ = capture
            .item
            .RemoveClosed(windows::Foundation::EventRegistrationToken {
                Value: capture.closed_token,
            });
        let _ = capture.session.Close();
        let _ = capture.pool.Close();
        self.shared.latest.lock().take();
        // A button left pressed would otherwise never be released in the target.
        self.input = InputState::default();
    }
}

impl Drop for WindowsDesktopSource {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// Key translation
// ---------------------------------------------------------------------------

/// Linux evdev key code (`KEY_*`) -> (Windows virtual key, extended-key flag).
fn evdev_to_vk(code: u32) -> Option<(VIRTUAL_KEY, bool)> {
    let plain = |vk: VIRTUAL_KEY| Some((vk, false));
    let extended = |vk: VIRTUAL_KEY| Some((vk, true));
    let letter = |c: u8| plain(VIRTUAL_KEY(c as u16));
    match code {
        1 => plain(VK_ESCAPE),
        // KEY_1..KEY_9 then KEY_0 map to '1'..'9','0'.
        2..=10 => plain(VIRTUAL_KEY(0x31 + (code - 2) as u16)),
        11 => plain(VIRTUAL_KEY(0x30)),
        12 => plain(VK_OEM_MINUS),
        13 => plain(VK_OEM_PLUS),
        14 => plain(VK_BACK),
        15 => plain(VK_TAB),
        16 => letter(b'Q'),
        17 => letter(b'W'),
        18 => letter(b'E'),
        19 => letter(b'R'),
        20 => letter(b'T'),
        21 => letter(b'Y'),
        22 => letter(b'U'),
        23 => letter(b'I'),
        24 => letter(b'O'),
        25 => letter(b'P'),
        26 => plain(VK_OEM_4),
        27 => plain(VK_OEM_6),
        28 => plain(VK_RETURN),
        29 => plain(VK_LCONTROL),
        30 => letter(b'A'),
        31 => letter(b'S'),
        32 => letter(b'D'),
        33 => letter(b'F'),
        34 => letter(b'G'),
        35 => letter(b'H'),
        36 => letter(b'J'),
        37 => letter(b'K'),
        38 => letter(b'L'),
        39 => plain(VK_OEM_1),
        40 => plain(VK_OEM_7),
        41 => plain(VK_OEM_3),
        42 => plain(VK_LSHIFT),
        43 => plain(VK_OEM_5),
        44 => letter(b'Z'),
        45 => letter(b'X'),
        46 => letter(b'C'),
        47 => letter(b'V'),
        48 => letter(b'B'),
        49 => letter(b'N'),
        50 => letter(b'M'),
        51 => plain(VK_OEM_COMMA),
        52 => plain(VK_OEM_PERIOD),
        53 => plain(VK_OEM_2),
        54 => plain(VK_RSHIFT),
        55 => plain(VK_MULTIPLY),
        56 => plain(VK_LMENU),
        57 => plain(VK_SPACE),
        58 => plain(VK_CAPITAL),
        // KEY_F1..KEY_F10.
        59..=68 => plain(VIRTUAL_KEY(VK_F1.0 + (code - 59) as u16)),
        69 => extended(VK_NUMLOCK),
        70 => plain(VK_SCROLL),
        // Numeric keypad: 7 8 9 - 4 5 6 + 1 2 3 0 .
        71..=73 => plain(VIRTUAL_KEY(VK_NUMPAD0.0 + 7 + (code - 71) as u16)),
        74 => plain(VK_SUBTRACT),
        75..=77 => plain(VIRTUAL_KEY(VK_NUMPAD0.0 + 4 + (code - 75) as u16)),
        78 => plain(VK_ADD),
        79..=81 => plain(VIRTUAL_KEY(VK_NUMPAD0.0 + 1 + (code - 79) as u16)),
        82 => plain(VK_NUMPAD0),
        83 => plain(VK_DECIMAL),
        86 => plain(VK_OEM_102),
        87 => plain(VIRTUAL_KEY(VK_F1.0 + 10)),
        88 => plain(VIRTUAL_KEY(VK_F1.0 + 11)),
        96 => extended(VK_RETURN),
        97 => extended(VK_RCONTROL),
        98 => extended(VK_DIVIDE),
        100 => extended(VK_RMENU),
        102 => extended(VK_HOME),
        103 => extended(VK_UP),
        104 => extended(VK_PRIOR),
        105 => extended(VK_LEFT),
        106 => extended(VK_RIGHT),
        107 => extended(VK_END),
        108 => extended(VK_DOWN),
        109 => extended(VK_NEXT),
        110 => extended(VK_INSERT),
        111 => extended(VK_DELETE),
        125 => extended(VK_LWIN),
        126 => extended(VK_RWIN),
        _ => None,
    }
}
