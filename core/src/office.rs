//! Office documents → PDF through an office suite already installed on the computer.
//!
//! Microsoft Office is used for its own formats when present (COM automation through PowerShell on
//! Windows, AppleScript on macOS), LibreOffice otherwise (headless, on both). Nothing is downloaded
//! and nothing leaves the computer. When no suite can do it, the caller falls back to the built-in
//! Word reader ([`crate::word`]) for .docx files.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// File extensions treated as office documents.
pub const EXTENSIONS: &[&str] = &[
    "doc", "docx", "docm", "dot", "dotx", "dotm", "rtf", "odt", "ott", "wpd", "xls", "xlsx", "xlsm", "xlsb", "xlt", "xltx", "ods", "ots", "csv", "ppt", "pptx", "pptm",
    "pps", "ppsx", "pot", "potx", "odp", "otp",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Word,
    Excel,
    PowerPoint,
}

impl Kind {
    pub fn app_name(self) -> &'static str {
        match self {
            Kind::Word => "Microsoft Word",
            Kind::Excel => "Microsoft Excel",
            Kind::PowerPoint => "Microsoft PowerPoint",
        }
    }
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub fn is_office(path: &Path) -> bool {
    EXTENSIONS.contains(&extension(path).as_str())
}

/// Files the built-in converter can read.
pub fn is_builtin(path: &Path) -> bool {
    matches!(extension(path).as_str(), "docx" | "docm" | "dotx" | "dotm")
}

pub fn kind(path: &Path) -> Option<Kind> {
    Some(match extension(path).as_str() {
        "doc" | "docx" | "docm" | "dot" | "dotx" | "dotm" | "rtf" | "odt" | "ott" | "wpd" => Kind::Word,
        "xls" | "xlsx" | "xlsm" | "xlsb" | "xlt" | "xltx" | "ods" | "ots" | "csv" => Kind::Excel,
        "ppt" | "pptx" | "pptm" | "pps" | "ppsx" | "pot" | "potx" | "odp" | "otp" => Kind::PowerPoint,
        _ => return None,
    })
}

#[derive(Debug)]
pub enum ExternalError {
    /// No installed suite can open this file.
    Unavailable,
    /// A suite was found but the conversion failed (message is user-facing).
    Failed(String),
}

const TIMEOUT: Duration = Duration::from_secs(240);

/// Runs a command with a time limit. Returns stdout, or stderr as the error.
fn run(mut command: Command) -> Result<(i32, String, String), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Dönüştürme zaman aşımına uğradı.".into());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok((out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

fn scratch() -> Result<PathBuf, String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("belge-araclari-{}-{n}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

fn place(produced: &Path, output: &Path) -> Result<(), String> {
    if !produced.exists() || std::fs::metadata(produced).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err("PDF oluşturulamadı.".into());
    }
    std::fs::rename(produced, output).or_else(|_| std::fs::copy(produced, output).map(|_| ())).map_err(|_| format!("\"{}\" kaydedilemedi.", output.display()))
}

// MARK: - LibreOffice

pub fn libreoffice() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![];
    if cfg!(target_os = "macos") {
        candidates.push("/Applications/LibreOffice.app/Contents/MacOS/soffice".into());
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join("Applications/LibreOffice.app/Contents/MacOS/soffice"));
        }
    } else if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
            if let Some(dir) = std::env::var_os(var) {
                candidates.push(PathBuf::from(dir).join("LibreOffice\\program\\soffice.exe"));
            }
        }
    } else {
        candidates.extend(["/usr/bin/soffice", "/usr/local/bin/soffice", "/opt/libreoffice/program/soffice"].map(PathBuf::from));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// file:// URL for LibreOffice's -env:UserInstallation.
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if text.starts_with('/') { "file://" } else { "file:///" });
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"/._-~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn via_libreoffice(soffice: &Path, input: &Path, output: &Path) -> Result<(), String> {
    let dir = scratch()?;
    let result = (|| {
        // A private profile, so a LibreOffice window the user has open does not block us.
        let mut cmd = Command::new(soffice);
        cmd.arg(format!("-env:UserInstallation={}", file_url(&dir.join("profile"))))
            .args(["--headless", "--norestore", "--nolockcheck", "--nodefault", "--nologo", "--convert-to", "pdf", "--outdir"])
            .arg(dir.join("out"))
            .arg(input);
        let (_, _, err) = run(cmd)?;
        let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        place(&dir.join("out").join(format!("{stem}.pdf")), output).map_err(|e| if err.trim().is_empty() { e } else { format!("{e} ({})", err.trim()) })
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

// MARK: - Microsoft Office on Windows

#[cfg(windows)]
const POWERSHELL: &str = r#"
$ErrorActionPreference = 'Stop'
$in = $env:BELGE_IN; $out = $env:BELGE_OUT; $kind = $env:BELGE_KIND
$progId = @{ word = 'Word.Application'; excel = 'Excel.Application'; powerpoint = 'PowerPoint.Application' }[$kind]
try { $app = New-Object -ComObject $progId } catch { exit 3 }
$code = 0
try {
  switch ($kind) {
    'word' {
      $app.Visible = $false; $app.DisplayAlerts = 0
      # A dummy password makes protected files fail instead of waiting for a password prompt.
      $d = $app.Documents.Open($in, $false, $true, $false, 'belge-x')
      try { $d.ExportAsFixedFormat($out, 17) } finally { $d.Close(0) }
      if ($app.Documents.Count -eq 0) { $app.Quit() }
    }
    'excel' {
      $app.Visible = $false; $app.DisplayAlerts = $false
      $b = $app.Workbooks.Open($in, 0, $true, [Type]::Missing, 'belge-x')
      try { $b.ExportAsFixedFormat(0, $out) } finally { $b.Close($false) }
      if ($app.Workbooks.Count -eq 0) { $app.Quit() }
    }
    'powerpoint' {
      $p = $app.Presentations.Open($in, -1, 0, 0)
      try { $p.SaveAs($out, 32) } finally { $p.Close() }
      if ($app.Presentations.Count -eq 0) { $app.Quit() }
    }
  }
} catch {
  [Console]::Error.WriteLine($_.Exception.Message); $code = 1
} finally {
  [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($app)
}
exit $code
"#;

#[cfg(windows)]
fn via_microsoft(kind: Kind, input: &Path, output: &Path) -> Result<(), ExternalError> {
    use base64::Engine as _;
    let script: Vec<u8> = POWERSHELL.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let dir = scratch().map_err(ExternalError::Failed)?;
    let produced = dir.join("out.pdf");
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
        .arg(base64::engine::general_purpose::STANDARD.encode(script))
        .env("BELGE_IN", input)
        .env("BELGE_OUT", &produced)
        .env("BELGE_KIND", match kind {
            Kind::Word => "word",
            Kind::Excel => "excel",
            Kind::PowerPoint => "powerpoint",
        });
    let result = match run(cmd) {
        Ok((3, _, _)) => Err(ExternalError::Unavailable),
        Ok((0, _, _)) => place(&produced, output).map_err(ExternalError::Failed),
        Ok((_, _, err)) => Err(ExternalError::Failed(first_line(&err, kind))),
        Err(e) => Err(ExternalError::Failed(e)),
    };
    let _ = std::fs::remove_dir_all(&dir);
    result
}

// MARK: - Microsoft Office on macOS

#[cfg(target_os = "macos")]
fn via_microsoft(kind: Kind, input: &Path, output: &Path) -> Result<(), ExternalError> {
    let (bundle, container) = match kind {
        Kind::Word => ("Microsoft Word", "com.microsoft.Word"),
        Kind::Excel => ("Microsoft Excel", "com.microsoft.Excel"),
        Kind::PowerPoint => ("Microsoft PowerPoint", "com.microsoft.Powerpoint"),
    };
    if !Path::new(&format!("/Applications/{bundle}.app")).exists() {
        return Err(ExternalError::Unavailable);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or(ExternalError::Unavailable)?;
    // Office for Mac is sandboxed: working inside its own container avoids "grant access" prompts.
    let dir = home.join(format!("Library/Containers/{container}/Data/BelgeAraclari-{}", rand_suffix()));
    std::fs::create_dir_all(&dir).map_err(|e| ExternalError::Failed(e.to_string()))?;
    let name = format!("belge-{}.{}", rand_suffix() % 1_000_000_000, extension(input));
    let copy = dir.join(&name);
    let produced = dir.join("out.pdf");
    let result = (|| {
        std::fs::copy(input, &copy).map_err(|e| ExternalError::Failed(e.to_string()))?;
        // Opening through Launch Services skips Office's start screen, which blocks scripting.
        let was_running = Command::new("/usr/bin/osascript")
            .args(["-e", &format!("application \"{bundle}\" is running")])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
            .unwrap_or(false);
        let mut open = Command::new("/usr/bin/open");
        open.args(["-g", "-a", bundle]).arg(&copy);
        if !matches!(run(open), Ok((0, _, _))) {
            return Err(ExternalError::Unavailable);
        }
        let (collection, save) = match kind {
            Kind::Word => ("document", "save as document docName file name outHFS file format format PDF"),
            Kind::Excel => ("workbook", "save workbook as workbook docName filename outHFS file format PDF file format"),
            Kind::PowerPoint => ("presentation", "save presentation docName in outHFS as save as PDF"),
        };
        let script = format!(
            r#"on run argv
	set {{outPath, docName, wasRunning}} to argv
	-- Converted outside the tell block: Office cannot coerce a POSIX path held in a variable.
	set outHFS to (POSIX file outPath) as text
	tell application "{bundle}"
		-- The document is listed before it has finished loading; retry the save until it works.
		set lastError to "zaman aşımı"
		repeat 480 times
			try
				if exists {collection} docName then
					{save}
					set lastError to ""
					exit repeat
				end if
			on error errorText
				set lastError to errorText
			end try
			delay 0.25
		end repeat
		try
			close {collection} docName saving no
		end try
		if (count of {collection}s) is 0 and wasRunning is "false" then quit saving no
	end tell
	if lastError is not "" then error lastError
end run"#
        );
        let mut cmd = Command::new("/usr/bin/osascript");
        cmd.arg("-e").arg(script).arg(&produced).arg(&name).arg(was_running.to_string());
        match run(cmd) {
            Ok((0, _, _)) => {}
            Ok((_, _, err)) if err.contains("-1743") => {
                return Err(ExternalError::Failed(format!(
                    "Belge Araçları'nın {bundle} uygulamasını kullanmasına izin verilmedi. Sistem Ayarları › Gizlilik ve Güvenlik › Otomasyon bölümünden izin verebilirsiniz."
                )))
            }
            Ok((_, _, err)) => return Err(ExternalError::Failed(first_line(&err, kind))),
            Err(e) => return Err(ExternalError::Failed(e)),
        }
        // Word sometimes reports success a moment before the file is complete.
        for _ in 0..40 {
            if produced.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        place(&produced, output).map_err(ExternalError::Failed)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(not(any(windows, target_os = "macos")))]
fn via_microsoft(_: Kind, _: &Path, _: &Path) -> Result<(), ExternalError> {
    Err(ExternalError::Unavailable)
}

fn first_line(err: &str, kind: Kind) -> String {
    let line = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.is_empty() { format!("{} dosyayı PDF'e çeviremedi.", kind.app_name()) } else { format!("{}: {line}", kind.app_name()) }
}

// MARK: - Entry point

/// Converts with the best installed suite. Returns the name of the program that did it.
pub fn convert_external(input: &Path, output: &Path) -> Result<&'static str, ExternalError> {
    let kind = kind(input).ok_or(ExternalError::Unavailable)?;
    let mut failure = None;
    // .odt/.ods/.odp and friends open best in LibreOffice; Microsoft formats in Microsoft Office.
    let open_document = extension(input).starts_with('o');
    if !open_document || libreoffice().is_none() {
        match via_microsoft(kind, input, output) {
            Ok(()) => return Ok(kind.app_name()),
            Err(ExternalError::Failed(e)) => failure = Some(e),
            Err(ExternalError::Unavailable) => {}
        }
    }
    if let Some(soffice) = libreoffice() {
        match via_libreoffice(&soffice, input, output) {
            Ok(()) => return Ok("LibreOffice"),
            Err(e) => failure = Some(failure.map_or(e.clone(), |f| format!("{f} / LibreOffice: {e}"))),
        }
    }
    Err(failure.map_or(ExternalError::Unavailable, ExternalError::Failed))
}

/// Office suites found on this computer, for the settings screen.
pub fn installed() -> Vec<&'static str> {
    let mut out = vec![];
    #[cfg(target_os = "macos")]
    for kind in [Kind::Word, Kind::Excel, Kind::PowerPoint] {
        if Path::new(&format!("/Applications/{}.app", kind.app_name())).exists() {
            out.push(kind.app_name());
        }
    }
    #[cfg(windows)]
    for (kind, key) in [(Kind::Word, "Word.Application"), (Kind::Excel, "Excel.Application"), (Kind::PowerPoint, "PowerPoint.Application")] {
        let mut cmd = Command::new("reg.exe");
        cmd.args(["query", &format!("HKCR\\{key}\\CLSID")]);
        if matches!(run(cmd), Ok((0, _, _))) {
            out.push(kind.app_name());
        }
    }
    if libreoffice().is_some() {
        out.push("LibreOffice");
    }
    out
}
