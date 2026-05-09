mod commands;
mod tui;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use facegate_core::config::{Config, DEFAULT_CONFIG_PATH};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "facegate",
    about = "Facegate facial authentication tool for Linux\n\nRun without arguments to open the interactive menu.",
    version
)]
struct Cli {
    #[arg(long, default_value = DEFAULT_CONFIG_PATH, global = true)]
    config: std::path::PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Interactive TUI configuration editor
    Configure,
    /// Run diagnostics on the installation
    Doctor,
    /// Test camera capture and face detection
    CameraTest {
        #[arg(long)]
        device: Option<String>,
    },
    /// Enroll a face for a user (requires root)
    Add {
        username: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// List enrolled templates for a user
    List { username: String },
    /// Remove an enrolled template (requires root)
    Remove { username: String, id: u32 },
    /// Live test authentication for a user (requires root)
    Test { username: String },
    /// Authenticate a user — used internally by the PAM module
    #[command(hide = true)]
    Auth {
        #[arg(long)]
        user: String,
    },
    /// Print shell completion script to stdout
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

fn main() {
    let cli = Cli::parse();

    if let Some(Command::Completions { shell }) = cli.command {
        generate(
            shell,
            &mut Cli::command(),
            "facegate",
            &mut std::io::stdout(),
        );
        return;
    }

    // Auth is called internally by the PAM module (already root). Every other
    // command touches sensitive face data or system config, so we require root.
    if !matches!(&cli.command, Some(Command::Auth { .. })) {
        // SAFETY: geteuid() is always safe to call.
        if unsafe { libc::geteuid() } != 0 {
            eprintln!("Error: facegate must be run as root (e.g. sudo facegate).");
            std::process::exit(1);
        }
    }

    let config = match load_config_or_default(
        &cli.config,
        matches!(&cli.command, Some(Command::Auth { .. })),
    ) {
        Ok(c) => c,
        Err(code) => std::process::exit(code as i32),
    };

    init_logging(&config.logging.level);
    let config_path = cli.config.clone();

    match cli.command {
        None => {
            // No subcommand → interactive TUI loop.
            // The menu loop only exits to open the config TUI (returns true)
            // or when the user quits (returns false).
            let mut config = config;
            loop {
                match tui::main_menu::run(&config) {
                    Err(e) => {
                        eprintln!("TUI error: {e}");
                        return;
                    }
                    Ok(false) => return,
                    Ok(true) => {
                        // User selected Configure → open config TUI then re-enter menu.
                        let exit =
                            commands::configure::run_from_menu(config.clone(), config_path.clone());
                        match exit {
                            Ok(tui::app::ConfigureExit::Back) => {
                                match load_config_or_default(&config_path, false) {
                                    Ok(new_config) => config = new_config,
                                    Err(_) => return,
                                }
                            }
                            Ok(tui::app::ConfigureExit::Quit) => return,
                            Err(e) => {
                                eprintln!("TUI error: {e}");
                                return;
                            }
                        }
                    }
                }
            }
        }
        Some(cmd) => {
            if let Err(e) = run_command(cmd, config, config_path) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn load_config_or_default(
    path: &std::path::Path,
    auth_mode: bool,
) -> Result<Config, facegate_core::error::AuthExitCode> {
    let config = match Config::load(path) {
        Ok(c) => c,
        Err(e) => {
            if auth_mode {
                eprintln!("Config error: {e}");
                return Err(facegate_core::error::AuthExitCode::ConfigError);
            }
            eprintln!("Warning: {e} — using default config.");
            Config::default()
        }
    };
    Ok(config)
}

fn run_command(
    cmd: Command,
    config: Config,
    config_path: std::path::PathBuf,
) -> anyhow::Result<()> {
    match cmd {
        Command::Configure => commands::configure::run(config, config_path),
        Command::Doctor => commands::doctor::run(&config),
        Command::CameraTest { device } => commands::camera_test::run(&config, device.as_deref()),
        Command::Add { username, label } => {
            commands::add::run(&config, &username, label.as_deref())
        }
        Command::List { username } => commands::list::run(&config, &username),
        Command::Remove { username, id } => commands::remove::run(&config, &username, id),
        Command::Test { username } => commands::test::run(&config, &username),
        Command::Auth { user } => {
            std::process::exit(commands::auth::run(&config, &user) as i32);
        }
        Command::Completions { .. } => unreachable!(),
    }
}

fn init_logging(level: &str) {
    let default_filter = format!("{level},ort=warn,ort::logging=warn");
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default_filter))
        .unwrap_or_else(|_| EnvFilter::new("info,ort=warn,ort::logging=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
