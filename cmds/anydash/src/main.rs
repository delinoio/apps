// SPDX-License-Identifier: Apache-2.0
use std::io::IsTerminal as _;
fn main() {
    let settings = anydash::settings::Settings::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = anydash::cli::parse(&args).unwrap_or_default();
    let foreground = parsed.words.first().is_some_and(|w| w == "run")
        && !parsed.switches.iter().any(|s| s == "service")
        && parsed.flags.get("owner").is_none_or(|o| o == "foreground");
    anydash::logging::init(
        &settings.dirs().logs,
        anydash::cli::log_color(&parsed, &settings),
        foreground,
    );
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        std::process::exit(1);
    };
    let mut env = anydash::cli::Env {
        settings,
        endpoints: anydash::providers::Endpoints::default(),
        os: std::sync::Arc::new(anydash::keys::OsStore),
        exe: std::env::current_exe().unwrap_or_else(|_| "anydash".into()),
        runner: Box::new(anydash::service::System),
        platform: anydash::service::Platform::current(),
        open_browser: true,
    };
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut input = std::io::BufReader::new(std::io::stdin());
    let tty = out.is_terminal();
    let code = runtime.block_on(async {
        let mut io = anydash::cli::Io {
            out: &mut out,
            err: &mut err,
            input: &mut input,
            tty,
        };
        anydash::cli::run(&args, &mut env, &mut io).await
    });
    std::process::exit(code);
}
