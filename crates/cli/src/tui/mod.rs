//! The terminal interface: conversations on the left, the open one on the
//! right, a box to type in under it, keys at the bottom. It attaches to a
//! running daemon and keeps nothing itself, so closing it leaves the daemon
//! running and reopening it finds everything where it was.

mod panel;
mod view;

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture, Event as Term,
    EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::style::Print;
use crossterm::terminal::SetTitle;
use futures::StreamExt as _;
use nightpost_core::api::{Client, Event, HistoryItem, Interface, LinkState, Network, Person, Reply, Request};
use nightpost_core::prefs::Prefs;
use nightpost_core::t;
use ratatui::DefaultTerminal;
use ratatui::layout::{Position, Rect};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, interval, sleep_until};

pub use view::groups;

const HISTORY: u32 = 200;
/// As the graphical interface: a ping at most this often while typing,
/// shown this long on the other side.
const TYPING_EVERY: Duration = Duration::from_secs(4);
const TYPING_SHOWN: Duration = Duration::from_secs(5);
/// How long a message for another conversation stays in its bubble.
const NOTICE_SHOWN: Duration = Duration::from_secs(8);

struct Conversation {
    name: String,
    /// The note to ourselves: kept here, nobody to reach.
    note: bool,
    /// Seconds after which messages disappear; 0 for never.
    timer: u32,
    person: Person,
    unread: usize,
    messages: Vec<HistoryItem>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Thread,
    Input,
}

enum Overlay {
    /// The shortcuts, scrolled by this many lines when they do not fit.
    Help(usize),
    /// Ctrl+K: the conversations, filtered by what is typed.
    Picker { filter: String, selected: usize },
    /// Ctrl+R: the messages of this conversation with what is typed in them.
    Search { query: String, selected: usize },
    Text { title: String, body: Vec<String> },
    /// The recovery phrase: `c` copies it.
    Recovery { body: Vec<String>, phrase: String },
    /// Shown with its QR code when the terminal is big enough for all of it.
    Invite { name: String, link: String, qr: Vec<String> },
    /// Someone used our code: keep them only if the numbers match.
    Confirm { name: String, number: String, selected: usize },
    /// Their safety number, to mark compared.
    Verify { name: String, number: String, selected: usize },
    /// Remove them, maybe with the conversation.
    Remove { name: String, selected: usize },
    /// What we know of them, and what can be done about them.
    Info { name: String, selected: usize },
    /// An invitation or a code being used: it can take minutes, and goes on
    /// once this is closed.
    Looking { name: String, since: Instant },
    Settings(panel::Settings),
    Passphrase(panel::Passphrase),
}

enum Update {
    Event(Event),
    /// A long request (accepting an invitation) finished.
    Done(String),
    DaemonGone,
}

/// A command of the menu that opens on `/`.
struct Command {
    name: &'static str,
    /// What follows it, as `<name>`; `[…]` when optional.
    args: String,
    about: String,
}

impl Command {
    fn needs_args(&self) -> bool {
        self.args.starts_with('<')
    }
}

fn commands() -> Vec<Command> {
    let arg = |a: String| format!("<{a}>");
    let c = |name, args: String, about| Command { name, args, about };
    vec![
        c("/help", String::new(), t!("help-help")),
        c("/invite", arg(t!("arg-name")), t!("help-invite")),
        c("/code", arg(t!("arg-name")), t!("help-code")),
        c("/accept", format!("{} {}", arg(t!("arg-link")), arg(t!("arg-name"))), t!("help-accept")),
        c("/acceptcode", format!("{} {}", arg(t!("arg-code")), arg(t!("arg-name"))), t!("help-acceptcode")),
        c("/info", String::new(), t!("help-info")),
        c("/verify", String::new(), t!("help-verify")),
        c("/rename", arg(t!("arg-new-name")), t!("help-rename")),
        c("/mute", "[1h | 8h]".into(), t!("help-mute")),
        c("/unmute", String::new(), t!("help-unmute")),
        c("/remove", String::new(), t!("help-remove")),
        c("/timer", "[off | 1h | 8h | 1d | 1w | 4w]".into(), t!("help-timer")),
        c("/copy", String::new(), t!("help-copy")),
        c("/log", String::new(), t!("help-log")),
        c("/settings", String::new(), t!("help-settings")),
        c("/network", String::new(), t!("help-network")),
        c("/lock", String::new(), t!("help-lock")),
        c("/quit", String::new(), t!("help-quit")),
    ]
}

/// The commands for what is typed: those it starts, then those it is in.
fn matching(typed: &str) -> Vec<Command> {
    if !typed.starts_with('/') || typed.contains(char::is_whitespace) {
        return Vec::new();
    }
    let (mut first, mut then): (Vec<Command>, Vec<Command>) = commands().into_iter().filter(|c| c.name.contains(&typed[1..])).partition(|c| c.name.starts_with(typed));
    first.append(&mut then);
    first
}

struct App {
    dir: PathBuf,
    conversations: Vec<Conversation>,
    selected: usize,
    focus: Focus,
    input: String,
    /// Byte offset in `input`.
    cursor: usize,
    /// The command picked in the menu, and the first one it shows.
    menu: usize,
    menu_top: usize,
    /// Lines scrolled up from the end of the conversation.
    scroll: usize,
    /// A message to bring into view (its index), set by the search.
    jump: Option<usize>,
    status: String,
    overlay: Option<Overlay>,
    /// Names we gave a code to and wait for.
    codes: Vec<String>,
    network: Network,
    links: HashMap<String, LinkState>,
    /// A message for another conversation: from, first line, when.
    notice: Option<(String, String, Instant)>,
    focused: bool,
    /// A notification to give after the frame: title, then text.
    notify: Option<(String, String)>,
    prefs: Prefs,
    read_receipts: bool,
    typing_indicators: bool,
    quit: bool,
    /// Who is writing to us, until when. ponytail: one person at a time,
    /// the last one heard; a map if two at once ever matters here.
    typing: Option<(String, Instant)>,
    /// When we last said we are writing.
    pinged: Option<Instant>,
    /// The terminal tells Shift+Enter from Enter.
    keys_told_apart: bool,
    /// Where the list, the conversation and the box were last drawn, and
    /// the first conversation the list showed: for the mouse.
    areas: [Rect; 3],
    list_top: usize,
    started: Instant,
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

    // Asked before the screen changes: the answer comes back as input.
    let kitty = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    let tmux = std::env::var_os("TMUX").is_some();
    let mut app = App {
        dir: dir.to_path_buf(),
        conversations: Vec::new(),
        selected: 0,
        focus: Focus::Input,
        input: String::new(),
        cursor: 0,
        menu: 0,
        menu_top: 0,
        scroll: 0,
        jump: None,
        status: String::new(),
        overlay: None,
        codes: Vec::new(),
        network: Network::default(),
        links: HashMap::new(),
        notice: None,
        focused: true,
        notify: None,
        prefs: Prefs::load(dir),
        read_receipts: false,
        typing_indicators: false,
        quit: false,
        typing: None,
        pinged: None,
        keys_told_apart: kitty || tmux,
        areas: [Rect::default(); 3],
        list_top: 0,
        started: Instant::now(),
    };
    app.reload_contacts(&mut requests).await?;
    if let Ok(Reply::Status { network, links }) = requests.call(&Request::Status).await {
        app.network = network;
        app.links = links.into_iter().collect();
    }
    if let Ok(Reply::ReadReceipts { on }) = requests.call(&Request::ReadReceipts { on: None }).await {
        app.read_receipts = on;
    }
    if let Ok(Reply::TypingIndicators { on }) = requests.call(&Request::TypingIndicators { on: None }).await {
        app.typing_indicators = on;
    }
    if app.conversations.is_empty() {
        app.focus = Focus::List;
    }
    // The note is first; on starting, the first person, if there is one.
    app.select(usize::from(app.conversations.len() > 1), &mut requests).await;

    let mut terminal = ratatui::init();
    // The tab says Nightpost, not the program's path; the terminal's own
    // title is saved first (CSI 22 t) and given back after (CSI 23 t).
    crossterm::execute!(std::io::stdout(), Print("\x1b[22;0t"), SetTitle("Nightpost"), EnableFocusChange, EnableBracketedPaste, EnableMouseCapture)?;
    // Shift+Enter apart from Enter. Asked once on the full screen, which
    // keeps its own setting (kitty's protocol); tmux speaks xterm's
    // modifyOtherKeys instead, at level 1: at 2, Ctrl+C would come as a
    // sequence we do not read.
    if kitty {
        crossterm::execute!(std::io::stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    } else if tmux {
        crossterm::execute!(std::io::stdout(), Print("\x1b[>4;1m"))?;
    }
    let result = app.run(&mut terminal, &mut requests, &mut updated, &updates).await;
    if kitty {
        let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    } else if tmux {
        let _ = crossterm::execute!(std::io::stdout(), Print("\x1b[>4m"));
    }
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture, DisableFocusChange, DisableBracketedPaste, Print("\x1b[23;0t"));
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
        // Spinners and the bubble's time: only while one is shown.
        let mut frames = interval(Duration::from_millis(120));
        while !self.quit {
            if self.notice.as_ref().is_some_and(|n| n.2.elapsed() > NOTICE_SHOWN) {
                self.notice = None;
            }
            terminal.draw(|frame| self.draw(frame))?;
            if let Some((title, text)) = self.notify.take() {
                // After the frame, never inside it.
                let mut out = std::io::stdout();
                let _ = out.write_all(notification(&title, &text, std::env::var_os("TMUX").is_some()).as_bytes());
                let _ = out.flush();
            }
            tokio::select! {
                Some(event) = keys.next() => match event? {
                    Term::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key, requests, updates).await,
                    Term::Mouse(mouse) => self.on_mouse(mouse, requests).await,
                    Term::Paste(text) => {
                        if !self.on_panel_paste(&text) && self.overlay.is_none() {
                            self.focus = Focus::Input;
                            self.insert(&text.replace("\r\n", "\n"));
                        }
                    }
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
                _ = frames.tick(), if self.moving() => {}
            }
        }
        Ok(())
    }

    /// Something on screen changes by itself.
    fn moving(&self) -> bool {
        self.notice.is_some()
            || self.typing.is_some()
            || self.current().is_some_and(|c| self.sending(c))
            || matches!(self.overlay, Some(Overlay::Looking { .. } | Overlay::Settings(_)))
    }

    async fn on_key(&mut self, key: KeyEvent, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        self.status.clear();
        match key.code {
            KeyCode::Char('c' | 'q') if ctrl => return self.quit = true,
            KeyCode::Char('k') if ctrl => return self.overlay = Some(Overlay::Picker { filter: String::new(), selected: 0 }),
            KeyCode::Char('f' | 'r') if ctrl && self.overlay.is_none() && self.current().is_some() => return self.overlay = Some(Overlay::Search { query: String::new(), selected: 0 }),
            _ => {}
        }
        if self.overlay.is_some() {
            return self.on_overlay_key(key, requests).await;
        }
        let empty = self.input.is_empty();
        match key.code {
            // With the command menu open, Tab completes (in on_input_key).
            KeyCode::Tab if self.focus != Focus::Input || matching(&self.input).is_empty() => return self.focus = next(self.focus, 1),
            KeyCode::BackTab => return self.focus = next(self.focus, 2),
            KeyCode::PageUp => return self.scroll += 10,
            KeyCode::PageDown => return self.scroll = self.scroll.saturating_sub(10),
            // Clamped when drawn.
            KeyCode::Home if self.focus != Focus::Input || empty => return self.scroll = usize::MAX,
            KeyCode::End if self.focus != Focus::Input || empty => return self.scroll = 0,
            _ => {}
        }
        match self.focus {
            Focus::List => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.select(self.selected + self.conversations.len().saturating_sub(1), requests).await,
                KeyCode::Down | KeyCode::Char('j') => self.select(self.selected + 1, requests).await,
                KeyCode::Enter | KeyCode::Char('i') => self.focus = Focus::Input,
                KeyCode::Char('/') => {
                    self.focus = Focus::Input;
                    self.insert("/");
                }
                KeyCode::Char('v') => self.run_command("verify", requests, updates).await,
                KeyCode::Char('?') => self.overlay = Some(Overlay::Help(0)),
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Focus::Thread => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.scroll += 1,
                KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::Esc => self.focus = Focus::List,
                KeyCode::Char('/') => {
                    self.focus = Focus::Input;
                    self.insert("/");
                }
                KeyCode::Char('?') => self.overlay = Some(Overlay::Help(0)),
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Focus::Input => self.on_input_key(key, requests, updates).await,
        }
    }

    async fn on_input_key(&mut self, key: KeyEvent, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let menu = matching(&self.input);
        if !menu.is_empty() {
            match key.code {
                KeyCode::Up => return self.menu = self.menu.saturating_sub(1),
                KeyCode::Down => return self.menu = (self.menu + 1).min(menu.len() - 1),
                KeyCode::Tab => return self.complete(&menu[self.menu.min(menu.len() - 1)]),
                KeyCode::Enter => {
                    let picked = &menu[self.menu.min(menu.len() - 1)];
                    if picked.needs_args() {
                        return self.complete(picked);
                    }
                    self.input = picked.name.to_string();
                    return self.submit(requests, updates).await;
                }
                KeyCode::Esc => return self.set_input(""),
                _ => {}
            }
        }
        match key.code {
            KeyCode::Esc => self.focus = Focus::List,
            // Enter with Shift, Ctrl or Alt where the terminal tells them
            // apart; Ctrl+J (a line feed, where Enter is a carriage return)
            // and a backslash before Enter everywhere.
            KeyCode::Enter if key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT) => self.insert("\n"),
            KeyCode::Char('j') if ctrl => self.insert("\n"),
            KeyCode::Enter if self.input[..self.cursor].ends_with('\\') => {
                self.input.remove(self.cursor - 1);
                self.cursor -= 1;
                self.insert("\n");
            }
            KeyCode::Enter => self.submit(requests, updates).await,
            KeyCode::Char('?') if self.input.is_empty() => self.overlay = Some(Overlay::Help(0)),
            KeyCode::Left => self.cursor = prev_boundary(&self.input, self.cursor),
            KeyCode::Right => self.cursor = next_boundary(&self.input, self.cursor),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                let start = prev_boundary(&self.input, self.cursor);
                self.input.replace_range(start..self.cursor, "");
                self.cursor = start;
                self.menu = 0;
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

    async fn on_overlay_key(&mut self, key: KeyEvent, requests: &mut Client) {
        if self.on_panel_key(key, requests).await {
            return;
        }
        let found = match &self.overlay {
            Some(Overlay::Picker { filter, .. }) => self.found(filter),
            _ => Vec::new(),
        };
        let hits = match &self.overlay {
            Some(Overlay::Search { query, .. }) => self.hits(query),
            _ => Vec::new(),
        };
        let help_len = match self.overlay {
            Some(Overlay::Help(_)) => self.help(crossterm::terminal::size().map_or(80, |s| s.0)).len(),
            _ => 0,
        };
        match self.overlay.as_mut() {
            Some(Overlay::Help(s)) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => *s = s.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *s = (*s + 1).min(help_len),
                KeyCode::PageUp => *s = s.saturating_sub(10),
                KeyCode::PageDown => *s = (*s + 10).min(help_len),
                KeyCode::Home => *s = 0,
                KeyCode::End => *s = help_len,
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?') => self.overlay = None,
                _ => {}
            },
            Some(Overlay::Confirm { name, selected, .. }) => {
                let Some(answer) = panel::pick(key.code, selected, 2) else { return };
                let (name, keep) = (name.clone(), answer == 0);
                self.overlay = None;
                self.codes.retain(|n| *n != name);
                self.status = match requests.call(&Request::Confirm { name: name.clone(), keep }).await {
                    Ok(Reply::Error { message }) => message,
                    Err(e) => format!("{e:#}"),
                    Ok(_) if keep => t!("kept", name = name),
                    Ok(_) => t!("turned-down", name = name),
                };
            }
            Some(Overlay::Verify { name, selected, .. }) => {
                let Some(answer) = panel::pick(key.code, selected, 2) else { return };
                let name = name.clone();
                self.overlay = None;
                if answer == 0 {
                    self.set_person(&name, |p| p.verified = true, requests).await;
                }
            }
            Some(Overlay::Remove { name, selected }) => {
                let Some(answer) = panel::pick(key.code, selected, 3) else { return };
                let name = name.clone();
                self.overlay = None;
                if answer == 2 {
                    return;
                }
                // The daemon's ContactRemoved event updates the list.
                if let Err(e) = expect_done(requests.call(&Request::Remove { name, wipe: answer == 1 }).await) {
                    self.status = format!("{e:#}");
                }
            }
            Some(Overlay::Recovery { phrase, .. }) if key.code == KeyCode::Char('c') => {
                copy(phrase);
                self.status = t!("tui-phrase-copied");
            }
            Some(Overlay::Invite { link, .. }) if key.code == KeyCode::Char('c') => {
                copy(link);
                self.status = t!("tui-link-copied");
            }
            Some(Overlay::Search { query, selected }) => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(hits.len().saturating_sub(1)),
                KeyCode::Backspace => {
                    query.pop();
                    *selected = 0;
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    *selected = 0;
                }
                KeyCode::Enter => {
                    self.jump = hits.get(*selected).copied();
                    self.overlay = None;
                    if self.jump.is_some() {
                        self.focus = Focus::Thread;
                    }
                }
                _ => {}
            },
            Some(Overlay::Picker { filter, selected }) => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(found.len().saturating_sub(1)),
                KeyCode::Backspace => {
                    filter.pop();
                    *selected = 0;
                }
                KeyCode::Char(c) => {
                    filter.push(c);
                    *selected = 0;
                }
                KeyCode::Enter => {
                    let picked = found.get(*selected).copied();
                    self.overlay = None;
                    if let Some(i) = picked {
                        self.select(i, requests).await;
                        self.focus = Focus::Input;
                    }
                }
                _ => {}
            },
            _ => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.overlay = None;
                }
            }
        }
    }

    async fn on_mouse(&mut self, mouse: MouseEvent, requests: &mut Client) {
        let at = Position::new(mouse.column, mouse.row);
        let [list, thread, input] = self.areas;
        let menu = if self.focus == Focus::Input { matching(&self.input).len() } else { 0 };
        match mouse.kind {
            MouseEventKind::ScrollUp if menu > 0 => self.menu = self.menu.saturating_sub(1),
            MouseEventKind::ScrollDown if menu > 0 => self.menu = (self.menu + 1).min(menu - 1),
            MouseEventKind::ScrollUp => self.scroll += 3,
            MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(3),
            MouseEventKind::Down(MouseButton::Left) if self.overlay.is_none() => {
                if list.contains(at) {
                    self.focus = Focus::List;
                    // Two rows each, under the frame.
                    let row = self.list_top + usize::from(mouse.row.saturating_sub(list.y + 1) / 2);
                    if row < self.conversations.len() && mouse.row > list.y {
                        self.select(row, requests).await;
                    }
                } else if thread.contains(at) {
                    self.focus = Focus::Thread;
                } else if input.contains(at) {
                    self.focus = Focus::Input;
                }
            }
            _ => {}
        }
    }

    fn complete(&mut self, command: &Command) {
        self.set_input(&format!("{} ", command.name));
    }

    fn set_input(&mut self, text: &str) {
        self.input = text.to_string();
        self.cursor = self.input.len();
        self.menu = 0;
    }

    /// At most one ping every few seconds; the daemon drops it unless
    /// typing indicators are on.
    async fn typed(&mut self, requests: &mut Client) {
        if self.input.starts_with('/') || self.pinged.is_some_and(|at| at.elapsed() < TYPING_EVERY) {
            return;
        }
        if let Some(c) = self.conversations.get(self.selected).filter(|c| !c.note) {
            self.pinged = Some(Instant::now());
            let _ = requests.call(&Request::Typing { name: c.name.clone() }).await;
        }
    }

    fn insert(&mut self, text: &str) {
        self.input.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.menu = 0;
    }

    async fn submit(&mut self, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) {
        let input = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.menu = 0;
        let line = input.trim();
        if line.is_empty() {
            return;
        }
        if let Some(command) = line.strip_prefix('/') {
            return self.run_command(command, requests, updates).await;
        }
        let Some(to) = self.conversations.get(self.selected).map(|c| c.name.clone()) else {
            self.status = t!("tui-no-conversation");
            self.set_input(&input);
            return;
        };
        match requests.call(&Request::Send { to, text: input.clone() }).await {
            // The message itself comes back as Event::Sent.
            Ok(Reply::Sent { .. }) => self.scroll = 0,
            Ok(Reply::Error { message }) => {
                self.status = message;
                self.set_input(&input);
            }
            Ok(other) => self.status = t!("err-unexpected", reply = format!("{other:?}")),
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    async fn run_command(&mut self, command: &str, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) {
        if let Err(e) = self.command(command, requests, updates).await {
            self.status = format!("{e:#}");
        }
    }

    async fn command(&mut self, command: &str, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) -> Result<()> {
        let words: Vec<&str> = command.split_whitespace().collect();
        match words.as_slice() {
            ["help"] => self.overlay = Some(Overlay::Help(0)),
            ["quit"] => self.quit = true,
            ["lock"] => self.lock(requests).await,
            ["copy"] => {
                // What they wrote; in the note, what we did.
                let last = self.current().and_then(|c| c.messages.iter().rev().find(|m| c.note || !m.outgoing)).map(|m| m.text.clone());
                let Some(text) = last else { bail!(t!("tui-nothing-to-copy")) };
                copy(&text);
                self.status = t!("tui-message-copied");
            }
            ["log"] => {
                let text = std::fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                // Nothing in it that could move the cursor or draw outside the box.
                let body = lines[lines.len().saturating_sub(30)..].iter().map(|l| l.chars().filter(|c| !c.is_control()).collect()).collect();
                self.overlay = Some(Overlay::Text { title: t!("log-title"), body });
            }
            ["settings"] => self.open_settings(0, requests).await,
            ["network"] => self.open_settings(panel::NETWORK, requests).await,
            ["info"] => self.overlay = Some(Overlay::Info { name: self.current_name()?, selected: 0 }),
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
            ["accept", link, name] => self.long(name, Request::Accept { link: link.to_string(), name: name.to_string() }, updates),
            ["acceptcode", code, name] => self.long(name, Request::AcceptCode { code: code.to_string(), name: name.to_string() }, updates),
            ["verify" | "mute" | "unmute" | "remove" | "info" | "timer", ..] if self.current().is_some_and(|c| c.note) => bail!(t!("tui-not-for-note")),
            ["timer", rest @ ..] => {
                let name = self.current_name()?;
                let seconds = match rest {
                    [] => {
                        let now = self.current().map_or(0, |c| c.timer);
                        self.status = t!("tui-timer-now", timer = nightpost_core::api::timer_label(now).to_lowercase());
                        return Ok(());
                    }
                    ["off"] => 0,
                    ["1h"] => 3_600,
                    ["8h"] => 28_800,
                    ["1d"] => 86_400,
                    ["1w"] => 604_800,
                    ["4w"] => 2_419_200,
                    _ => bail!(t!("tui-timer-usage")),
                };
                // The daemon's TimerChanged event updates the conversation and says so.
                expect_done(requests.call(&Request::Timer { name, seconds }).await)?;
            }
            ["verify"] => self.verify(requests).await?,
            ["rename", new @ ..] if !new.is_empty() => {
                let name = self.current_name()?;
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
                let name = self.current_name()?;
                self.set_person(&name, |p| p.muted_until = Some(until), requests).await;
                self.status = if until == u64::MAX { t!("tui-muted-forever", name = name) } else { t!("tui-muted-until", name = name, time = view::clock(until)) };
            }
            ["unmute"] => {
                let name = self.current_name()?;
                self.set_person(&name, |p| p.muted_until = None, requests).await;
                self.status = t!("tui-unmuted", name = name);
            }
            // Keeping them is the answer given by default.
            ["remove"] => self.overlay = Some(Overlay::Remove { name: self.current_name()?, selected: 2 }),
            _ => bail!(t!("tui-unknown-command")),
        }
        Ok(())
    }

    /// Their safety number, to compare.
    async fn verify(&mut self, requests: &mut Client) -> Result<()> {
        let name = self.current_name()?;
        let number = match requests.call(&Request::Verify { name: name.clone() }).await? {
            Reply::SafetyNumber { number, .. } => number,
            Reply::Error { message } => bail!("{message}"),
            other => bail!(t!("err-unexpected", reply = format!("{other:?}"))),
        };
        self.overlay = Some(Overlay::Verify { name, number, selected: 0 });
        Ok(())
    }

    fn current(&self) -> Option<&Conversation> {
        self.conversations.get(self.selected)
    }

    fn current_name(&self) -> Result<String> {
        Ok(self.current().with_context(|| t!("tui-no-selection"))?.name.clone())
    }

    /// The messages of this conversation with `query` in them, the latest first.
    fn hits(&self, query: &str) -> Vec<usize> {
        let query = query.to_lowercase();
        let Some(c) = self.current().filter(|_| !query.is_empty()) else { return Vec::new() };
        (0..c.messages.len()).rev().filter(|&i| c.messages[i].text.to_lowercase().contains(&query)).collect()
    }

    /// The conversations whose name has `filter` in it, as indices.
    fn found(&self, filter: &str) -> Vec<usize> {
        let filter = filter.to_lowercase();
        (0..self.conversations.len()).filter(|&i| self.conversations[i].name.to_lowercase().contains(&filter)).collect()
    }

    /// A message to them waits while a link to them is being made: the
    /// spinner turns only then, as in the window.
    fn sending(&self, c: &Conversation) -> bool {
        c.messages.iter().any(|m| m.outgoing && !m.delivered) && matches!(self.links.get(&c.name), Some(LinkState::Trying | LinkState::Open))
    }

    /// Reachable now: Tor is up and the last link to them worked.
    fn reachable(&self, name: &str) -> bool {
        self.network.percent == 100 && !self.network.blocked() && matches!(self.links.get(name), Some(LinkState::Open | LinkState::Idle))
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
    fn long(&mut self, name: &str, request: Request, updates: &mpsc::UnboundedSender<Update>) {
        self.overlay = Some(Overlay::Looking { name: name.to_string(), since: Instant::now() });
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
            Update::Done(status) => {
                if matches!(self.overlay, Some(Overlay::Looking { .. })) {
                    self.overlay = None;
                }
                self.status = status;
            }
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
                    let first = text.lines().next().unwrap_or_default().to_string();
                    c.messages.push(HistoryItem { id, outgoing: false, time, text, delivered: true, read: false });
                    if i == selected && focused {
                        read = true;
                    } else {
                        c.unread += 1;
                        if i != selected {
                            self.notice = Some((from.clone(), first, Instant::now()));
                        }
                        if c.person.muted_until.is_none_or(|until| until <= now_millis()) {
                            // As the settings say: a notification can show on a locked screen.
                            let prefs = Prefs::load(&self.dir);
                            self.notify = prefs.notify.then(|| match (prefs.show_name, prefs.show_text) {
                                (true, true) => (from.clone(), nightpost_core::text::notification(c.messages.last().map_or("", |m| m.text.as_str()))),
                                (true, false) => ("Nightpost".into(), t!("notify-from", name = from)),
                                _ => ("Nightpost".into(), t!("notify-new")),
                            });
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
                    c.name = to.clone();
                }
                if let Some(state) = self.links.remove(&name) {
                    self.links.insert(to, state);
                }
            }
            Update::Event(Event::ContactRemoved { name }) => {
                self.conversations.retain(|c| c.name != name);
                self.links.remove(&name);
                self.selected = self.selected.min(self.conversations.len().saturating_sub(1));
                self.status = t!("removed", name = name);
            }
            Update::Event(Event::TimerChanged { name, seconds }) => {
                if let Some(c) = self.conversations.iter_mut().find(|c| c.name == name) {
                    c.timer = seconds;
                    self.status = t!("tui-timer-set", name = name, timer = nightpost_core::api::timer_label(seconds).to_lowercase());
                }
            }
            Update::Event(Event::Expired { name }) => {
                if let Ok(Reply::History { messages }) = requests.call(&Request::History { name: name.clone(), limit: HISTORY }).await
                    && let Some(c) = self.conversations.iter_mut().find(|c| c.name == name)
                {
                    c.messages = messages;
                }
            }
            Update::Event(Event::PersonChanged { name, person }) => {
                if let Some(c) = self.conversations.iter_mut().find(|c| c.name == name) {
                    c.person = person;
                }
            }
            Update::Event(Event::Network { network }) => self.network = network,
            Update::Event(Event::Link { name, state }) => {
                // One side must update.
                match state {
                    LinkState::Older => self.status = t!("link-older", name = name),
                    LinkState::Newer => self.status = t!("link-newer", name = name),
                    _ => {}
                }
                self.links.insert(name, state);
            }
            Update::Event(Event::ReadReceipts { on }) => self.read_receipts = on,
            Update::Event(Event::TypingIndicators { on }) => self.typing_indicators = on,
            Update::Event(Event::Close { interface: Interface::Terminal }) => self.quit = true,
            Update::Event(Event::Show | Event::Close { .. }) => {}
            Update::Event(Event::Typing { from }) => self.typing = Some((from, Instant::now() + TYPING_SHOWN)),
            Update::Event(Event::Fingerprint { name, safety_number }) => {
                if Prefs::load(&self.dir).notify {
                    self.notify = Some(("Nightpost".into(), t!("notify-used-invite", name = name)));
                }
                self.overlay = Some(if self.codes.contains(&name) {
                    Overlay::Confirm { name, number: safety_number, selected: 0 }
                } else {
                    Overlay::Text {
                        title: t!("waiting-title", name = name),
                        body: [vec![t!("waiting-help", name = name), String::new()], view::number_lines(&safety_number)].concat(),
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
                self.conversations.push(Conversation { name: c.name, note: c.note, timer: c.timer, person: c.person, unread: 0, messages });
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
        if self.notice.as_ref().is_some_and(|n| n.0 == c.name) {
            self.notice = None;
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
}

/// The pane `steps` after `focus`, round the three.
fn next(focus: Focus, steps: usize) -> Focus {
    const ORDER: [Focus; 3] = [Focus::List, Focus::Thread, Focus::Input];
    let at = ORDER.iter().position(|f| *f == focus).unwrap_or(0);
    ORDER[(at + steps) % 3]
}

/// The link as a QR code, two modules per character cell.
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
fn notification(title: &str, text: &str, tmux: bool) -> String {
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_control() && *c != ';').collect() };
    let (title, text) = (clean(title), clean(text));
    // OSC 9 has no title: the name goes before the text.
    let alone = if title == "Nightpost" { text.clone() } else { t!("tui-notify-what", name = title, text = text) };
    let osc = format!("\x1b]9;{alone}\x07\x1b]777;notify;{title};{text}\x07");
    if tmux { format!("\x07\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b")) } else { format!("\x07{osc}") }
}

fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

fn next_boundary(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(i, |c| i + c.len_utf8())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_pass_through_tmux_and_keep_the_bell_outside() {
        assert_eq!(notification("Nightpost", "hi", false), "\x07\x1b]9;hi\x07\x1b]777;notify;Nightpost;hi\x07");
        assert_eq!(notification("bob", "yo", false), "\x07\x1b]9;bob: yo\x07\x1b]777;notify;bob;yo\x07");
        let wrapped = notification("Nightpost", "a;b\x1b", true);
        assert!(wrapped.starts_with("\x07\x1bPtmux;\x1b\x1b]9;ab\x07"));
        assert!(wrapped.ends_with("\x1b\\"));
    }

    #[test]
    fn the_menu_lists_what_a_command_starts_with_then_what_it_contains() {
        let names = |typed| matching(typed).iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("/ac"), ["/accept", "/acceptcode"]);
        // Starts with first, then contains.
        assert_eq!(names("/mute"), ["/mute", "/unmute"]);
        assert_eq!(names("/co"), ["/code", "/copy", "/acceptcode"]);
        // Arguments being typed: the menu is gone.
        assert!(names("/invite bob").is_empty());
        assert!(names("salut").is_empty());
        assert_eq!(names("/").len(), commands().len());
        assert!(commands().iter().find(|c| c.name == "/invite").unwrap().needs_args());
        assert!(!commands().iter().find(|c| c.name == "/mute").unwrap().needs_args());
    }

    #[test]
    fn tab_goes_round_the_panes() {
        assert!(next(Focus::List, 1) == Focus::Thread);
        assert!(next(Focus::Input, 1) == Focus::List);
        assert!(next(Focus::List, 2) == Focus::Input);
    }
}
