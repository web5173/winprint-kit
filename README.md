# winprint-kit

Windows printing / conversion toolkit: direct printing of PDF / image / XPS, and HTML / Office via PDF conversion (optional features). Includes file download and type detection.

> Windows only.

## Features

| Feature | Description | Default |
|---|---|---|
| `html` | HTML → PDF (WebView2) | off |
| `office` | Office → PDF (PowerShell + COM, requires MS Office 2010+) | off |

## Usage

```toml
[dependencies]
winprint-kit = { version = "0.1", features = ["html", "office"] }
```

```rust,no_run
use std::sync::Arc;
use winprint_kit::{PrintPipeline, PrintStatus, StatusSink};

struct Sink;
impl StatusSink for Sink {
    fn on_status(&self, id: &str, status: PrintStatus, error: Option<String>) {
        println!("[{id}] {status:?} {error:?}");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread().build()?;
    rt.block_on(async {
        let pipeline = PrintPipeline::new();
        pipeline.list_printers().await?;

        let sink = Arc::new(Sink);
        pipeline
            .print_document(
                "job-1",
                "https://example.com/file.pdf",
                "Microsoft Print to PDF",
                &Default::default(),
                sink,
            )
            .await;
        Ok(())
    })
}
```

Print progress is reported via `StatusSink::on_status` (`Printing` → `Success` / `Failed`; `error` is non-empty only on failure). The event sink is implemented by the caller (e.g. tauri `emit`).

## Environment checks

HTML / Office depend on local components; probe them in advance:

```rust
use winprint_kit::html::webview2_available;
use winprint_kit::office::{is_office_installed, office_install_status, OfficeKind};

let wv2 = webview2_available();
let status = office_install_status().await;
let has_word = is_office_installed(OfficeKind::Word).await;
```

A failed probe is treated as not installed.

## Notes

The WebView2 user-data folder used for HTML printing can be set with `PrintPipeline::with_html_user_data_folder(...)`; defaults to `%LOCALAPPDATA%\winprint-kit\webview2_print`.

