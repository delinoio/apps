// SPDX-License-Identifier: Apache-2.0
use std::{
    io::{self, IsTerminal},
    sync::atomic::Ordering,
};
fn main() {
    let mut runtime = lane::runtime::Runtime::default();
    let cancelled = runtime.auth.cancelled.clone();
    if ctrlc::set_handler(move || cancelled.store(true, Ordering::Relaxed)).is_err() {
        eprintln!("Unable to install termination handler.");
        std::process::exit(1);
    }
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none())
        .init();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let code = lane::cli::run(
        &args,
        &mut runtime,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    std::process::exit(code);
}
