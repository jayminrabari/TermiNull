use crate::layout::{LayoutPreset, PaneSpec};
use anyhow::{anyhow, Context, Result};
use nix::{
    libc,
    pty::{forkpty, ForkptyResult},
    sys::{
        signal::{kill, Signal},
        wait::{waitpid, WaitPidFlag},
    },
    unistd::{close, dup, execvp, read, write, Pid},
};
use ratatui::style::{Color, Modifier, Style};
use std::{
    ffi::CString,
    os::fd::{BorrowedFd, IntoRawFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
use vte::{Params, Parser, Perform};

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
        }
    }
}

pub struct PaneBuffer {
    lines: Vec<Vec<Cell>>,
    cursor_x: usize,
    cursor_y: usize,
    width: usize,
    max_lines: usize,
    current_style: Style,
}

impl PaneBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: vec![Vec::new()],
            cursor_x: 0,
            cursor_y: 0,
            width: 80,
            max_lines: max_lines.max(200),
            current_style: Style::default(),
        }
    }

    pub fn resize(&mut self, width: usize) {
        self.width = width.max(1);
        if self.cursor_x >= self.width {
            self.cursor_x = self.width - 1;
        }
    }

    pub fn visible_lines(&self, height: usize) -> Vec<Vec<Cell>> {
        self.visible_lines_at(height, 0)
    }

    pub fn visible_lines_at(&self, height: usize, scrollback: usize) -> Vec<Vec<Cell>> {
        let height = height.max(1);
        self.lines[self.visible_start_at(height, scrollback)..]
            .iter()
            .take(height)
            .cloned()
            .collect()
    }

    pub fn visible_start_at(&self, height: usize, scrollback: usize) -> usize {
        self.lines
            .len()
            .saturating_sub(height.max(1).saturating_add(scrollback))
    }

    pub fn max_scrollback(&self, height: usize) -> usize {
        self.lines.len().saturating_sub(height.max(1))
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    fn push_char(&mut self, ch: char) {
        match ch {
            '\n' => self.line_feed(),
            '\r' => self.cursor_x = 0,
            '\u{0008}' => self.cursor_x = self.cursor_x.saturating_sub(1),
            _ => {
                self.ensure_cursor();
                let line = &mut self.lines[self.cursor_y];
                if self.cursor_x >= line.len() {
                    line.resize(self.cursor_x + 1, Cell::default());
                }
                line[self.cursor_x] = Cell {
                    ch,
                    style: self.current_style,
                };
                self.cursor_x += 1;
                if self.cursor_x >= self.width {
                    self.cursor_x = 0;
                    self.line_feed();
                }
            }
        }
    }

    fn line_feed(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;
        if self.cursor_y >= self.lines.len() {
            self.lines.push(Vec::new());
        }
        if self.lines.len() > self.max_lines {
            self.lines.remove(0);
            self.cursor_y = self.cursor_y.saturating_sub(1);
        }
    }

    fn ensure_cursor(&mut self) {
        while self.cursor_y >= self.lines.len() {
            self.lines.push(Vec::new());
        }
    }

    fn clear_screen(&mut self) {
        self.lines.clear();
        self.lines.push(Vec::new());
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    pub fn replace_with_message(&mut self, message: &str) {
        self.lines.clear();
        self.lines.push(
            message
                .chars()
                .map(|ch| Cell {
                    ch,
                    style: Style::default().fg(Color::DarkGray),
                })
                .collect(),
        );
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn clear_line_from_cursor(&mut self) {
        self.ensure_cursor();
        let line = &mut self.lines[self.cursor_y];
        if self.cursor_x < line.len() {
            line.truncate(self.cursor_x);
        }
    }

    fn move_cursor(&mut self, x: usize, y: usize) {
        self.cursor_x = x.min(self.width.saturating_sub(1));
        self.cursor_y = y.min(self.max_lines.saturating_sub(1));
        self.ensure_cursor();
    }

    fn sgr(&mut self, params: &Params) {
        let values = params
            .iter()
            .map(|p| p.first().copied().unwrap_or(0))
            .collect::<Vec<_>>();
        let values = if values.is_empty() { vec![0] } else { values };
        for value in values {
            match value {
                0 => self.current_style = Style::default(),
                1 => self.current_style = self.current_style.add_modifier(Modifier::BOLD),
                30..=37 => {
                    self.current_style = self.current_style.fg(ansi_color(value - 30, false))
                }
                90..=97 => self.current_style = self.current_style.fg(ansi_color(value - 90, true)),
                40..=47 => {
                    self.current_style = self.current_style.bg(ansi_color(value - 40, false))
                }
                100..=107 => {
                    self.current_style = self.current_style.bg(ansi_color(value - 100, true))
                }
                39 => self.current_style = self.current_style.fg(Color::Reset),
                49 => self.current_style = self.current_style.bg(Color::Reset),
                _ => {}
            }
        }
    }
}

impl Perform for PaneBuffer {
    fn print(&mut self, c: char) {
        self.push_char(c);
    }
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.push_char('\n'),
            b'\r' => self.push_char('\r'),
            0x08 => self.push_char('\u{0008}'),
            _ => {}
        }
    }
    fn csi_dispatch(&mut self, params: &Params, _: &[u8], _: bool, action: char) {
        let param = |idx: usize, default: u16| {
            params
                .iter()
                .nth(idx)
                .and_then(|p| p.first().copied())
                .filter(|v| *v != 0)
                .unwrap_or(default) as usize
        };
        match action {
            'A' => self.cursor_y = self.cursor_y.saturating_sub(param(0, 1)),
            'B' => self.move_cursor(self.cursor_x, self.cursor_y + param(0, 1)),
            'C' => self.cursor_x = (self.cursor_x + param(0, 1)).min(self.width.saturating_sub(1)),
            'D' => self.cursor_x = self.cursor_x.saturating_sub(param(0, 1)),
            'H' | 'f' => {
                self.move_cursor(param(1, 1).saturating_sub(1), param(0, 1).saturating_sub(1))
            }
            'J' => self.clear_screen(),
            'K' => self.clear_line_from_cursor(),
            'm' => self.sgr(params),
            _ => {}
        }
    }
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
}

pub struct Pane {
    pub fd: i32,
    pub pid: Option<Pid>,
    pub title: String,
    pub shell: String,
    pub command: Option<String>,
    pub closed: bool,
    pub buffer: Arc<Mutex<PaneBuffer>>,
}

pub struct App {
    pub preset: LayoutPreset,
    pub panes: Vec<Pane>,
    dirty: Arc<AtomicBool>,
    scrollback_lines: usize,
}

impl Drop for App {
    fn drop(&mut self) {
        for pane in &self.panes {
            if let Some(pid) = pane.pid {
                let _ = kill(pid, Signal::SIGHUP);
                let _ = waitpid(pid, Some(WaitPidFlag::WNOHANG));
            }
            if pane.fd >= 0 {
                let _ = close(pane.fd);
            }
        }
    }
}

impl App {
    pub fn new_with_scrollback(
        preset: LayoutPreset,
        shell: String,
        panes_config: Vec<PaneSpec>,
        scrollback_lines: usize,
    ) -> Result<Self> {
        let count = preset.pane_count();
        let mut panes = Vec::with_capacity(count);
        let mut handles = Vec::with_capacity(count);
        let dirty = Arc::new(AtomicBool::new(true));
        for idx in 0..count {
            let shell = shell.clone();
            let command = panes_config
                .get(idx)
                .and_then(PaneSpec::command)
                .map(str::to_string);
            handles.push(thread::spawn(move || {
                spawn_pane(&shell, command.as_deref())
            }));
        }
        for (idx, handle) in handles.into_iter().enumerate() {
            let (fd, pid) = handle
                .join()
                .map_err(|_| anyhow!("spawn thread panicked"))??;
            let reader_fd = dup(fd)?;
            let buffer = Arc::new(Mutex::new(PaneBuffer::new(scrollback_lines)));
            start_reader(reader_fd, Arc::clone(&buffer), Arc::clone(&dirty));
            let command = panes_config
                .get(idx)
                .and_then(PaneSpec::command)
                .map(str::to_string);
            let title = panes_config
                .get(idx)
                .and_then(PaneSpec::title)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Pane {}", idx + 1));
            panes.push(Pane {
                fd,
                pid: Some(pid),
                title,
                shell: shell.clone(),
                command,
                closed: false,
                buffer,
            });
        }
        Ok(Self {
            preset,
            panes,
            dirty,
            scrollback_lines,
        })
    }

    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn close_pane(&mut self, idx: usize) {
        let Some(pane) = self.panes.get_mut(idx) else {
            return;
        };
        if let Some(pid) = pane.pid.take() {
            let _ = kill(pid, Signal::SIGHUP);
            let _ = waitpid(pid, Some(WaitPidFlag::WNOHANG));
        }
        if pane.fd >= 0 {
            let _ = close(pane.fd);
            pane.fd = -1;
        }
        pane.closed = true;
        if let Ok(mut buffer) = pane.buffer.lock() {
            buffer.replace_with_message("pane closed - press Ctrl+Shift+R to restart");
        }
        self.mark_dirty();
    }

    pub fn restart_pane(&mut self, idx: usize) -> Result<()> {
        self.close_pane(idx);
        let pane = self.panes.get_mut(idx).context("pane index out of range")?;
        let (fd, pid) = spawn_pane(&pane.shell, pane.command.as_deref())?;
        let reader_fd = dup(fd)?;
        let buffer = Arc::new(Mutex::new(PaneBuffer::new(self.scrollback_lines)));
        start_reader(reader_fd, Arc::clone(&buffer), Arc::clone(&self.dirty));
        pane.fd = fd;
        pane.pid = Some(pid);
        pane.closed = false;
        pane.buffer = buffer;
        self.mark_dirty();
        Ok(())
    }
}

fn spawn_pane(shell: &str, command: Option<&str>) -> Result<(i32, Pid)> {
    let fork = unsafe { forkpty(None, None)? };
    match fork {
        ForkptyResult::Child => {
            let shell_path =
                CString::new(shell).unwrap_or_else(|_| CString::new("/bin/bash").unwrap());
            if let Some(cmd) = command {
                let arg0 = CString::new(shell).unwrap_or_else(|_| CString::new("sh").unwrap());
                let arg1 = CString::new("-lc").unwrap();
                let cmd = CString::new(cmd).context("invalid command")?;
                let _ = execvp(&shell_path, &[arg0, arg1, cmd]);
            } else {
                let arg0 = CString::new(shell).unwrap_or_else(|_| CString::new("sh").unwrap());
                let _ = execvp(&shell_path, &[arg0]);
            }
            std::process::exit(127);
        }
        ForkptyResult::Parent { child, master } => Ok((master.into_raw_fd(), child)),
    }
}

pub fn send_input(pane: &Pane, data: &[u8]) -> Result<()> {
    if pane.fd < 0 || pane.closed {
        return Ok(());
    }
    let fd: BorrowedFd<'_> = unsafe { BorrowedFd::borrow_raw(pane.fd) };
    write(fd, data)?;
    Ok(())
}

pub fn resize_pane(pane: &Pane, cols: u16, rows: u16) {
    if pane.fd < 0 || pane.closed {
        return;
    }
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(pane.fd, libc::TIOCSWINSZ, &size);
    }
    if let Ok(mut buffer) = pane.buffer.lock() {
        buffer.resize(cols as usize);
    }
}

fn start_reader(fd: i32, buffer: Arc<Mutex<PaneBuffer>>, dirty: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut parser = Parser::new();
        let mut buf = [0u8; 4096];
        loop {
            match read(fd, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut guard = buffer.lock().unwrap();
                    for byte in &buf[..n] {
                        parser.advance(&mut *guard, *byte);
                    }
                    dirty.store(true, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
        let _ = close(fd);
    });
}

fn ansi_color(value: u16, bright: bool) -> Color {
    match (value, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        _ => Color::White,
    }
}
