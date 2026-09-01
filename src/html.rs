//! HTML → PDF printing (WebView2).

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use webview2_com::{
    wait_with_pump, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, NavigationCompletedEventHandler,
    PermissionRequestedEventHandler, PrintToPdfCompletedHandler,
};
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{E_POINTER, E_UNEXPECTED, FALSE, HWND};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use webview2_com::Microsoft::Web::WebView2::Win32::*;

#[derive(Debug, Clone)]
pub struct PageSizeInfo {
    pub display_name: String,
    pub width_inches: f64,
    pub height_inches: f64,
}

pub struct HtmlToPdfParams {
    pub url: String,
    pub output_path: PathBuf,
    pub paper_size: Option<PageSizeInfo>,
    pub orientation: Option<String>,
    pub user_data_folder: Option<PathBuf>,
}

fn map_orientation(input: Option<&str>) -> COREWEBVIEW2_PRINT_ORIENTATION {
    match input.map(|s| s.trim().to_lowercase()).as_deref() {
        Some("landscape") => COREWEBVIEW2_PRINT_ORIENTATION_LANDSCAPE,
        _ => COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT,
    }
}

struct HwndGuard(HWND);

impl Drop for HwndGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}

fn wait_with_pump_timeout<T>(
    rx: mpsc::Receiver<T>,
    timeout: Duration,
    context: &str,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(val) => return Ok(val),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "{}: timed out after {}s",
                        context,
                        timeout.as_secs()
                    ));
                }
                unsafe {
                    let mut msg = std::mem::zeroed();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(format!("{}: channel disconnected", context));
            }
        }
    }
}

unsafe fn create_hidden_window() -> Result<HWND, String> {
    let class_name = windows::core::w!("Static");
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        windows::core::w!(""),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        Some(HWND_MESSAGE),
        None,
        None,
        None,
    )
    .map_err(|e| format!("CreateWindowExW failed: {}", e))?;
    Ok(hwnd)
}

fn default_user_data_folder() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("winprint-kit")
        .join("webview2_print")
}

pub fn webview2_available() -> bool {
    unsafe {
        let mut version = PWSTR::null();
        let result =
            GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version);
        let ok = result.is_ok() && !version.is_null();
        if ok {
            let _ = CoTaskMemFree(Some(version.0 as *const _));
        }
        ok
    }
}

pub fn html_to_pdf(params: HtmlToPdfParams) -> Result<PathBuf, String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| format!("CoInitializeEx failed: {}", e))?;
    }
    let result = (|| {
        if !webview2_available() {
            return Err(
                "WebView2 Runtime is not installed. HTML printing requires WebView2 Runtime."
                    .to_string(),
            );
        }
        html_to_pdf_inner(params)
    })();
    unsafe {
        CoUninitialize();
    }
    result
}

fn html_to_pdf_inner(params: HtmlToPdfParams) -> Result<PathBuf, String> {
    let env_timeout = Duration::from_secs(10);
    let nav_timeout = Duration::from_secs(15);
    let pdf_timeout = Duration::from_secs(45);

    unsafe {
        let hwnd = create_hidden_window()?;
        let _hwnd_guard = HwndGuard(hwnd);
        let user_data_folder = params
            .user_data_folder
            .clone()
            .unwrap_or_else(default_user_data_folder);
        std::fs::create_dir_all(&user_data_folder)
            .map_err(|e| format!("Failed to create WebView2 user data folder: {}", e))?;
        let user_data_wide: Vec<u16> = user_data_folder
            .to_str()
            .ok_or_else(|| "WebView2 user data folder contains invalid UTF-8".to_string())?
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let (tx, rx) = mpsc::channel();
        let env_handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |error_code, env| {
                let result: Result<ICoreWebView2Environment, webview2_com::Error> = (|| {
                    error_code?;
                    env.ok_or_else(|| {
                        webview2_com::Error::WindowsError(windows::core::Error::from(E_POINTER))
                    })
                })(
                );
                tx.send(result)
                    .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
            },
        ));

        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            PCWSTR(user_data_wide.as_ptr()),
            None,
            &env_handler,
        )
        .map_err(|e| format!("CreateCoreWebView2EnvironmentWithOptions failed: {}", e))?;

        let environment = wait_with_pump(rx)
            .map_err(|e| format!("wait_with_pump outer: {}", e))?
            .map_err(|e| format!("wait_with_pump inner: {}", e))?;

        let (tx, rx) = mpsc::channel();
        let controller_handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |error_code, controller| {
                let result: Result<ICoreWebView2Controller, webview2_com::Error> = (|| {
                    error_code?;
                    controller.ok_or_else(|| {
                        webview2_com::Error::WindowsError(windows::core::Error::from(E_POINTER))
                    })
                })(
                );
                tx.send(result)
                    .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
            },
        ));

        environment
            .CreateCoreWebView2Controller(hwnd, &controller_handler)
            .map_err(|e| format!("CreateCoreWebView2Controller failed: {}", e))?;

        let controller = wait_with_pump_timeout(rx, env_timeout, "Controller creation")?
            .map_err(|e| format!("Controller creation callback error: {}", e))?;

        let webview = controller
            .CoreWebView2()
            .map_err(|e| format!("Failed to get CoreWebView2: {}", e))?;

        let settings = webview
            .Settings()
            .map_err(|e| format!("Failed to get settings: {}", e))?;
        settings
            .SetIsScriptEnabled(FALSE.into())
            .map_err(|e| format!("Failed to disable JavaScript: {}", e))?;

        settings
            .SetAreDefaultScriptDialogsEnabled(FALSE.into())
            .map_err(|e| format!("Failed to disable script dialogs: {}", e))?;

        let perm_handler =
            PermissionRequestedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                }
                Ok(())
            }));
        let mut perm_token = 0i64;
        webview
            .add_PermissionRequested(&perm_handler, &mut perm_token)
            .map_err(|e| format!("Failed to add PermissionRequested handler: {}", e))?;

        let (tx, rx) = mpsc::channel();
        let nav_handler =
            NavigationCompletedEventHandler::create(Box::new(move |_sender, args| {
                let mut success = windows::core::BOOL(0);
                if let Some(ref a) = args {
                    let _ = a.IsSuccess(&mut success);
                }
                let _ = tx.send(success.0 != 0);
                Ok(())
            }));
        let mut nav_token = 0i64;
        webview
            .add_NavigationCompleted(&nav_handler, &mut nav_token)
            .map_err(|e| format!("Failed to add NavigationCompleted handler: {}", e))?;

        let url_wide: Vec<u16> = params
            .url
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        webview
            .Navigate(PCWSTR(url_wide.as_ptr()))
            .map_err(|e| format!("Failed to navigate: {}", e))?;

        let nav_success = wait_with_pump_timeout(rx, nav_timeout, "Navigation")?;
        let _ = webview.remove_NavigationCompleted(nav_token);

        if !nav_success {
            return Err(format!("Navigation failed for URL: {}", params.url));
        }

        let env6 = environment
            .cast::<ICoreWebView2Environment6>()
            .map_err(|e| format!("Environment does not support PrintSettings: {}", e))?;

        let print_settings = env6
            .CreatePrintSettings()
            .map_err(|e| format!("CreatePrintSettings failed: {}", e))?;

        print_settings
            .SetShouldPrintHeaderAndFooter(FALSE.into())
            .map_err(|e| format!("SetShouldPrintHeaderAndFooter failed: {}", e))?;

        print_settings
            .SetOrientation(map_orientation(params.orientation.as_deref()))
            .map_err(|e| format!("SetOrientation failed: {}", e))?;

        if let Some(ref paper) = params.paper_size {
            print_settings
                .SetPageWidth(paper.width_inches)
                .map_err(|e| format!("SetPageWidth failed: {}", e))?;
            print_settings
                .SetPageHeight(paper.height_inches)
                .map_err(|e| format!("SetPageHeight failed: {}", e))?;
        }

        let webview7 = webview
            .cast::<ICoreWebView2_7>()
            .map_err(|e| format!("CoreWebView2 does not support ICoreWebView2_7: {}", e))?;

        let output_path_str = params
            .output_path
            .to_str()
            .ok_or_else(|| "Output path contains invalid UTF-8".to_string())?;
        let output_path_wide: Vec<u16> = output_path_str
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let (tx, rx) = mpsc::channel();
        let pdf_handler =
            PrintToPdfCompletedHandler::create(Box::new(move |error_code, success| {
                let result: Result<bool, webview2_com::Error> = (|| {
                    error_code?;
                    Ok(success.into())
                })();
                tx.send(result)
                    .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
            }));

        webview7
            .PrintToPdf(
                PCWSTR(output_path_wide.as_ptr()),
                &print_settings,
                &pdf_handler,
            )
            .map_err(|e| format!("PrintToPdf call failed: {}", e))?;

        let pdf_success = wait_with_pump_timeout(rx, pdf_timeout, "PrintToPdf")?
            .map_err(|e| format!("PrintToPdf callback error: {}", e))?;

        if !pdf_success {
            return Err("PrintToPdf failed".to_string());
        }

        let _ = webview.remove_PermissionRequested(perm_token);

        Ok(params.output_path)
    }
}
