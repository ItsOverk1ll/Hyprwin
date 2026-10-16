//! Hiding foreign windows without minimising or SW_HIDE.
//!
//! `DwmSetWindowAttribute(DWMWA_CLOAK)` returns E_ACCESSDENIED for windows owned
//! by other processes. The shell's undocumented `IApplicationView::SetCloak`
//! does the same job for any top-level window the shell has a view for, and the
//! window stays composited (DWM thumbnails of it keep rendering). Verified on
//! Windows 11 26H2 build 26300 by `spikes/win-api-probe`.

use std::cell::RefCell;
use std::mem::ManuallyDrop;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, IServiceProvider};
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, interface};

const CLSID_IMMERSIVE_SHELL: GUID = GUID::from_u128(0xC2F03A33_21F5_47FA_B4BB_156362A2F239);

/// `IApplicationViewCollection`. Only the vtable slot of `get_view_for_hwnd`
/// matters; `m*` are placeholders for methods we never call.
#[interface("1841C6D7-4F9D-42C0-AF41-8747538F10E5")]
unsafe trait IApplicationViewCollection: IUnknown {
    fn m1(&self) -> HRESULT;
    fn m2(&self) -> HRESULT;
    fn m3(&self) -> HRESULT;
    fn get_view_for_hwnd(&self, hwnd: HWND, view: *mut Option<IApplicationView>) -> HRESULT;
}

/// `IApplicationView` derives from IInspectable (3 methods) and has six more
/// methods before `SetCloak`.
#[interface("372E1D3B-38D3-42E4-A15B-8AB2B178F513")]
unsafe trait IApplicationView: IUnknown {
    fn m1(&self) -> HRESULT;
    fn m2(&self) -> HRESULT;
    fn m3(&self) -> HRESULT;
    fn m4(&self) -> HRESULT;
    fn m5(&self) -> HRESULT;
    fn m6(&self) -> HRESULT;
    fn m7(&self) -> HRESULT;
    fn m8(&self) -> HRESULT;
    fn m9(&self) -> HRESULT;
    fn set_cloak(&self, cloak_type: u32, flags: i32) -> HRESULT;
}

thread_local! {
    // ManuallyDrop: releasing a COM proxy from a TLS destructor during process
    // exit runs under the loader lock and can deadlock. Call `release()` instead.
    static VIEWS: ManuallyDrop<RefCell<Option<IApplicationViewCollection>>> = const { ManuallyDrop::new(RefCell::new(None)) };
}

fn with_views<T>(f: impl Fn(&IApplicationViewCollection) -> windows::core::Result<T>) -> windows::core::Result<T> {
    VIEWS.with(|cell| {
        // Two attempts: the cached collection dies when explorer.exe restarts.
        for attempt in 0..2 {
            if cell.borrow().is_none() || attempt == 1 {
                crate::com_init();
                let sp: IServiceProvider = unsafe { CoCreateInstance(&CLSID_IMMERSIVE_SHELL, None, CLSCTX_ALL)? };
                let coll: IApplicationViewCollection = unsafe { sp.QueryService(&IApplicationViewCollection::IID)? };
                *cell.borrow_mut() = Some(coll);
            }
            let result = f(cell.borrow().as_ref().expect("set above"));
            match result {
                Ok(v) => return Ok(v),
                Err(e) if attempt == 0 && is_disconnected(&e) => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    })
}

fn is_disconnected(e: &windows::core::Error) -> bool {
    // RPC_E_DISCONNECTED, RPC_S_SERVER_UNAVAILABLE
    matches!(e.code().0 as u32, 0x8001_0108 | 0x8007_06BA)
}

/// Shell-cloaks (`true`) or uncloaks (`false`) a top-level window owned by any
/// process. Fails with `TYPE_E_ELEMENTNOTFOUND` (0x8002802B) when the shell has
/// no view for the window; callers should fall back to off-screen parking.
pub fn set_cloaked(hwnd: HWND, cloaked: bool) -> windows::core::Result<()> {
    with_views(|coll| {
        let mut view: Option<IApplicationView> = None;
        unsafe { coll.get_view_for_hwnd(hwnd, &mut view) }.ok()?;
        let view = view.ok_or_else(|| windows::core::Error::from(HRESULT(0x8002_802Bu32 as i32)))?;
        unsafe { view.set_cloak(1, if cloaked { 2 } else { 0 }) }.ok()
    })
}

/// Releases this thread's cached shell interfaces. Call before the thread or
/// process exits.
pub fn release() {
    VIEWS.with(|cell| drop(cell.borrow_mut().take()));
}

/// Checks the undocumented interfaces still work on this Windows build.
pub fn self_test() -> windows::core::Result<()> {
    with_views(|_| Ok(()))
}
