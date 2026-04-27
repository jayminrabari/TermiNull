use crate::{
    layout::{leaf_rects, ThemeConfig, UiConfig},
    pty::{resize_pane, send_input, App, Cell},
};
use anyhow::{anyhow, Result};
use arboard::Clipboard;
use fontdue::{Font, FontSettings};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier},
};
use softbuffer::{Context as SoftContext, Surface};
use std::{
    collections::HashMap,
    fs,
    num::NonZeroU32,
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    dpi::{LogicalSize, PhysicalPosition},
    event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey},
    window::WindowBuilder,
};

pub fn run(mut app: App, ui: UiConfig) -> Result<()> {
    let event_loop = EventLoop::new()?;
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("TermiNull")
            .with_inner_size(LogicalSize::new(1000.0, 700.0))
            .with_min_inner_size(LogicalSize::new(640.0, 420.0))
            .build(&event_loop)?,
    );

    let context = SoftContext::new(window.clone())
        .map_err(|err| anyhow!("failed to create GUI context: {err}"))?;
    let mut surface = Surface::new(&context, window.clone())
        .map_err(|err| anyhow!("failed to create GUI surface: {err}"))?;
    let mut renderer = GuiRenderer::new(&ui)?;
    let mut active = 0usize;
    let mut rename = None::<String>;
    let mut modifiers = ModifiersState::empty();
    let mut mouse = PhysicalPosition::new(0.0, 0.0);
    let mut pane_rects = Vec::<Rect>::new();
    let mut scroll_offsets = vec![0usize; app.panes.len()];
    let mut clipboard = Clipboard::new().ok();
    let mut needs_redraw = true;

    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(50),
        ));

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::RedrawRequested => {
                    let size = window.inner_size();
                    let Some(width) = NonZeroU32::new(size.width.max(1)) else {
                        return;
                    };
                    let Some(height) = NonZeroU32::new(size.height.max(1)) else {
                        return;
                    };
                    if surface.resize(width, height).is_err() {
                        target.exit();
                        return;
                    }
                    let Ok(mut buffer) = surface.buffer_mut() else {
                        target.exit();
                        return;
                    };
                    pane_rects = renderer.render(
                        &mut buffer,
                        size.width.max(1),
                        size.height.max(1),
                        &mut app,
                        active,
                        rename.as_deref(),
                        &mut scroll_offsets,
                    );
                    if buffer.present().is_err() {
                        target.exit();
                    }
                }
                WindowEvent::Resized(_) => needs_redraw = true,
                WindowEvent::CursorMoved { position, .. } => mouse = position,
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if let Some(idx) = pane_rects
                        .iter()
                        .position(|rect| contains(*rect, mouse.x, mouse.y))
                    {
                        active = idx;
                        rename = None;
                        needs_redraw = true;
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let target_idx = pane_rects
                        .iter()
                        .position(|rect| contains(*rect, mouse.x, mouse.y))
                        .unwrap_or(active);
                    active = target_idx;
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as i32,
                        MouseScrollDelta::PixelDelta(pos) => {
                            (pos.y / renderer.cell_h as f64) as i32
                        }
                    };
                    adjust_scroll(&mut scroll_offsets, active, lines);
                    needs_redraw = true;
                }
                WindowEvent::ModifiersChanged(value) => modifiers = value.state(),
                WindowEvent::KeyboardInput { event, .. } => {
                    handle_key(
                        event,
                        modifiers,
                        &mut app,
                        &mut active,
                        &mut rename,
                        &mut clipboard,
                        &mut renderer,
                        &mut scroll_offsets,
                        || target.exit(),
                    );
                    needs_redraw = true;
                }
                _ => {}
            },
            Event::AboutToWait => {
                if app.take_dirty() {
                    if active < scroll_offsets.len() && scroll_offsets[active] == 0 {
                        needs_redraw = true;
                    } else if scroll_offsets.iter().any(|offset| *offset == 0) {
                        needs_redraw = true;
                    }
                }
                if needs_redraw {
                    needs_redraw = false;
                    window.request_redraw();
                }
            }
            _ => {}
        }
    })?;

    Ok(())
}

fn handle_key(
    event: KeyEvent,
    modifiers: ModifiersState,
    app: &mut App,
    active: &mut usize,
    rename: &mut Option<String>,
    clipboard: &mut Option<Clipboard>,
    renderer: &mut GuiRenderer,
    scroll_offsets: &mut Vec<usize>,
    mut exit: impl FnMut(),
) {
    if event.state != ElementState::Pressed || app.panes.is_empty() {
        return;
    }

    if let Some(value) = rename.as_mut() {
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Enter) => {
                if !value.trim().is_empty() {
                    app.panes[*active].title = value.trim().to_string();
                }
                *rename = None;
            }
            Key::Named(NamedKey::Escape) => *rename = None,
            Key::Named(NamedKey::Backspace) => {
                value.pop();
            }
            Key::Named(NamedKey::Space) => {
                value.push(' ');
            }
            Key::Character(text) if !modifiers.control_key() => {
                value.push_str(text);
            }
            _ => {}
        }
        return;
    }

    match event.logical_key.as_ref() {
        Key::Named(NamedKey::Space) => {
            reset_scroll(scroll_offsets, *active);
            write_active(app, *active, b" ");
        }
        Key::Named(NamedKey::Tab) if modifiers.shift_key() => {
            *active = if *active == 0 {
                app.panes.len() - 1
            } else {
                *active - 1
            };
        }
        Key::Named(NamedKey::Tab) => {
            *active = (*active + 1) % app.panes.len();
        }
        Key::Named(NamedKey::Enter) => write_scrolled(app, scroll_offsets, *active, b"\r"),
        Key::Named(NamedKey::Backspace) => write_scrolled(app, scroll_offsets, *active, b"\x7f"),
        Key::Named(NamedKey::ArrowLeft) => write_scrolled(app, scroll_offsets, *active, b"\x1b[D"),
        Key::Named(NamedKey::ArrowRight) => write_scrolled(app, scroll_offsets, *active, b"\x1b[C"),
        Key::Named(NamedKey::ArrowUp) => write_scrolled(app, scroll_offsets, *active, b"\x1b[A"),
        Key::Named(NamedKey::ArrowDown) => write_scrolled(app, scroll_offsets, *active, b"\x1b[B"),
        Key::Named(NamedKey::Delete) => write_scrolled(app, scroll_offsets, *active, b"\x1b[3~"),
        Key::Named(NamedKey::Home) => write_scrolled(app, scroll_offsets, *active, b"\x1b[H"),
        Key::Named(NamedKey::End) => write_scrolled(app, scroll_offsets, *active, b"\x1b[F"),
        Key::Named(NamedKey::Escape) => write_scrolled(app, scroll_offsets, *active, b"\x1b"),
        Key::Named(NamedKey::PageUp) => adjust_scroll(scroll_offsets, *active, 10),
        Key::Named(NamedKey::PageDown) => adjust_scroll(scroll_offsets, *active, -10),
        Key::Character(text) => {
            let lower = text.to_ascii_lowercase();
            if modifiers.control_key() && lower == "q" {
                exit();
            } else if modifiers.control_key() && lower == "w" {
                app.close_pane(*active);
                reset_scroll(scroll_offsets, *active);
            } else if modifiers.control_key() && modifiers.shift_key() && lower == "r" {
                let _ = app.restart_pane(*active);
                reset_scroll(scroll_offsets, *active);
            } else if modifiers.control_key() && lower == "r" {
                *rename = Some(app.panes[*active].title.clone());
            } else if modifiers.control_key() && (text == "+" || text == "=") {
                renderer.adjust_font_size(1.0);
                reset_all_scroll(scroll_offsets);
            } else if modifiers.control_key() && text == "-" {
                renderer.adjust_font_size(-1.0);
                reset_all_scroll(scroll_offsets);
            } else if modifiers.control_key() && text == "0" {
                renderer.reset_font_size();
                reset_all_scroll(scroll_offsets);
            } else if modifiers.control_key() && modifiers.shift_key() && lower == "v" {
                if let Some(clipboard) = clipboard.as_mut() {
                    if let Ok(text) = clipboard.get_text() {
                        reset_scroll(scroll_offsets, *active);
                        write_active(app, *active, text.as_bytes());
                    }
                }
            } else if modifiers.control_key() {
                if let Some(byte) = control_byte(text) {
                    reset_scroll(scroll_offsets, *active);
                    write_active(app, *active, &[byte]);
                }
            } else if let Some(input) = event.text {
                reset_scroll(scroll_offsets, *active);
                write_active(app, *active, input.as_bytes());
            } else {
                reset_scroll(scroll_offsets, *active);
                write_active(app, *active, text.as_bytes());
            }
        }
        _ => {}
    }
}

fn adjust_scroll(scroll_offsets: &mut [usize], active: usize, delta: i32) {
    let Some(offset) = scroll_offsets.get_mut(active) else {
        return;
    };
    if delta >= 0 {
        *offset = offset.saturating_add(delta as usize);
    } else {
        *offset = offset.saturating_sub(delta.unsigned_abs() as usize);
    }
}

fn reset_scroll(scroll_offsets: &mut [usize], active: usize) {
    if let Some(offset) = scroll_offsets.get_mut(active) {
        *offset = 0;
    }
}

fn reset_all_scroll(scroll_offsets: &mut [usize]) {
    scroll_offsets.fill(0);
}

fn write_active(app: &App, active: usize, data: &[u8]) {
    if let Some(pane) = app.panes.get(active) {
        let _ = send_input(pane, data);
    }
}

fn write_scrolled(app: &App, scroll_offsets: &mut [usize], active: usize, data: &[u8]) {
    reset_scroll(scroll_offsets, active);
    write_active(app, active, data);
}

fn control_byte(text: &str) -> Option<u8> {
    let mut chars = text.chars();
    let c = chars.next()?.to_ascii_lowercase();
    if chars.next().is_none() && c.is_ascii_alphabetic() {
        Some((c as u8) & 0x1f)
    } else {
        None
    }
}

struct GuiRenderer {
    font: Font,
    font_size: f32,
    default_font_size: f32,
    cell_w: u32,
    cell_h: u32,
    title_h: u32,
    glyphs: HashMap<char, Glyph>,
    pane_sizes: Vec<(u16, u16)>,
    theme: Theme,
}

impl GuiRenderer {
    fn new(ui: &UiConfig) -> Result<Self> {
        let font = load_font(ui.font.as_deref())?;
        let font_size = ui.font_size.unwrap_or(13.0).clamp(9.0, 28.0);
        let mut renderer = Self {
            font,
            font_size,
            default_font_size: font_size,
            cell_w: 1,
            cell_h: 1,
            title_h: 1,
            glyphs: HashMap::new(),
            pane_sizes: Vec::new(),
            theme: Theme::from_config(&ui.theme),
        };
        renderer.recalculate_font();
        Ok(renderer)
    }

    fn adjust_font_size(&mut self, delta: f32) {
        self.font_size = (self.font_size + delta).clamp(9.0, 28.0);
        self.recalculate_font();
    }

    fn reset_font_size(&mut self) {
        self.font_size = self.default_font_size;
        self.recalculate_font();
    }

    fn recalculate_font(&mut self) {
        let (metrics, _) = self.font.rasterize('M', self.font_size);
        self.cell_w = (metrics.advance_width.ceil() as u32).max(7) + 1;
        self.cell_h = ((self.font_size * 1.55).ceil() as u32).max(17);
        self.title_h = self.cell_h + 9;
        self.glyphs.clear();
        self.pane_sizes.clear();
    }

    fn render(
        &mut self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        app: &mut App,
        active: usize,
        rename: Option<&str>,
        scroll_offsets: &mut [usize],
    ) -> Vec<Rect> {
        pixels.fill(self.theme.background);
        if self.pane_sizes.len() != app.panes.len() {
            self.pane_sizes = vec![(0, 0); app.panes.len()];
        }

        let area = Rect::new(
            0,
            0,
            width.min(u16::MAX as u32) as u16,
            height.min(u16::MAX as u32) as u16,
        );
        let mut rects = Vec::new();
        leaf_rects(&app.preset.tree(), area, &mut rects);

        for idx in 0..app.panes.len() {
            let rect = rects.get(idx).copied().unwrap_or(area);
            let pane = &app.panes[idx];
            let x = rect.x as i32;
            let y = rect.y as i32;
            let w = rect.width as u32;
            let h = rect.height as u32;
            if w < 8 || h < 8 {
                continue;
            }

            let is_active = idx == active;
            draw_rect(
                pixels,
                width,
                height,
                x,
                y,
                w,
                h,
                self.theme.pane_background,
            );
            draw_rect(
                pixels,
                width,
                height,
                x,
                y,
                w,
                1,
                if is_active {
                    self.theme.border_active
                } else {
                    self.theme.border
                },
            );
            draw_rect(
                pixels,
                width,
                height,
                x,
                y + h as i32 - 1,
                w,
                1,
                if is_active {
                    self.theme.border_active
                } else {
                    self.theme.border
                },
            );
            draw_rect(
                pixels,
                width,
                height,
                x,
                y,
                1,
                h,
                if is_active {
                    self.theme.border_active
                } else {
                    self.theme.border
                },
            );
            draw_rect(
                pixels,
                width,
                height,
                x + w as i32 - 1,
                y,
                1,
                h,
                if is_active {
                    self.theme.border_active
                } else {
                    self.theme.border
                },
            );
            draw_rect(
                pixels,
                width,
                height,
                x + 1,
                y + 1,
                w.saturating_sub(2),
                self.title_h.min(h.saturating_sub(2)),
                if is_active {
                    self.theme.title_active_background
                } else {
                    self.theme.title_background
                },
            );

            let title = if is_active {
                rename
                    .map(|value| format!("Rename: {}", value))
                    .unwrap_or_else(|| pane.title.clone())
            } else {
                pane.title.clone()
            };
            self.draw_text(
                pixels,
                width,
                height,
                x + 8,
                y + 6,
                &truncate(&title, ((w.saturating_sub(16)) / self.cell_w) as usize),
                if is_active {
                    self.theme.text
                } else {
                    self.theme.dim_text
                },
            );

            let content_x = x + 6;
            let content_y = y + self.title_h as i32 + 5;
            let content_w = w.saturating_sub(12);
            let content_h = h.saturating_sub(self.title_h + 9);
            let cols = (content_w / self.cell_w).max(1).min(u16::MAX as u32) as u16;
            let rows = (content_h / self.cell_h).max(1).min(u16::MAX as u32) as u16;
            if self.pane_sizes[idx] != (cols, rows) {
                resize_pane(pane, cols, rows);
                self.pane_sizes[idx] = (cols, rows);
            }

            let (visible, start, cursor, max_scroll) = {
                let guard = pane.buffer.lock().unwrap();
                (
                    guard.visible_lines_at(
                        rows as usize,
                        scroll_offsets.get(idx).copied().unwrap_or(0),
                    ),
                    guard.visible_start_at(
                        rows as usize,
                        scroll_offsets.get(idx).copied().unwrap_or(0),
                    ),
                    guard.cursor_position(),
                    guard.max_scrollback(rows as usize),
                )
            };
            if let Some(offset) = scroll_offsets.get_mut(idx) {
                *offset = (*offset).min(max_scroll);
            }
            self.draw_terminal(
                pixels, width, height, content_x, content_y, cols, rows, &visible,
            );
            if is_active && cursor.1 >= start {
                let cursor_row = cursor.1 - start;
                if cursor_row < rows as usize {
                    draw_rect(
                        pixels,
                        width,
                        height,
                        content_x + (cursor.0 as u32 * self.cell_w) as i32,
                        content_y
                            + (cursor_row as u32 * self.cell_h + self.cell_h.saturating_sub(3))
                                as i32,
                        self.cell_w.max(2),
                        2,
                        self.theme.border_active,
                    );
                }
            }
            if scroll_offsets.get(idx).copied().unwrap_or(0) > 0 {
                let marker = format!("+{}", scroll_offsets[idx]);
                self.draw_text(
                    pixels,
                    width,
                    height,
                    x + w as i32 - (marker.len() as u32 * self.cell_w) as i32 - 8,
                    y + 6,
                    &marker,
                    self.theme.border_active,
                );
            }
        }

        rects
    }

    fn draw_terminal(
        &mut self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        x: i32,
        y: i32,
        cols: u16,
        rows: u16,
        lines: &[Vec<Cell>],
    ) {
        for row in 0..rows as usize {
            let Some(line) = lines.get(row) else {
                continue;
            };
            for col in 0..cols as usize {
                let Some(cell) = line.get(col) else {
                    continue;
                };
                let cell_x = x + (col as u32 * self.cell_w) as i32;
                let cell_y = y + (row as u32 * self.cell_h) as i32;
                if let Some(bg) = cell.style.bg {
                    draw_rect(
                        pixels,
                        width,
                        height,
                        cell_x,
                        cell_y,
                        self.cell_w,
                        self.cell_h,
                        color(bg, self.theme.pane_background),
                    );
                }
                if cell.ch != ' ' {
                    let fg = color(cell.style.fg.unwrap_or(Color::Reset), self.theme.text);
                    self.draw_char(pixels, width, height, cell_x, cell_y, cell.ch, fg);
                    if cell.style.add_modifier.contains(Modifier::BOLD) {
                        self.draw_char(pixels, width, height, cell_x + 1, cell_y, cell.ch, fg);
                    }
                }
            }
        }
    }

    fn draw_text(
        &mut self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        x: i32,
        y: i32,
        text: &str,
        fg: u32,
    ) {
        for (idx, ch) in text.chars().enumerate() {
            self.draw_char(
                pixels,
                width,
                height,
                x + (idx as u32 * self.cell_w) as i32,
                y,
                ch,
                fg,
            );
        }
    }

    fn draw_char(
        &mut self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        x: i32,
        y: i32,
        ch: char,
        fg: u32,
    ) {
        let glyph = self.glyph(ch);
        let draw_x = x + ((self.cell_w.saturating_sub(glyph.width)) / 2) as i32;
        let draw_y = y + ((self.cell_h.saturating_sub(glyph.height)) / 2) as i32;
        for gy in 0..glyph.height {
            let py = draw_y + gy as i32;
            if py < 0 || py >= height as i32 {
                continue;
            }
            for gx in 0..glyph.width {
                let alpha = glyph.bitmap[(gy * glyph.width + gx) as usize];
                if alpha == 0 {
                    continue;
                }
                let px = draw_x + gx as i32;
                if px < 0 || px >= width as i32 {
                    continue;
                }
                let idx = py as usize * width as usize + px as usize;
                pixels[idx] = blend(pixels[idx], fg, alpha);
            }
        }
    }

    fn glyph(&mut self, ch: char) -> Glyph {
        if let Some(glyph) = self.glyphs.get(&ch) {
            return glyph.clone();
        }
        let (metrics, bitmap) = self.font.rasterize(ch, self.font_size);
        let glyph = Glyph {
            width: metrics.width as u32,
            height: metrics.height as u32,
            bitmap,
        };
        self.glyphs.insert(ch, glyph.clone());
        glyph
    }
}

#[derive(Clone)]
struct Glyph {
    width: u32,
    height: u32,
    bitmap: Vec<u8>,
}

#[derive(Clone)]
struct Theme {
    background: u32,
    pane_background: u32,
    title_background: u32,
    title_active_background: u32,
    border: u32,
    border_active: u32,
    text: u32,
    dim_text: u32,
}

impl Theme {
    fn from_config(config: &ThemeConfig) -> Self {
        Self {
            background: parse_hex(config.background.as_deref()).unwrap_or(0x101010),
            pane_background: parse_hex(config.pane_background.as_deref()).unwrap_or(0x111111),
            title_background: parse_hex(config.title_background.as_deref()).unwrap_or(0x202020),
            title_active_background: parse_hex(config.title_active_background.as_deref())
                .unwrap_or(0x263b2b),
            border: parse_hex(config.border.as_deref()).unwrap_or(0x5a5a5a),
            border_active: parse_hex(config.border_active.as_deref()).unwrap_or(0x4ec36b),
            text: parse_hex(config.text.as_deref()).unwrap_or(0xe8e8e8),
            dim_text: parse_hex(config.dim_text.as_deref()).unwrap_or(0xa8a8a8),
        }
    }
}

fn parse_hex(value: Option<&str>) -> Option<u32> {
    let value = value?.trim().trim_start_matches('#');
    if value.len() != 6 {
        return None;
    }
    u32::from_str_radix(value, 16).ok()
}

const FALLBACK_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/ubuntu/UbuntuSansMono[wght].ttf",
    "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/liberation2/LiberationMono-Regular.ttf",
    "/usr/share/fonts/opentype/urw-base35/NimbusMonoPS-Regular.otf",
];

fn looks_like_font_path(s: &str) -> bool {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    if lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc") {
        return true;
    }
    t.starts_with('/')
        || t.starts_with("./")
        || t.starts_with("../")
        || t.starts_with("~/")
}

fn expand_user_path(path: &str) -> PathBuf {
    let p = path.trim();
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

/// Resolves a fontconfig pattern (e.g. family name or `JetBrains Mono:style=Bold`) to a font file path.
fn font_file_from_fontconfig(pattern: &str) -> Option<PathBuf> {
    let output = Command::new("fc-match")
        .args(["--format=%{file}\n", pattern.trim()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout);
    let line = path.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let pb = PathBuf::from(line);
    pb.exists().then_some(pb)
}

fn load_font(preferred: Option<&str>) -> Result<Font> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(raw) = preferred.map(str::trim).filter(|s| !s.is_empty()) {
        if looks_like_font_path(raw) {
            candidates.push(expand_user_path(raw));
        } else {
            if let Some(p) = font_file_from_fontconfig(raw) {
                candidates.push(p);
            }
            candidates.extend(font_name_candidates(raw).into_iter().map(PathBuf::from));
        }
    }
    candidates.extend(FALLBACK_FONT_PATHS.iter().copied().map(PathBuf::from));

    let mut tried = Vec::new();
    for path in candidates {
        if tried.iter().any(|p: &PathBuf| p == &path) {
            continue;
        }
        tried.push(path.clone());
        if let Ok(bytes) = fs::read(&path) {
            return Font::from_bytes(bytes, FontSettings::default())
                .map_err(|err| anyhow!("failed to parse font {}: {err}", path.display()));
        }
    }

    let hint = preferred
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!(" (requested {:?})", s.trim()))
        .unwrap_or_default();
    Err(anyhow!(
        "could not load any font file{}. \
         Set ui.font to a path (.ttf/.otf/.ttc), a font family name (needs `fc-match` / fontconfig), \
         or install fonts under the usual system paths.",
        hint
    ))
}

fn font_name_candidates(value: &str) -> Vec<String> {
    let lower = value.to_ascii_lowercase();
    if lower.contains("ubuntu sans mono") {
        vec!["/usr/share/fonts/truetype/ubuntu/UbuntuSansMono[wght].ttf".to_string()]
    } else if lower.contains("ubuntu mono") {
        vec!["/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf".to_string()]
    } else if lower.contains("noto sans mono") {
        vec!["/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf".to_string()]
    } else if lower.contains("dejavu") {
        vec!["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf".to_string()]
    } else {
        Vec::new()
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars && max_chars > 1 {
        out.pop();
        out.push('>');
    }
    out
}

fn contains(rect: Rect, x: f64, y: f64) -> bool {
    x >= rect.x as f64
        && y >= rect.y as f64
        && x < (rect.x + rect.width) as f64
        && y < (rect.y + rect.height) as f64
}

fn draw_rect(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    color: u32,
) {
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = (x + w as i32).clamp(0, width as i32) as u32;
    let y1 = (y + h as i32).clamp(0, height as i32) as u32;
    for py in y0..y1 {
        let start = py as usize * width as usize + x0 as usize;
        let end = py as usize * width as usize + x1 as usize;
        pixels[start..end].fill(color);
    }
}

fn blend(dst: u32, src: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let inv = 255 - a;
    let sr = (src >> 16) & 0xff;
    let sg = (src >> 8) & 0xff;
    let sb = src & 0xff;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    (((sr * a + dr * inv) / 255) << 16)
        | (((sg * a + dg * inv) / 255) << 8)
        | ((sb * a + db * inv) / 255)
}

fn color(color: Color, fallback: u32) -> u32 {
    match color {
        Color::Reset => fallback,
        Color::Black => 0x000000,
        Color::Red => 0xcd3131,
        Color::Green => 0x0dbc79,
        Color::Yellow => 0xe5e510,
        Color::Blue => 0x2472c8,
        Color::Magenta => 0xbc3fbc,
        Color::Cyan => 0x11a8cd,
        Color::Gray => 0xe5e5e5,
        Color::DarkGray => 0x666666,
        Color::LightRed => 0xf14c4c,
        Color::LightGreen => 0x23d18b,
        Color::LightYellow => 0xf5f543,
        Color::LightBlue => 0x3b8eea,
        Color::LightMagenta => 0xd670d6,
        Color::LightCyan => 0x29b8db,
        Color::White => 0xffffff,
        Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
        Color::Indexed(idx) => indexed_color(idx),
    }
}

fn indexed_color(idx: u8) -> u32 {
    const BASE: [u32; 16] = [
        0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666,
        0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
    ];
    if idx < 16 {
        return BASE[idx as usize];
    }
    if (16..=231).contains(&idx) {
        let idx = idx - 16;
        let r = idx / 36;
        let g = (idx % 36) / 6;
        let b = idx % 6;
        let conv = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
        return (conv(r) << 16) | (conv(g) << 8) | conv(b);
    }
    let gray = 8 + (idx.saturating_sub(232) as u32 * 10);
    (gray << 16) | (gray << 8) | gray
}
