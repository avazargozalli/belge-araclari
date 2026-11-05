//! Command-line front end the macOS app bundles. Errors go to stderr.
//! - `belge-core udf-to-docx <output.docx> <input.udf>…` writes Word files; several inputs are merged
//!   into one document, one section each.
//! - `belge-core docx-to-udf <input.docx> <output.udf>` is the built-in Word reader; the app then
//!   draws the UDF as a PDF.
//! - `belge-core to-udf <input> <output.udf> [--no-office]` converts a PDF or Word document to UDF.
//! - `belge-core office-to-pdf <input> <output.pdf>` uses an installed Microsoft Office or
//!   LibreOffice; exit code 3 means none can open the file. `office-engines` lists what is installed.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, output, inputs @ ..] if command == "udf-to-docx" && !inputs.is_empty() => {
            let inputs: Vec<PathBuf> = inputs.iter().map(PathBuf::from).collect();
            match belge_core::udf::docx::convert(&inputs, &PathBuf::from(output)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        [command, input, output, rest @ ..] if command == "to-udf" => {
            let use_office = !rest.iter().any(|a| a == "--no-office");
            match belge_core::convert::to_udf(&PathBuf::from(input), &PathBuf::from(output), use_office) {
                Ok(how) => {
                    println!("{how}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        // pages-to-udf <output.udf> <image> <width pt> <height pt> …
        [command, output, rest @ ..] if command == "pages-to-udf" && !rest.is_empty() && rest.len() % 3 == 0 => {
            let pages: Result<Vec<_>, String> = rest
                .chunks(3)
                .map(|c| Ok((std::fs::read(&c[0]).map_err(|e| e.to_string())?, c[1].parse::<f32>().map_err(|e| e.to_string())?, c[2].parse::<f32>().map_err(|e| e.to_string())?)))
                .collect();
            match pages.and_then(|p| belge_core::pdf::pages_to_udf(&p, &PathBuf::from(output))) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        // number-pages <file.pdf> <bottom-right|top-left|…> <n|n/N|sayfa-n|sayfa-n/N|dash> [start]
        [command, file, position, format, rest @ ..] if command == "number-pages" && rest.len() <= 1 => {
            let start = rest.first().and_then(|s| s.parse().ok()).unwrap_or(1);
            let Some(numbering) = belge_core::pdf::numbers::Numbering::parse(position, format, start) else {
                eprintln!("Sayfa numarası konumu veya biçimi anlaşılamadı.");
                return ExitCode::from(2);
            };
            match belge_core::pdf::numbers::stamp(&PathBuf::from(file), &numbering) {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        [command, input, output] if command == "docx-to-udf" => match belge_core::word::docx_to_udf(&PathBuf::from(input), &PathBuf::from(output)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        // Exit code 3: no installed office suite can convert this file.
        [command, input, output] if command == "office-to-pdf" => {
            match belge_core::office::convert_external(&PathBuf::from(input), &PathBuf::from(output)) {
                Ok(engine) => {
                    println!("{engine}");
                    ExitCode::SUCCESS
                }
                Err(belge_core::office::ExternalError::Unavailable) => ExitCode::from(3),
                Err(belge_core::office::ExternalError::Failed(e)) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        [command] if command == "office-engines" => {
            for name in belge_core::office::installed() {
                println!("{name}");
            }
            ExitCode::SUCCESS
        }
        [command, input] if command == "udf-check" => {
            let xml = std::fs::File::open(input).ok().and_then(|f| zip::ZipArchive::new(f).ok()).and_then(|mut z| {
                use std::io::Read;
                let mut s = String::new();
                z.by_name("content.xml").ok()?.read_to_string(&mut s).ok()?;
                Some(s)
            });
            let problems = xml.map_or_else(|| vec!["content.xml okunamadı".to_string()], |x| belge_core::udfout::validate(&x));
            for p in &problems {
                println!("{p}");
            }
            if problems.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        [command, input] if command == "pdf-dump" => match std::fs::read(input).map_err(|e| e.to_string()).and_then(|b| belge_core::pdf::dump(&b)) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        [flag] if flag == "--version" => {
            println!("belge-core {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("Kullanım:\n  belge-core udf-to-docx <çıktı.docx> <girdi.udf>…\n  belge-core docx-to-udf <girdi.docx> <çıktı.udf>\n  belge-core to-udf <girdi.pdf|.docx|.doc> <çıktı.udf> [--no-office]\n  belge-core number-pages <dosya.pdf> <bottom-right|…> <n|n/N|sayfa-n|sayfa-n/N|dash> [başlangıç]\n  belge-core office-to-pdf <girdi> <çıktı.pdf>\n  belge-core office-engines");
            ExitCode::from(2)
        }
    }
}
