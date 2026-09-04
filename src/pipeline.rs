//! Print orchestration: download ? type detection ? dispatch to PDF / image / XPS / HTML / Office printers.
//!
//! Progress is reported via the [`StatusSink`] callback; the event sink is implemented by the caller (e.g. tauri `emit`).

use crate::download;
use crate::file_type;
use crate::print::{build_print_ticket, extract_capabilities, PrintOptions, PrinterCapabilities};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use winprint_ext::printer::{FilePrinter, ImagePrinter, PdfiumPrinter, PrinterDevice, XpsPrinter};
use winprint_ext::ticket::PrintCapabilities;

#[cfg(feature = "html")]
use winprint_ext::ticket::FeatureOptionPack;

#[cfg(feature = "html")]
use crate::html;
#[cfg(feature = "html")]
use tempfile::Builder as TempFileBuilder;
#[cfg(feature = "office")]
use crate::office;
#[cfg(feature = "office")]
use crate::office::OfficeKind;
#[cfg(feature = "office")]
use std::fs::remove_file;
#[cfg(feature = "office")]
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintStatus {
    Printing,
    Success,
    Failed,
}

/// Machine-readable cause of a failed print. Applications localize `kind`; `detail` is a
/// concise English fallback for logs and for kinds the app does not localize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintFailure {
    pub kind: PrintFailureKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintFailureKind {
    #[cfg(feature = "html")]
    WebView2NotInstalled,
    #[cfg(feature = "office")]
    OfficeNotInstalled(OfficeKind),
    Other,
}

impl PrintFailure {
    /// Generic failure; `detail` is the cause.
    pub fn other(detail: impl Into<String>) -> Self {
        Self {
            kind: PrintFailureKind::Other,
            detail: detail.into(),
        }
    }

    #[cfg(feature = "html")]
    pub fn webview2_missing() -> Self {
        Self {
            kind: PrintFailureKind::WebView2NotInstalled,
            detail: "WebView2 Runtime is not installed (required for web page printing)"
                .to_string(),
        }
    }

    #[cfg(feature = "office")]
    pub fn office_missing(kind: OfficeKind) -> Self {
        Self {
            kind: PrintFailureKind::OfficeNotInstalled(kind),
            detail: format!(
                "{} is not installed (required for Office document printing)",
                kind.app_name()
            ),
        }
    }
}

pub trait StatusSink: Send + Sync {
    fn on_status(&self, id: &str, status: PrintStatus, error: Option<PrintFailure>);
}

struct CacheInner {
    printer_cache: HashMap<String, PrinterDevice>,
    capabilities_cache: HashMap<String, PrintCapabilities>,
}

pub(crate) struct PrintCache {
    inner: tokio::sync::Mutex<CacheInner>,
}

impl PrintCache {
    pub fn new() -> Self {
        Self {
            inner: tokio::sync::Mutex::new(CacheInner {
                printer_cache: HashMap::new(),
                capabilities_cache: HashMap::new(),
            }),
        }
    }

    pub async fn list_printers(&self) -> Result<Vec<String>, String> {
        let printers =
            PrinterDevice::all().map_err(|_| "Failed to retrieve printer list".to_string())?;
        let mut inner = self.inner.lock().await;
        inner.printer_cache.clear();
        for p in printers {
            inner.printer_cache.insert(p.name().to_string(), p);
        }
        Ok(inner.printer_cache.keys().cloned().collect())
    }

    pub async fn printer_capabilities(
        &self,
        printer_name: &str,
    ) -> Result<PrinterCapabilities, String> {
        let device = self
            .inner
            .lock()
            .await
            .printer_cache
            .get(printer_name)
            .cloned()
            .ok_or_else(|| format!("Printer not found: {}", printer_name))?;
        if let Some(caps) = self
            .inner
            .lock()
            .await
            .capabilities_cache
            .get(printer_name)
            .cloned()
        {
            return Ok(extract_capabilities(&caps));
        }
        let caps = fetch_capabilities_blocking(&device).await?;
        if let Ok(mut s) = self.inner.try_lock() {
            s.capabilities_cache.insert(printer_name.to_string(), caps.clone());
        }
        Ok(extract_capabilities(&caps))
    }

    async fn resolve(
        &self,
        printer_name: &str,
    ) -> Result<(PrinterDevice, PrintCapabilities), String> {
        let (device, capabilities) = {
            let inner = self.inner.lock().await;
            (
                inner.printer_cache.get(printer_name).cloned(),
                inner.capabilities_cache.get(printer_name).cloned(),
            )
        };
        let device = device.ok_or_else(|| format!("Printer not found: {}", printer_name))?;
        let capabilities = match capabilities {
            Some(c) => c,
            None => {
                let caps = fetch_capabilities_blocking(&device).await.map_err(|e| {
                    format!("Failed to fetch capabilities for printer '{}': {}", printer_name, e)
                })?;
                if let Ok(mut s) = self.inner.try_lock() {
                    s.capabilities_cache
                        .insert(printer_name.to_string(), caps.clone());
                }
                caps
            }
        };
        Ok((device, capabilities))
    }
}

async fn fetch_capabilities_blocking(device: &PrinterDevice) -> Result<PrintCapabilities, String> {
    let device = device.clone();
    tokio::task::spawn_blocking(move || {
        PrintCapabilities::fetch(&device).map_err(|e| format!("Failed to fetch capabilities: {e}"))
    })
    .await
    .map_err(|e| format!("Capabilities task failed: {e}"))?
}

pub struct PrintPipeline {
    cache: PrintCache,
    html_user_data_folder: Option<PathBuf>,
}

impl PrintPipeline {
    pub fn new() -> Self {
        Self {
            cache: PrintCache::new(),
            html_user_data_folder: None,
        }
    }

    pub fn with_html_user_data_folder(mut self, folder: impl Into<PathBuf>) -> Self {
        self.html_user_data_folder = Some(folder.into());
        self
    }

    pub async fn list_printers(&self) -> Result<Vec<String>, String> {
        self.cache.list_printers().await
    }

    pub async fn printer_capabilities(
        &self,
        printer_name: &str,
    ) -> Result<PrinterCapabilities, String> {
        self.cache.printer_capabilities(printer_name).await
    }

    pub async fn print_document(
        &self,
        id: &str,
        url: &str,
        printer_name: &str,
        options: &PrintOptions,
        sink: Arc<dyn StatusSink>,
    ) {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            sink.on_status(
                id,
                PrintStatus::Failed,
                Some(PrintFailure::other("Only HTTP/HTTPS URLs are allowed")),
            );
            return;
        }

        let (printer_device, capabilities) = match self.cache.resolve(printer_name).await {
            Ok(v) => v,
            Err(e) => {
                sink.on_status(id, PrintStatus::Failed, Some(PrintFailure::other(e)));
                return;
            }
        };

        let id = id.to_string();
        let url = url.to_string();
        let printer_name = printer_name.to_string();
        let options = options.clone();
        let html_user_data_folder = self.html_user_data_folder.clone();

        tokio::spawn(async move {
            macro_rules! sink_error {
                ($msg:expr) => {
                    sink.on_status(&id, PrintStatus::Failed, Some(PrintFailure::other($msg)));
                };
            }
            macro_rules! sink_fail {
                ($failure:expr) => {
                    sink.on_status(&id, PrintStatus::Failed, Some($failure));
                };
            }
            macro_rules! sink_status {
                ($status:expr) => {
                    sink.on_status(&id, $status, None);
                };
            }

            if let Some(ref url_type) = file_type::detect_type_from_url(&url) {
                if file_type::is_html_type(url_type) {
                    sink_status!(PrintStatus::Printing);
                    #[cfg(feature = "html")]
                    if !crate::html::webview2_available() {
                        sink_fail!(PrintFailure::webview2_missing());
                        return;
                    }
                    match print_html(
                        &url,
                        &printer_name,
                        printer_device.clone(),
                        &capabilities,
                        &options,
                        html_user_data_folder.clone(),
                    )
                    .await
                    {
                        Ok(_) => {
                            sink_status!(PrintStatus::Success);
                        }
                        Err(e) => {
                            sink_error!(e);
                        }
                    }
                    return;
                }
            }

            let (temp_path, file_type) = match download::download_and_detect(&url).await {
                Ok(t) => t,
                Err(e) => {
                    sink_error!(format!("Download failed: {}", e));
                    return;
                }
            };

            if file_type == "html" || file_type == "htm" {
                sink_status!(PrintStatus::Printing);
                #[cfg(feature = "html")]
                if !crate::html::webview2_available() {
                    drop(temp_path);
                    sink_fail!(PrintFailure::webview2_missing());
                    return;
                }
                match print_html(
                    &url,
                    &printer_name,
                    printer_device.clone(),
                    &capabilities,
                    &options,
                    html_user_data_folder,
                )
                .await
                {
                    Ok(_) => {
                        sink_status!(PrintStatus::Success);
                    }
                    Err(e) => {
                        sink_error!(e);
                    }
                }
                drop(temp_path);
                return;
            }

            #[cfg(feature = "office")]
            if file_type::is_office_type(&file_type) {
                if let Some(kind) = office::kind_for_file_type(&file_type) {
                    if !office::is_office_installed(kind).await {
                        sink_fail!(PrintFailure::office_missing(kind));
                        drop(temp_path);
                        return;
                    }
                }
            }

            let print_ticket = match build_print_ticket(&printer_device, &capabilities, &options) {
                Ok(t) => t,
                Err(e) => {
                    sink_error!(format!("Print ticket error: {}", e));
                    return;
                }
            };

            let result = if file_type == "pdf" {
                sink_status!(PrintStatus::Printing);
                print_pdf(&temp_path, print_ticket, printer_device.clone()).await
            } else if file_type::is_image_type(&file_type) {
                sink_status!(PrintStatus::Printing);
                let auto_rotate = options.orientation.is_none();
                print_image(
                    &temp_path,
                    print_ticket,
                    printer_device.clone(),
                    auto_rotate,
                )
                .await
            } else if file_type == "xps" {
                sink_status!(PrintStatus::Printing);
                print_xps(&temp_path, print_ticket, printer_device.clone()).await
            } else if file_type::is_office_type(&file_type) {
                print_office(&id, &temp_path, print_ticket, printer_device.clone(), &sink).await
            } else {
                Err(format!("Unsupported file type: {}", file_type))
            };

            drop(temp_path);

            match result {
                Ok(_) => {
                    sink_status!(PrintStatus::Success);
                }
                Err(e) => {
                    sink_error!(e);
                }
            }
        });
    }
}

impl Default for PrintPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "html")]
async fn print_html(
    url: &str,
    _printer_name: &str,
    printer_device: PrinterDevice,
    capabilities: &PrintCapabilities,
    options: &PrintOptions,
    user_data_folder: Option<PathBuf>,
) -> Result<(), String> {
    let paper_size = options.page_size.as_ref().and_then(|input| {
        let normalized = input.trim().to_lowercase();
        capabilities
            .page_media_sizes()
            .find(|o| {
                o.display_name()
                    .map(|d| d.trim().to_lowercase() == normalized)
                    .unwrap_or(false)
            })
            .map(|o| {
                let size = o.size();
                crate::html::PageSizeInfo {
                    display_name: o.display_name().unwrap_or("").to_string(),
                    width_inches: size.width_in_micron() as f64 / 25400.0,
                    height_inches: size.height_in_micron() as f64 / 25400.0,
                }
            })
    });

    let pdf_temp_path = TempFileBuilder::new()
        .suffix(".pdf")
        .tempfile()
        .map_err(|e| format!("Failed to create temp file: {}", e))?
        .into_temp_path();
    let pdf_path = pdf_temp_path.to_path_buf();

    let params = html::HtmlToPdfParams {
        url: url.to_string(),
        output_path: pdf_path.clone(),
        paper_size,
        orientation: options.orientation.clone(),
        user_data_folder,
    };

    tokio::task::spawn_blocking(move || html::html_to_pdf(params))
        .await
        .map_err(|e| format!("Blocking task failed: {}", e))??;

    let print_ticket = build_print_ticket(&printer_device, capabilities, options)?;

    print_pdf(&pdf_path, print_ticket, printer_device).await
}

#[cfg(not(feature = "html"))]
async fn print_html(
    _url: &str,
    _printer_name: &str,
    _printer_device: PrinterDevice,
    _capabilities: &PrintCapabilities,
    _options: &PrintOptions,
    _user_data_folder: Option<PathBuf>,
) -> Result<(), String> {
    Err("HTML printing is not enabled (enable the 'html' feature)".to_string())
}

async fn print_pdf(
    path: impl AsRef<Path>,
    ticket: winprint_ext::ticket::PrintTicket,
    device: PrinterDevice,
) -> Result<(), String> {
    let printer = PdfiumPrinter::new(device);
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || {
        printer
            .print(&path, ticket)
            .map_err(|e| format!("PDF printing failed: {}", e))
    })
    .await
    .map_err(|e| format!("Blocking task failed: {}", e))?
}

async fn print_image(
    path: impl AsRef<Path>,
    ticket: winprint_ext::ticket::PrintTicket,
    device: PrinterDevice,
    auto_rotate: bool,
) -> Result<(), String> {
    let printer = ImagePrinter::new(device);
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || {
        printer
            .print_with_options(&path, ticket, auto_rotate)
            .map_err(|e| format!("Image printing failed: {}", e))
    })
    .await
    .map_err(|e| format!("Blocking task failed: {}", e))?
}

async fn print_xps(
    path: impl AsRef<Path>,
    ticket: winprint_ext::ticket::PrintTicket,
    device: PrinterDevice,
) -> Result<(), String> {
    let printer = XpsPrinter::new(device);
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || {
        printer
            .print(&path, ticket)
            .map_err(|e| format!("XPS printing failed: {}", e))
    })
    .await
    .map_err(|e| format!("Blocking task failed: {}", e))?
}

#[cfg(feature = "office")]
struct TempFileGuard(PathBuf);

#[cfg(feature = "office")]
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Err(e) = remove_file(&self.0) {
            eprintln!(
                "[TempFileGuard] Failed to remove '{}': {}",
                self.0.display(),
                e
            );
        }
    }
}

#[cfg(feature = "office")]
static OFFICE_CONVERSION_SEMAPHORE: std::sync::OnceLock<Semaphore> =
    std::sync::OnceLock::new();

#[cfg(feature = "office")]
fn get_office_semaphore() -> &'static Semaphore {
    OFFICE_CONVERSION_SEMAPHORE.get_or_init(|| Semaphore::new(1))
}

#[cfg(feature = "office")]
async fn print_office(
    id: &str,
    input_path: impl AsRef<Path>,
    ticket: winprint_ext::ticket::PrintTicket,
    device: PrinterDevice,
    sink: &Arc<dyn StatusSink>,
) -> Result<(), String> {
    let input_path = input_path.as_ref();
    let input_str = input_path
        .to_str()
        .ok_or_else(|| "Input path contains invalid UTF-8".to_string())?;
    if let Some(kind) = office::kind_from_path(input_str) {
        if !office::is_office_installed(kind).await {
            return Err(format!(
                "{} is not installed. Office printing requires Microsoft Office 2010+.",
                office::kind_display_name(kind)
            ));
        }
    }
    let _permit = get_office_semaphore()
        .acquire()
        .await
        .map_err(|_| "Office conversion semaphore acquire failed".to_string())?;

    sink.on_status(id, PrintStatus::Printing, None);

    let pdf_path = input_path.with_extension("pdf");
    let _pdf_guard = TempFileGuard(pdf_path.clone());

    let output_str = pdf_path
        .to_str()
        .ok_or_else(|| "Output path contains invalid UTF-8".to_string())?;

    office::convert_office_to_pdf(input_str, output_str)
        .await
        .map_err(|e| format!("Office to PDF conversion failed: {}", e))?;

    print_pdf(pdf_path.clone(), ticket, device).await
}

#[cfg(not(feature = "office"))]
async fn print_office(
    _id: &str,
    _input_path: impl AsRef<Path>,
    _ticket: winprint_ext::ticket::PrintTicket,
    _device: PrinterDevice,
    _sink: &Arc<dyn StatusSink>,
) -> Result<(), String> {
    Err("Office printing is not enabled (enable the 'office' feature)".to_string())
}

