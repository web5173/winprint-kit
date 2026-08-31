//! Print options, capability probing, and print ticket building.

use winprint::printer::PrinterDevice;
use winprint::ticket::{
    Copies, FeatureOptionPack, PrintCapabilities, PrintTicket, PrintTicketBuilder,
};

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PrintOptions {
    #[serde(default = "default_copies")]
    pub copies: u16,
    pub page_size: Option<String>,
    pub duplex: Option<String>,
    pub color: Option<String>,
    pub orientation: Option<String>,
    pub resolution: Option<String>,
}

impl Default for PrintOptions {
    fn default() -> Self {
        Self {
            copies: 1,
            page_size: None,
            duplex: None,
            color: None,
            orientation: None,
            resolution: None,
        }
    }
}

fn default_copies() -> u16 {
    1
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PageSizeDetail {
    pub display_name: String,
    pub width_inches: f64,
    pub height_inches: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterCapabilities {
    pub page_size: Vec<String>,
    pub duplex: Vec<String>,
    pub color: Vec<String>,
    pub orientation: Vec<String>,
    pub resolution: Vec<String>,
    pub page_sizes: Vec<PageSizeDetail>,
}

pub(crate) fn extract_capabilities(caps: &PrintCapabilities) -> PrinterCapabilities {
    PrinterCapabilities {
        page_size: caps
            .page_media_sizes()
            .filter_map(|o| o.display_name().map(|s| s.to_string()))
            .collect(),
        duplex: caps
            .duplexes()
            .filter_map(|o| o.display_name().map(|s| s.to_string()))
            .collect(),
        color: caps
            .page_output_colors()
            .filter_map(|o| o.display_name().map(|s| s.to_string()))
            .collect(),
        orientation: caps
            .page_orientations()
            .filter_map(|o| o.display_name().map(|s| s.to_string()))
            .collect(),
        resolution: caps
            .page_resolutions()
            .filter_map(|o| o.display_name().map(|s| s.to_string()))
            .collect(),
        page_sizes: caps
            .page_media_sizes()
            .filter_map(|o| {
                let name = o.display_name()?;
                let size = o.size();
                Some(PageSizeDetail {
                    display_name: name.to_string(),
                    width_inches: size.width_in_micron() as f64 / 25400.0,
                    height_inches: size.height_in_micron() as f64 / 25400.0,
                })
            })
            .collect(),
    }
}

pub(crate) fn build_print_ticket(
    device: &PrinterDevice,
    capabilities: &PrintCapabilities,
    options: &PrintOptions,
) -> Result<PrintTicket, String> {
    if options.copies == 0 {
        return Err("copies must be greater than 0".to_string());
    }
    let mut builder = PrintTicketBuilder::new(device)
        .map_err(|e| format!("Failed to create print ticket builder: {}", e))?;

    builder
        .merge(Copies(options.copies))
        .map_err(|e| format!("Failed to set copies: {}", e))?;

    if let Some(ref input) = options.page_size {
        let items: Vec<_> = capabilities.page_media_sizes().collect();
        let option = find_option(&items, input).ok_or_else(|| {
            format!(
                "Printer does not support paper size '{}'. Supported: {}",
                input,
                list_display_names(&items)
            )
        })?;
        builder
            .merge(option)
            .map_err(|e| format!("Failed to set page size: {}", e))?;
    }

    if let Some(ref input) = options.duplex {
        let items: Vec<_> = capabilities.duplexes().collect();
        let option = find_option(&items, input).ok_or_else(|| {
            format!(
                "Printer does not support duplex '{}'. Supported: {}",
                input,
                list_display_names(&items)
            )
        })?;
        builder
            .merge(option)
            .map_err(|e| format!("Failed to set duplex: {}", e))?;
    }

    if let Some(ref input) = options.color {
        let items: Vec<_> = capabilities.page_output_colors().collect();
        let option = find_option(&items, input).ok_or_else(|| {
            format!(
                "Printer does not support color mode '{}'. Supported: {}",
                input,
                list_display_names(&items)
            )
        })?;
        builder
            .merge(option)
            .map_err(|e| format!("Failed to set color: {}", e))?;
    }

    if let Some(ref input) = options.orientation {
        let items: Vec<_> = capabilities.page_orientations().collect();
        let option = find_option(&items, input).ok_or_else(|| {
            format!(
                "Printer does not support orientation '{}'. Supported: {}",
                input,
                list_display_names(&items)
            )
        })?;
        builder
            .merge(option)
            .map_err(|e| format!("Failed to set orientation: {}", e))?;
    }

    if let Some(ref input) = options.resolution {
        let items: Vec<_> = capabilities.page_resolutions().collect();
        let option = find_option(&items, input).ok_or_else(|| {
            format!(
                "Printer does not support resolution '{}'. Supported: {}",
                input,
                list_display_names(&items)
            )
        })?;
        builder
            .merge(option)
            .map_err(|e| format!("Failed to set resolution: {}", e))?;
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build print ticket: {}", e))
}

fn find_option<T: FeatureOptionPack + Clone>(options: &[T], input: &str) -> Option<T> {
    let normalized = input.trim().to_lowercase();
    options
        .iter()
        .find(|o| {
            o.display_name()
                .map(|d| d.trim().to_lowercase() == normalized)
                .unwrap_or(false)
        })
        .cloned()
}

fn list_display_names<T: FeatureOptionPack>(options: &[T]) -> String {
    options
        .iter()
        .filter_map(|o| o.display_name())
        .collect::<Vec<_>>()
        .join(", ")
}
