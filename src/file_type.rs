//! Detect file types from URL or Content-Type.

use url::Url;

const SUPPORTED_TYPES: &[&str] = &[
    "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp", "jpg", "jpeg", "png",
    "gif", "bmp", "tif", "tiff", "webp", "html", "htm",
    "xps",
];

const BOUNDARY_CHARS: &[u8] = b"?&#/\"' ;:";

fn validate_extension(ext: &str) -> Option<String> {
    let ext = ext.trim().to_lowercase();
    if ext.is_empty() || ext.len() > 10 {
        return None;
    }
    SUPPORTED_TYPES.contains(&ext.as_str()).then_some(ext)
}

fn is_boundary(s: &str, pos: usize) -> bool {
    pos >= s.len() || BOUNDARY_CHARS.contains(&s.as_bytes()[pos])
}

fn extract_ext_from_filename(filename: &str) -> Option<String> {
    let dot_pos = filename.rfind('.')?;
    validate_extension(&filename[dot_pos + 1..])
}

pub fn detect_type_from_url(url: &str) -> Option<String> {
    if let Ok(parsed) = Url::parse(url) {
        if let Some(filename) = parsed.path_segments().and_then(|mut segs| segs.next_back()) {
            if let Some(ext) = extract_ext_from_filename(filename) {
                return Some(ext);
            }
        }

        for (_key, value) in parsed.query_pairs() {
            if let Some(ext) = extract_ext_from_filename(&value) {
                return Some(ext);
            }
        }
    }

    let url_lower = url.to_lowercase();

    SUPPORTED_TYPES.iter().find_map(|ext| {
        [format!(".{}", ext), format!("%2e{}", ext)]
            .into_iter()
            .find_map(|pattern| {
                url_lower.find(&pattern).and_then(|pos| {
                    is_boundary(&url_lower, pos + pattern.len()).then(|| ext.to_string())
                })
            })
    })
}

pub fn detect_type_from_content_type(content_type: &str) -> Option<String> {
    let ct = content_type.split(';').next()?.trim().to_lowercase();
    let extension = match ct.as_str() {
        "application/pdf" => "pdf",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/vnd.ms-powerpoint" => "ppt",
        "application/vnd.oasis.opendocument.text" => "odt",
        "application/vnd.oasis.opendocument.spreadsheet" => "ods",
        "application/vnd.oasis.opendocument.presentation" => "odp",
        "application/vnd.ms-xpsdocument" => "xps",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/webp" => "webp",
        "text/html" => "html",
        "application/xhtml+xml" => "htm",
        _ => return None,
    };
    Some(extension.to_string())
}

pub fn is_supported_type(file_type: &str) -> bool {
    SUPPORTED_TYPES.contains(&file_type)
}

pub fn is_office_type(typ: &str) -> bool {
    matches!(
        typ,
        "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp"
    )
}

pub fn is_image_type(typ: &str) -> bool {
    matches!(
        typ,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tiff" | "tif" | "webp"
    )
}

pub fn is_html_type(typ: &str) -> bool {
    matches!(typ, "html" | "htm")
}
