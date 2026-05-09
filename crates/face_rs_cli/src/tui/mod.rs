pub mod app;
pub mod main_menu;
mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, time::Duration};

use app::{App, Focus, Mode};
use face_rs_core::config::Config;

pub fn run_configure(config: Config, config_path: std::path::PathBuf) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config, config_path);
    let result = event_loop(&mut terminal, &mut app);

    // Always restore the terminal, even on error
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
    );
    let _ = terminal.show_cursor();

    result
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key.code);
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode) {
    match app.mode {
        Mode::Editing => match code {
            KeyCode::Enter => app.confirm_edit(),
            KeyCode::Esc => app.cancel_edit(),
            KeyCode::Backspace => {
                app.edit_buffer.pop();
            }
            KeyCode::Char(c) => app.edit_buffer.push(c),
            _ => {}
        },
        Mode::Normal => match code {
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            KeyCode::Char('s') => app.save(),
            KeyCode::Tab => toggle_focus(app),
            KeyCode::Up | KeyCode::Char('k') => app.move_up(),
            KeyCode::Down | KeyCode::Char('j') => app.move_down(),
            KeyCode::Left | KeyCode::Char('h') => app.focus = Focus::Sections,
            KeyCode::Right | KeyCode::Char('l') => app.focus = Focus::Fields,
            KeyCode::Enter => app.enter(),
            _ => {}
        },
    }
}

fn toggle_focus(app: &mut App) {
    app.focus = match app.focus {
        Focus::Sections => Focus::Fields,
        Focus::Fields => Focus::Sections,
    };
}
