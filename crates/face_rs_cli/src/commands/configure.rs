use anyhow::Result;
use face_rs_core::config::Config;
use std::path::PathBuf;

pub fn run(config: Config, config_path: PathBuf) -> Result<()> {
    crate::tui::run_configure(config, config_path)
}
