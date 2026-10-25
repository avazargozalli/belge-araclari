//! `belge-core udf-to-docx <output.docx> <input.udf>…` — the macOS app runs this to write Word
//! files. Several inputs are merged into one document, one section each. Errors go to stderr.

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
        [flag] if flag == "--version" => {
            println!("belge-core {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("Kullanım: belge-core udf-to-docx <çıktı.docx> <girdi.udf>…");
            ExitCode::from(2)
        }
    }
}
