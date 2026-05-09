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
pub fn run(config: &Config) -> Result<bool> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(config);
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
    CameraTest,
    Add,
    List,
    Test,
}

struct MenuItem {
    icon: &'static str,
    label: &'static str,
    description: String,
    action: Option<Action>,
    needs_user: bool,
}

fn build_items(sudo_enabled: bool) -> Vec<MenuItem> {
    let sudo_desc = if sudo_enabled {
        "Disable sudo face authentication  (root)"
    } else {
        "Enable sudo face authentication   (root)"
    };
    vec![
        MenuItem {
            icon: "⚙ ",
            label: "Configure",
            description: "Edit settings".into(),
            action: Some(Action::Configure),
            needs_user: false,
        },
        MenuItem {
            icon: "✓ ",
            label: "Doctor",
            description: "Check installation status".into(),
            action: Some(Action::Doctor),
            needs_user: false,
        },
        MenuItem {
            icon: "⊞ ",
            label: "Sudo Auth",
            description: sudo_desc.into(),
            action: Some(Action::SudoToggle),
            needs_user: false,
        },
        MenuItem {
            icon: "◉ ",
            label: "Camera Test",
            description: "Test camera and face detection".into(),
            action: Some(Action::CameraTest),
            needs_user: false,
        },
        MenuItem {
            icon: "+ ",
            label: "Enroll Face",
            description: "Add a new face template  (root)".into(),
            action: Some(Action::Add),
            needs_user: true,
        },
        MenuItem {
            icon: "= ",
            label: "List Templates",
            description: "View enrolled templates".into(),
            action: Some(Action::List),
            needs_user: true,
        },
        MenuItem {
            icon: "~ ",
            label: "Test Recognition",
            description: "Live recognition test      (root)".into(),
            action: Some(Action::Test),
            needs_user: true,
        },
        MenuItem {
            icon: "  ",
            label: "---",
            description: "".into(),
            action: None,
            needs_user: false,
        },
        MenuItem {
            icon: "x ",
            label: "Quit",
            description: "Exit Facegate".into(),
            action: None,
            needs_user: false,
        },
    ]
}

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum InputMode {
    Menu,
    UsernameInput,
    SampleCountInput,
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
    items: Vec<MenuItem>,
    selected: usize,
    input_mode: InputMode,
    username_buf: String,
    sample_count_buf: String,
    pending: Option<Action>,
    pending_username: Option<String>,
    panel: PanelState,
    sudo_enabled: bool,
    /// When set, the loop exits and main re-opens the config TUI.
    open_config: bool,
    should_quit: bool,
}

impl<'a> App<'a> {
    fn new(config: &'a Config) -> Self {
        let sudo_enabled = commands::sudo_toggle::is_enabled();
        App {
            config,
            items: build_items(sudo_enabled),
            selected: 0,
            input_mode: InputMode::Menu,
            username_buf: String::new(),
            sample_count_buf: String::new(),
            pending: None,
            pending_username: None,
            panel: PanelState::Idle,
            sudo_enabled,
            open_config: false,
            should_quit: false,
        }
    }

    fn is_sep(&self, i: usize) -> bool {
        self.items[i].label == "---"
    }
    fn is_quit(&self, i: usize) -> bool {
        self.items[i].label == "Quit"
    }

    fn move_up(&mut self) {
        loop {
            self.selected = if self.selected == 0 {
                self.items.len() - 1
            } else {
                self.selected - 1
            };
            if !self.is_sep(self.selected) {
                break;
            }
        }
    }
    fn move_down(&mut self) {
        loop {
            self.selected = (self.selected + 1) % self.items.len();
            if !self.is_sep(self.selected) {
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
            if self.items[self.selected].needs_user {
                self.pending = Some(action);
                self.username_buf.clear();
                self.input_mode = InputMode::UsernameInput;
            } else {
                self.launch(action, None, 1);
            }
        }
    }

    fn confirm_username(&mut self) {
        let name = self.username_buf.trim().to_owned();
        if name.is_empty() {
            return;
        }
        if let Some(action) = self.pending.clone() {
            if action == Action::Add {
                self.pending_username = Some(name);
                self.sample_count_buf = "3".to_owned();
                self.input_mode = InputMode::SampleCountInput;
            } else {
                self.pending = None;
                self.launch(action, Some(name), 1);
                self.input_mode = InputMode::Menu;
            }
        }
    }

    fn confirm_sample_count(&mut self) {
        let s = self.sample_count_buf.trim().to_owned();
        let samples = if s.is_empty() {
            3
        } else {
            match s.parse::<u32>() {
                Ok(n) if n >= 1 && n <= 10 => n,
                _ => return,
            }
        };
        if let (Some(action), Some(username)) = (self.pending.take(), self.pending_username.take())
        {
            self.launch(action, Some(username), samples);
            self.input_mode = InputMode::Menu;
        }
    }

    fn cancel_input(&mut self) {
        self.input_mode = InputMode::Menu;
        self.pending = None;
        self.pending_username = None;
        self.username_buf.clear();
        self.sample_count_buf.clear();
    }

    fn launch(&mut self, action: Action, username: Option<String>, samples: u32) {
        let (tx, rx) = mpsc::channel::<String>();
        let config = self.config.clone();

        thread::spawn(move || {
            let result = match &action {
                Action::Doctor => commands::doctor::run_streaming(&config, None, &tx),
                Action::SudoToggle => {
                    commands::sudo_toggle::run_streaming(username.as_deref(), &tx)
                }
                Action::CameraTest => commands::camera_test::run_streaming(&config, None, &tx),
                Action::Add => commands::add::run_streaming(
                    &config,
                    username.as_deref(),
                    None,
                    samples,
                    false,
                    &tx,
                ),
                Action::List => commands::list::run_streaming(&config, username.as_deref(), &tx),
                Action::Test => commands::test::run_streaming(&config, username.as_deref(), &tx),
                Action::Configure => unreachable!(),
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
            self.items = build_items(self.sudo_enabled);
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
            // In Running state, ignore nav except quit
            if matches!(app.panel, PanelState::Running { .. }) {
                if code == KeyCode::Char('q') {
                    app.should_quit = true;
                }
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
            if item.label == "---" {
                return ListItem::new(Line::from(Span::styled(
                    "  ──────────────────────",
                    Style::default().fg(Color::DarkGray),
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
                Span::styled(format!("{:<18}", item.label), label_s),
                Span::styled(
                    format!(" {}", item.description),
                    Style::default().fg(Color::DarkGray),
                ),
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
        PanelState::Idle => (
            " Output ",
            Color::DarkGray,
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Select an action from the menu.",
                    Style::default().fg(Color::DarkGray),
                )),
            ],
            None,
        ),
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
            Span::raw("  quit"),
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
    f.render_widget(
        Paragraph::new(hints).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
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
