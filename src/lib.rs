//! # winprint-kit
//!
//! ```no_run
//! use std::sync::Arc;
//! use winprint_kit::{PrintPipeline, PrintStatus, StatusSink};
//!
//! struct Sink;
//! impl StatusSink for Sink {
//!     fn on_status(&self, id: &str, status: PrintStatus, error: Option<String>) {
//!         println!("[{id}] {status:?} {error:?}");
//!     }
//! }
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let rt = tokio::runtime::Builder::new_current_thread().build()?;
//!     rt.block_on(async {
//!         let pipeline = PrintPipeline::new();
//!         pipeline.list_printers().await?;
//!
//!         let sink = Arc::new(Sink);
//!         pipeline
//!             .print_document(
//!                 "job-1",
//!                 "https://example.com/file.pdf",
//!                 "Microsoft Print to PDF",
//!                 &Default::default(),
//!                 sink,
//!             )
//!             .await;
//!         Ok(())
//!     })
//! }
//! ```

pub mod file_type;
pub mod http;

#[cfg(feature = "html")]
pub mod html;
#[cfg(feature = "office")]
pub mod office;

mod download;
mod pipeline;
mod print;

pub use pipeline::{PrintPipeline, PrintStatus, StatusSink};
pub use print::{PageSizeDetail, PrintOptions, PrinterCapabilities};
