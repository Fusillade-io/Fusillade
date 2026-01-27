use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use super::{App, InputMode};

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(7), // Stats
            Constraint::Length(4), // Status codes
            Constraint::Min(5),    // Endpoints table
            Constraint::Length(3), // Footer / controls
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_stats(f, app, chunks[1]);
    draw_status_codes(f, app, chunks[2]);
    draw_endpoints(f, app, chunks[3]);
    draw_footer(f, app, chunks[4]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let elapsed = app
        .start_time
        .elapsed()
        .saturating_sub(app.control_state.total_paused());
    let elapsed_str = format_duration(elapsed);

    let duration_str = match app.total_duration {
        Some(d) => format!("{} / {}", elapsed_str, format_duration(d)),
        None => elapsed_str,
    };

    let state_str = if app.control_state.is_stopped() {
        ("STOPPED", Color::Red)
    } else if app.control_state.is_paused() {
        ("PAUSED", Color::Yellow)
    } else {
        ("RUNNING", Color::Green)
    };

    let workers = app.control_state.get_target_workers();

    let header = Line::from(vec![
        Span::styled(
            format!(" {} ", state_str.0),
            Style::default()
                .fg(Color::White)
                .bg(state_str.1)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("Workers: {}", format_number(workers)),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  │  "),
        Span::styled(
            format!("Elapsed: {}", duration_str),
            Style::default().fg(Color::White),
        ),
    ]);

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " Fusillade ",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));
    let paragraph = Paragraph::new(header).block(block);
    f.render_widget(paragraph, area);
}

fn draw_stats(f: &mut Frame, app: &App, area: Rect) {
    let snap = match &app.snapshot {
        Some(s) => s,
        None => {
            let block = Block::default().borders(Borders::ALL);
            let p = Paragraph::new("  Waiting for data...").block(block);
            f.render_widget(p, area);
            return;
        }
    };

    let mb_sent = snap.data_sent_bytes as f64 / 1_048_576.0;
    let mb_recv = snap.data_recv_bytes as f64 / 1_048_576.0;

    let lines = vec![
        Line::from(vec![
            Span::raw("  Requests: "),
            Span::styled(
                format_number(snap.total_requests),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    RPS: "),
            Span::styled(format!("{:.1}", snap.rps), Style::default().fg(Color::Cyan)),
            Span::raw("    Errors: "),
            Span::styled(
                format!(
                    "{} ({:.1}%)",
                    format_number(snap.error_count),
                    snap.error_pct
                ),
                if snap.error_count > 0 {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
        ]),
        Line::from(vec![
            Span::raw("  Sent: "),
            Span::styled(
                format!("{:.1} MB", mb_sent),
                Style::default().fg(Color::White),
            ),
            Span::raw("       Received: "),
            Span::styled(
                format!("{:.1} MB", mb_recv),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Avg: "),
            Span::styled(format_ms(snap.avg_ms), Style::default().fg(Color::Yellow)),
            Span::raw("  Min: "),
            Span::styled(format_ms(snap.min_ms), Style::default().fg(Color::Green)),
            Span::raw("  Max: "),
            Span::styled(format_ms(snap.max_ms), Style::default().fg(Color::Red)),
        ]),
        Line::from(vec![
            Span::raw("  P50: "),
            Span::styled(format_ms(snap.p50_ms), Style::default().fg(Color::White)),
            Span::raw("  P90: "),
            Span::styled(format_ms(snap.p90_ms), Style::default().fg(Color::White)),
            Span::raw("  P95: "),
            Span::styled(format_ms(snap.p95_ms), Style::default().fg(Color::Yellow)),
            Span::raw("  P99: "),
            Span::styled(format_ms(snap.p99_ms), Style::default().fg(Color::Red)),
        ]),
    ];

    let block = Block::default().borders(Borders::ALL);
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn draw_status_codes(f: &mut Frame, app: &App, area: Rect) {
    let snap = match &app.snapshot {
        Some(s) => s,
        None => return,
    };

    let spans: Vec<Span> = snap
        .status_codes
        .iter()
        .enumerate()
        .flat_map(|(i, (code, count))| {
            let color = match code {
                200..=299 => Color::Green,
                300..=399 => Color::Yellow,
                400..=499 => Color::Magenta,
                500..=599 => Color::Red,
                _ => Color::Gray,
            };
            let mut v = vec![Span::styled(
                format!("{}: {}", code, format_number(*count)),
                Style::default().fg(color),
            )];
            if i < snap.status_codes.len() - 1 {
                v.push(Span::raw("  │  "));
            }
            v
        })
        .collect();

    let line = Line::from(
        std::iter::once(Span::raw("  "))
            .chain(spans)
            .collect::<Vec<_>>(),
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Status Codes ");
    let paragraph = Paragraph::new(line).block(block);
    f.render_widget(paragraph, area);
}

fn draw_endpoints(f: &mut Frame, app: &App, area: Rect) {
    let snap = match &app.snapshot {
        Some(s) => s,
        None => return,
    };

    let header = Row::new(vec![
        Cell::from("Name").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Reqs").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Avg").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("P95").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Errs").style(Style::default().add_modifier(Modifier::BOLD)),
    ])
    .style(Style::default().fg(Color::Cyan));

    let rows: Vec<Row> = snap
        .endpoints
        .iter()
        .skip(app.endpoint_scroll)
        .map(|ep| {
            let err_style = if ep.errors > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(truncate_name(&ep.name, 30)),
                Cell::from(format_number(ep.reqs)),
                Cell::from(format_ms(ep.avg_ms)),
                Cell::from(format_ms(ep.p95_ms)),
                Cell::from(format_number(ep.errors)).style(err_style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(30),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Endpoints "));

    f.render_widget(table, area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let content = match &app.input_mode {
        InputMode::Normal => {
            let pause_label = if app.control_state.is_paused() {
                "[p] Resume"
            } else {
                "[p] Pause"
            };
            Line::from(vec![
                Span::styled(
                    format!(" {}  ", pause_label),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("[s] Stop  ", Style::default().fg(Color::Red)),
                Span::styled("[+/-] Workers ±10  ", Style::default().fg(Color::Cyan)),
                Span::styled("[r] Ramp  ", Style::default().fg(Color::Green)),
                Span::styled("[t] Tag  ", Style::default().fg(Color::Magenta)),
                Span::styled("[↑/↓] Scroll  ", Style::default().fg(Color::White)),
                Span::styled("[q] Quit", Style::default().fg(Color::Gray)),
            ])
        }
        InputMode::RampInput(s) => Line::from(vec![
            Span::styled(" Ramp to workers: ", Style::default().fg(Color::Green)),
            Span::styled(
                s.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::White)),
            Span::raw("  (Enter to submit, Esc to cancel)"),
        ]),
        InputMode::TagInput(s) => Line::from(vec![
            Span::styled(" Tag (key=value): ", Style::default().fg(Color::Magenta)),
            Span::styled(
                s.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::White)),
            Span::raw("  (Enter to submit, Esc to cancel)"),
        ]),
    };

    let block = Block::default().borders(Borders::ALL);
    let paragraph = Paragraph::new(content).block(block);
    f.render_widget(paragraph, area);
}

pub(crate) fn format_duration(d: std::time::Duration) -> String {
    let total_secs = d.as_secs();
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{:02}:{:02}", mins, secs)
}

pub(crate) fn format_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.1}s", ms / 1000.0)
    } else if ms >= 1.0 {
        format!("{:.1}ms", ms)
    } else {
        format!("{:.2}ms", ms)
    }
}

pub(crate) fn format_number(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub(crate) fn truncate_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        name.to_string()
    } else {
        format!("{}...", &name[..max - 3])
    }
}
