//! Command-line front end the macOS app bundles. Errors go to stderr.
//! - `belge-core udf-to-docx <output.docx> <input.udf>…` writes Word files; several inputs are merged
//!   into one document, one section each.
//! - `belge-core docx-to-udf <input.docx> <output.udf>` is the built-in Word reader; the app then
//!   draws the UDF as a PDF.
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
        [flag] if flag == "--version" => {
            println!("belge-core {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("Kullanım:\n  belge-core udf-to-docx <çıktı.docx> <girdi.udf>…\n  belge-core docx-to-udf <girdi.docx> <çıktı.udf>\n  belge-core office-to-pdf <girdi> <çıktı.pdf>\n  belge-core office-engines");
            ExitCode::from(2)
        }
    }
}
