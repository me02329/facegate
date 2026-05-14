mod commands;
mod tui;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
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
    /// Print a compact installation and enrollment summary
    Status,
    /// Show the current user's Facegate diagnostic log
    Logs {
        /// Number of recent log lines to print
        #[arg(long, default_value_t = 80)]
        lines: usize,
    },
    /// Restore PAM backups and stop Facegate services for emergency recovery
    EmergencyDisable {
        /// Print the rollback plan without changing files or services
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect or manage the Facegate broker daemon
    Broker {
        #[command(subcommand)]
        command: BrokerCommand,
    },
    /// List enrolled users and broker storage ownership state
    Users {
        /// Emit JSON for scripts
        #[arg(long)]
        json: bool,
    },
    /// Guided first-time setup flow
    Setup {
        /// User to enroll; defaults to SUDO_USER or USER
        username: Option<String>,
    },
    /// Run diagnostics on the installation
    Doctor,
    /// Test camera capture and face detection
    CameraTest {
        #[arg(long)]
        device: Option<String>,
    },
    /// List V4L2 cameras with format and IR/RGB hints
    Cameras,
    /// Enroll a face for a user (requires root)
    Add {
        username: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long = "for", value_enum, default_value_t = EnrollmentPurpose::Sudo)]
        purpose: EnrollmentPurpose,
    },
    /// Toggle face authentication for login/session PAM services
    SessionAuth {
        /// Extra PAM service name(s) to include beyond auto-detected ones (e.g. "gdm3")
        #[arg(long = "pam-service", value_name = "SERVICE")]
        pam_services: Vec<String>,
        /// Extra PAM file path(s) to include (e.g. "/etc/pam.d/custom")
        #[arg(long = "pam-file", value_name = "PATH")]
        pam_files: Vec<String>,
    },
    /// List enrolled templates for a user
    List { username: String },
    /// Remove an enrolled template (requires root)
    Remove { username: String, id: u32 },
    /// Remove ALL enrolled templates for a user (requires root)
    Forget {
        username: String,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Live test authentication for a user (requires root)
    Test {
        username: String,
        /// Which scope to match against (defaults to "all")
        #[arg(long = "for", value_enum, default_value_t = TestPurpose::All)]
        purpose: TestPurpose,
    },
    /// Capture positive samples and recommend a recognition threshold
    Calibrate {
        username: String,
        /// Which scope to calibrate (defaults to "session")
        #[arg(long = "for", value_enum, default_value_t = CalibrationPurpose::Session)]
        purpose: CalibrationPurpose,
        /// Number of positive samples to capture
        #[arg(long, default_value_t = 5)]
        samples: u32,
        /// Offer to write the recommended threshold to the config
        #[arg(long)]
        write: bool,
    },
    /// Calibrate RGB+IR camera alignment for dual-stream cross-check
    CalibrateCameras {
        /// Override the primary RGB camera device (defaults to camera.device)
        #[arg(long)]
        rgb_device: Option<String>,
        /// Override the IR camera device (defaults to camera.ir.device)
        #[arg(long)]
        ir_device: Option<String>,
        /// Number of accepted RGB+IR pairs to collect
        #[arg(long, default_value_t = 5)]
        samples: u32,
        /// Offer to write camera.device, [camera.ir].device, and homography to the config
        #[arg(long)]
        write: bool,
        /// Also enable [camera.cross_check] when writing the config
        #[arg(long)]
        enable: bool,
    },
    /// Authenticate a user — used internally by the PAM module
    #[command(hide = true)]
    Auth {
        #[arg(long)]
        user: String,
        #[arg(long)]
        service: Option<String>,
    },
    /// Watch for session lock events and unlock via face recognition (run as user)
    #[command(hide = true)]
    Watch,
    /// Print shell completion script to stdout
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EnrollmentPurpose {
    Sudo,
    Session,
    Both,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TestPurpose {
    All,
    Sudo,
    Session,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CalibrationPurpose {
    Sudo,
    Session,
}

#[derive(Debug, Subcommand)]
enum BrokerCommand {
    /// Show broker service, socket, audit, and storage status
    Status,
    /// Ping the broker over IPC
    Health,
    /// Restart facegate-brokerd.service
    Restart,
    /// Show broker journal logs
    Logs {
        /// Number of recent journal lines to print
        #[arg(long, default_value_t = 80)]
        lines: usize,
    },
    /// Re-apply broker-owned template/audit permissions
    RepairPermissions,
}

impl From<CalibrationPurpose> for facegate_core::storage::AuthScope {
    fn from(value: CalibrationPurpose) -> Self {
        match value {
            CalibrationPurpose::Sudo => facegate_core::storage::AuthScope::Sudo,
            CalibrationPurpose::Session => facegate_core::storage::AuthScope::Session,
        }
    }
}

impl From<TestPurpose> for commands::test::TestScope {
    fn from(value: TestPurpose) -> Self {
        use facegate_core::storage::AuthScope;
        match value {
            TestPurpose::All => commands::test::TestScope::All,
            TestPurpose::Sudo => commands::test::TestScope::Auth(AuthScope::Sudo),
            TestPurpose::Session => commands::test::TestScope::Auth(AuthScope::Session),
        }
    }
}

impl From<EnrollmentPurpose> for commands::add::EnrollmentTarget {
    fn from(value: EnrollmentPurpose) -> Self {
        match value {
            EnrollmentPurpose::Sudo => commands::add::EnrollmentTarget::Sudo,
            EnrollmentPurpose::Session => commands::add::EnrollmentTarget::Session,
            EnrollmentPurpose::Both => commands::add::EnrollmentTarget::Both,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let auth_mode = matches!(&cli.command, Some(Command::Auth { .. }));
    let watch_mode = matches!(&cli.command, Some(Command::Watch));
    let status_mode = matches!(&cli.command, Some(Command::Status));
    let logs_mode = matches!(&cli.command, Some(Command::Logs { .. }));
    let broker_unprivileged_mode = matches!(
        &cli.command,
        Some(Command::Broker {
            command: BrokerCommand::Status | BrokerCommand::Health | BrokerCommand::Logs { .. }
        }) | Some(Command::Users { .. })
    );
    // `cameras` only opens /dev/video* in read-only-ish ways; it should be
    // runnable as a normal user so people can discover their IR camera before
    // running anything privileged.
    let cameras_mode = matches!(&cli.command, Some(Command::Cameras));

    if let Some(Command::Completions { shell }) = cli.command {
        generate(
            shell,
            &mut Cli::command(),
            "facegate",
            &mut std::io::stdout(),
        );
        return;
    }

    // Auth, watch, status, logs, and the read-only `cameras` listing run as an
    // unprivileged user. Every other command touches sensitive face data or
    // system config, so we require root.
    if !auth_mode
        && !watch_mode
        && !status_mode
        && !logs_mode
        && !cameras_mode
        && !broker_unprivileged_mode
    {
        // SAFETY: geteuid() is always safe to call.
        if unsafe { libc::geteuid() } != 0 {
            eprintln!("Error: facegate must be run as root (e.g. sudo facegate).");
            std::process::exit(1);
        }
    }

    let config = match load_config(&cli.config, config_policy(&cli.command)) {
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
                match tui::main_menu::run(&config, &config_path) {
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
                                match load_config(&config_path, ConfigPolicy::DefaultOnError) {
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

#[derive(Debug, Clone, Copy)]
enum ConfigPolicy {
    Strict,
    StrictSilent,
    DefaultOnError,
}

fn config_policy(command: &Option<Command>) -> ConfigPolicy {
    match command {
        Some(Command::Auth { .. }) => ConfigPolicy::StrictSilent,
        Some(Command::Configure)
        | Some(Command::Doctor)
        | Some(Command::Status)
        | Some(Command::Broker { .. })
        | Some(Command::Users { .. })
        | Some(Command::EmergencyDisable { .. })
        | None => ConfigPolicy::DefaultOnError,
        // `cameras` does not need a config at all (it walks /dev/video*),
        // so don't fail if /etc/facegate/config.toml is missing.
        Some(Command::Watch) | Some(Command::Cameras) => ConfigPolicy::DefaultOnError,
        _ => ConfigPolicy::Strict,
    }
}

fn load_config(
    path: &std::path::Path,
    policy: ConfigPolicy,
) -> Result<Config, facegate_core::error::AuthExitCode> {
    let config = match Config::load(path) {
        Ok(c) => c,
        Err(e) => match policy {
            ConfigPolicy::Strict => {
                eprintln!("Config error: {e}");
                return Err(facegate_core::error::AuthExitCode::ConfigError);
            }
            ConfigPolicy::StrictSilent => {
                return Err(facegate_core::error::AuthExitCode::ConfigError);
            }
            ConfigPolicy::DefaultOnError => {
                eprintln!("Warning: {e} — using default config.");
                Config::default()
            }
        },
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
        Command::Status => commands::status::run(&config, &config_path),
        Command::Logs { lines } => commands::user_log::run(lines),
        Command::EmergencyDisable { dry_run } => commands::emergency_disable::run(dry_run),
        Command::Broker { command } => match command {
            BrokerCommand::Status => commands::broker_admin::status(&config),
            BrokerCommand::Health => commands::broker_admin::health(&config),
            BrokerCommand::Restart => commands::broker_admin::restart(),
            BrokerCommand::Logs { lines } => commands::broker_admin::logs(lines),
            BrokerCommand::RepairPermissions => commands::broker_admin::repair_permissions(&config),
        },
        Command::Users { json } => commands::users::run(json),
        Command::Setup { username } => commands::setup::run(config, config_path, username),
        Command::Doctor => commands::doctor::run(&config),
        Command::CameraTest { device } => commands::camera_test::run(&config, device.as_deref()),
        Command::Cameras => commands::cameras::run(),
        Command::Add {
            username,
            label,
            purpose,
        } => commands::add::run(&config, &username, label.as_deref(), purpose.into()),
        Command::SessionAuth {
            pam_services,
            pam_files,
        } => {
            let services: Vec<&str> = pam_services.iter().map(String::as_str).collect();
            let files: Vec<&str> = pam_files.iter().map(String::as_str).collect();
            commands::session_toggle::run(&services, &files)
        }
        Command::List { username } => commands::list::run(&config, &username),
        Command::Remove { username, id } => commands::remove::run(&config, &username, id),
        Command::Forget { username, yes } => commands::forget::run(&config, &username, yes),
        Command::Test { username, purpose } => {
            commands::test::run(&config, &username, purpose.into())
        }
        Command::Calibrate {
            username,
            purpose,
            samples,
            write,
        } => commands::calibrate::run(
            config,
            config_path,
            &username,
            purpose.into(),
            samples,
            write,
        ),
        Command::CalibrateCameras {
            rgb_device,
            ir_device,
            samples,
            write,
            enable,
        } => commands::calibrate_cameras::run(
            config,
            config_path,
            rgb_device.as_deref(),
            ir_device.as_deref(),
            samples,
            write,
            enable,
        ),
        Command::Auth { user, service } => {
            std::process::exit(commands::auth::run(&config, &user, service.as_deref()) as i32);
        }
        Command::Watch => commands::watch::run(config),
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
