//! The `ratchet` binary — a thin wrapper over the library.

fn main() -> std::process::ExitCode {
    let code = ratchet::run(std::env::args_os());
    std::process::ExitCode::from(code as u8)
}
