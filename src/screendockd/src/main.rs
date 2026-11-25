//! `screendockd` binary entry point; the command line lives in `screendockd::cli`.

fn main() -> anyhow::Result<std::process::ExitCode> {
    screendockd::cli::run()
}
