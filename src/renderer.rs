use crate::{
    layout::leaf_rects,
    pty::{resize_pane, send_input, App, Cell},
};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use std::{io, time::Duration};

pub fn render(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut active = 0usize;
    let mut rename = None::<String>;
    let mut last_sizes = vec![(0u16, 0u16); app.panes.len()];
    let result = loop {
        terminal.draw(|f| {
            let mut chunks = Vec::new();
            leaf_rects(&app.preset.tree(), f.area(), &mut chunks);
            for (idx, pane) in app.panes.iter().enumerate() {
                let area = chunks
                    .get(idx.min(chunks.len().saturating_sub(1)))
                    .copied()
                    .unwrap_or(f.area());
                let cols = area.width.saturating_sub(2).max(1);
                let rows = area.height.saturating_sub(2).max(1);
                if last_sizes.get(idx).copied() != Some((cols, rows)) {
                    resize_pane(pane, cols, rows);
                    if let Some(size) = last_sizes.get_mut(idx) {
                        *size = (cols, rows);
                    }
                }
                let visible = {
                    let guard = pane.buffer.lock().unwrap();
                    guard.visible_lines(rows as usize)
                };
                let title = if idx == active {
                    rename
                        .as_ref()
                        .map(|value| format!(" Rename: {} ", value))
                        .unwrap_or_else(|| format!(" {} ", pane.title))
                } else {
                    format!(" {} ", pane.title)
                };
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(if idx == active {
                        Style::default().fg(Color::LightGreen)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    })
                    .title(title);
                f.render_widget(Paragraph::new(to_lines(visible)).block(block), area);
            }
        })?;
        if event::poll(Duration::from_millis(30))? {
            if let Event::Key(key) = event::read()? {
                if let Some(value) = rename.as_mut() {
                    match key.code {
                        KeyCode::Enter => {
                            if !value.trim().is_empty() {
                                app.panes[active].title = value.trim().to_string();
                            }
                            rename = None;
                        }
                        KeyCode::Esc => rename = None,
                        KeyCode::Backspace => {
                            value.pop();
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            value.push(c);
                        }
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        break Ok(())
                    }
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        rename = Some(app.panes[active].title.clone());
                    }
                    KeyCode::Tab => {
                        active = (active + 1) % app.panes.len().max(1);
                    }
                    KeyCode::BackTab => {
                        active = if active == 0 {
                            app.panes.len().saturating_sub(1)
                        } else {
                            active - 1
                        };
                    }
                    KeyCode::Char(c) => {
                        if let Some(byte) = control_byte(c, key.modifiers) {
                            let _ = send_input(&app.panes[active], &[byte]);
                        } else {
                            let mut bytes = [0u8; 4];
                            let s = c.encode_utf8(&mut bytes);
                            let _ = send_input(&app.panes[active], s.as_bytes());
                        }
                    }
                    KeyCode::Enter => {
                        let _ = send_input(&app.panes[active], b"\r");
                    }
                    KeyCode::Backspace => {
                        let _ = send_input(&app.panes[active], b"\x7f");
                    }
                    KeyCode::Left => {
                        let _ = send_input(&app.panes[active], b"\x1b[D");
                    }
                    KeyCode::Right => {
                        let _ = send_input(&app.panes[active], b"\x1b[C");
                    }
                    KeyCode::Up => {
                        let _ = send_input(&app.panes[active], b"\x1b[A");
                    }
                    KeyCode::Down => {
                        let _ = send_input(&app.panes[active], b"\x1b[B");
                    }
                    KeyCode::Delete => {
                        let _ = send_input(&app.panes[active], b"\x1b[3~");
                    }
                    KeyCode::Home => {
                        let _ = send_input(&app.panes[active], b"\x1b[H");
                    }
                    KeyCode::End => {
                        let _ = send_input(&app.panes[active], b"\x1b[F");
                    }
                    KeyCode::Esc => {
                        let _ = send_input(&app.panes[active], b"\x1b");
                    }
                    _ => {}
                }
            }
        }
    };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn to_lines(rows: Vec<Vec<Cell>>) -> Vec<Line<'static>> {
    rows.into_iter()
        .map(|row| {
            let mut spans = Vec::new();
            let mut buf = String::new();
            let mut style = Style::default();
            for cell in row {
                if cell.style != style && !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), style));
                }
                style = cell.style;
                buf.push(cell.ch);
            }
            if !buf.is_empty() {
                spans.push(Span::styled(buf, style));
            }
            Line::from(spans)
        })
        .collect()
}

fn control_byte(c: char, modifiers: KeyModifiers) -> Option<u8> {
    if !modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_alphabetic() {
        Some((lower as u8) & 0x1f)
    } else {
        None
    }
}
