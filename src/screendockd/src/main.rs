//! `screendockd` binary entry point; the command line lives in `screendockd::cli`.

fn main() -> anyhow::Result<()> {
    screendockd::cli::run()
}
