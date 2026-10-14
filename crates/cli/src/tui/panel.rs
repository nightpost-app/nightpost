//! Windows over the panes that hold more than a few lines: the settings
//! (Tor and the bridges among them), what we know of someone, a passphrase
//! being changed. Questions take numbered answers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nightpost_core::api::{Client, Reply, Request, Route};
use nightpost_core::prefs::Prefs;
use nightpost_core::{dbkey, i18n, t};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use super::view::{ACCENT, BOLD, DIM, STRONG, cols, keycaps, popup, wrap, wrap_words};
use super::{App, Overlay, now_millis};

/// The settings' tabs, in order.
const APPEARANCE: usize = 0;
pub(super) const NETWORK: usize = 1;
const NOTIFICATIONS: usize = 2;
const SECURITY: usize = 3;

/// What a key does to numbered answers: the one chosen, if any. Esc
/// chooses the last, which is always the way out.
pub(super) fn pick(code: KeyCode, selected: &mut usize, n: usize) -> Option<usize> {
    match code {
        KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(n - 1),
        KeyCode::Enter => return Some(*selected),
        KeyCode::Esc => return Some(n - 1),
        KeyCode::Char(c @ '1'..='9') if (c as usize - '1' as usize) < n => return Some(c as usize - '1' as usize),
        _ => {}
    }
    None
}

/// Numbered answers, the selected one marked.
pub(super) fn answers(answers: &[(String, Style)], selected: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    for (i, (text, style)) in answers.iter().enumerate() {
        let on = i == selected;
        let style = if !on { *style } else if *style == Style::new() { STRONG } else { style.patch(BOLD) };
        lines.push(Line::from(vec![
            Span::styled(if on { "› " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled(format!("{}. ", i + 1), DIM),
            Span::styled(text.clone(), style),
        ]));
    }
    lines
}

pub(super) fn answer_keys() -> Line<'static> {
    keycaps(&[("↑↓", t!("hint-choose")), (t!("key-enter").as_str(), t!("hint-confirm")), ("Esc", t!("hint-cancel"))])
}

pub(super) struct Settings {
    tab: usize,
    row: usize,
    /// The bridge lines, as the daemon keeps them until saved.
    own: bool,
    lines: String,
    editing: bool,
    /// What saving them said, and whether it went wrong.
    said: Option<(String, bool)>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Change {
    Add,
    Replace,
    Remove,
}

pub(super) struct Passphrase {
    change: Change,
    /// Current, new, again: those the change asks for.
    fields: Vec<String>,
    at: usize,
    error: String,
}

/// What someone's sheet offers.
#[derive(Clone, Copy)]
enum Do {
    Verify,
    Rename,
    Mute(u64),
    Unmute,
    Remove,
    Close,
}

/// What a row of the settings does.
#[derive(Clone, Copy)]
enum Act {
    Language(Option<&'static str>),
    Toggle(fn(&mut Prefs) -> &mut bool),
    Shows(bool, bool),
    Bridges(bool),
    EditLines,
    SaveBridges,
    Passphrase(Change),
    ReadReceipts,
    Typing,
    Lock,
}

enum Mark {
    Radio(bool),
    Switch(bool),
    Plain(Style),
}

enum Item {
    Title(String),
    Text(String, Style),
    Spans(Vec<Span<'static>>),
    Gap,
    Row { label: String, mark: Mark, help: Option<String>, act: Act },
    /// The bridge lines, under the row that edits them.
    Lines,
}

const HOUR: u64 = 3_600_000;

impl App {
    pub(super) async fn open_settings(&mut self, tab: usize, requests: &mut Client) {
        self.prefs = Prefs::load(&self.dir);
        let (own, lines) = match requests.call(&Request::Bridges { lines: None }).await {
            Ok(Reply::Bridges { lines }) => (!lines.is_empty(), lines.join("\n")),
            _ => (false, String::new()),
        };
        self.overlay = Some(Overlay::Settings(Settings { tab, row: 0, own, lines, editing: false, said: None }));
    }

    /// Keys for the windows of this file; false: not one of them.
    pub(super) async fn on_panel_key(&mut self, key: KeyEvent, requests: &mut Client) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self.overlay.take() {
            Some(Overlay::Settings(mut s)) if s.editing => {
                match key.code {
                    KeyCode::Esc => s.editing = false,
                    KeyCode::Enter => s.lines.push('\n'),
                    KeyCode::Char('j') if ctrl => s.lines.push('\n'),
                    KeyCode::Backspace => {
                        s.lines.pop();
                    }
                    KeyCode::Char(c) if !ctrl => s.lines.push(c),
                    _ => {}
                }
                s.said = None;
                self.overlay = Some(Overlay::Settings(s));
            }
            Some(Overlay::Settings(mut s)) => {
                let acts: Vec<Act> = self.items(&s).into_iter().filter_map(|i| if let Item::Row { act, .. } = i { Some(act) } else { None }).collect();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => return true,
                    KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => (s.tab, s.row) = ((s.tab + 1) % 5, 0),
                    KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => (s.tab, s.row) = ((s.tab + 4) % 5, 0),
                    KeyCode::Up | KeyCode::Char('k') => s.row = s.row.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => s.row = (s.row + 1).min(acts.len().saturating_sub(1)),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if let Some(&act) = acts.get(s.row) {
                            self.act(&mut s, act, requests).await;
                        }
                    }
                    _ => {}
                }
                // Unless the row opened another window.
                if self.overlay.is_none() {
                    self.overlay = Some(Overlay::Settings(s));
                }
            }
            Some(Overlay::Passphrase(mut p)) => {
                let last = p.fields.len() - 1;
                match key.code {
                    KeyCode::Esc => {
                        self.open_settings(SECURITY, requests).await;
                        return true;
                    }
                    KeyCode::Tab | KeyCode::Down => p.at = (p.at + 1).min(last),
                    KeyCode::BackTab | KeyCode::Up => p.at = p.at.saturating_sub(1),
                    KeyCode::Enter if p.at < last => p.at += 1,
                    KeyCode::Enter => {
                        self.set_passphrase(p, requests).await;
                        return true;
                    }
                    KeyCode::Backspace => {
                        p.fields[p.at].pop();
                    }
                    KeyCode::Char(c) if !ctrl => p.fields[p.at].push(c),
                    _ => {}
                }
                p.error.clear();
                self.overlay = Some(Overlay::Passphrase(p));
            }
            Some(Overlay::Info { name, mut selected }) => {
                let options = self.sheet(&name);
                match pick(key.code, &mut selected, options.len()) {
                    Some(i) => self.sheet_do(&name, options[i].2, requests).await,
                    None => self.overlay = Some(Overlay::Info { name, selected }),
                }
            }
            Some(Overlay::Looking { name, since }) => {
                if !matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.overlay = Some(Overlay::Looking { name, since });
                }
            }
            other => {
                self.overlay = other;
                return false;
            }
        }
        true
    }

    /// Pasted bridge lines or passphrase; false: not for these windows.
    pub(super) fn on_panel_paste(&mut self, text: &str) -> bool {
        match &mut self.overlay {
            Some(Overlay::Settings(s)) if s.editing => s.lines.push_str(text),
            Some(Overlay::Passphrase(p)) => p.fields[p.at].push_str(text.trim_end_matches(['\r', '\n'])),
            _ => return false,
        }
        true
    }

    async fn act(&mut self, s: &mut Settings, act: Act, requests: &mut Client) {
        match act {
            Act::Language(code) => match i18n::choose(code) {
                // The daemon's words too: errors, Tor's progress.
                Ok(()) => {
                    let _ = requests.call(&Request::Language).await;
                }
                Err(e) => self.status = format!("{e:#}"),
            },
            Act::Toggle(field) => {
                let mut prefs = Prefs::load(&self.dir);
                let on = field(&mut prefs);
                *on = !*on;
                self.prefs = prefs.save(&self.dir);
            }
            Act::Shows(name, text) => {
                let prefs = Prefs::load(&self.dir);
                self.prefs = Prefs { show_name: name, show_text: text, ..prefs }.save(&self.dir);
            }
            Act::Bridges(own) => (s.own, s.said) = (own, None),
            Act::EditLines => s.editing = true,
            Act::SaveBridges => {
                let lines = if s.own { s.lines.lines().map(String::from).collect() } else { Vec::new() };
                s.said = Some(match requests.call(&Request::Bridges { lines: Some(lines) }).await {
                    Ok(Reply::Bridges { lines }) => (if lines.is_empty() { t!("bridges-reset") } else { t!("bridges-saved") }, false),
                    Ok(Reply::Error { message }) => (message, true),
                    Ok(other) => (t!("err-unexpected", reply = format!("{other:?}")), true),
                    Err(e) => (format!("{e:#}"), true),
                });
            }
            Act::Passphrase(change) => {
                let fields = vec![String::new(); if change == Change::Replace { 3 } else if change == Change::Add { 2 } else { 1 }];
                self.overlay = Some(Overlay::Passphrase(Passphrase { change, fields, at: 0, error: String::new() }));
            }
            Act::ReadReceipts => match requests.call(&Request::ReadReceipts { on: Some(!self.read_receipts) }).await {
                Ok(Reply::ReadReceipts { on }) => self.read_receipts = on,
                other => self.status = unexpected(other),
            },
            Act::Typing => match requests.call(&Request::TypingIndicators { on: Some(!self.typing_indicators) }).await {
                Ok(Reply::TypingIndicators { on }) => self.typing_indicators = on,
                other => self.status = unexpected(other),
            },
            Act::Lock => self.lock(requests).await,
        }
    }

    async fn set_passphrase(&mut self, mut p: Passphrase, requests: &mut Client) {
        let f = &p.fields;
        let (current, new, again) = match p.change {
            Change::Add => (None, f[0].clone(), f[1].clone()),
            Change::Replace => (Some(f[0].clone()), f[1].clone(), f[2].clone()),
            Change::Remove => (Some(f[0].clone()), String::new(), String::new()),
        };
        let wants = p.change != Change::Remove;
        p.error = if wants && new.is_empty() {
            t!("need-passphrase")
        } else if new != again {
            t!("passphrases-differ")
        } else {
            match requests.call(&Request::SetPassphrase { current, new: wants.then_some(new) }).await {
                Ok(Reply::Done) => {
                    self.open_settings(SECURITY, requests).await;
                    self.status = if wants { t!("passphrase-saved") } else { t!("passphrase-removed") };
                    return;
                }
                Ok(Reply::Error { message }) => message,
                other => unexpected(other),
            }
        };
        self.overlay = Some(Overlay::Passphrase(p));
    }

    /// Stops the daemon: with a passphrase, everything stays locked until
    /// the next start. It tells every interface to close, this one too.
    pub(super) async fn lock(&mut self, requests: &mut Client) {
        match requests.call(&Request::Shutdown).await {
            Ok(Reply::Done) => self.quit = true,
            other => self.status = unexpected(other),
        }
    }

    /// What can be done about them, each with its look.
    fn sheet(&self, name: &str) -> Vec<(String, Style, Do)> {
        let muted = self.conversations.iter().find(|c| c.name == name).and_then(|c| c.person.muted_until).is_some_and(|u| u > now_millis());
        let mut options = vec![(t!("compare"), Style::new(), Do::Verify), (t!("rename"), Style::new(), Do::Rename)];
        if muted {
            options.push((t!("tui-unmute"), Style::new(), Do::Unmute));
        } else {
            options.extend([
                (t!("tui-mute-1h"), Style::new(), Do::Mute(now_millis() + HOUR)),
                (t!("tui-mute-8h"), Style::new(), Do::Mute(now_millis() + 8 * HOUR)),
                (t!("tui-mute"), Style::new(), Do::Mute(u64::MAX)),
            ]);
        }
        options.extend([(t!("remove-name", name = name), Style::new().fg(Color::Red), Do::Remove), (t!("close"), Style::new(), Do::Close)]);
        options
    }

    async fn sheet_do(&mut self, name: &str, what: Do, requests: &mut Client) {
        match what {
            Do::Verify => {
                if let Err(e) = self.verify(requests).await {
                    self.status = format!("{e:#}");
                }
            }
            Do::Rename => {
                self.focus = super::Focus::Input;
                self.set_input("/rename ");
            }
            Do::Mute(until) => {
                self.set_person(name, |p| p.muted_until = Some(until), requests).await;
                self.status = if until == u64::MAX { t!("tui-muted-forever", name = name) } else { t!("tui-muted-until", name = name, time = super::view::clock(until)) };
            }
            Do::Unmute => {
                self.set_person(name, |p| p.muted_until = None, requests).await;
                self.status = t!("tui-unmuted", name = name);
            }
            Do::Remove => self.overlay = Some(Overlay::Remove { name: name.to_string(), selected: 2 }),
            Do::Close => {}
        }
    }

    /// The rows of a tab of the settings, as the window has them.
    fn items(&self, s: &Settings) -> Vec<Item> {
        let row = |label: String, mark: Mark, help: Option<String>, act: Act| Item::Row { label, mark, help, act };
        let mut items = Vec::new();
        match s.tab {
            APPEARANCE => {
                let chosen = i18n::chosen();
                items.push(Item::Title(t!("language")));
                items.push(row(t!("theme-system"), Mark::Radio(chosen.is_none()), None, Act::Language(None)));
                for (code, name) in i18n::languages() {
                    items.push(row(name.to_string(), Mark::Radio(chosen.as_deref() == Some(code)), None, Act::Language(Some(code))));
                }
                items.extend([Item::Text(t!("language-help"), DIM), Item::Gap, Item::Title(t!("theme")), Item::Text(t!("tui-theme-help"), DIM)]);
            }
            NETWORK => {
                let (title, detail) = self.network.summary();
                let dot = if self.network.blocked() {
                    Span::styled("● ", Style::new().fg(Color::Red))
                } else if self.network.percent < 100 {
                    Span::styled(format!("{} ", self.spinner()), Style::new().fg(ACCENT))
                } else {
                    Span::styled("● ", Style::new().fg(ACCENT))
                };
                items.extend([Item::Title(t!("connection")), Item::Spans(vec![dot, Span::styled(title, BOLD)]), Item::Text(detail, DIM)]);
                if let Some(c) = self.current() {
                    let (reach, style) = self.reach(c);
                    items.extend([Item::Gap, Item::Title(t!("route-label", name = c.name)), Item::Spans(self.route(&c.name)), Item::Text(reach, style)]);
                }
                items.extend([
                    Item::Gap,
                    Item::Title(t!("bridges")),
                    Item::Text(t!("bridges-help"), DIM),
                    row(t!("bridges-builtin"), Mark::Radio(!s.own), Some(t!("bridges-builtin-help")), Act::Bridges(false)),
                    row(t!("bridges-own"), Mark::Radio(s.own), Some(t!("bridges-own-help")), Act::Bridges(true)),
                ]);
                if s.own {
                    items.extend([row(t!("bridges-lines"), Mark::Plain(Style::new()), None, Act::EditLines), Item::Lines]);
                }
                items.push(row(t!("bridges-save"), Mark::Plain(Style::new()), None, Act::SaveBridges));
                if let Some((said, wrong)) = &s.said {
                    items.push(Item::Text(said.clone(), Style::new().fg(if *wrong { Color::Red } else { ACCENT })));
                }
            }
            NOTIFICATIONS => {
                let p = self.prefs;
                let shown = (p.show_name, p.show_name && p.show_text);
                items.extend([
                    Item::Title(t!("notifications")),
                    row(t!("notify-show"), Mark::Switch(p.notify), Some(t!("notify-show-help")), Act::Toggle(|p| &mut p.notify)),
                    row(t!("notify-sound"), Mark::Switch(p.sound), Some(t!("notify-sound-help")), Act::Toggle(|p| &mut p.sound)),
                    row(t!("notify-history"), Mark::Switch(p.keep_history), Some(t!("notify-history-help")), Act::Toggle(|p| &mut p.keep_history)),
                    Item::Gap,
                    Item::Title(t!("notify-shows")),
                ]);
                for (choice, label) in [((true, true), t!("notify-who-what")), ((true, false), t!("notify-who")), ((false, false), t!("notify-nothing"))] {
                    items.push(row(label, Mark::Radio(shown == choice), None, Act::Shows(choice.0, choice.1)));
                }
                items.extend([Item::Text(t!("notify-locked"), DIM), Item::Gap, Item::Text(t!("tui-notify-terminal"), DIM)]);
            }
            SECURITY => {
                let has = dbkey::uses_passphrase(&self.dir);
                let (title, help) = if has { (t!("locked-both"), t!("locked-both-help")) } else { (t!("locked-keychain"), t!("locked-keychain-help")) };
                items.extend([Item::Title(t!("how-locked")), Item::Text(title, BOLD), Item::Text(help, DIM)]);
                if has {
                    items.push(row(t!("passphrase-change"), Mark::Plain(Style::new()), None, Act::Passphrase(Change::Replace)));
                    items.push(row(t!("passphrase-remove"), Mark::Plain(Style::new()), None, Act::Passphrase(Change::Remove)));
                } else {
                    items.push(row(t!("passphrase-add"), Mark::Plain(Style::new()), None, Act::Passphrase(Change::Add)));
                }
                items.extend([
                    Item::Gap,
                    Item::Title(t!("read-receipts")),
                    row(t!("read-receipts-send"), Mark::Switch(self.read_receipts), Some(t!("read-receipts-help")), Act::ReadReceipts),
                    Item::Gap,
                    Item::Title(t!("typing-indicators")),
                    row(t!("typing-indicators-send"), Mark::Switch(self.typing_indicators), Some(t!("typing-indicators-help")), Act::Typing),
                    Item::Gap,
                    Item::Title(t!("leaving")),
                    row(
                        if has { t!("quit-lock") } else { t!("quit") },
                        Mark::Plain(Style::new()),
                        Some(if has { t!("leave-lock-help") } else { t!("leave-quit-help") }),
                        Act::Lock,
                    ),
                ]);
            }
            _ => {
                items.extend([
                    Item::Spans(vec![Span::styled("Nightpost", BOLD), Span::styled(format!("  {}", nightpost_core::VERSION), DIM)]),
                    Item::Text(t!("tagline"), DIM),
                    Item::Gap,
                    Item::Title(t!("your-data")),
                    Item::Text(self.dir.display().to_string(), Style::new()),
                    Item::Gap,
                    Item::Title(t!("licences")),
                    Item::Spans(vec![Span::raw("Nightpost  "), Span::styled("AGPL-3.0", DIM)]),
                    Item::Spans(vec![Span::raw(format!("{}  ", t!("licence-bridges"))), Span::styled("BSD-3-Clause, Tor Project", DIM)]),
                ]);
            }
        }
        items
    }

    /// The way to them, step by step, as the window draws it: lit as far as
    /// it got, the step being tried turning, the one that failed in red.
    fn route(&self, name: &str) -> Vec<Span<'static>> {
        let route = Route::new(&self.network, self.links.get(name).copied());
        let entrance = if self.network.bridged { t!("step-private-entrance") } else { t!("step-entrance") };
        let labels = [t!("step-you"), entrance, t!("step-relay"), t!("step-relay"), t!("step-meeting"), name.to_string()];
        let style = |state: &str| match state {
            "done" => Style::new().fg(ACCENT),
            "live" => STRONG,
            "fail" => Style::new().fg(Color::Red),
            _ => DIM,
        };
        let mut spans = Vec::new();
        for (i, label) in labels.into_iter().enumerate() {
            if i > 0 {
                let hop = route.hop(i - 1);
                let line = if hop == "live" { format!(" ─{}─ ", self.spinner()) } else { " ─── ".into() };
                spans.push(Span::styled(line, style(hop)));
            }
            spans.push(Span::styled(label, style(route.step(i))));
        }
        spans
    }

    pub(super) fn draw_settings(&self, frame: &mut Frame, s: &Settings) {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(86);
        let height = area.height.saturating_sub(2);
        let rect = Rect::new((area.width - width) / 2, (area.height - height) / 2, width, height);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .title(Span::styled(format!(" {} ", t!("settings")), STRONG))
            .padding(Padding::new(2, 2, 1, 0));
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        if inner.height < 5 {
            return;
        }

        let mut tabs = Vec::new();
        for (i, name) in [t!("appearance"), t!("network"), t!("notifications"), t!("security"), t!("about")].into_iter().enumerate() {
            tabs.push(Span::styled(name, if i == s.tab { STRONG.add_modifier(ratatui::style::Modifier::UNDERLINED) } else { DIM }));
            tabs.push(Span::raw("   "));
        }
        frame.render_widget(Line::from(tabs), Rect { height: 1, ..inner });

        let body = Rect { y: inner.y + 2, height: inner.height - 4, ..inner };
        let (lines, start, end) = self.render(self.items(s), s, body.width as usize);
        // The selected row and its help in view, the rest as it comes.
        let shown = body.height as usize;
        let top = if end >= shown { (end + 1 - shown).min(start) } else { 0 };
        frame.render_widget(Paragraph::new(lines).scroll((top as u16, 0)), body);

        let keys = if s.editing {
            keycaps(&[(t!("key-enter").as_str(), t!("hint-newline")), ("Esc", t!("hint-done"))])
        } else {
            keycaps(&[("←→", t!("hint-section")), ("↑↓", t!("hint-choose")), (t!("key-enter").as_str(), t!("hint-change")), ("Esc", t!("hint-close"))])
        };
        frame.render_widget(keys, Rect { y: inner.bottom() - 1, height: 1, ..inner });
    }

    /// The items as lines `width` wide, and the lines of the selected row.
    fn render(&self, items: Vec<Item>, s: &Settings, width: usize) -> (Vec<Line<'static>>, usize, usize) {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let (mut start, mut end, mut row, mut selected) = (0, 0, 0, false);
        for item in items {
            match item {
                Item::Title(text) => lines.push(Line::styled(text, BOLD)),
                Item::Text(text, style) => lines.extend(wrap_words(&text, width).into_iter().map(|l| Line::styled(l, style))),
                Item::Spans(spans) => {
                    // Cut between spans, like words.
                    let mut line: Vec<Span<'static>> = Vec::new();
                    for span in spans {
                        if !line.is_empty() && line.iter().map(|s| cols(&s.content)).sum::<usize>() + cols(&span.content) > width {
                            lines.push(Line::from(std::mem::take(&mut line)));
                        }
                        line.push(span);
                    }
                    lines.push(Line::from(line));
                }
                Item::Gap => lines.push(Line::default()),
                Item::Row { label, mark, help, .. } => {
                    selected = row == s.row;
                    row += 1;
                    if selected {
                        start = lines.len();
                    }
                    let (sign, sign_style, label_style) = match mark {
                        Mark::Radio(on) => (if on { "(•) " } else { "( ) " }, if on { Style::new().fg(ACCENT) } else { DIM }, Style::new()),
                        Mark::Switch(on) => (if on { "[✓] " } else { "[ ] " }, if on { Style::new().fg(ACCENT) } else { DIM }, Style::new()),
                        Mark::Plain(style) => ("", Style::new(), style),
                    };
                    lines.push(Line::from(vec![
                        Span::styled(if selected { "› " } else { "  " }, Style::new().fg(ACCENT)),
                        Span::styled(sign, sign_style),
                        Span::styled(label, if selected { label_style.patch(STRONG) } else { label_style }),
                    ]));
                    let indent = 2 + cols(sign);
                    if let Some(help) = help {
                        let pad = " ".repeat(indent);
                        lines.extend(wrap_words(&help, width.saturating_sub(indent)).into_iter().map(|l| Line::styled(format!("{pad}{l}"), DIM)));
                    }
                    if selected {
                        end = lines.len() - 1;
                    }
                }
                Item::Lines => {
                    let bar = Style::new().fg(if s.editing { ACCENT } else { Color::DarkGray });
                    let rows: Vec<String> = if s.lines.is_empty() && !s.editing {
                        vec![String::new()]
                    } else {
                        s.lines.split('\n').flat_map(|l| wrap(l, width.saturating_sub(6))).collect()
                    };
                    let last = rows.len() - 1;
                    for (i, text) in rows.into_iter().enumerate() {
                        let mut spans = vec![Span::styled("  │ ", bar)];
                        if text.is_empty() && s.lines.is_empty() && !s.editing {
                            spans.push(Span::styled("webtunnel [2001:db8::1]:443 … url=https://…", DIM));
                        } else {
                            spans.push(Span::raw(text));
                        }
                        if s.editing && i == last {
                            spans.push(Span::styled("▏", Style::new().fg(ACCENT)));
                        }
                        lines.push(Line::from(spans));
                    }
                    if selected {
                        end = lines.len() - 1;
                    }
                }
            }
        }
        (lines, start, end)
    }

    pub(super) fn draw_passphrase(&self, frame: &mut Frame, p: &Passphrase) {
        let labels = match p.change {
            Change::Add => vec![t!("passphrase-new"), t!("passphrase-again")],
            Change::Replace => vec![t!("passphrase-current"), t!("passphrase-new"), t!("passphrase-again")],
            Change::Remove => vec![t!("passphrase-current")],
        };
        let mut lines = Vec::new();
        for (i, (label, value)) in labels.into_iter().zip(&p.fields).enumerate() {
            let on = i == p.at;
            lines.push(Line::styled(label, if on { STRONG } else { BOLD }));
            let mut field = vec![Span::styled("│ ", Style::new().fg(if on { ACCENT } else { Color::DarkGray })), Span::raw("•".repeat(value.chars().count()))];
            if on {
                field.push(Span::styled("▏", Style::new().fg(ACCENT)));
            }
            lines.extend([Line::from(field), Line::default()]);
        }
        lines.push(if p.change == Change::Remove { Line::styled(t!("passphrase-remove-help"), DIM) } else { Line::styled(t!("passphrase-warning"), Style::new().fg(Color::Red)) });
        if !p.error.is_empty() {
            lines.extend([Line::default(), Line::styled(p.error.clone(), Style::new().fg(Color::Red).add_modifier(ratatui::style::Modifier::BOLD))]);
        }
        let title = match p.change {
            Change::Add => t!("passphrase-add"),
            Change::Replace => t!("passphrase-change"),
            Change::Remove => t!("passphrase-remove"),
        };
        let keys = keycaps(&[("Tab", t!("hint-next-field")), (t!("key-enter").as_str(), t!("hint-confirm")), ("Esc", t!("hint-cancel"))]);
        popup(frame, &title, lines, Some(keys));
    }

    pub(super) fn draw_info(&self, frame: &mut Frame, name: &str, selected: usize) {
        let Some(c) = self.conversations.iter().find(|c| c.name == name) else { return };
        let (reach, style) = self.reach(c);
        let mut lines = vec![Line::styled(reach, style)];
        lines.push(if c.person.verified { Line::styled(format!("✓ {}", t!("verified-title")), Style::new().fg(ACCENT)) } else { Line::styled(format!("○ {}", t!("not-compared")), DIM) });
        lines.push(match c.person.muted_until.filter(|&u| u > now_millis()) {
            Some(u64::MAX) => Line::styled(t!("muted-help", name = name), DIM),
            Some(until) => Line::styled(t!("mute-until-help", time = super::view::clock(until)), DIM),
            None => Line::styled(t!("mute-on-help", name = name), DIM),
        });
        let options: Vec<(String, Style)> = self.sheet(name).into_iter().map(|(text, style, _)| (text, style)).collect();
        lines.extend(answers(&options, selected));
        popup(frame, &t!("about-name", name = name), lines, Some(answer_keys()));
    }

    pub(super) fn draw_looking(&self, frame: &mut Frame, name: &str, since: tokio::time::Instant) {
        let seconds = since.elapsed().as_secs();
        let now = if self.network.percent < 100 { t!("looking-tor", percent = self.network.percent) } else { t!("looking-help", name = name) };
        let lines = vec![
            Line::from(vec![Span::styled(format!("{} ", self.spinner()), Style::new().fg(ACCENT)), Span::styled(format!("{}:{:02}", seconds / 60, seconds % 60), DIM)]),
            Line::default(),
            Line::from(now),
            Line::default(),
            Line::styled(t!("looking-close", name = name), DIM),
        ];
        popup(frame, &t!("looking-title", name = name), lines, Some(keycaps(&[("Esc", t!("hint-hide"))])));
    }
}

/// An answer the daemon should not give, or its error.
fn unexpected(reply: anyhow::Result<Reply>) -> String {
    match reply {
        Ok(Reply::Error { message }) => message,
        Ok(other) => t!("err-unexpected", reply = format!("{other:?}")),
        Err(e) => format!("{e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_go_by_arrow_number_or_esc_and_esc_is_the_way_out() {
        let mut selected = 0;
        assert_eq!(pick(KeyCode::Down, &mut selected, 3), None);
        assert_eq!(pick(KeyCode::Down, &mut selected, 3), None);
        assert_eq!(pick(KeyCode::Down, &mut selected, 3), None);
        assert_eq!(selected, 2);
        assert_eq!(pick(KeyCode::Enter, &mut selected, 3), Some(2));
        assert_eq!(pick(KeyCode::Char('1'), &mut selected, 3), Some(0));
        // No fourth answer.
        assert_eq!(pick(KeyCode::Char('4'), &mut selected, 3), None);
        assert_eq!(pick(KeyCode::Esc, &mut selected, 3), Some(2));
    }
}
