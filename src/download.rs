//! Download files to a temp directory and detect their type.

use crate::file_type;
use crate::http;
use std::io::Write;
use tempfile::{Builder as TempFileBuilder, TempPath};

pub(crate) async fn download_and_detect(url: &str) -> Result<(TempPath, String), String> {
    const MAX_DOWNLOAD_SIZE: u64 = 200 * 1024 * 1024;

    let mut response = http::client()
        .get(url)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;
    if !response.status().is_success() {
        return Err(format!("HTTP error: {}", response.status().as_u16()));
    }

    if let Some(len) = response.content_length() {
        if len > MAX_DOWNLOAD_SIZE {
            return Err(format!(
                "File too large: {}MB (max 200MB)",
                len / (1024 * 1024)
            ));
        }
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let mut file_type = file_type::detect_type_from_url(url)
        .or_else(|| file_type::detect_type_from_content_type(content_type));

    let first_chunk = response
        .chunk()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let is_pdf = first_chunk
        .as_deref()
        .map(|b| b.starts_with(b"%PDF"))
        .unwrap_or(false);

    if is_pdf {
        file_type = Some("pdf".to_string());
    }

    let file_type = file_type.ok_or_else(|| "Unable to determine file type".to_string())?;

    if !file_type::is_supported_type(&file_type) {
        return Err(format!("Unsupported file type: {}", file_type));
    }
    if file_type == "pdf" && !is_pdf {
        return Err("File is not a valid PDF".to_string());
    }

    let mut temp_file = TempFileBuilder::new()
        .suffix(&format!(".{}", file_type))
        .tempfile()
        .map_err(|e| format!("Failed to create temporary file: {}", e))?;

    let mut total: u64 = 0;
    if let Some(chunk) = first_chunk {
        if chunk.len() as u64 > MAX_DOWNLOAD_SIZE {
            return Err(format!(
                "File too large: {}MB (max 200MB)",
                chunk.len() / (1024 * 1024)
            ));
        }
        temp_file
            .write_all(&chunk)
            .map_err(|e| format!("Failed to write to temporary file: {}", e))?;
        total = chunk.len() as u64;
    }

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?
    {
        total += chunk.len() as u64;
        if total > MAX_DOWNLOAD_SIZE {
            return Err(format!(
                "File too large: {}MB (max 200MB)",
                total / (1024 * 1024)
            ));
        }
        temp_file
            .write_all(&chunk)
            .map_err(|e| format!("Failed to write to temporary file: {}", e))?;
    }

    temp_file
        .flush()
        .map_err(|e| format!("Failed to flush temporary file: {}", e))?;

    let path = temp_file.into_temp_path();

    Ok((path, file_type))
}
