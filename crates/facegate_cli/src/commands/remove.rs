use anyhow::{bail, Result};
use facegate_core::config::Config;

use crate::commands::broker;

pub fn run(config: &Config, username: &str, id: u32) -> Result<()> {
    let _ = config;
    require_root()?;
    broker::remove_template(username, id)?;
    println!("Removed template {id} for user '{username}'.");
    Ok(())
}

fn require_root() -> Result<()> {
    // SAFETY: getuid() has no preconditions.
    if unsafe { libc::getuid() } != 0 {
        bail!("this command requires root privileges (run with sudo)");
    }
    Ok(())
}
