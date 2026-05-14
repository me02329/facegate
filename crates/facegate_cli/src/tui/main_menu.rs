use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame, Terminal,
};
use std::{io, time::Duration};

use facegate_core::config::Config;

use crate::commands;

// ── Public API ────────────────────────────────────────────────────────────────

/// Run the interactive main menu.
///
/// Returns `Ok(true)` when the user selects **Configure** (caller opens the
/// config TUI then calls this again).  Returns `Ok(false)` when the user quits.
pub fn run(config: &Config, config_path: &std::path::Path) -> Result<bool> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(config, config_path);
    let result = event_loop(&mut terminal, &mut app);

    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    result
}

// ── Menu items ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Configure,
    Doctor,
    SudoToggle,
    SessionToggle,
    WatchToggle,
    CameraTest,
    Cameras,
    Enroll,
    AddSudo,
    AddSession,
    AddBoth,
    List,
    Test,
}

#[derive(Clone, Copy, PartialEq)]
enum ItemKind {
    /// Selectable action.
    Action,
    /// Selectable quit item.
    Quit,
    /// Group header — not selectable, rendered as "── Title ──".
    Section,
}

struct MenuItem {
    icon: &'static str,
    label: &'static str,
    description: String,
    action: Option<Action>,
    needs_user: bool,
    kind: ItemKind,
}

fn section(title: &'static str) -> MenuItem {
    MenuItem {
        icon: "",
        label: title,
        description: String::new(),
        action: None,
        needs_user: false,
        kind: ItemKind::Section,
    }
}

fn build_items(sudo_enabled: bool, session_enabled: bool, watch_active: bool) -> Vec<MenuItem> {
    let sudo_label: &'static str = if sudo_enabled {
        "Disable Sudo Auth"
    } else {
        "Enable Sudo Auth"
    };
    let session_label: &'static str = if session_enabled {
        "Disable Session Auth"
    } else {
        "Enable Session Auth"
    };
    let sudo_desc = if sudo_enabled {
        "Remove face auth from sudo/su    (root)"
    } else {
        "Add face scan before sudo prompt  (root)"
    };
    let session_desc = if session_enabled {
        "Remove face auth from login & screen unlock"
    } else {
        "Add face auth at login & screen unlock"
    };
    let watch_label: &'static str = if watch_active {
        "Stop Watch Daemon"
    } else {
        "Start Watch Daemon"
    };
    let watch_desc = if watch_active {
        "● Running — auto-unlock screen lock on face match"
    } else {
        "○ Stopped — enable auto-unlock (Windows Hello style)"
    };
    vec![
        section("Authentication"),
        MenuItem {
            icon: "+ ",
            label: "Enroll",
            description: "Register a user's face for authentication".into(),
            action: Some(Action::Enroll),
            needs_user: true,
            kind: ItemKind::Action,
        },
        MenuItem {
            icon: "⊞ ",
            label: sudo_label,
            description: sudo_desc.into(),
            action: Some(Action::SudoToggle),
            needs_user: false,
            kind: ItemKind::Action,
        },
        MenuItem {
            icon: "▣ ",
            label: session_label,
            description: session_desc.into(),
            action: Some(Action::SessionToggle),
            needs_user: false,
            kind: ItemKind::Action,
        },
        MenuItem {
            icon: "◎ ",
            label: watch_label,
            description: watch_desc.into(),
            action: Some(Action::WatchToggle),
            needs_user: false,
            kind: ItemKind::Action,
        },
        section("Templates"),
        MenuItem {
            icon: "= ",
            label: "Templates",
            description: "Browse & delete enrolled face templates".into(),
            action: Some(Action::List),
            needs_user: true,
            kind: ItemKind::Action,
        },
        MenuItem {
            icon: "~ ",
            label: "Test Recognition",
            description: "Live match test for a user         (root)".into(),
            action: Some(Action::Test),
            needs_user: true,
            kind: ItemKind::Action,
        },
        section("Hardware"),
        MenuItem {
            icon: "▤ ",
            label: "List Cameras",
            description: "Detect /dev/video* devices and recommend an IR cam".into(),
            action: Some(Action::Cameras),
            needs_user: false,
            kind: ItemKind::Action,
        },
        MenuItem {
            icon: "◉ ",
            label: "Camera Test",
            description: "Live capture + face detection on the configured camera".into(),
            action: Some(Action::CameraTest),
            needs_user: false,
            kind: ItemKind::Action,
        },
        section("System"),
        MenuItem {
            icon: "✓ ",
            label: "Doctor",
            description: "Verify libs, PAM config & model files".into(),
            action: Some(Action::Doctor),
            needs_user: false,
            kind: ItemKind::Action,
        },
        MenuItem {
            icon: "⚙ ",
            label: "Configure",
            description: "Edit thresholds, camera & model settings".into(),
            action: Some(Action::Configure),
            needs_user: false,
            kind: ItemKind::Action,
        },
        section("Exit"),
        MenuItem {
            icon: "x ",
            label: "Quit",
            description: "Exit Facegate".into(),
            action: None,
            needs_user: false,
            kind: ItemKind::Quit,
        },
    ]
}

// ── App state ─────────────────────────────────────────────────────────────────

struct SessionEntry {
    name: &'static str,
    service: &'static str,
    path: String,
    keep: bool,
}

struct TemplateEntry {
    id: u32,
    label: String,
    created_at: String,
    scope: &'static str,
    marked: bool,
}

#[derive(PartialEq)]
enum InputMode {
    Menu,
    UsernameInput,
    SampleCountInput,
    PamServiceInput,
    EnrollTargetSelect,
    SessionServiceSelect,
    TemplateList,
}

enum PanelState {
    /// Idle — show welcome text.
    Idle,
    /// A command is running in a background thread.
    Running {
        rx: Receiver<String>,
        lines: Vec<String>,
        tick: u8,
    },
    /// Command finished — show full output, wait for Enter.
    Done {
        lines: Vec<String>,
        scroll: usize,
        had_error: bool,
    },
}

struct App<'a> {
    config: &'a Config,
    config_path: std::path::PathBuf,
    items: Vec<MenuItem>,
    selected: usize,
    input_mode: InputMode,
    username_buf: String,
    sample_count_buf: String,
    pam_service_buf: String,
    enroll_sudo: bool,
    enroll_session: bool,
    enroll_cursor: usize,
    session_entries: Vec<SessionEntry>,
    session_cursor: usize,
    template_username: String,
    template_entries: Vec<TemplateEntry>,
    template_cursor: usize,
    pending: Option<Action>,
    pending_username: Option<String>,
    panel: PanelState,
    sudo_enabled: bool,
    session_enabled: bool,
    watch_active: bool,
    /// When set, the loop exits and main re-opens the config TUI.
    open_config: bool,
    should_quit: bool,
}

impl<'a> App<'a> {
    fn new(config: &'a Config, config_path: &std::path::Path) -> Self {
        let sudo_enabled = commands::sudo_toggle::is_enabled();
        let session_enabled = commands::session_toggle::is_enabled();
        let watch_active = commands::watch_toggle::is_active();
        let items = build_items(sudo_enabled, session_enabled, watch_active);
        // First section header sits at index 0; start the cursor on the first
        // selectable action after it.
        let selected = items
            .iter()
            .position(|item| item.kind != ItemKind::Section)
            .unwrap_or(0);
        App {
            config,
            config_path: config_path.to_path_buf(),
            items,
            selected,
            input_mode: InputMode::Menu,
            username_buf: String::new(),
            sample_count_buf: String::new(),
            pam_service_buf: String::new(),
            enroll_sudo: true,
            enroll_session: false,
            enroll_cursor: 0,
            session_entries: Vec::new(),
            session_cursor: 0,
            template_username: String::new(),
            template_entries: Vec::new(),
            template_cursor: 0,
            pending: None,
            pending_username: None,
            panel: PanelState::Idle,
            sudo_enabled,
            session_enabled,
            watch_active,
            open_config: false,
            should_quit: false,
        }
    }

    fn is_section(&self, i: usize) -> bool {
        self.items[i].kind == ItemKind::Section
    }
    fn is_quit(&self, i: usize) -> bool {
        self.items[i].kind == ItemKind::Quit
    }

    fn move_up(&mut self) {
        loop {
            self.selected = if self.selected == 0 {
                self.items.len() - 1
            } else {
                self.selected - 1
            };
            if !self.is_section(self.selected) {
                break;
            }
        }
    }
    fn move_down(&mut self) {
        loop {
            self.selected = (self.selected + 1) % self.items.len();
            if !self.is_section(self.selected) {
                break;
            }
        }
    }

    fn enter_menu(&mut self) {
        if self.is_quit(self.selected) {
            self.should_quit = true;
            return;
        }
        if let Some(action) = self.items[self.selected].action.clone() {
            if action == Action::Configure {
                self.open_config = true;
                return;
            }
            if action == Action::SessionToggle {
                if self.session_enabled {
                    // Show which services are enabled so the user can uncheck to disable
                    self.session_entries = commands::session_toggle::enabled_service_entries()
                        .into_iter()
                        .map(|(name, service, path)| SessionEntry {
                            name,
                            service,
                            path,
                            keep: true,
                        })
                        .collect();
                    self.session_cursor = 0;
                    self.input_mode = InputMode::SessionServiceSelect;
                } else {
                    // Enabling — offer a custom PAM service in case auto-detection misses it
                    self.pending = Some(action);
                    self.pam_service_buf.clear();
                    self.input_mode = InputMode::PamServiceInput;
                }
                return;
            }
            if self.items[self.selected].needs_user {
                self.pending = Some(action);
                self.username_buf.clear();
                self.input_mode = InputMode::UsernameInput;
            } else {
                self.launch(action, None, 1, None);
            }
        }
    }

    fn confirm_username(&mut self) {
        let name = self.username_buf.trim().to_owned();
        if name.is_empty() {
            return;
        }
        if let Some(action) = self.pending.clone() {
            if action == Action::Enroll {
                self.pending_username = Some(name);
                self.enroll_sudo = true;
                self.enroll_session = false;
                self.enroll_cursor = 0;
                self.input_mode = InputMode::EnrollTargetSelect;
            } else if matches!(
                action,
                Action::AddSudo | Action::AddSession | Action::AddBoth
            ) {
                self.pending_username = Some(name);
                self.sample_count_buf = "3".to_owned();
                self.input_mode = InputMode::SampleCountInput;
            } else if action == Action::List {
                self.pending = None;
                self.username_buf.clear();
                self.input_mode = InputMode::Menu;
                match commands::list::load_templates(self.config, &name) {
                    Ok(ts) if ts.is_empty() => {
                        self.panel = PanelState::Done {
                            lines: vec![format!("No enrolled templates for '{name}'.")],
                            scroll: 0,
                            had_error: false,
                        };
                    }
                    Ok(ts) => {
                        self.template_username = name;
                        self.template_entries = ts
                            .into_iter()
                            .map(|t| TemplateEntry {
                                id: t.id,
                                label: t.label,
                                created_at: t.created_at,
                                scope: t.scope.label(),
                                marked: false,
                            })
                            .collect();
                        self.template_cursor = 0;
                        self.input_mode = InputMode::TemplateList;
                    }
                    Err(e) => {
                        self.panel = PanelState::Done {
                            lines: vec![format!("\nError: {e}")],
                            scroll: 0,
                            had_error: true,
                        };
                    }
                }
            } else {
                self.pending = None;
                self.launch(action, Some(name), 1, None);
                self.input_mode = InputMode::Menu;
            }
        }
    }

    fn toggle_enroll_target(&mut self) {
        match self.enroll_cursor {
            0 => self.enroll_sudo = !self.enroll_sudo,
            1 => self.enroll_session = !self.enroll_session,
            2 => {
                let all = self.enroll_sudo && self.enroll_session;
                self.enroll_sudo = !all;
                self.enroll_session = !all;
            }
            _ => {}
        }
    }

    fn confirm_enroll_targets(&mut self) {
        if !self.enroll_sudo && !self.enroll_session {
            return;
        }
        let action = match (self.enroll_sudo, self.enroll_session) {
            (true, true) => Action::AddBoth,
            (true, false) => Action::AddSudo,
            (false, true) => Action::AddSession,
            _ => unreachable!(),
        };
        self.pending = Some(action);
        self.sample_count_buf = "3".to_owned();
        self.input_mode = InputMode::SampleCountInput;
    }

    fn confirm_sample_count(&mut self) {
        let s = self.sample_count_buf.trim().to_owned();
        let samples = if s.is_empty() {
            3
        } else {
            match s.parse::<u32>() {
                Ok(n) if (1..=10).contains(&n) => n,
                _ => return,
            }
        };
        if let (Some(action), Some(username)) = (self.pending.take(), self.pending_username.take())
        {
            self.launch(action, Some(username), samples, None);
            self.input_mode = InputMode::Menu;
        }
    }

    fn confirm_pam_service(&mut self) {
        let service = self.pam_service_buf.trim().to_owned();
        if let Some(action) = self.pending.take() {
            let pam_service = if service.is_empty() {
                None
            } else {
                Some(service)
            };
            self.launch(action, None, 1, pam_service);
            self.input_mode = InputMode::Menu;
        }
    }

    fn toggle_session_entry(&mut self) {
        if let Some(e) = self.session_entries.get_mut(self.session_cursor) {
            e.keep = !e.keep;
        }
    }

    fn confirm_session_services(&mut self) {
        self.input_mode = InputMode::Menu;
        let (tx, rx) = mpsc::channel::<String>();
        let entries: Vec<_> = std::mem::take(&mut self.session_entries)
            .into_iter()
            .filter(|e| !e.keep)
            .collect();

        thread::spawn(move || {
            if entries.is_empty() {
                let _ = tx.send("No changes.".to_owned());
                return;
            }
            let _ = tx.send("Session face authentication disabled.".to_owned());
            let _ = tx.send("".to_owned());
            let _ = tx.send("Removed from:".to_owned());
            for e in entries {
                match commands::pam_edit::set_service_enabled(e.service, false) {
                    Ok(_) => {
                        let _ = tx.send(format!("  {}: {}", e.name, e.path));
                    }
                    Err(err) => {
                        let _ = tx.send(format!("\nError: {}: {err}", e.name));
                    }
                }
            }
            let _ = tx.send("".to_owned());
            let _ = tx.send(format!("  {}", commands::pam_edit::PAM_LINE));
        });

        self.panel = PanelState::Running {
            rx,
            lines: Vec::new(),
            tick: 0,
        };
    }

    fn toggle_template_mark(&mut self) {
        if let Some(e) = self.template_entries.get_mut(self.template_cursor) {
            e.marked = !e.marked;
        }
    }

    fn confirm_template_delete(&mut self) {
        self.input_mode = InputMode::Menu;
        let mut ids: Vec<u32> = self
            .template_entries
            .iter()
            .filter(|e| e.marked)
            .map(|e| e.id)
            .collect();
        self.template_entries.clear();

        if ids.is_empty() {
            self.panel = PanelState::Done {
                lines: vec!["No templates deleted.".to_owned()],
                scroll: 0,
                had_error: false,
            };
            self.template_username.clear();
            return;
        }

        // Delete highest IDs first so lower IDs stay stable after each reassignment
        ids.sort_unstable_by(|a, b| b.cmp(a));

        let (tx, rx) = mpsc::channel::<String>();
        let username = std::mem::take(&mut self.template_username);

        thread::spawn(move || {
            let _ = tx.send(format!(
                "Removed {} template(s) for '{username}'.",
                ids.len()
            ));
            let _ = tx.send(String::new());
            for id in ids {
                match commands::broker::remove_template(&username, id) {
                    Ok(_) => {
                        let _ = tx.send(format!("  Removed template {id}"));
                    }
                    Err(e) => {
                        let _ = tx.send(format!("\nError: template {id}: {e}"));
                    }
                }
            }
        });

        self.panel = PanelState::Running {
            rx,
            lines: Vec::new(),
            tick: 0,
        };
    }

    fn cancel_input(&mut self) {
        self.input_mode = InputMode::Menu;
        self.pending = None;
        self.pending_username = None;
        self.username_buf.clear();
        self.sample_count_buf.clear();
        self.pam_service_buf.clear();
        self.enroll_sudo = true;
        self.enroll_session = false;
        self.enroll_cursor = 0;
        self.session_entries.clear();
        self.session_cursor = 0;
        self.template_entries.clear();
        self.template_username.clear();
        self.template_cursor = 0;
    }

    fn launch(
        &mut self,
        action: Action,
        username: Option<String>,
        samples: u32,
        pam_service: Option<String>,
    ) {
        let (tx, rx) = mpsc::channel::<String>();
        let config = self.config.clone();

        thread::spawn(move || {
            let extra: Vec<&str> = pam_service.as_deref().into_iter().collect();
            let result = match &action {
                Action::Doctor => commands::doctor::run_streaming(&config, None, &tx),
                Action::SudoToggle => {
                    commands::sudo_toggle::run_streaming(username.as_deref(), &tx)
                }
                Action::SessionToggle => {
                    commands::session_toggle::run_streaming(username.as_deref(), &extra, &[], &tx)
                }
                Action::WatchToggle => {
                    // enable if currently inactive, disable if active
                    let enable = !commands::watch_toggle::is_active();
                    commands::watch_toggle::run_streaming(enable, &tx)
                }
                Action::CameraTest => commands::camera_test::run_streaming(&config, None, &tx),
                Action::Cameras => commands::cameras::run_streaming(&tx),
                Action::AddSudo => commands::add::run_streaming(
                    &config,
                    username.as_deref(),
                    None,
                    samples,
                    false,
                    commands::add::EnrollmentTarget::Sudo,
                    &tx,
                ),
                Action::AddSession => commands::add::run_streaming(
                    &config,
                    username.as_deref(),
                    None,
                    samples,
                    false,
                    commands::add::EnrollmentTarget::Session,
                    &tx,
                ),
                Action::AddBoth => commands::add::run_streaming(
                    &config,
                    username.as_deref(),
                    None,
                    samples,
                    false,
                    commands::add::EnrollmentTarget::Both,
                    &tx,
                ),
                Action::List => commands::list::run_streaming(&config, username.as_deref(), &tx),
                Action::Test => commands::test::run_streaming(
                    &config,
                    username.as_deref(),
                    commands::test::TestScope::All,
                    &tx,
                ),
                Action::Configure | Action::Enroll => unreachable!(),
            };
            if let Err(e) = result {
                let _ = tx.send(format!("\nError: {e}"));
            }
            // tx dropped → rx sees Disconnected → panel switches to Done
        });

        self.panel = PanelState::Running {
            rx,
            lines: Vec::new(),
            tick: 0,
        };
    }

    /// Drain the channel. Returns true if we transitioned to Done.
    fn poll_channel(&mut self) {
        if let PanelState::Running { rx, lines, tick } = &mut self.panel {
            *tick = tick.wrapping_add(1);
            loop {
                match rx.try_recv() {
                    Ok(line) => lines.push(line),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let done_lines = std::mem::take(lines);
                        let had_error = done_lines
                            .iter()
                            .any(|l| l.starts_with("\nError:") || l.starts_with("Error:"));
                        self.panel = PanelState::Done {
                            lines: done_lines,
                            scroll: 0,
                            had_error,
                        };
                        return;
                    }
                }
            }
        }
    }

    fn scroll_output(&mut self, delta: i32) {
        if let PanelState::Done { scroll, .. } = &mut self.panel {
            *scroll = ((*scroll as i32) + delta).max(0) as usize;
        }
    }

    fn back_to_menu(&mut self) {
        if matches!(self.panel, PanelState::Done { .. }) {
            self.sudo_enabled = commands::sudo_toggle::is_enabled();
            self.session_enabled = commands::session_toggle::is_enabled();
            self.watch_active = commands::watch_toggle::is_active();
            self.items = build_items(self.sudo_enabled, self.session_enabled, self.watch_active);
            // The toggle items may have shifted; snap the cursor to a valid
            // selectable entry instead of trusting the stored index.
            if self.selected >= self.items.len() || self.is_section(self.selected) {
                self.selected = self
                    .items
                    .iter()
                    .position(|item| item.kind != ItemKind::Section)
                    .unwrap_or(0);
            }
            self.panel = PanelState::Idle;
        }
    }
}

// ── Event loop ────────────────────────────────────────────────────────────────

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<bool> {
    loop {
        terminal.draw(|f| render(f, app))?;
        app.poll_channel();

        if app.should_quit {
            return Ok(false);
        }
        if app.open_config {
            return Ok(true);
        }

        // Use a short timeout so we keep animating the spinner
        if event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                handle_key(app, key.code);
            }
        }
    }
}

fn handle_key(app: &mut App, code: KeyCode) {
    match app.input_mode {
        InputMode::UsernameInput => match code {
            KeyCode::Enter => app.confirm_username(),
            KeyCode::Esc => app.cancel_input(),
            KeyCode::Backspace => {
                app.username_buf.pop();
            }
            KeyCode::Char(c) => app.username_buf.push(c),
            _ => {}
        },
        InputMode::SampleCountInput => match code {
            KeyCode::Enter => app.confirm_sample_count(),
            KeyCode::Esc => app.cancel_input(),
            KeyCode::Backspace => {
                app.sample_count_buf.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() => app.sample_count_buf.push(c),
            _ => {}
        },
        InputMode::PamServiceInput => match code {
            KeyCode::Enter => app.confirm_pam_service(),
            KeyCode::Esc => app.cancel_input(),
            KeyCode::Backspace => {
                app.pam_service_buf.pop();
            }
            KeyCode::Char(c) => app.pam_service_buf.push(c),
            _ => {}
        },
        InputMode::SessionServiceSelect => match code {
            KeyCode::Up | KeyCode::Char('k') if app.session_cursor > 0 => {
                app.session_cursor -= 1;
            }
            KeyCode::Down | KeyCode::Char('j')
                if app.session_cursor + 1 < app.session_entries.len() =>
            {
                app.session_cursor += 1;
            }
            KeyCode::Char(' ') => app.toggle_session_entry(),
            KeyCode::Enter => app.confirm_session_services(),
            KeyCode::Esc => app.cancel_input(),
            _ => {}
        },
        InputMode::TemplateList => match code {
            KeyCode::Up | KeyCode::Char('k') if app.template_cursor > 0 => {
                app.template_cursor -= 1;
            }
            KeyCode::Down | KeyCode::Char('j')
                if app.template_cursor + 1 < app.template_entries.len() =>
            {
                app.template_cursor += 1;
            }
            KeyCode::Char(' ') | KeyCode::Char('d') => app.toggle_template_mark(),
            KeyCode::Enter => app.confirm_template_delete(),
            KeyCode::Esc => app.cancel_input(),
            _ => {}
        },
        InputMode::EnrollTargetSelect => match code {
            KeyCode::Up | KeyCode::Char('k') if app.enroll_cursor > 0 => {
                app.enroll_cursor -= 1;
            }
            KeyCode::Down | KeyCode::Char('j') if app.enroll_cursor < 2 => {
                app.enroll_cursor += 1;
            }
            KeyCode::Char(' ') => app.toggle_enroll_target(),
            KeyCode::Enter => app.confirm_enroll_targets(),
            KeyCode::Esc => app.cancel_input(),
            _ => {}
        },
        InputMode::Menu => {
            // In Done state, arrows scroll; Enter/Esc go back to menu
            if matches!(app.panel, PanelState::Done { .. }) {
                match code {
                    KeyCode::Up | KeyCode::Char('k') => app.scroll_output(-1),
                    KeyCode::Down | KeyCode::Char('j') => app.scroll_output(1),
                    KeyCode::PageUp => app.scroll_output(-10),
                    KeyCode::PageDown => app.scroll_output(10),
                    KeyCode::Enter | KeyCode::Esc => app.back_to_menu(),
                    KeyCode::Char('q') => app.should_quit = true,
                    _ => {}
                }
                return;
            }
            // In Running state, ignore *everything*. Quitting here would
            // detach the worker thread mid-write — it could keep editing PAM
            // files or downloading models with no UI to report progress.
            if matches!(app.panel, PanelState::Running { .. }) {
                return;
            }
            // Normal menu navigation
            match code {
                KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                KeyCode::Enter => app.enter_menu(),
                _ => {}
            }
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

const LOGO: &[&str] = &[
    " ███████╗ █████╗  ██████╗███████╗ ██████╗  █████╗ ████████╗███████╗",
    " ██╔════╝██╔══██╗██╔════╝██╔════╝██╔════╝ ██╔══██╗╚══██╔══╝██╔════╝",
    " █████╗  ███████║██║     █████╗  ██║  ███╗███████║   ██║   █████╗  ",
    " ██╔══╝  ██╔══██║██║     ██╔══╝  ██║   ██║██╔══██║   ██║   ██╔══╝  ",
    " ██║     ██║  ██║╚██████╗███████╗╚██████╔╝██║  ██║   ██║   ███████╗",
    " ╚═╝     ╚═╝  ╚═╝ ╚═════╝╚══════╝ ╚═════╝ ╚═╝  ╚═╝   ╚═╝   ╚══════╝",
];
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn render(f: &mut Frame, app: &App) {
    let root = Layout::vertical([
        Constraint::Length(9),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(f.area());

    render_header(f, root[0]);

    let panes =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).split(root[1]);

    render_menu(f, panes[0], app);
    render_panel(f, panes[1], app);
    render_footer(f, root[2], app);

    if app.input_mode == InputMode::UsernameInput {
        render_username_popup(f, app);
    }
    if app.input_mode == InputMode::SampleCountInput {
        render_sample_count_popup(f, app);
    }
    if app.input_mode == InputMode::PamServiceInput {
        render_pam_service_popup(f, app);
    }
    if app.input_mode == InputMode::EnrollTargetSelect {
        render_enroll_target_popup(f, app);
    }
    if app.input_mode == InputMode::SessionServiceSelect {
        render_session_service_popup(f, app);
    }
    if app.input_mode == InputMode::TemplateList {
        render_template_list_popup(f, app);
    }
}

fn render_header(f: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = LOGO
        .iter()
        .map(|row| {
            Line::from(Span::styled(
                *row,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    lines.push(Line::from(Span::styled(
        "  native facial authentication for Linux",
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn render_menu(f: &mut Frame, area: Rect, app: &App) {
    let panel_active = matches!(app.panel, PanelState::Idle);
    let border_color = if panel_active {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let items: Vec<ListItem> = app
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            if item.kind == ItemKind::Section {
                // "── Section Title ──" rendered in DarkGray bold so it
                // separates groups visibly without competing with the
                // selectable rows.
                let title_len = item.label.chars().count();
                let total = 22u16; // overall section-line width inside the panel
                let pad = (total as usize).saturating_sub(title_len + 2);
                let left = pad / 2;
                let right = pad - left;
                let line = format!(
                    "  {} {} {}",
                    "─".repeat(left.max(1)),
                    item.label,
                    "─".repeat(right.max(1)),
                );
                return ListItem::new(Line::from(Span::styled(
                    line,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            let sel = i == app.selected && panel_active;
            let (icon_s, label_s) = if sel {
                let hl = Style::default().fg(Color::Black).bg(Color::Cyan);
                let hl_b = Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD);
                (hl, hl_b)
            } else {
                (
                    Style::default().fg(Color::Cyan),
                    Style::default().fg(Color::White),
                )
            };
            let prefix = if sel { " ▶ " } else { "   " };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{prefix}{}", item.icon), icon_s),
                Span::styled(item.label, label_s),
            ]))
        })
        .collect();

    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(Span::styled(
                    " Facegate ",
                    Style::default()
                        .fg(border_color)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn render_panel(f: &mut Frame, area: Rect, app: &App) {
    let (title, border_color, content_lines, hint) = match &app.panel {
        PanelState::Idle => {
            let sel = &app.items[app.selected];
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}{}", sel.icon, sel.label),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            if sel.kind == ItemKind::Section || sel.description.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  Select an action from the menu.",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  {}", sel.description),
                    Style::default().fg(Color::White),
                )));
            }
            (" Info ", Color::DarkGray, lines, None)
        }
        PanelState::Running { lines, tick, .. } => {
            let spinner = SPINNER[(*tick as usize) % SPINNER.len()];
            let mut out: Vec<Line> = lines
                .iter()
                .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(Color::White))))
                .collect();
            out.push(Line::from(Span::styled(
                format!("{spinner} running…"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            (" Running… ", Color::Yellow, out, None)
        }
        PanelState::Done {
            lines,
            scroll,
            had_error,
        } => {
            let color = if *had_error { Color::Red } else { Color::Green };
            let title = if *had_error { " Error " } else { " Done " };
            let visible: Vec<Line> = lines
                .iter()
                .skip(*scroll)
                .map(|l| {
                    let c = if l.starts_with("  [✗]") || l.starts_with("Error") {
                        Color::Red
                    } else if l.starts_with("  [✓]") {
                        Color::Green
                    } else {
                        Color::White
                    };
                    Line::from(Span::styled(l.clone(), Style::default().fg(c)))
                })
                .collect();
            let hint = Some(Line::from(vec![
                Span::styled(
                    "[↑↓]",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" scroll   "),
                Span::styled(
                    "[Enter]",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" back to menu"),
            ]));
            (title, color, visible, hint)
        }
    };

    // Split panel: output area + optional hint bar
    let inner_layout = if hint.is_some() {
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area)
    } else {
        Layout::vertical([Constraint::Min(1), Constraint::Length(0)]).split(area)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(inner_layout[0]);
    f.render_widget(block, inner_layout[0]);
    f.render_widget(Paragraph::new(content_lines), inner);

    if let Some(hint_line) = hint {
        f.render_widget(
            Paragraph::new(hint_line)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray)),
            inner_layout[1],
        );
    }
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    let hints = match &app.panel {
        PanelState::Running { .. } => Line::from(vec![
            Span::styled(
                "[q]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  cancels are blocked while a command is running"),
        ]),
        _ => Line::from(vec![
            Span::styled(
                "[↑↓]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" navigate   "),
            Span::styled(
                "[Enter]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" select   "),
            Span::styled(
                "[q]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" quit"),
        ]),
    };
    let split = Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).split(area);
    f.render_widget(
        Paragraph::new(hints).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        split[0],
    );
    let cfg_line = Line::from(vec![
        Span::styled("config: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.config_path.display().to_string(),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(
        Paragraph::new(cfg_line)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray)),
        split[1],
    );
}

fn render_username_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(44, 36, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Username ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new("  Enter the username:").style(Style::default().fg(Color::DarkGray)),
        layout[1],
    );
    f.render_widget(
        Paragraph::new(format!(" {}_", app.username_buf))
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            ),
        layout[3],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "[Enter]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirm   "),
            Span::styled(
                "[Esc]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray)),
        layout[5],
    );
}

fn render_sample_count_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(44, 36, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Samples ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new("  How many samples to capture? (1–10)")
            .style(Style::default().fg(Color::DarkGray)),
        layout[1],
    );
    f.render_widget(
        Paragraph::new(format!(" {}_", app.sample_count_buf))
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            ),
        layout[3],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "[Enter]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirm   "),
            Span::styled(
                "[Esc]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray)),
        layout[5],
    );
}

fn render_template_list_popup(f: &mut Frame, app: &App) {
    let n = app.template_entries.len().max(1) as u16;
    let height = (n + 7).min(f.area().height.saturating_sub(4));

    let v = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .split(f.area());
    let area = Layout::horizontal([
        Constraint::Percentage(10),
        Constraint::Percentage(80),
        Constraint::Percentage(10),
    ])
    .split(v[1])[1];

    f.render_widget(Clear, area);

    let title = format!(" Templates — {} ", app.template_username);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let inner_layout = Layout::vertical([
        Constraint::Length(1), // padding
        Constraint::Length(1), // header row
        Constraint::Length(1), // separator
        Constraint::Min(1),    // rows
        Constraint::Length(1), // key hints
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "     ID   Scope    Created              Label",
            Style::default().fg(Color::DarkGray),
        )])),
        inner_layout[1],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            "     ".to_owned() + &"─".repeat(55),
            Style::default().fg(Color::DarkGray),
        )),
        inner_layout[2],
    );

    let mut rows: Vec<Line> = Vec::new();
    for (i, e) in app.template_entries.iter().enumerate() {
        let sel = i == app.template_cursor;
        let (fg, prefix) = if sel {
            (Color::Cyan, " ▶ ")
        } else {
            (Color::White, "   ")
        };
        let checkbox = if e.marked { "[x]" } else { "[ ]" };
        let created = e.created_at.get(..16).unwrap_or(&e.created_at);
        rows.push(Line::from(vec![
            Span::styled(
                format!("{prefix}{checkbox} "),
                Style::default()
                    .fg(if e.marked { Color::Red } else { fg })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<5} {:<8} {:<20} {}", e.id, e.scope, created, e.label),
                Style::default().fg(if e.marked {
                    Color::Red
                } else if sel {
                    Color::Cyan
                } else {
                    Color::White
                }),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(rows), inner_layout[3]);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "[d/Space]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" mark delete   "),
            Span::styled(
                "[Enter]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirm   "),
            Span::styled(
                "[Esc]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray)),
        inner_layout[4],
    );
}

fn render_session_service_popup(f: &mut Frame, app: &App) {
    let n = app.session_entries.len().max(1);
    let height = (n as u16 + 6).min(f.area().height.saturating_sub(4));
    let _area = centered_rect(60, 0, f.area()); // width only, height below
                                                // Build a vertically-centred rect of fixed height
    let v = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .split(f.area());
    let area = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Percentage(60),
        Constraint::Percentage(20),
    ])
    .split(v[1])[1];

    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Session Auth — enabled services ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let inner_layout = Layout::vertical([
        Constraint::Length(1), // padding
        Constraint::Length(1), // hint text
        Constraint::Length(1), // padding
        Constraint::Min(1),    // service list
        Constraint::Length(1), // key hints
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(Span::styled(
            "  Uncheck services to disable, then press Enter.",
            Style::default().fg(Color::DarkGray),
        )),
        inner_layout[1],
    );

    let mut list_lines: Vec<Line> = Vec::new();
    for (i, e) in app.session_entries.iter().enumerate() {
        let selected = i == app.session_cursor;
        let (fg, prefix) = if selected {
            (Color::Cyan, " ▶ ")
        } else {
            (Color::White, "   ")
        };
        let checkbox = if e.keep { "[x]" } else { "[ ]" };
        list_lines.push(Line::from(vec![
            Span::styled(
                format!("{prefix}{checkbox} "),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<24}", e.name),
                Style::default().fg(if selected { Color::Cyan } else { Color::White }),
            ),
            Span::styled(e.path.clone(), Style::default().fg(Color::DarkGray)),
        ]));
    }
    f.render_widget(Paragraph::new(list_lines), inner_layout[3]);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "[Space]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" toggle   "),
            Span::styled(
                "[Enter]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" save   "),
            Span::styled(
                "[Esc]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray)),
        inner_layout[4],
    );
}

fn render_enroll_target_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(50, 44, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Enroll — select targets ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let all_checked = app.enroll_sudo && app.enroll_session;
    let items = [
        (app.enroll_sudo, "Sudo", "face auth for sudo/su commands"),
        (
            app.enroll_session,
            "Session",
            "face auth at login & screen unlock",
        ),
        (all_checked, "All", "select both"),
    ];

    for (i, (checked, label, desc)) in items.iter().enumerate() {
        let row = layout[2 + i];
        let selected = app.enroll_cursor == i;
        let (fg, prefix) = if selected {
            (Color::Cyan, " ▶ ")
        } else {
            (Color::White, "   ")
        };
        let checkbox = if *checked { "[x]" } else { "[ ]" };
        let line = Line::from(vec![
            Span::styled(
                format!("{prefix}{checkbox} "),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{label:<10}", label = label),
                Style::default()
                    .fg(if selected { Color::Cyan } else { Color::White })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {desc}"), Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(Paragraph::new(line), row);
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "[Space]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" toggle   "),
            Span::styled(
                "[Enter]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirm   "),
            Span::styled(
                "[Esc]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray)),
        layout[7],
    );
}

fn render_pam_service_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(52, 40, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Custom PAM service (optional) ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new("  Extra PAM service to include (e.g. gdm3).\n  Leave empty to use auto-detection only.")
            .style(Style::default().fg(Color::DarkGray)),
        layout[1],
    );
    f.render_widget(
        Paragraph::new(format!(" {}_", app.pam_service_buf))
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            ),
        layout[3],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "[Enter]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirm   "),
            Span::styled(
                "[Esc]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray)),
        layout[5],
    );
}

fn centered_rect(px: u16, py: u16, r: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - py) / 2),
        Constraint::Percentage(py),
        Constraint::Percentage((100 - py) / 2),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage((100 - px) / 2),
        Constraint::Percentage(px),
        Constraint::Percentage((100 - px) / 2),
    ])
    .split(v[1])[1]
}
