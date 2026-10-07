//! The terminal interface: conversations on the left, the selected one on
//! the right, a box to type in. It attaches to a running daemon and keeps
//! nothing itself, so closing it leaves the daemon running and reopening
//! it finds everything where it was.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange, Event as Term, EventStream,
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures::StreamExt as _;
use nightpost_core::api::{Client, Event, HistoryItem, Reply, Request};
use ratatui::DefaultTerminal;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthChar as _;

/// The one accent, kept for what needs attention: unread, unconfirmed, danger.
const ACCENT: Color = Color::Rgb(0xb0, 0x26, 0x26);
const DIM: Style = Style::new().fg(Color::DarkGray);
const HISTORY: u32 = 200;
const HELP: &str = "\
Enter            send
Alt+Enter        new line
Tab, Shift+Tab   next, previous conversation
PgUp, PgDn       scroll
Esc              close this, or clear the box
Ctrl+C           quit (the daemon keeps running)

/invite <name>               a one-time link, with its QR code
/code <name>                 a 12-digit code to read out
/accept <link> <name>        use a link
/acceptcode <code> <name>    use a code read out to you
/verify                      the safety number of this conversation
/quit";

struct Conversation {
    name: String,
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
}

pub async fn run(dir: &Path) -> Result<()> {
    let mut requests = Client::connect(dir)
        .await
        .context("no daemon running: start `nightpost daemon` first (in tmux, for one)")?;
    let mut events = Client::connect(dir).await?;
    if events.call(&Request::Listen).await? != Reply::Listening {
        bail!("the daemon did not accept to stream events");
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
        status: "/help for keys and commands".into(),
        overlay: None,
        codes: Vec::new(),
        focused: true,
        notify: None,
        quit: false,
    };
    app.reload_contacts(&mut requests).await?;
    app.select(0, &mut requests).await;

    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), EnableFocusChange, EnableBracketedPaste)?;
    let result = app.run(&mut terminal, &mut requests, &mut updated, &updates).await;
    let _ = crossterm::execute!(std::io::stdout(), DisableFocusChange, DisableBracketedPaste);
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
                    Term::FocusGained => self.focused = true,
                    Term::FocusLost => self.focused = false,
                    _ => {}
                },
                Some(update) = updated.recv() => self.on_update(update, requests).await,
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
                Ok(_) if keep => format!("{name} kept"),
                Ok(_) => format!("{name} turned down"),
            };
            return;
        }
        if let (Some(Overlay::Invite { link, .. }), KeyCode::Char('c')) = (&self.overlay, key.code) {
            copy(link);
            self.status = "link copied (if the terminal allows it)".into();
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
            KeyCode::Char(c) if !ctrl => self.insert(c.encode_utf8(&mut [0; 4])),
            _ => {}
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
            self.status = "no conversation yet: /invite or /code someone".into();
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
            Ok(other) => self.status = format!("unexpected reply {other:?}"),
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    async fn command(&mut self, command: &str, requests: &mut Client, updates: &mpsc::UnboundedSender<Update>) -> Result<()> {
        let words: Vec<&str> = command.split_whitespace().collect();
        match words.as_slice() {
            ["help"] => self.overlay = Some(Overlay::Text { title: "Keys and commands".into(), body: HELP.lines().map(Into::into).collect() }),
            ["quit"] => self.quit = true,
            ["invite", name] => {
                let link = match requests.call(&Request::Invite { name: name.to_string() }).await? {
                    Reply::Invite { link } => link,
                    Reply::Error { message } => bail!("{message}"),
                    other => bail!("unexpected reply {other:?}"),
                };
                let qr = qr(&link.to_ascii_uppercase());
                self.overlay = Some(Overlay::Invite { name: name.to_string(), link, qr });
            }
            ["code", name] => {
                let code = match requests.call(&Request::InviteCode { name: name.to_string() }).await? {
                    Reply::Code { code } => code,
                    Reply::Error { message } => bail!("{message}"),
                    other => bail!("unexpected reply {other:?}"),
                };
                self.codes.push(name.to_string());
                self.overlay = Some(Overlay::Text {
                    title: format!("Code for {name}"),
                    body: vec![
                        format!("Read this to {name}, valid 10 minutes:"),
                        String::new(),
                        format!("        {code}"),
                        String::new(),
                        "You will be asked to compare a safety number before anything is kept.".into(),
                    ],
                });
            }
            ["accept", link, name] => self.long(Request::Accept { link: link.to_string(), name: name.to_string() }, updates),
            ["acceptcode", code, name] => self.long(Request::AcceptCode { code: code.to_string(), name: name.to_string() }, updates),
            ["verify"] => {
                let name = self.conversations.get(self.selected).context("no conversation selected")?.name.clone();
                let number = match requests.call(&Request::Verify { name: name.clone() }).await? {
                    Reply::SafetyNumber { number, .. } => number,
                    Reply::Error { message } => bail!("{message}"),
                    other => bail!("unexpected reply {other:?}"),
                };
                self.overlay = Some(Overlay::Text {
                    title: format!("Safety number with {name}"),
                    body: vec![
                        "Read it to each other, on a call or in person. Same digits: nobody is in between.".into(),
                        String::new(),
                        format!("    {}", groups(&number)),
                    ],
                });
            }
            _ => bail!("unknown command: /help lists them"),
        }
        Ok(())
    }

    /// A request that can take minutes runs on its own connection, so the
    /// interface stays usable meanwhile.
    fn long(&mut self, request: Request, updates: &mpsc::UnboundedSender<Update>) {
        self.status = "looking for them (up to a few minutes)…".into();
        let (dir, updates) = (self.dir.clone(), updates.clone());
        tokio::spawn(async move {
            let status = match Client::connect(&dir).await {
                Ok(mut client) => match client.call(&request).await {
                    Ok(Reply::Added { name }) => format!("{name} added"),
                    Ok(Reply::Error { message }) => message,
                    Ok(other) => format!("unexpected reply {other:?}"),
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
                self.status = "the daemon stopped".into();
                self.quit = true;
            }
            Update::Event(Event::Message { from, id, time, text }) => {
                let selected = self.selected;
                let focused = self.focused;
                if let Some((i, c)) = self.conversations.iter_mut().enumerate().find(|(_, c)| c.name == from) {
                    c.messages.push(HistoryItem { id, outgoing: false, time, text, delivered: true });
                    if i != selected || !focused {
                        c.unread += 1;
                        // The name, never the text: a notification can show on a locked screen.
                        self.notify = Some(format!("Message from {from}"));
                    }
                }
            }
            Update::Event(Event::Sent { to, id, time, text }) => {
                if let Some(c) = self.conversations.iter_mut().find(|c| c.name == to) {
                    c.messages.push(HistoryItem { id, outgoing: true, time, text, delivered: false });
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
            Update::Event(Event::ContactAdded { name }) => {
                self.status = format!("{name} is now a contact");
                if let Err(e) = self.reload_contacts(requests).await {
                    self.status = format!("{e:#}");
                }
            }
            Update::Event(Event::Fingerprint { name, safety_number }) => {
                self.notify = Some(format!("{name} used your invitation"));
                self.overlay = Some(if self.codes.contains(&name) {
                    Overlay::Confirm { name, number: safety_number }
                } else {
                    Overlay::Text {
                        title: format!("Waiting for {name} to confirm"),
                        body: vec![format!("Read this safety number to {name}:"), String::new(), format!("    {}", groups(&safety_number))],
                    }
                });
            }
        }
    }

    async fn reload_contacts(&mut self, requests: &mut Client) -> Result<()> {
        let contacts = match requests.call(&Request::Contacts).await? {
            Reply::Contacts { contacts } => contacts,
            Reply::Error { message } => bail!("{message}"),
            other => bail!("unexpected reply {other:?}"),
        };
        for c in contacts {
            if !self.conversations.iter().any(|known| known.name == c.name) {
                self.conversations.push(Conversation { name: c.name, unread: 0, messages: Vec::new(), loaded: false });
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
                Ok(other) => self.status = format!("unexpected reply {other:?}"),
                Err(e) => self.status = format!("{e:#}"),
            }
        }
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let width = frame.area().width.saturating_sub(24).max(10);
        let input_lines = wrap(&self.input, width.saturating_sub(2) as usize).len().clamp(1, 6) as u16;
        let [main, input, status] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(input_lines + 2), Constraint::Length(1)]).areas(frame.area());
        let [list, thread] = Layout::horizontal([Constraint::Length(24), Constraint::Min(10)]).areas(main);

        let items: Vec<ListItem> = self
            .conversations
            .iter()
            .map(|c| {
                if c.unread > 0 {
                    ListItem::new(Line::from(vec![
                        Span::raw(c.name.clone()),
                        Span::styled(format!(" {}", c.unread), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
                    ]))
                } else {
                    ListItem::new(c.name.clone())
                }
            })
            .collect();
        let mut state = ListState::default().with_selected((!self.conversations.is_empty()).then_some(self.selected));
        frame.render_stateful_widget(
            List::new(items).block(Block::bordered().title(" Nightpost ")).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
            list,
            &mut state,
        );

        let current = self.conversations.get(self.selected);
        let mut lines: Vec<Line> = Vec::new();
        for m in current.map(|c| c.messages.as_slice()).unwrap_or_default() {
            let who = if m.outgoing { "me".to_string() } else { current.map_or(String::new(), |c| c.name.clone()) };
            let mut header = vec![Span::styled(format!("{} ", clock(m.time)), DIM), Span::styled(who, Style::new().add_modifier(Modifier::BOLD))];
            if m.outgoing && !m.delivered {
                header.push(Span::styled("  not delivered yet", DIM));
            }
            lines.push(Line::from(header));
            lines.extend(m.text.lines().map(|l| Line::from(l.to_string())));
            lines.push(Line::default());
        }
        let title = current.map_or(" no conversation ".to_string(), |c| format!(" {} ", c.name));
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        let inner_height = thread.height.saturating_sub(2) as usize;
        let total = paragraph.line_count(thread.width.saturating_sub(2));
        self.scroll = self.scroll.min(total.saturating_sub(inner_height));
        let top = total.saturating_sub(inner_height + self.scroll);
        frame.render_widget(paragraph.block(Block::bordered().title(title)).scroll((top as u16, 0)), thread);

        // The box wraps by characters, the same way the cursor is placed.
        let rows = wrap(&self.input, input.width.saturating_sub(2) as usize);
        let (row, col) = cursor_at(&self.input, self.cursor, input.width.saturating_sub(2) as usize);
        let first = row.saturating_sub(input_lines as usize - 1);
        let visible: Vec<Line> = rows.into_iter().skip(first).map(Line::from).collect();
        let hint = current.map_or(" type /help ".to_string(), |c| format!(" to {} — Enter sends, Alt+Enter new line ", c.name));
        frame.render_widget(Paragraph::new(visible).block(Block::bordered().title(hint)), input);
        if self.overlay.is_none() {
            frame.set_cursor_position(Position::new(input.x + 1 + col as u16, input.y + 1 + (row - first) as u16));
        }
        frame.render_widget(Paragraph::new(self.status.as_str()).style(DIM), status);

        match &self.overlay {
            Some(Overlay::Text { title, body }) => popup(frame, title, body.iter().map(|l| Line::from(l.as_str())).collect(), Style::new()),
            Some(Overlay::Invite { name, link, qr }) => {
                let area = frame.area();
                let qr_width = qr.first().map_or(0, |l| l.chars().count());
                let mut lines: Vec<Line> = if qr.len() + 8 <= area.height as usize && qr_width + 4 <= area.width as usize {
                    qr.iter().map(|l| Line::from(l.as_str())).collect()
                } else {
                    vec![Line::styled(
                        format!("The QR code needs a {}×{} terminal: enlarge it, or copy the link.", qr_width + 4, qr.len() + 8),
                        DIM,
                    )]
                };
                lines.extend([
                    Line::default(),
                    Line::from(link.as_str()),
                    Line::default(),
                    Line::from(format!("For {name} only: one use, 7 days. c copies the link.")),
                ]);
                popup(frame, &format!("Invitation for {name}"), lines, Style::new());
            }
            Some(Overlay::Confirm { name, number }) => popup(
                frame,
                &format!("{name} used your code"),
                vec![
                    Line::from(format!("Ask {name} to read their safety number. Yours:")),
                    Line::default(),
                    Line::from(format!("    {}", groups(number))),
                    Line::default(),
                    Line::from(vec![
                        Span::styled("Same digits? ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
                        Span::raw("y keeps them, n turns them down"),
                    ]),
                ],
                Style::new(),
            ),
            None => {}
        }
    }
}

/// A bordered box in the middle of the screen, as small as its content and
/// never wider than the screen: long lines wrap.
fn popup(frame: &mut ratatui::Frame, title: &str, lines: Vec<Line>, style: Style) {
    let area = frame.area();
    let width = (lines.iter().map(Line::width).max().unwrap_or(0).max(title.len() + 4) as u16 + 4).min(area.width);
    let paragraph = Paragraph::new(lines).style(style).wrap(Wrap { trim: false });
    let height = (paragraph.line_count(width.saturating_sub(4)) as u16 + 2).min(area.height);
    let rect = Rect { x: (area.width - width) / 2, y: (area.height - height) / 2, width, height };
    frame.render_widget(Clear, rect);
    let block = Block::bordered().title(format!(" {title} ")).title_bottom(" Esc ").padding(ratatui::widgets::Padding::horizontal(1));
    frame.render_widget(paragraph.block(block), rect);
}

/// The link as a QR code, two modules per character cell, light on dark.
fn qr(link: &str) -> Vec<String> {
    use qrcode::render::unicode::Dense1x2;
    match qrcode::QrCode::new(link.as_bytes()) {
        Ok(code) => code
            .render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
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

fn clock(millis: u64) -> String {
    let Ok(ts) = jiff::Timestamp::from_millisecond(millis as i64) else { return "--:--".into() };
    let zone = jiff::tz::TimeZone::system();
    let (then, now) = (ts.to_zoned(zone.clone()), jiff::Timestamp::now().to_zoned(zone));
    if then.date() == now.date() { then.strftime("%H:%M").to_string() } else { then.strftime("%d/%m %H:%M").to_string() }
}

pub fn groups(number: &str) -> String {
    number.as_bytes().chunks(5).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
