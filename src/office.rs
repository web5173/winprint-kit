//! Office → PDF conversion (PowerShell + COM, requires MS Office 2010+).

use anyhow::{anyhow, Result};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const CREATE_NO_WINDOW: u32 = 0x08000000;
const CONVERT_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeKind {
    Word,
    Excel,
    PowerPoint,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeStatus {
    pub word: bool,
    pub excel: bool,
    pub powerpoint: bool,
}

pub async fn office_install_status() -> OfficeStatus {
    const SCRIPT: &str = r#"
$w = [type]::GetTypeFromProgID('Word.Application')
$e = [type]::GetTypeFromProgID('Excel.Application')
$p = [type]::GetTypeFromProgID('PowerPoint.Application')
Write-Output ('{0},{1},{2}' -f ($null -ne $w),($null -ne $e),($null -ne $p))
"#;

    let child = match Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            SCRIPT,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return OfficeStatus::default(),
    };

    let output = match timeout(
        Duration::from_secs(CONVERT_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        _ => return OfficeStatus::default(),
    };

    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let mut parts = line.split(',');
    let parse = |s: Option<&str>| s.map(|v| v.trim().eq_ignore_ascii_case("true")).unwrap_or(false);
    OfficeStatus {
        word: parse(parts.next()),
        excel: parse(parts.next()),
        powerpoint: parse(parts.next()),
    }
}

pub async fn is_office_installed(kind: OfficeKind) -> bool {
    let status = office_install_status().await;
    match kind {
        OfficeKind::Word => status.word,
        OfficeKind::Excel => status.excel,
        OfficeKind::PowerPoint => status.powerpoint,
    }
}

pub async fn convert_office_to_pdf(input_path: &str, output_path: &str) -> Result<()> {
    let ps_script = build_powershell_script(input_path, output_path)?;

    let child = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &ps_script,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("Failed to start PowerShell: {}", e))?;

    let output = timeout(
        Duration::from_secs(CONVERT_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "Conversion timed out after {} seconds",
            CONVERT_TIMEOUT_SECS
        )
    })?
    .map_err(|e| anyhow!("Process execution failed: {}", e))?;

    if output.status.success() && std::path::Path::new(output_path).exists() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!(
            "Conversion failed (exit code: {:?})\n{}",
            output.status.code(),
            stderr.trim()
        ))
    }
}

fn escape_path_for_powershell(path: &str) -> String {
    path.replace('\'', "''")
}

fn build_powershell_script(input: &str, output: &str) -> Result<String> {
    let ext = std::path::Path::new(input)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .ok_or_else(|| anyhow!("Unable to get file extension: {}", input))?;

    let input_escaped = escape_path_for_powershell(input);
    let output_escaped = escape_path_for_powershell(output);

    let script = match ext.as_str() {
        "doc" | "docx" | "odt" => {
            format!(
                r#"
                $ErrorActionPreference = "Stop"
                $word = $null
                $doc = $null
                try {{
                    $word = New-Object -ComObject Word.Application
                    $word.Visible = $false
                    $word.DisplayAlerts = 0
                    $word.FeatureInstall = 2
                    $word.AutomationSecurity = 3
                    $doc = $word.Documents.Open('{0}', $false, $true)
                    $doc.ExportAsFixedFormat('{1}', 17)
                }}
                catch {{
                    Write-Error $_.Exception.Message
                    throw
                }}
                finally {{
                    if ($doc -ne $null) {{
                        $doc.Close()
                        [System.Runtime.Interopservices.Marshal]::ReleaseComObject($doc) | Out-Null
                    }}
                    if ($word -ne $null) {{
                        $word.Quit()
                        [System.Runtime.Interopservices.Marshal]::ReleaseComObject($word) | Out-Null
                    }}
                    [System.GC]::Collect()
                    [System.GC]::WaitForPendingFinalizers()
                }}
                "#,
                input_escaped, output_escaped
            )
        }
        "xls" | "xlsx" | "ods" => {
            format!(
                r#"
                $ErrorActionPreference = "Stop"
                $excel = $null
                $wb = $null
                try {{
                    $excel = New-Object -ComObject Excel.Application
                    $excel.Visible = $false
                    $excel.DisplayAlerts = $false
                    $excel.FeatureInstall = 2
                    $excel.AutomationSecurity = 3
                    $wb = $excel.Workbooks.Open('{0}', $false, $true)
                    $wb.ExportAsFixedFormat(0, '{1}')
                }}
                catch {{
                    Write-Error $_.Exception.Message
                    throw
                }}
                finally {{
                    if ($wb -ne $null) {{
                        $wb.Close()
                        [System.Runtime.Interopservices.Marshal]::ReleaseComObject($wb) | Out-Null
                    }}
                    if ($excel -ne $null) {{
                        $excel.Quit()
                        [System.Runtime.Interopservices.Marshal]::ReleaseComObject($excel) | Out-Null
                    }}
                    [System.GC]::Collect()
                    [System.GC]::WaitForPendingFinalizers()
                }}
                "#,
                input_escaped, output_escaped
            )
        }
        "ppt" | "pptx" | "odp" => {
            format!(
                r#"
                $ErrorActionPreference = "Stop"
                $ppt = $null
                $pres = $null
                $origWindowState = $null
                $origSecurity = $null
                try {{
                    $ppt = New-Object -ComObject PowerPoint.Application
                    $origSecurity = $ppt.AutomationSecurity
                    $ppt.AutomationSecurity = 3
                    $origWindowState = $ppt.WindowState
                    $ppt.WindowState = 2
                    $pres = $ppt.Presentations.Open('{0}', 0, 0, 0)
                    try {{
                        if ($ppt.Windows.Count -gt 0) {{
                            $helper = 'using System; using System.Runtime.InteropServices; public class Win32 {{ [DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr hWnd, int nCmdShow); }}'
                            Add-Type -TypeDefinition $helper
                            $hwnd = $ppt.Windows.Item(1).Hwnd
                            if ($hwnd -and $hwnd -ne 0) {{
                                [Win32]::ShowWindow([IntPtr]$hwnd, 0) | Out-Null
                            }}
                        }}
                    }} catch {{
                    }}
                    $pres.SaveAs('{1}', 32)
                }}
                catch {{
                    Write-Error $_.Exception.Message
                    throw
                }}
                finally {{
                    if ($pres -ne $null) {{
                        $pres.Close()
                        [System.Runtime.Interopservices.Marshal]::ReleaseComObject($pres) | Out-Null
                    }}
                    if ($ppt -ne $null) {{
                        try {{ $ppt.WindowState = $origWindowState }} catch {{ }}
                        try {{ $ppt.AutomationSecurity = $origSecurity }} catch {{ }}
                        if ($ppt.Presentations.Count -eq 0) {{
                            $ppt.Quit()
                        }}
                        [System.Runtime.Interopservices.Marshal]::ReleaseComObject($ppt) | Out-Null
                    }}
                    [System.GC]::Collect()
                    [System.GC]::WaitForPendingFinalizers()
                }}
                "#,
                input_escaped, output_escaped
            )
        }

        _ => return Err(anyhow!("Unsupported file format: {}", ext)),
    };

    Ok(script)
}
