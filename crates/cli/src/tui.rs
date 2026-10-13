//! The terminal interface: conversations on the left, the selected one on
//! the right, a box to type in. It attaches to a running daemon and keeps
//! nothing itself, so closing it leaves the daemon running and reopening
//! it finds everything where it was.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use crossterm::style::Print;
use crossterm::terminal::SetTitle;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange, Event as Term, EventStream,
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures::StreamExt as _;
use nightpost_core::api::{Client, Event, HistoryItem, Interface, LinkState, Person, Reply, Request};
use nightpost_core::{i18n, t};
use ratatui::DefaultTerminal;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep_until};
use unicode_width::UnicodeWidthChar as _;

// Night post, as in the graphical interface: a desk at night, their
// messages on onionskin paper, yours as blue carbon copies, post-box red
// only for what needs you (unread, comparing numbers).
const NIGHT: Color = Color::Rgb(0x16, 0x1b, 0x2b);
const NIGHT_DEEP: Color = Color::Rgb(0x10, 0x14, 0x1f);
const RAISED: Color = Color::Rgb(0x23, 0x2a, 0x40);
const ONIONSKIN: Color = Color::Rgb(0xe7, 0xe5, 0xda);
const INK: Color = Color::Rgb(0x1d, 0x21, 0x30);
const INK_SOFT: Color = Color::Rgb(0x4a, 0x4f, 0x5e);
const CARBON: Color = Color::Rgb(0x2c, 0x3a, 0x63);
const CARBON_INK: Color = Color::Rgb(0xe3, 0xe8, 0xf6);
const LAMP: Color = Color::Rgb(0x8e, 0x96, 0xad);
const POST: Color = Color::Rgb(0xc8, 0x46, 0x3d);
const POST_ON_PAPER: Color = Color::Rgb(0xa8, 0x35, 0x2d);
const DIM: Style = Style::new().fg(LAMP);
const PAPER: Style = Style::new().fg(INK).bg(ONIONSKIN);
const SIDEBAR: u16 = 28;
const HISTORY: u32 = 200;
/// As the graphical interface: a ping at most this often while typing,
/// shown this long on the other side.
const TYPING_EVERY: Duration = Duration::from_secs(4);
const TYPING_SHOWN: Duration = Duration::from_secs(5);
fn help() -> Vec<String> {
    let keys = [
        ("Enter", t!("help-send")),
        ("Alt+Enter", t!("help-newline")),
        ("Tab, Shift+Tab", t!("help-tab")),
        ("PgUp, PgDn", t!("help-scroll")),
        ("Esc", t!("help-esc")),
        ("Ctrl+C", t!("help-quit")),
    ];
    let commands = [
        ("/invite <name>", t!("help-invite")),
        ("/code <name>", t!("help-code")),
        ("/accept <link> <name>", t!("help-accept")),
        ("/acceptcode <code> <name>", t!("help-acceptcode")),
        ("/verify", t!("help-verify")),
        ("/rename <new name>", t!("help-rename")),
        ("/mute [1h | 8h]", t!("help-mute")),
        ("/unmute", String::new()),
        ("/remove", t!("help-remove")),
        ("/quit", String::new()),
    ];
    let keys = keys.iter().map(|(k, v)| format!("{k:<17}{v}"));
    let commands = commands.iter().map(|(c, v)| format!("{c:<29}{v}").trim_end().to_string());
    keys.chain([String::new()]).chain(commands).collect()
}

struct Conversation {
    name: String,
    person: Person,
    unread: usize,
    messages: Vec<HistoryItem>,
    loaded: bool,
}

enum Overlay {
    Text { title: String, body: Vec<String> },
    /// Shown with its QR code when the terminal is big enough for all of it.
    Invite { name: String, link: String, qr: Vec<String> },
    /// Someone used our code: keep them only if the numbers match.
    Confirm { name: String, number: String },
    /// Their safety number; y marks it compared.
    Verify { name: String, number: String },
    /// y removes them, w also deletes the conversation.
    Remove { name: String },
}

enum Update {
    Event(Event),
    /// A long request (accepting an invitation) finished.
    Done(String),
    DaemonGone,
}

struct App {
    dir: PathBuf,
    conversations: Vec<Conversation>,
    selected: usize,
    input: String,
    /// Byte offset in `input`.
    cursor: usize,
    /// Lines scrolled up from the end of the conversation.
    scroll: usize,
    status: String,
    overlay: Option<Overlay>,
    /// Names we gave a code to and wait for.
    codes: Vec<String>,
    focused: bool,
    notify: Option<String>,
    quit: bool,
    /// Who is writing to us, until when. ponytail: one person at a time,
    /// the last one heard; a map if two at once ever matters here.
    typing: Option<(String, Instant)>,
    /// When we last said we are writing.
    pinged: Option<Instant>,
}

pub async fn run(dir: &Path) -> Result<()> {
    let mut requests = Client::connect(dir)
        .await
        .with_context(|| t!("tui-no-daemon"))?;
    let mut events = Client::connect(dir).await?;
    if events.call(&Request::Listen).await? != Reply::Listening {
        bail!(t!("tui-no-stream"));
    }
    let (updates, mut updated) = mpsc::unbounded_channel();
    let forward = updates.clone();
    tokio::spawn(async move {
        while let Ok(Some(event)) = events.event().await {
            if forward.send(Update::Event(event)).is_err() {
                return;
            }
        }
        let _ = forward.send(Update::DaemonGone);
    });

    let mut app = App {
        dir: dir.to_path_buf(),
        conversations: Vec::new(),
        selected: 0,
        input: String::new(),
        cursor: 0,
        scroll: 0,
        status: t!("tui-help-hint"),
        overlay: None,
        codes: Vec::new(),
        focused: true,
        notify: None,
        quit: false,
        typing: None,
        pinged: None,
    };
    app.reload_contacts(&mut requests).await?;
    app.select(0, &mut requests).await;

    let mut terminal = ratatui::init();
    // The tab says Nightpost, not the program's path; the terminal's own
    // title is saved first (CSI 22 t) and given back after (CSI 23 t).
    crossterm::execute!(std::io::stdout(), Print("\x1b[22;0t"), SetTitle("Nightpost"), EnableFocusChange, EnableBracketedPaste)?;
    let result = app.run(&mut terminal, &mut requests, &mut updated, &updates).await;
    let _ = crossterm::execute!(std::io::stdout(), DisableFocusChange, DisableBracketedPaste, Print("\x1b[23;0t"));
    ratatui::restore();
    result
}

impl App {
    async fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        requests: &mut Client,
        updated: &mut mpsc::UnboundedReceiver<Update>,
        updates: &mpsc::UnboundedSender<Update>,
    ) -> Result<()> {
        let mut keys = EventStream::new();
        while !self.quit {
            terminal.draw(|frame| self.draw(frame))?;
            if let Some(text) = self.notify.take() {
                // After the frame, never inside it.
                let mut out = std::io::stdout();
                let _ = out.write_all(notification(&text, std::env::var_os("TMUX").is_some()).as_bytes());
                let _ = out.flush();
            }
            tokio::select! {
                Some(event) = keys.next() => match event? {
                    Term::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key, requests, updates).await,
                    Term::Paste(text) => self.insert(&text.replace("\r\n", "\n")),
                    Term::FocusGained => {
                        self.focused = true;
                        self.mark_read(requests).await;
                    }
                    Term::FocusLost => self.focused = false,
                    _ => {}
                },
                Some(update) = updated.recv() => self.on_update(update, requests).await,
                // Only to draw again without it.
                _ = sleep_until(self.typing.as_ref().map_or_else(Instant::now, |t| t.1)), if self.typing.is_some() => self.typing = None,
            }
        }
        Ok(())
    }

    async fn on_key(&mut self, key: KeyEvent, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if let Some(Overlay::Confirm { name, .. }) = &self.overlay {
            let keep = match key.code {
                KeyCode::Char('y' | 'Y') => true,
                KeyCode::Char('n' | 'N') | KeyCode::Esc => false,
                _ => return,
            };
            let name = name.clone();
            self.overlay = None;
            self.codes.retain(|n| *n != name);
            self.status = match requests.call(&Request::Confirm { name: name.clone(), keep }).await {
                Ok(Reply::Error { message }) => message,
                Err(e) => format!("{e:#}"),
                Ok(_) if keep => t!("kept", name = name),
                Ok(_) => t!("turned-down", name = name),
            };
            return;
        }
        if let (Some(Overlay::Verify { name, .. }), KeyCode::Char('y' | 'Y')) = (&self.overlay, key.code) {
            let name = name.clone();
            self.overlay = None;
            self.set_person(&name, |p| p.verified = true, requests).await;
            return;
        }
        if let Some(Overlay::Remove { name }) = &self.overlay {
            let wipe = match key.code {
                KeyCode::Char('y' | 'Y') => false,
                KeyCode::Char('w' | 'W') => true,
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    self.overlay = None;
                    return;
                }
                _ => return,
            };
            let name = name.clone();
            self.overlay = None;
            // The daemon's ContactRemoved event updates the list.
            if let Err(e) = expect_done(requests.call(&Request::Remove { name, wipe }).await) {
                self.status = format!("{e:#}");
            }
            return;
        }
        if let (Some(Overlay::Invite { link, .. }), KeyCode::Char('c')) = (&self.overlay, key.code) {
            copy(link);
            self.status = t!("tui-link-copied");
            return;
        }
        match key.code {
            KeyCode::Char('c' | 'q') if ctrl => self.quit = true,
            KeyCode::Esc if self.overlay.is_some() => self.overlay = None,
            KeyCode::Esc => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Enter if alt => self.insert("\n"),
            KeyCode::Enter => self.submit(requests, updates).await,
            KeyCode::Tab => self.select(self.selected + 1, requests).await,
            KeyCode::BackTab => {
                let n = self.conversations.len().max(1);
                self.select(self.selected + n - 1, requests).await;
            }
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Left => self.cursor = prev_boundary(&self.input, self.cursor),
            KeyCode::Right => self.cursor = next_boundary(&self.input, self.cursor),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                let start = prev_boundary(&self.input, self.cursor);
                self.input.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Delete if self.cursor < self.input.len() => {
                let end = next_boundary(&self.input, self.cursor);
                self.input.replace_range(self.cursor..end, "");
            }
            KeyCode::Char(c) if !ctrl => {
                self.insert(c.encode_utf8(&mut [0; 4]));
                self.typed(requests).await;
            }
            _ => {}
        }
    }

    /// At most one ping every few seconds; the daemon drops it unless
    /// typing indicators are on.
    async fn typed(&mut self, requests: &mut Client) {
        if self.input.starts_with('/') || self.pinged.is_some_and(|at| at.elapsed() < TYPING_EVERY) {
            return;
        }
        if let Some(c) = self.conversations.get(self.selected) {
            self.pinged = Some(Instant::now());
            let _ = requests.call(&Request::Typing { name: c.name.clone() }).await;
        }
    }

    fn insert(&mut self, text: &str) {
        self.input.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    async fn submit(&mut self, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) {
        let input = std::mem::take(&mut self.input);
        self.cursor = 0;
        let line = input.trim();
        if line.is_empty() {
            return;
        }
        if let Some(command) = line.strip_prefix('/') {
            if let Err(e) = self.command(command, requests, updates).await {
                self.status = format!("{e:#}");
            }
            return;
        }
        let Some(to) = self.conversations.get(self.selected).map(|c| c.name.clone()) else {
            self.status = t!("tui-no-conversation");
            self.input = input;
            self.cursor = self.input.len();
            return;
        };
        match requests.call(&Request::Send { to, text: input.clone() }).await {
            // The message itself comes back as Event::Sent.
            Ok(Reply::Sent { .. }) => self.scroll = 0,
            Ok(Reply::Error { message }) => {
                self.status = message;
                self.input = input;
                self.cursor = self.input.len();
            }
            Ok(other) => self.status = t!("err-unexpected", reply = format!("{other:?}")),
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    async fn command(&mut self, command: &str, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) -> Result<()> {
        let words: Vec<&str> = command.split_whitespace().collect();
        match words.as_slice() {
            ["help"] => self.overlay = Some(Overlay::Text { title: t!("tui-help-title"), body: help() }),
            ["quit"] => self.quit = true,
            ["invite", name] => {
                let link = match requests.call(&Request::Invite { name: name.to_string() }).await? {
                    Reply::Invite { link } => link,
                    Reply::Error { message } => bail!("{message}"),
                    other => bail!(t!("err-unexpected", reply = format!("{other:?}"))),
                };
                let qr = qr(&link.to_ascii_uppercase());
                self.overlay = Some(Overlay::Invite { name: name.to_string(), link, qr });
            }
            ["code", name] => {
                let code = match requests.call(&Request::InviteCode { name: name.to_string() }).await? {
                    Reply::Code { code } => code,
                    Reply::Error { message } => bail!("{message}"),
                    other => bail!(t!("err-unexpected", reply = format!("{other:?}"))),
                };
                self.codes.push(name.to_string());
                self.overlay = Some(Overlay::Text {
                    title: t!("code-title", name = name),
                    body: vec![
                        t!("code-help"),
                        String::new(),
                        format!("    {}", code.replace('-', "  ")),
                        String::new(),
                        t!("code-after", name = name),
                    ],
                });
            }
            ["accept", link, name] => self.long(Request::Accept { link: link.to_string(), name: name.to_string() }, updates),
            ["acceptcode", code, name] => self.long(Request::AcceptCode { code: code.to_string(), name: name.to_string() }, updates),
            ["verify"] => {
                let name = self.conversations.get(self.selected).with_context(|| t!("tui-no-selection"))?.name.clone();
                let number = match requests.call(&Request::Verify { name: name.clone() }).await? {
                    Reply::SafetyNumber { number, .. } => number,
                    Reply::Error { message } => bail!("{message}"),
                    other => bail!(t!("err-unexpected", reply = format!("{other:?}"))),
                };
                self.overlay = Some(Overlay::Verify { name, number });
            }
            ["rename", new @ ..] if !new.is_empty() => {
                let name = self.current()?;
                expect_done(requests.call(&Request::Rename { name, to: new.join(" ") }).await)?;
            }
            ["mute", rest @ ..] => {
                const HOUR: u64 = 3_600_000;
                let until = match rest {
                    [] => u64::MAX,
                    ["1h"] => now_millis() + HOUR,
                    ["8h"] => now_millis() + 8 * HOUR,
                    _ => bail!(t!("tui-mute-usage")),
                };
                let name = self.current()?;
                self.set_person(&name, |p| p.muted_until = Some(until), requests).await;
                self.status = if until == u64::MAX { t!("tui-muted-forever", name = name) } else { t!("tui-muted-until", name = name, time = clock(until)) };
            }
            ["unmute"] => {
                let name = self.current()?;
                self.set_person(&name, |p| p.muted_until = None, requests).await;
                self.status = t!("tui-unmuted", name = name);
            }
            ["remove"] => self.overlay = Some(Overlay::Remove { name: self.current()? }),
            _ => bail!(t!("tui-unknown-command")),
        }
        Ok(())
    }

    fn current(&self) -> Result<String> {
        Ok(self.conversations.get(self.selected).with_context(|| t!("tui-no-selection"))?.name.clone())
    }

    /// Changes what we keep about them; the daemon's PersonChanged event
    /// brings the result back.
    async fn set_person(&mut self, name: &str, change: impl FnOnce(&mut Person), requests: &mut Client) {
        let Some(c) = self.conversations.iter().find(|c| c.name == name) else { return };
        let mut person = c.person.clone();
        change(&mut person);
        if let Err(e) = expect_done(requests.call(&Request::SetPerson { name: name.into(), person }).await) {
            self.status = format!("{e:#}");
        }
    }

    /// A request that can take minutes runs on its own connection, so the
    /// interface stays usable meanwhile.
    fn long(&mut self, request: Request, updates: &mpsc::UnboundedSender<Update>) {
        self.status = t!("tui-looking");
        let (dir, updates) = (self.dir.clone(), updates.clone());
        tokio::spawn(async move {
            let status = match Client::connect(&dir).await {
                Ok(mut client) => match client.call(&request).await {
                    Ok(Reply::Added { name }) => t!("added", name = name),
                    Ok(Reply::Error { message }) => message,
                    Ok(other) => t!("err-unexpected", reply = format!("{other:?}")),
                    Err(e) => format!("{e:#}"),
                },
                Err(e) => format!("{e:#}"),
            };
            let _ = updates.send(Update::Done(status));
        });
    }

    async fn on_update(&mut self, update: Update, requests: &mut Client) {
        match update {
            Update::Done(status) => self.status = status,
            Update::DaemonGone => {
                self.status = t!("daemon-stopped");
                self.quit = true;
            }
            Update::Event(Event::Message { from, id, time, text }) => {
                if self.typing.as_ref().is_some_and(|t| t.0 == from) {
                    self.typing = None;
                }
                let selected = self.selected;
                let focused = self.focused;
                let mut read = false;
                if let Some((i, c)) = self.conversations.iter_mut().enumerate().find(|(_, c)| c.name == from) {
                    c.messages.push(HistoryItem { id, outgoing: false, time, text, delivered: true, read: false });
                    if i == selected && focused {
                        read = true;
                    } else {
                        c.unread += 1;
                        if c.person.muted_until.is_none_or(|until| until <= now_millis()) {
                            // The name, never the text: a notification can show on a locked screen.
                            self.notify = Some(t!("notify-from", name = from));
                        }
                    }
                }
                if read {
                    self.mark_read(requests).await;
                }
            }
            Update::Event(Event::Sent { to, id, time, text }) => {
                if let Some(c) = self.conversations.iter_mut().find(|c| c.name == to) {
                    c.messages.push(HistoryItem { id, outgoing: true, time, text, delivered: false, read: false });
                }
            }
            Update::Event(Event::Delivered { to, id }) => {
                if let Some(m) = self
                    .conversations
                    .iter_mut()
                    .filter(|c| c.name == to)
                    .flat_map(|c| c.messages.iter_mut())
                    .find(|m| m.id == id)
                {
                    m.delivered = true;
                }
            }
            Update::Event(Event::Read { to, ids }) => {
                for m in self.conversations.iter_mut().filter(|c| c.name == to).flat_map(|c| c.messages.iter_mut()) {
                    if ids.contains(&m.id) {
                        m.read = true;
                    }
                }
            }
            Update::Event(Event::ContactAdded { name }) => {
                self.status = t!("now-contact", name = name);
                if let Err(e) = self.reload_contacts(requests).await {
                    self.status = format!("{e:#}");
                }
            }
            Update::Event(Event::ContactRenamed { name, to }) => {
                if let Some(c) = self.conversations.iter_mut().find(|c| c.name == name) {
                    self.status = t!("tui-renamed", name = name, to = to);
                    c.name = to;
                }
            }
            Update::Event(Event::ContactRemoved { name }) => {
                self.conversations.retain(|c| c.name != name);
                self.selected = self.selected.min(self.conversations.len().saturating_sub(1));
                self.status = t!("removed", name = name);
            }
            Update::Event(Event::PersonChanged { name, person }) => {
                if let Some(c) = self.conversations.iter_mut().find(|c| c.name == name) {
                    c.person = person;
                }
            }
            // The route is the graphical interface's.
            // Except that one side must update.
            Update::Event(Event::Link { name, state: LinkState::Older }) => self.status = t!("link-older", name = name),
            Update::Event(Event::Link { name, state: LinkState::Newer }) => self.status = t!("link-newer", name = name),
            Update::Event(Event::Network { .. } | Event::Link { .. } | Event::ReadReceipts { .. } | Event::TypingIndicators { .. }) => {}
            Update::Event(Event::Close { interface: Interface::Terminal }) => self.quit = true,
            Update::Event(Event::Show | Event::Close { .. }) => {}
            Update::Event(Event::Typing { from }) => self.typing = Some((from, Instant::now() + TYPING_SHOWN)),
            Update::Event(Event::Fingerprint { name, safety_number }) => {
                self.notify = Some(t!("notify-used-invite", name = name));
                self.overlay = Some(if self.codes.contains(&name) {
                    Overlay::Confirm { name, number: safety_number }
                } else {
                    Overlay::Text {
                        title: t!("waiting-title", name = name),
                        body: [vec![t!("waiting-help", name = name), String::new()], number_lines(&safety_number)].concat(),
                    }
                });
            }
        }
    }

    async fn reload_contacts(&mut self, requests: &mut Client) -> Result<()> {
        let contacts = match requests.call(&Request::Contacts).await? {
            Reply::Contacts { contacts } => contacts,
            Reply::Error { message } => bail!("{message}"),
            other => bail!(t!("err-unexpected", reply = format!("{other:?}"))),
        };
        for c in contacts {
            if !self.conversations.iter().any(|known| known.name == c.name) {
                // Loaded now: the list shows each one's last message.
                let messages = match requests.call(&Request::History { name: c.name.clone(), limit: HISTORY }).await? {
                    Reply::History { messages } => messages,
                    Reply::Error { message } => bail!("{message}"),
                    other => bail!(t!("err-unexpected", reply = format!("{other:?}"))),
                };
                self.conversations.push(Conversation { name: c.name, person: c.person, unread: 0, messages, loaded: true });
            }
        }
        Ok(())
    }

    async fn select(&mut self, index: usize, requests: &mut Client) {
        if self.conversations.is_empty() {
            return;
        }
        self.selected = index % self.conversations.len();
        self.scroll = 0;
        let c = &mut self.conversations[self.selected];
        c.unread = 0;
        if !c.loaded {
            match requests.call(&Request::History { name: c.name.clone(), limit: HISTORY }).await {
                Ok(Reply::History { messages }) => {
                    c.messages = messages;
                    c.loaded = true;
                }
                Ok(Reply::Error { message }) => self.status = message,
                Ok(other) => self.status = t!("err-unexpected", reply = format!("{other:?}")),
                Err(e) => self.status = format!("{e:#}"),
            }
        }
        self.mark_read(requests).await;
    }

    /// What the open conversation shows is read (the daemon tells them only
    /// with read receipts on).
    async fn mark_read(&mut self, requests: &mut Client) {
        let Some(c) = self.conversations.get(self.selected) else { return };
        match requests.call(&Request::MarkRead { name: c.name.clone() }).await {
            Ok(Reply::Done) => {}
            Ok(Reply::Error { message }) => self.status = message,
            Ok(other) => self.status = t!("err-unexpected", reply = format!("{other:?}")),
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        frame.render_widget(Block::new().style(Style::new().fg(ONIONSKIN).bg(NIGHT)), area);
        let [list, right] = Layout::horizontal([Constraint::Length(SIDEBAR), Constraint::Min(16)]).areas(area);
        // The box: 2 columns of margin and 1 of padding on each side.
        let text_width = right.width.saturating_sub(6).max(1);
        let input_lines = wrap(&self.input, text_width as usize).len().clamp(1, 6) as u16;
        let [_, header, _, thread, _, composer, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(input_lines + 2),
            Constraint::Length(1),
        ])
        .areas(right);

        self.draw_list(frame, list);

        let current = self.conversations.get(self.selected);
        let margin = |r: Rect| Rect { x: r.x + 2, width: r.width.saturating_sub(4), ..r };
        let mut title = vec![Span::styled(current.map_or(String::new(), |c| c.name.clone()), Style::new().add_modifier(Modifier::BOLD))];
        if let Some(c) = current {
            if c.person.verified {
                title.push(Span::styled("  verified", Style::new().fg(LAMP)));
            }
            if c.person.muted_until.is_some_and(|until| until > now_millis()) {
                title.push(Span::styled("  muted", Style::new().fg(INK_SOFT)));
            }
            if self.typing.as_ref().is_some_and(|t| t.0 == c.name) {
                title.push(Span::styled(format!("  {}", t!("typing", name = c.name)), DIM));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(title)), margin(header));

        let thread = margin(thread);
        let mut lines = slips(current.map(|c| c.messages.as_slice()).unwrap_or_default(), thread.width as usize);
        let height = thread.height as usize;
        self.scroll = self.scroll.min(lines.len().saturating_sub(height));
        // Few messages sit at the bottom, by the box, as in any conversation.
        if lines.len() < height {
            lines.splice(0..0, std::iter::repeat_n(Line::default(), height - lines.len()));
        }
        let top = lines.len().saturating_sub(height + self.scroll);
        frame.render_widget(Paragraph::new(lines).scroll((top as u16, 0)), thread);

        // The box wraps by characters, the same way the cursor is placed.
        let field = margin(composer);
        frame.render_widget(Block::new().style(Style::new().bg(RAISED)), field);
        let text = Rect { x: field.x + 1, y: field.y + 1, width: field.width.saturating_sub(2), height: input_lines };
        let (row, col) = cursor_at(&self.input, self.cursor, text_width as usize);
        let first = row.saturating_sub(input_lines as usize - 1);
        if self.input.is_empty() {
            let hint = current.map_or_else(|| t!("tui-type-help"), |c| t!("write-to", name = c.name));
            frame.render_widget(Paragraph::new(Span::styled(hint, DIM)), text);
        } else {
            let visible: Vec<Line> = wrap(&self.input, text_width as usize).into_iter().skip(first).map(Line::from).collect();
            frame.render_widget(Paragraph::new(visible), text);
        }
        if self.overlay.is_none() {
            frame.set_cursor_position(Position::new(text.x + col as u16, text.y + (row - first) as u16));
        }
        let keys_hint = t!("tui-keys-hint");
        let status_text = if self.status.is_empty() { keys_hint.as_str() } else { self.status.as_str() };
        frame.render_widget(Paragraph::new(Span::styled(status_text, DIM)), margin(status));

        match &self.overlay {
            Some(Overlay::Text { title, body }) => popup(frame, title, body.iter().map(|l| Line::from(l.as_str())).collect()),
            Some(Overlay::Invite { name, link, qr }) => {
                let qr_width = qr.first().map_or(0, |l| l.chars().count());
                let mut lines: Vec<Line> = if qr.len() + 10 <= area.height as usize && qr_width + 6 <= area.width as usize {
                    qr.iter().map(|l| Line::from(l.as_str())).collect()
                } else {
                    vec![Line::styled(
                        t!("tui-qr-too-small", width = qr_width + 6, height = qr.len() + 10),
                        Style::new().fg(INK_SOFT),
                    )]
                };
                lines.extend([
                    Line::default(),
                    Line::from(link.as_str()),
                    Line::default(),
                    Line::styled(t!("tui-invite-help", name = name), Style::new().fg(INK_SOFT)),
                ]);
                popup(frame, &t!("invite-title", name = name), lines);
            }
            Some(Overlay::Confirm { name, number }) => {
                let mut lines = vec![Line::from(t!("confirm-help")), Line::default()];
                lines.extend(number_lines(number).into_iter().map(|l| Line::styled(l, Style::new().add_modifier(Modifier::BOLD))));
                lines.extend([
                    Line::default(),
                    Line::styled(t!("confirm-warning", name = name), Style::new().fg(POST_ON_PAPER).add_modifier(Modifier::BOLD)),
                    Line::default(),
                    Line::from(t!("tui-confirm-keep")),
                    Line::from(t!("tui-confirm-refuse")),
                ]);
                popup(frame, &t!("confirm-title", name = name), lines);
            }
            Some(Overlay::Verify { name, number }) => {
                let mut lines = vec![
                    Line::from(t!("safety-help")),
                    Line::default(),
                ];
                lines.extend(number_lines(number).into_iter().map(|l| Line::styled(l, Style::new().add_modifier(Modifier::BOLD))));
                lines.extend([
                    Line::default(),
                    Line::styled(t!("safety-warning", name = name), Style::new().fg(POST_ON_PAPER).add_modifier(Modifier::BOLD)),
                    Line::default(),
                    Line::from(t!("tui-verify-yes")),
                ]);
                popup(frame, &t!("safety-label", name = name), lines);
            }
            Some(Overlay::Remove { name }) => popup(
                frame,
                &t!("remove-title", name = name),
                vec![
                    Line::from(t!("remove-help", name = name)),
                    Line::default(),
                    Line::from(t!("tui-remove-keep")),
                    Line::styled(t!("tui-remove-wipe"), Style::new().fg(POST_ON_PAPER)),
                    Line::from(t!("tui-remove-no")),
                ],
            ),
            None => {}
        }
    }

    /// The people, each on two rows: name and time of the last message,
    /// then that message.
    fn draw_list(&self, frame: &mut ratatui::Frame, area: Rect) {
        frame.render_widget(Block::new().style(Style::new().bg(NIGHT_DEEP)), area);
        let inner = (area.width as usize).saturating_sub(4);
        let mut lines = vec![Line::default(), Line::styled("  Nightpost", Style::new().add_modifier(Modifier::BOLD)), Line::default()];
        for (i, c) in self.conversations.iter().enumerate() {
            let row = Style::new().bg(if i == self.selected { RAISED } else { NIGHT_DEEP });
            let last = c.messages.last();
            let when = last.map(|m| short_time(m.time)).unwrap_or_default();
            let name = fit(&c.name, inner.saturating_sub(cols(&when) + 1));
            let name_style = if c.unread > 0 { row.add_modifier(Modifier::BOLD).fg(Color::White) } else { row.add_modifier(Modifier::BOLD) };
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(" ", row),
                Span::styled(name, name_style),
                Span::styled(format!(" {when} "), row.fg(LAMP)),
            ]));
            let preview = match last {
                Some(m) if m.outgoing => t!("preview-you", text = m.text.lines().next().unwrap_or_default()),
                Some(m) => m.text.lines().next().unwrap_or_default().to_string(),
                None => t!("no-messages"),
            };
            let count = if c.unread > 0 { format!(" {} ", c.unread) } else { String::new() };
            let preview_style = if c.unread > 0 { row.fg(ONIONSKIN) } else { row.fg(LAMP) };
            let mut second = vec![Span::raw(" "), Span::styled(" ", row)];
            if count.is_empty() {
                second.push(Span::styled(fit(&preview, inner), preview_style));
            } else {
                second.push(Span::styled(fit(&preview, inner.saturating_sub(cols(&count) + 1)), preview_style));
                second.push(Span::styled(" ", row));
                second.push(Span::styled(count, Style::new().fg(Color::White).bg(POST).add_modifier(Modifier::BOLD)));
            }
            second.push(Span::styled(" ", row));
            lines.push(Line::from(second));
            lines.push(Line::default());
        }
        // Keep the selected one in view.
        let bottom = 3 + (self.selected + 1) * 3;
        let skip = bottom.saturating_sub(area.height as usize);
        frame.render_widget(Paragraph::new(lines).scroll((skip as u16, 0)), area);
    }
}

/// The conversation as slips of paper: theirs on the left on onionskin,
/// yours on the right in carbon, half a row of margin above and below each
/// (the half blocks), the time under the last of a run, the day between
/// days.
fn slips(messages: &[HistoryItem], width: usize) -> Vec<Line<'static>> {
    let widest = (width * 3 / 4).clamp(8, 72);
    let mut lines = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let day = date(m.time);
        if i == 0 || date(messages[i - 1].time) != day {
            lines.extend([Line::default(), Line::styled(day_label(m.time), DIM).alignment(Alignment::Center), Line::default()]);
        }
        let (paper, align) = if m.outgoing { (CARBON, Alignment::Right) } else { (ONIONSKIN, Alignment::Left) };
        let text = Style::new().bg(paper).fg(if m.outgoing { CARBON_INK } else { INK });
        let rows = wrap_words(&m.text, widest - 2);
        let inside = rows.iter().map(|r| cols(r)).max().unwrap_or(0);
        lines.push(Line::styled("▄".repeat(inside + 2), Style::new().fg(paper)).alignment(align));
        for r in rows {
            let pad = " ".repeat(inside - cols(&r));
            lines.push(Line::from(Span::styled(format!(" {r}{pad} "), text)).alignment(align));
        }
        lines.push(Line::styled("▀".repeat(inside + 2), Style::new().fg(paper)).alignment(align));
        let pending = m.outgoing && !m.delivered;
        let run_ends = messages.get(i + 1).is_none_or(|n| n.outgoing != m.outgoing || n.time.saturating_sub(m.time) > 5 * 60_000 || date(n.time) != day);
        if run_ends || pending {
            let meta = match (m.outgoing, pending, m.read) {
                (true, true, _) => t!("tui-not-delivered", time = clock(m.time)),
                (true, false, false) => t!("tui-delivered", time = clock(m.time)),
                (true, false, true) => t!("tui-read", time = clock(m.time)),
                (false, ..) => clock(m.time),
            };
            lines.push(Line::styled(meta, DIM).alignment(align));
            lines.push(Line::default());
        }
    }
    lines
}

/// A sheet of paper in the middle of the screen, as small as its content
/// and never wider than the screen: long lines wrap.
fn popup(frame: &mut ratatui::Frame, title: &str, lines: Vec<Line>) {
    let area = frame.area();
    let mut all = vec![Line::styled(title.to_string(), Style::new().add_modifier(Modifier::BOLD)), Line::default()];
    all.extend(lines);
    all.extend([Line::default(), Line::styled(t!("tui-esc-closes"), Style::new().fg(INK_SOFT))]);
    let width = (all.iter().map(Line::width).max().unwrap_or(0) as u16 + 6).min(area.width);
    let paragraph = Paragraph::new(all).wrap(Wrap { trim: false });
    let height = (paragraph.line_count(width.saturating_sub(6)) as u16 + 2).min(area.height);
    let rect = Rect { x: (area.width - width) / 2, y: (area.height - height) / 2, width, height };
    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph.block(Block::new().style(PAPER).padding(ratatui::widgets::Padding::new(3, 3, 1, 1))), rect);
}

/// The link as a QR code, two modules per character cell, ink on the paper
/// of the popup: not every phone reads one inverted.
fn qr(link: &str) -> Vec<String> {
    use qrcode::render::unicode::Dense1x2;
    match qrcode::QrCode::new(link.as_bytes()) {
        Ok(code) => code
            .render::<Dense1x2>()
            .quiet_zone(true)
            .build()
            .lines()
            .map(Into::into)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Puts `text` on the clipboard with OSC 52: the terminal does it for us, so
/// no X11 or Wayland library, and it works over SSH. tmux relays it with its
/// default `set-clipboard external`.
fn copy(text: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", data_encoding::BASE64.encode(text.as_bytes()));
    let _ = out.flush();
}

/// A bell, then the text as a desktop notification (OSC 9 and OSC 777;
/// terminals ignore the one they do not know). Inside tmux the notification
/// goes through its passthrough (`allow-passthrough on`); the bell stays
/// outside, so tmux flags the window either way.
fn notification(text: &str, tmux: bool) -> String {
    let text: String = text.chars().filter(|c| !c.is_control() && *c != ';').collect();
    let osc = format!("\x1b]9;{text}\x07\x1b]777;notify;Nightpost;{text}\x07");
    if tmux { format!("\x07\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b")) } else { format!("\x07{osc}") }
}

/// Logical lines cut to `width` columns, by character: what the input box
/// shows, row by row.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let (mut row, mut used) = (String::new(), 0);
        for c in line.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > width {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(c);
            used += w;
        }
        rows.push(row);
    }
    rows
}

/// Row and column of byte offset `cursor` in [`wrap`]'s rows.
fn cursor_at(text: &str, cursor: usize, width: usize) -> (usize, usize) {
    let rows = wrap(&text[..cursor], width);
    let col: usize = rows.last().map_or(0, |r| r.chars().map(|c| c.width().unwrap_or(0)).sum());
    // A full row puts the cursor at the start of the next one.
    if col >= width.max(1) { (rows.len(), 0) } else { (rows.len() - 1, col) }
}

fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

fn next_boundary(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(i, |c| i + c.len_utf8())
}

/// Display columns.
fn cols(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Exactly `width` columns: padded, or cut with an ellipsis.
fn fit(text: &str, width: usize) -> String {
    let mut out = String::new();
    if cols(text) <= width {
        out.push_str(text);
    } else {
        for c in text.chars() {
            if cols(&out) + c.width().unwrap_or(0) + 1 > width {
                break;
            }
            out.push(c);
        }
        out.push('…');
    }
    let pad = width.saturating_sub(cols(&out));
    out + &" ".repeat(pad)
}

/// Lines cut between words to fit `width` columns; a longer word is cut
/// by character, as [`wrap`] does.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        for word in line.split(' ') {
            if !row.is_empty() && cols(&row) + 1 + cols(word) > width {
                rows.push(std::mem::take(&mut row));
            } else if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
            if cols(&row) > width {
                let mut pieces = wrap(&row, width);
                row = pieces.pop().unwrap_or_default();
                rows.extend(pieces);
            }
        }
        rows.push(row);
    }
    rows
}

fn zoned(millis: u64) -> Option<jiff::Zoned> {
    Some(jiff::Timestamp::from_millisecond(millis as i64).ok()?.to_zoned(jiff::tz::TimeZone::system()))
}

fn date(millis: u64) -> Option<jiff::civil::Date> {
    zoned(millis).map(|z| z.date())
}

fn now_millis() -> u64 {
    jiff::Timestamp::now().as_millisecond() as u64
}

/// `Done`, or the daemon's error as ours.
fn expect_done(reply: Result<Reply>) -> Result<()> {
    match reply? {
        Reply::Done => Ok(()),
        Reply::Error { message } => bail!("{message}"),
        other => bail!(t!("err-unexpected", reply = format!("{other:?}"))),
    }
}

fn clock(millis: u64) -> String {
    zoned(millis).map_or("--:--".into(), |z| z.strftime("%H:%M").to_string())
}

fn today() -> Option<jiff::Zoned> {
    zoned(jiff::Timestamp::now().as_millisecond() as u64)
}

/// "Today", "Yesterday", "Monday 5 October", "5 October 2025".
fn day_label(millis: u64) -> String {
    let (Some(then), Some(today)) = (zoned(millis), today()) else { return String::new() };
    match (today.date() - then.date()).get_days() {
        0 => t!("today"),
        1 => t!("yesterday"),
        _ if then.year() == today.year() => {
            t!("date-this-year", weekday = i18n::weekday(then.weekday().to_monday_one_offset(), false), day = then.day(), month = i18n::month(then.month(), false))
        }
        _ => t!("date-other-year", day = then.day(), month = i18n::month(then.month(), false), year = then.year()),
    }
}

/// For the list: the time today, then "Yesterday", the weekday, the date.
fn short_time(millis: u64) -> String {
    let (Some(then), Some(today)) = (zoned(millis), today()) else { return String::new() };
    match (today.date() - then.date()).get_days() {
        0 => then.strftime("%H:%M").to_string(),
        1 => t!("yesterday"),
        2..=6 => i18n::weekday(then.weekday().to_monday_one_offset(), true),
        _ => t!("date-short", day = then.day(), month = i18n::month(then.month(), true)),
    }
}

/// The 60 digits as 3 rows of 4 groups, the way they are read out.
fn number_lines(number: &str) -> Vec<String> {
    let groups: Vec<String> = number.as_bytes().chunks(5).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
    groups.chunks(4).map(|row| format!("    {}", row.join("  "))).collect()
}

pub fn groups(number: &str) -> String {
    number.as_bytes().chunks(5).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_break_between_words_and_long_words_by_character() {
        assert_eq!(wrap_words("un deux trois", 8), ["un deux", "trois"]);
        assert_eq!(wrap_words("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap_words("a\n\nb", 4), ["a", "", "b"]);
        assert_eq!(fit("bonjour", 5), "bonj…");
        assert_eq!(fit("日本", 5), "日本 ");
    }

    #[test]
    fn wide_characters_wrap_and_place_the_cursor_by_columns() {
        // 日本 takes four columns: in a box three wide it breaks after 日.
        assert_eq!(wrap("a日本", 3), ["a日", "本"]);
        assert_eq!(cursor_at("a日本", "a日".len(), 3), (1, 0));
        assert_eq!(cursor_at("ab\ncd", 4, 10), (1, 1));
        assert_eq!(wrap("", 5), [""]);
    }

    #[test]
    fn notifications_pass_through_tmux_and_keep_the_bell_outside() {
        assert_eq!(notification("hi", false), "\x07\x1b]9;hi\x07\x1b]777;notify;Nightpost;hi\x07");
        let wrapped = notification("a;b\x1b", true);
        assert!(wrapped.starts_with("\x07\x1bPtmux;\x1b\x1b]9;ab\x07"));
        assert!(wrapped.ends_with("\x1b\\"));
    }
}
