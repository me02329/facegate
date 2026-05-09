use anyhow::{bail, Result};
use face_rs_core::config::Config;
use face_rs_core::storage::TemplateStore;

pub fn run(config: &Config, username: &str, id: u32) -> Result<()> {
    require_root()?;
    let store = TemplateStore::new(&config.storage.base_dir);
    store.remove_template(username, id)?;
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
