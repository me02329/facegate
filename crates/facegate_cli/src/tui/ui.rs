use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::app::{App, Focus, Mode};

// Block-letter logo for "Facegate"  (~70 chars wide, 6 lines tall)
const LOGO: &[&str] = &[
    " ███████╗ █████╗  ██████╗███████╗ ██████╗  █████╗ ████████╗███████╗",
    " ██╔════╝██╔══██╗██╔════╝██╔════╝██╔════╝ ██╔══██╗╚══██╔══╝██╔════╝",
    " █████╗  ███████║██║     █████╗  ██║  ███╗███████║   ██║   █████╗  ",
    " ██╔══╝  ██╔══██║██║     ██╔══╝  ██║   ██║██╔══██║   ██║   ██╔══╝  ",
    " ██║     ██║  ██║╚██████╗███████╗╚██████╔╝██║  ██║   ██║   ███████╗",
    " ╚═╝     ╚═╝  ╚═╝ ╚═════╝╚══════╝ ╚═════╝ ╚═╝  ╚═╝   ╚═╝   ╚══════╝",
];

const SUBTITLE: &str = "native facial authentication for Linux";

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    let root = Layout::vertical([
        Constraint::Length(9), // header: logo (6) + subtitle (1) + border (2)
        Constraint::Min(6),    // content
        Constraint::Length(3), // footer
    ])
    .split(area);

    render_header(f, root[0]);
    render_content(f, root[1], app);
    render_footer(f, root[2], app);

    if app.mode == Mode::Editing {
        render_edit_popup(f, app);
    }
}

// ── Header ────────────────────────────────────────────────────────────────────

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
        format!("  {SUBTITLE}"),
        Style::default().fg(Color::DarkGray),
    )));

    let header = Paragraph::new(lines).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(header, area);
}

// ── Content: sections + fields ────────────────────────────────────────────────

fn render_content(f: &mut Frame, area: Rect, app: &App) {
    let panes =
        Layout::horizontal([Constraint::Percentage(28), Constraint::Percentage(72)]).split(area);

    render_sections(f, panes[0], app);
    render_fields(f, panes[1], app);
}

fn render_sections(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Sections;
    let border = border_style(focused);

    let items: Vec<ListItem> = app
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = i == app.selected_section;
            let style = item_style(selected, focused);
            let prefix = if selected && focused { " ▶ " } else { "   " };
            ListItem::new(format!("{prefix}{}", s.name)).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .title(title(" Sections ", focused)),
    );

    f.render_widget(list, area);
}

fn render_fields(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Fields;
    let border = border_style(focused);
    let section = &app.sections[app.selected_section];

    // Split: field list on top, description hint at bottom
    let inner = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).split(area);

    let items: Vec<ListItem> = section
        .fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let selected = i == app.selected_field;
            let (key_style, val_style) = field_styles(selected, focused);
            let prefix = if selected && focused { " ▶ " } else { "   " };
            let line = Line::from(vec![
                Span::styled(format!("{prefix}{:<20}", field.key), key_style),
                Span::styled(field.value.clone(), val_style),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .title(title(&format!(" {} ", section.name), focused)),
    );
    f.render_widget(list, inner[0]);

    // Description of the currently selected field
    let desc = section
        .fields
        .get(app.selected_field)
        .map(|f| f.description)
        .unwrap_or("");

    let desc_widget = Paragraph::new(format!("  {desc}"))
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::NONE));
    f.render_widget(desc_widget, inner[1]);
}

// ── Footer ────────────────────────────────────────────────────────────────────

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    let content = if let Some((msg, is_err)) = &app.status {
        let color = if *is_err { Color::Red } else { Color::Green };
        Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(vec![
            key_hint("↑↓"),
            Span::raw(" navigate  "),
            key_hint("Tab"),
            Span::raw(" switch pane  "),
            key_hint("Enter"),
            Span::raw(" edit  "),
            key_hint("s"),
            Span::raw(" save  "),
            key_hint("Esc"),
            Span::raw(" back  "),
            key_hint("q"),
            Span::raw(" quit"),
        ])
    };

    let footer = Paragraph::new(content).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(footer, area);
}

fn key_hint(key: &str) -> Span<'static> {
    Span::styled(
        format!("[{key}]"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

// ── Edit popup ────────────────────────────────────────────────────────────────

fn render_edit_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 40, f.area());

    let field = &app.sections[app.selected_section].fields[app.selected_field];

    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            format!(" Edit: {} ", field.key),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::vertical([
        Constraint::Length(1), // padding
        Constraint::Length(1), // description
        Constraint::Length(1), // padding
        Constraint::Length(3), // input box
        Constraint::Min(1),    // padding
        Constraint::Length(1), // hint
    ])
    .split(inner);

    let desc = Paragraph::new(format!("  {}", field.description))
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(desc, layout[1]);

    // Show cursor as a trailing underscore since we manage it ourselves
    let input_text = format!("{}_", app.edit_buffer);
    let input = Paragraph::new(format!(" {input_text}"))
        .style(Style::default().fg(Color::Yellow))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );
    f.render_widget(input, layout[3]);

    let hint = Paragraph::new(Line::from(vec![
        key_hint("Enter"),
        Span::raw(" confirm   "),
        key_hint("Esc"),
        Span::raw(" cancel"),
    ]))
    .alignment(Alignment::Center)
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(hint, layout[5]);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(v[1])[1]
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn title(text: &str, focused: bool) -> Span<'static> {
    let color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    Span::styled(
        text.to_owned(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn item_style(selected: bool, focused: bool) -> Style {
    match (selected, focused) {
        (true, true) => Style::default().fg(Color::Black).bg(Color::Cyan),
        (true, false) => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::White),
    }
}

fn field_styles(selected: bool, focused: bool) -> (Style, Style) {
    match (selected, focused) {
        (true, true) => (
            Style::default().fg(Color::Black).bg(Color::Cyan),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        (true, false) => (
            Style::default().fg(Color::Cyan),
            Style::default().fg(Color::Yellow),
        ),
        _ => (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::White),
        ),
    }
}
