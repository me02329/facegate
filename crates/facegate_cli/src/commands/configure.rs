use anyhow::Result;
use facegate_core::config::Config;
use std::path::PathBuf;

pub fn run(config: Config, config_path: PathBuf) -> Result<()> {
    crate::tui::run_configure(config, config_path).map(|_| ())
}

pub fn run_from_menu(
    config: Config,
    config_path: PathBuf,
) -> Result<crate::tui::app::ConfigureExit> {
    crate::tui::run_configure(config, config_path)
}
