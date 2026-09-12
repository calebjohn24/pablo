use super::{Shared, available, markdown, view};
use crate::{CancellationToken, TaskErrorCode, config::Options};
use rustix::termios::{self, OptionalActions, Termios};
use std::{
    fs::{File, OpenOptions},
    future::{Future, pending},
    io::{self, Read, Write},
    pin::Pin,
    process::ExitCode,
    sync::{Arc, Mutex},
    time::Duration,
};
const INPUT_BYTES: usize = 16 * 1024;
struct Terminal {
    io: File,
    saved: Termios,
    restored: std::cell::Cell<bool>,
}
impl Terminal {
    fn open() -> io::Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        let input = termios::ttyname(std::io::stdin(), Vec::new())?;
        let output = termios::ttyname(std::io::stdout(), Vec::new())?;
        if input != output {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(std::ffi::OsStr::from_bytes(input.to_bytes()))?;
        let saved = termios::tcgetattr(&file)?;
        let mut raw = saved.clone();
        raw.make_raw();
        // Nonblocking terminal I/O avoids registering macOS tty descriptors with kqueue.
        let flags = rustix::fs::fcntl_getfl(&file)?;
        rustix::fs::fcntl_setfl(&file, flags | rustix::fs::OFlags::NONBLOCK)?;
        let terminal = Self {
            io: file.try_clone()?,
            saved,
            restored: std::cell::Cell::new(false),
        };
        termios::tcsetattr(&file, OptionalActions::Now, &raw)?;
        file.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[H\x1b[2J")?;
        Ok(terminal)
    }
    async fn read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            match (&self.io).read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(Duration::from_millis(5)).await
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }
    async fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            match (&self.io).write(&bytes[offset..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(written) => offset += written,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(Duration::from_millis(5)).await
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    async fn restore(&self) {
        let _ = tokio::time::timeout(
            Duration::from_millis(250),
            self.write(b"\x1b[?2026l\x1b[0m\x1b[?1000l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[?1049l"),
        )
        .await;
        if termios::tcsetattr(&self.io, OptionalActions::Now, &self.saved).is_ok() {
            self.restored.set(true);
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        if self.restored.get() {
            return;
        }
        let _ = termios::tcsetattr(&self.io, OptionalActions::Now, &self.saved);
        let _ = self
            .io
            .write_all(b"\x1b[?2026l\x1b[0m\x1b[?1000l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[?1049l");
    }
}
#[derive(Default)]
struct Editor {
    text: String,
    cursor: usize,
    escape: Vec<u8>,
    utf8: Vec<u8>,
    paste: bool,
    scroll: usize,
    transcript_lines: usize,
    revision: u64,
}
enum Action {
    Submit(String),
    Cancel,
    Quit,
}
impl Editor {
    fn key(&mut self, byte: u8) -> Option<Action> {
        self.revision = self.revision.wrapping_add(1);
        if byte == 3 && !self.paste {
            self.escape.clear();
            self.utf8.clear();
            return Some(Action::Cancel);
        }
        if !self.escape.is_empty() {
            self.escape.push(byte);
            if self.escape.len() == 2 && byte != b'[' {
                self.escape.clear();
                return None;
            }
            if self.escape.len() > 2 && (0x40..=0x7e).contains(&byte) {
                match self.escape.as_slice() {
                    b"\x1b[5~" => self.scroll = self.scroll.saturating_add(12),
                    b"\x1b[6~" => self.scroll = self.scroll.saturating_sub(12),
                    b"\x1b[A" => self.scroll = self.scroll.saturating_add(1),
                    b"\x1b[B" => self.scroll = self.scroll.saturating_sub(1),
                    b"\x1b[1;5H" | b"\x1b[1;5~" => self.scroll = usize::MAX,
                    b"\x1b[1;5F" | b"\x1b[4;5~" => self.scroll = 0,
                    b"\x1b[D" => {
                        if self.cursor > 0 {
                            self.cursor -= 1;
                            while !self.text.is_char_boundary(self.cursor) {
                                self.cursor -= 1;
                            }
                        }
                    }
                    b"\x1b[C" => {
                        if self.cursor < self.text.len() {
                            self.cursor +=
                                self.text[self.cursor..].chars().next().unwrap().len_utf8();
                        }
                    }
                    b"\x1b[H" => self.cursor = 0,
                    b"\x1b[F" => self.cursor = self.text.len(),
                    b"\x1b[3~" => {
                        if self.cursor < self.text.len() {
                            self.text.remove(self.cursor);
                        }
                    }
                    b"\x1b[200~" => self.paste = true,
                    b"\x1b[201~" => self.paste = false,
                    value if value.starts_with(b"\x1b[<64;") => {
                        self.scroll = self.scroll.saturating_add(3)
                    }
                    value if value.starts_with(b"\x1b[<65;") => {
                        self.scroll = self.scroll.saturating_sub(3)
                    }
                    _ => {}
                }
                self.escape.clear();
            } else if self.escape.len() > 16 {
                self.escape.clear();
            }
            return None;
        }
        match byte {
            0x1b => {
                self.utf8.clear();
                self.escape.push(byte);
            }
            3 if !self.paste => return Some(Action::Cancel),
            4 if !self.paste && self.text.is_empty() => return Some(Action::Quit),
            b'\r' | b'\n' if !self.paste => {
                if !self.text.trim().is_empty() {
                    self.cursor = 0;
                    return Some(Action::Submit(std::mem::take(&mut self.text)));
                }
            }
            127 | 8 if !self.paste => {
                if self.cursor > 0 {
                    let end = self.cursor;
                    self.cursor -= 1;
                    while !self.text.is_char_boundary(self.cursor) {
                        self.cursor -= 1;
                    }
                    self.text.drain(self.cursor..end);
                }
            }
            0..=31 if !self.paste => {}
            _ => {
                self.utf8.push(byte);
                match std::str::from_utf8(&self.utf8) {
                    Ok(text) => {
                        let safe: String = text
                            .chars()
                            .filter(|c| !c.is_control() || (*c == '\n' && self.paste))
                            .collect();
                        if self.text.len() + safe.len() <= INPUT_BYTES {
                            self.text.insert_str(self.cursor, &safe);
                            self.cursor += safe.len();
                        }
                        self.utf8.clear();
                    }
                    Err(error) if error.error_len().is_some() || self.utf8.len() >= 4 => {
                        self.utf8.clear()
                    }
                    _ => {}
                }
            }
        }
        None
    }
}
fn clip(text: &str, width: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|&c| {
            used += markdown::columns(c);
            used <= width
        })
        .collect()
}
fn frame(
    view: &view::View,
    editor: &mut Editor,
    width: usize,
    height: usize,
    busy: bool,
) -> Vec<String> {
    if width < 20 || height < 8 {
        let mut lines = vec![clip("Resize terminal to continue", width)];
        lines.resize(height, String::new());
        return lines;
    }
    let mut lines = vec![
        format!("pablo | {} | {}", view.status, view.model),
        view.policy.clone(),
        format!("Trace {}", view.trace),
        view.usage.clone(),
    ];
    if !view.skills.is_empty() {
        lines.push(format!("Skills: {}", view.skills.join(", ")));
    }
    for (id, status) in view.activity.iter().take(3) {
        lines.push(format!(
            "{}: {}",
            id.chars().take(8).collect::<String>(),
            status
        ));
    }
    // Reserve transcript space even on short terminals.
    lines.truncate(height.saturating_sub(5));
    lines.push(if view.trimmed {
        "[oldest display text discarded]".into()
    } else {
        String::new()
    });
    for line in &mut lines {
        *line = clip(line, width);
    }
    let room = height.saturating_sub(lines.len() + 2);
    let transcript = markdown::render(&view.transcript, width);
    if editor.scroll > 0 {
        editor.scroll = editor
            .scroll
            .saturating_add(transcript.len().saturating_sub(editor.transcript_lines));
    }
    editor.transcript_lines = transcript.len();
    editor.scroll = editor.scroll.min(transcript.len().saturating_sub(room));
    let end = transcript.len().saturating_sub(editor.scroll);
    let start = end.saturating_sub(room);
    lines.extend_from_slice(&transcript[start..end]);
    lines.resize(height - 2, String::new());
    let controls = if busy {
        "Ctrl-C: cancel and join | Ctrl-D: cancel and exit"
    } else {
        "Enter: new independent task | Ctrl-D: exit"
    };
    lines.push(clip(
        &format!("{controls} | PgUp/PgDn: history | Ctrl-End: latest"),
        width,
    ));
    let prompt = format!(
        "> {}▏{}",
        view::label(
            &editor.text[editor.text[..editor.cursor]
                .char_indices()
                .rev()
                .nth(width.saturating_sub(6) / 2)
                .map(|(i, _)| i)
                .unwrap_or(0)..editor.cursor]
        ),
        view::label(&editor.text[editor.cursor..])
    );
    lines.push(clip(&prompt, width));
    lines
}
#[derive(Default)]
struct Screen {
    rows: Vec<String>,
    width: usize,
}
impl Screen {
    fn paint(&mut self, rows: Vec<String>, width: usize) -> String {
        let resized = self.width != width || self.rows.len() != rows.len();
        let mut output = String::new();
        for (index, row) in rows.iter().enumerate() {
            if resized || self.rows.get(index) != Some(row) {
                if output.is_empty() {
                    output.push_str("\x1b[?2026h");
                }
                output.push_str(&format!("\x1b[{};1H\x1b[0m{}\x1b[0m\x1b[K", index + 1, row));
            }
        }
        if !output.is_empty() {
            output.push_str("\x1b[?2026l");
        }
        self.rows = rows;
        self.width = width;
        output
    }
}
pub async fn serve(options: Options) -> Result<ExitCode, String> {
    if !available() {
        return Err(
            "TUI requires terminal stdin/stdout and TERM; use pablo run for redirected output"
                .into(),
        );
    }
    let terminal = Terminal::open().map_err(|_| "cannot enter terminal mode")?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "cannot register terminal interrupt")?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| "cannot register terminal termination")?;
    let view: Shared = Arc::new(Mutex::new(view::View::default()));
    {
        let mut display = view.lock().unwrap();
        display.status = "Ready".into();
        display.policy = "Static policy; no approvals".into();
    }
    let editor = Arc::new(Mutex::new(Editor::default()));
    let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let renderer = async {
        let mut screen = Screen::default();
        let mut rendered = None;
        loop {
            let size = termios::tcgetwinsize(&terminal.io).ok();
            let width = size
                .as_ref()
                .map(|s| usize::from(s.ws_col))
                .unwrap_or(80)
                .clamp(1, 240);
            let height = size
                .as_ref()
                .map(|s| usize::from(s.ws_row))
                .unwrap_or(24)
                .clamp(1, 80);
            let text = {
                let display = view.lock().unwrap();
                let mut editor = editor.lock().unwrap();
                let busy = busy.load(std::sync::atomic::Ordering::Relaxed);
                let key = (display.revision, editor.revision, width, height, busy);
                if rendered == Some(key) {
                    String::new()
                } else {
                    let rows = frame(&display, &mut editor, width, height, busy);
                    rendered = Some(key);
                    screen.paint(rows, width)
                }
            };
            if !text.is_empty() {
                tokio::time::timeout(Duration::from_millis(500), terminal.write(text.as_bytes()))
                    .await
                    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        #[allow(unreachable_code)]
        Ok::<(), io::Error>(())
    };
    tokio::pin!(renderer);
    type Job = Pin<Box<dyn Future<Output = Result<ExitCode, String>>>>;
    let mut job: Option<Job> = None;
    let mut cancel = CancellationToken::new();
    let mut last = ExitCode::SUCCESS;
    let mut exiting = false;
    let mut rendering = true;
    let mut display_failed = false;
    let mut read_error = None;
    let mut signal_exit = None;
    let mut initial =
        (!options.task_input().is_empty()).then(|| Action::Submit(options.task_input().into()));
    let mut buffer = [0u8; 256];
    loop {
        let action = if initial.is_some() {
            initial.take()
        } else {
            tokio::select! {
                _ = interrupt.recv() => Some(Action::Cancel),
                _ = terminate.recv() => { signal_exit = Some(ExitCode::from(143)); Some(Action::Quit) },
                result = &mut renderer, if rendering => {
                    rendering = false; exiting = true; cancel.cancel();
                    if result.is_err() { display_failed = true; }
                    None
                }
                result = async { match job.as_mut() { Some(job) => job.await, None => pending().await } } => {
                    job = None; busy.store(false,std::sync::atomic::Ordering::Relaxed);
                    match result { Ok(code) => { last = code;let mut display=view.lock().unwrap();if !display.terminal_seen { display.status="Run ended; terminal event unavailable".into();display.invalidate(); } }, Err(error) => { let mut view=view.lock().unwrap();view.status="Setup failed".into();view.push(&error);last=ExitCode::from(2); } }
                    None
                }
                read = terminal.read(&mut buffer), if !exiting => {
                    match read {
                        Ok(0) => Some(Action::Quit),
                        Err(error) => { read_error = Some(format!("terminal input failed: {error}")); Some(Action::Quit) },
                        Ok(count) => {
                            let mut action = None;
                            for &byte in &buffer[..count] {
                                let next = if job.is_some() {
                                    match byte { 3 => Some(Action::Cancel), 4 => Some(Action::Quit), _ => {
                                        let mut editor = editor.lock().unwrap();
                                        if byte == 0x1b || !editor.escape.is_empty() { editor.key(byte); }
                                        None
                                    }}
                                } else { editor.lock().unwrap().key(byte) };
                                if let Some(next)=next { action=Some(next);break; }
                            }
                            action
                        }
                    }
                }
            }
        };
        match action {
            Some(Action::Submit(input)) if job.is_none() && !exiting => {
                view.lock().unwrap().reset(&input);
                {
                    let mut editor = editor.lock().unwrap();
                    editor.scroll = 0;
                    editor.transcript_lines = 0;
                    editor.paste = false;
                    editor.escape.clear();
                }
                cancel = CancellationToken::new();
                let token = cancel.clone();
                let display = view.clone();
                let options = options.for_task(input);
                busy.store(true, std::sync::atomic::Ordering::Relaxed);
                job = Some(Box::pin(async move {
                    let mut stage = TaskErrorCode::InvalidArguments;
                    crate::run_options(options, &mut stage, Some(display), token).await
                }));
            }
            Some(Action::Cancel) if job.is_some() => {
                {
                    let mut editor = editor.lock().unwrap();
                    editor.escape.clear();
                    editor.utf8.clear();
                    editor.paste = false;
                }
                cancel.cancel();
                let mut display = view.lock().unwrap();
                display.status = "Cancelling; joining owned work".into();
                display.invalidate();
            }
            Some(Action::Cancel) => {
                last = ExitCode::from(130);
                exiting = true;
            }
            Some(Action::Quit) => {
                exiting = true;
                cancel.cancel();
            }
            _ => {}
        }
        if exiting && job.is_none() {
            break;
        }
    }
    terminal.restore().await;
    if let Some(error) = read_error {
        return Err(error);
    }
    if let Some(code) = signal_exit {
        return Ok(code);
    }
    Ok(if display_failed {
        ExitCode::FAILURE
    } else {
        last
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composer_edits_unicode_and_bracketed_paste_does_not_submit() {
        let mut editor = Editor::default();
        for &b in "a🦀c".as_bytes() {
            assert!(editor.key(b).is_none());
        }
        for &b in b"\x1b[D\x7f" {
            editor.key(b);
        }
        assert_eq!(editor.text, "ac");
        assert_eq!(editor.cursor, 1);
        for &b in b"\x1b[200~one\ntwo\x1b[201~" {
            assert!(editor.key(b).is_none());
        }
        assert_eq!(editor.text, "aone\ntwoc");
        assert!(matches!(editor.key(b'\r'),Some(Action::Submit(text)) if text=="aone\ntwoc"));
        assert!(editor.text.is_empty());
    }
    #[test]
    fn history_remains_anchored_while_new_output_streams_and_only_changed_rows_paint() {
        let mut view = view::View::default();
        view.reset("first");
        for i in 0..50 {
            view.push(&format!("line {i}\n"));
        }
        let mut editor = Editor::default();
        frame(&view, &mut editor, 60, 16, true);
        for byte in b"\x1b[5~" {
            editor.key(*byte);
        }
        let before = frame(&view, &mut editor, 60, 16, true);
        view.push("new output\nmore output\n");
        assert_eq!(before, frame(&view, &mut editor, 60, 16, true));
        let mut screen = Screen::default();
        screen.paint(vec!["one".into(), "two".into()], 60);
        let update = screen.paint(vec!["one".into(), "short".into()], 60);
        assert!(!update.contains("\x1b[1;1H") && update.contains("\x1b[2;1H"));
        assert!(!update.contains("\x1b[2J"));
    }

    #[test]
    fn input_and_rendering_remain_bounded() {
        let mut editor = Editor::default();
        for _ in 0..INPUT_BYTES + 100 {
            editor.key(b'a');
        }
        assert_eq!(editor.text.len(), INPUT_BYTES);
        let mut view = view::View::default();
        view.push(&"界\x1b[2J".repeat(20000));
        let rows = frame(&view, &mut editor, 80, 24, false);
        let mut screen = Screen::default();
        let frame = screen.paint(rows.clone(), 80);
        assert!(screen.paint(rows, 80).is_empty());
        assert!(frame.len() < 12_000);
        assert!(frame.matches("\r\n").count() < 24);
        assert!(!frame.contains("\x1b[2J"));
        assert!(super::frame(&view, &mut editor, 5, 2, false).len() == 2);
    }
}
