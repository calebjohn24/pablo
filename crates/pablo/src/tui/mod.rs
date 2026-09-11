//! Reference terminal client; every task uses the same run function as the CLI.
mod view;
use std::{
    io::IsTerminal,
    sync::{Arc, Mutex},
};
pub type Shared = Arc<Mutex<view::View>>;
pub fn available() -> bool {
    cfg!(unix)
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}
#[cfg(not(unix))]
pub async fn serve(_: crate::config::Options) -> Result<std::process::ExitCode, String> {
    Err("TUI requires a supported Unix terminal".into())
}
#[cfg(unix)]
mod terminal;
#[cfg(unix)]
pub use terminal::serve;
