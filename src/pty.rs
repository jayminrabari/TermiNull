use crate::layout::LayoutPreset;
use anyhow::{anyhow, Context, Result};
use nix::{
    libc,
    pty::{forkpty, ForkptyResult},
    sys::{
        signal::{kill, Signal},
        wait::{waitpid, WaitPidFlag},
    },
    unistd::{close, execvp, read, write, Pid},
};
use ratatui::style::{Color, Modifier, Style};
use std::{
    ffi::CString,
    os::fd::{BorrowedFd, IntoRawFd},
    sync::{Arc, Mutex},
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
        Self { ch: ' ', style: Style::default() }
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
    pub fn new() -> Self {
        Self {
            lines: vec![Vec::new()],
            cursor_x: 0,
            cursor_y: 0,
            width: 80,
            max_lines: 2000,
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
        let height = height.max(1);
        let start = self.lines.len().saturating_sub(height);
        self.lines[start..].to_vec()
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
                line[self.cursor_x] = Cell { ch, style: self.current_style };
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
                30..=37 => self.current_style = self.current_style.fg(ansi_color(value - 30, false)),
                90..=97 => self.current_style = self.current_style.fg(ansi_color(value - 90, true)),
                40..=47 => self.current_style = self.current_style.bg(ansi_color(value - 40, false)),
                100..=107 => self.current_style = self.current_style.bg(ansi_color(value - 100, true)),
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
            'H' | 'f' => self.move_cursor(param(1, 1).saturating_sub(1), param(0, 1).saturating_sub(1)),
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
    pub pid: Pid,
    pub buffer: Arc<Mutex<PaneBuffer>>,
}

pub struct App {
    pub preset: LayoutPreset,
    pub panes: Vec<Pane>,
}

impl Drop for App {
    fn drop(&mut self) {
        for pane in &self.panes {
            let _ = kill(pane.pid, Signal::SIGHUP);
            let _ = close(pane.fd);
            let _ = waitpid(pane.pid, Some(WaitPidFlag::WNOHANG));
        }
    }
}

impl App {
    pub fn new(preset: LayoutPreset, shell: String, commands: Vec<String>) -> Result<Self> {
        let count = preset.pane_count();
        let mut panes = Vec::with_capacity(count);
        let mut handles = Vec::with_capacity(count);
        for idx in 0..count {
            let shell = shell.clone();
            let command = commands.get(idx).cloned();
            handles.push(thread::spawn(move || spawn_pane(&shell, command.as_deref())));
        }
        for handle in handles {
            let (fd, pid) = handle.join().map_err(|_| anyhow!("spawn thread panicked"))??;
            let buffer = Arc::new(Mutex::new(PaneBuffer::new()));
            start_reader(fd, Arc::clone(&buffer));
            panes.push(Pane { fd, pid, buffer });
        }
        Ok(Self { preset, panes })
    }
}

fn spawn_pane(shell: &str, command: Option<&str>) -> Result<(i32, Pid)> {
    let fork = unsafe { forkpty(None, None)? };
    match fork {
        ForkptyResult::Child => {
            let shell_path = CString::new(shell).unwrap_or_else(|_| CString::new("/bin/bash").unwrap());
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
    let fd: BorrowedFd<'_> = unsafe { BorrowedFd::borrow_raw(pane.fd) };
    write(fd, data)?;
    Ok(())
}

pub fn resize_pane(pane: &Pane, cols: u16, rows: u16) {
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

fn start_reader(fd: i32, buffer: Arc<Mutex<PaneBuffer>>) {
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
                }
                Err(_) => break,
            }
        }
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
