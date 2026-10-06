//! Compiler-backed siox language server. Stdout belongs exclusively to LSP.

mod analysis;
mod protocol;
mod server;
mod text;

use std::{io, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut std_root = PathBuf::from("./std");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stdio" => {}
            "--std" => match args.next() {
                Some(path) => std_root = path.into(),
                None => {
                    eprintln!("siox-lsp: --std needs a directory");
                    return ExitCode::from(2);
                }
            },
            "-h" | "--help" => {
                println!("siox-lsp\n\nUSAGE: siox-lsp [--stdio] [--std <dir>]\n\nRequires matching core/ and std/ compiler libraries; no LLVM is needed.");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("siox-lsp: unknown argument `{other}`");
                return ExitCode::from(2);
            }
        }
    }
    if std_root.join("std/prelude.siox").is_file() {
        std_root = std_root.join("std");
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    match server::Server::new(std_root).run(&mut stdin.lock(), &mut stdout.lock()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("siox-lsp: {error}");
            ExitCode::FAILURE
        }
    }
}
