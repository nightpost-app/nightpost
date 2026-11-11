//! What the terminal interface draws. It takes the terminal's own colours
//! (no background of ours), with one accent, grey for what is secondary,
//! red for what is wrong, and each name in the colour of its rings in the
//! window.

use nightpost_core::api::LinkState;
use nightpost_core::{i18n, t};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use unicode_width::UnicodeWidthChar as _;

use super::panel::{answer_keys, answers};
use super::{App, Command, Conversation, Focus, Overlay, matching, now_millis};

pub(super) const ACCENT: Color = Color::Cyan;
pub(super) const DIM: Style = Style::new().fg(Color::DarkGray);
pub(super) const STRONG: Style = Style::new().fg(ACCENT).add_modifier(ratatui::style::Modifier::BOLD);
pub(super) const BOLD: Style = Style::new().add_modifier(ratatui::style::Modifier::BOLD);
const LIST_WIDTH: u16 = 30;
/// Below this many columns the list and the conversation take turns.
const NARROW: u16 = 70;
const SPINNER: [&str; 6] = ["·", "✢", "✳", "✶", "✻", "✽"];
/// Rows of the command menu.
const MENU_ROWS: usize = 8;

/// A name in the middle colour of its rings.
fn name_style(name: &str) -> Style {
    let hex = nightpost_core::rings(name)[1];
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0x80);
    BOLD.fg(Color::Rgb(byte(1), byte(3), byte(5)))
}

/// A line of keys cut to `width` by dropping keys, the last one (how to
/// get out) never: the one before it goes first.
pub(super) fn fit_keys(mut line: Line<'static>, width: usize) -> Line<'static> {
    while line.width() > width && line.spans.len() > 2 {
        let at = line.spans.len() - 4.min(line.spans.len());
        line.spans.drain(at..at + 2);
    }
    line
}

/// Keys drawn as keys, each followed by what it does.
pub(super) fn keycaps(pairs: &[(impl AsRef<str>, String)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (key, what) in pairs {
        spans.push(Span::styled(format!(" {} ", key.as_ref()), Style::new().fg(Color::Black).bg(Color::Gray)));
        spans.push(Span::styled(format!(" {what}  "), DIM));
    }
    Line::from(spans)
}

impl App {
    pub(super) fn spinner(&self) -> &'static str {
        SPINNER[(self.started.elapsed().as_millis() / 120) as usize % SPINNER.len()]
    }

    fn pane(&self, focus: Focus, title: Span<'static>) -> Block<'static> {
        let on = self.focus == focus && self.overlay.is_none();
        let title = if on { Span::styled(title.content, STRONG) } else { title };
        Block::bordered().border_type(BorderType::Rounded).border_style(if on { Style::new().fg(ACCENT) } else { DIM }).title(title)
    }

    pub(super) fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let [main, bar] = Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).areas(area);
        // Wide: the list beside the conversation. Narrow: one at a time, the
        // list when it has the focus, else the conversation.
        let narrow = area.width < NARROW;
        let list_width = (area.width / 3).clamp(20, LIST_WIDTH);
        let (list, right) = if !narrow {
            let [list, right] = Layout::horizontal([Constraint::Length(list_width), Constraint::Min(20)]).areas(main);
            (list, right)
        } else if self.focus == Focus::List {
            (main, Rect { width: 0, height: 0, ..main })
        } else {
            (Rect { width: 0, height: 0, ..main }, main)
        };
        let text_width = right.width.saturating_sub(4).max(1) as usize;
        let rows = wrap(&self.input, text_width).len().clamp(1, 6) as u16;
        let [thread, input] = Layout::vertical([Constraint::Min(3), Constraint::Length(rows + 2)]).areas(right);
        self.areas = [list, thread, input];
        if list.width > 0 {
            self.draw_list(frame, list);
        }
        if right.width > 0 {
            self.draw_thread(frame, thread);
            self.draw_input(frame, input, text_width, rows);
        }
        self.draw_bar(frame, bar);
        self.draw_notice(frame, thread);
        self.draw_menu(frame, input);
        self.draw_overlay(frame, area);
    }

    /// Each person on two rows (name, then their last message), and Tor
    /// at the bottom.
    fn draw_list(&mut self, frame: &mut Frame, area: Rect) {
        let block = self.pane(Focus::List, Span::styled(format!(" {} ", t!("tui-conversations")), BOLD));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let width = inner.width as usize;
        // How Tor stands, over as many rows as it takes, then who is reachable.
        let (state, _) = self.network.summary();
        let state = wrap_words(&state, width.saturating_sub(3));
        let [rows, _, tor] = Layout::vertical([Constraint::Min(2), Constraint::Length(1), Constraint::Length(state.len() as u16 + 1)]).areas(inner);
        // Keep the selected one in view.
        let shown = (rows.height as usize / 2).max(1);
        self.list_top = self.list_top.min(self.selected).max((self.selected + 1).saturating_sub(shown));
        let mut lines = Vec::new();
        for (i, c) in self.conversations.iter().enumerate().skip(self.list_top).take(shown) {
            let on = i == self.selected;
            let bar = Span::styled(if on { "▌" } else { " " }, Style::new().fg(ACCENT));
            let reachable = self.reachable(&c.name);
            let last = c.messages.last();
            let right = if c.unread > 0 {
                Span::styled(format!(" {} ", c.unread), Style::new().fg(Color::Black).bg(ACCENT).add_modifier(ratatui::style::Modifier::BOLD))
            } else {
                Span::styled(last.map(|m| short_time(m.time)).unwrap_or_default(), DIM)
            };
            let name = fit(&c.name, width.saturating_sub(4 + cols(&right.content)));
            lines.push(Line::from(vec![
                bar.clone(),
                if c.note { Span::styled("✎ ", Style::new().fg(ACCENT)) } else { Span::styled(if reachable { "● " } else { "○ " }, if reachable { Style::new().fg(ACCENT) } else { DIM }) },
                Span::styled(name, name_style(&c.name)),
                Span::raw(" "),
                right,
            ]));
            let preview = preview(c);
            let style = if on || c.unread > 0 { Style::new() } else { DIM };
            lines.push(Line::from(vec![bar, Span::styled(format!("  {}", fit(&preview, width.saturating_sub(3))), style)]));
        }
        // Alone with the note: say how to add someone.
        if self.conversations.len() <= 1 {
            lines.push(Line::default());
            lines.extend(wrap_words(&t!("tui-add-hint"), width.saturating_sub(2)).into_iter().map(|row| Line::styled(format!(" {row}"), Style::new().fg(ACCENT))));
        }
        frame.render_widget(Paragraph::new(lines), rows);

        let ok = self.network.percent == 100 && !self.network.blocked();
        let dot = if self.network.blocked() { Span::styled("● ", Style::new().fg(Color::Red)) } else if ok { Span::styled("● ", Style::new().fg(ACCENT)) } else { Span::styled(format!("{} ", self.spinner()), Style::new().fg(ACCENT)) };
        let reachable = self.conversations.iter().filter(|c| !c.note && self.reachable(&c.name)).count();
        let mut lines: Vec<Line> = state
            .into_iter()
            .enumerate()
            .map(|(i, row)| Line::from(vec![Span::raw(" "), if i == 0 { dot.clone() } else { Span::raw("  ") }, Span::styled(row, DIM)]))
            .collect();
        lines.push(Line::styled(format!("   {}", t!("reachable-count", reachable = reachable, total = self.conversations.iter().filter(|c| !c.note).count())), DIM));
        frame.render_widget(Paragraph::new(lines), tor);
    }

    fn draw_thread(&mut self, frame: &mut Frame, area: Rect) {
        let Some(c) = self.current() else {
            let block = self.pane(Focus::Thread, Span::raw(""));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(Paragraph::new(Line::styled(t!("tui-no-conversation"), DIM)).wrap(Wrap { trim: false }), pad(inner));
            return;
        };
        let (reach, reach_style) = self.reach(c);
        let room = (area.width as usize).saturating_sub(cols(&c.name) + 8);
        let mut block = self
            .pane(Focus::Thread, Span::styled(format!(" {} ", c.name), name_style(&c.name)))
            .title_top(Line::styled(format!(" {} ", fit(&reach, room)), reach_style).right_aligned());
        let mut marks = Vec::new();
        if c.person.verified {
            marks.push(format!("✓ {}", t!("verified")));
        }
        if c.timer > 0 {
            marks.push(format!("⏱ {}", nightpost_core::api::timer_label(c.timer)));
        }
        if let Some(until) = c.person.muted_until.filter(|&u| u > now_millis()) {
            marks.push(if until == u64::MAX { t!("muted") } else { t!("muted-until", time = clock(until)) });
        }
        if !marks.is_empty() {
            block = block.title_bottom(Line::styled(format!(" {} ", marks.join("  ")), DIM).right_aligned());
        }
        let inner = pad(block.inner(area));
        frame.render_widget(block, area);

        let (mut lines, starts) = self.thread_lines(c, inner.width as usize);
        let height = inner.height as usize;
        let room = lines.len().saturating_sub(height);
        if let Some(i) = self.jump.take() {
            // The message a little below the top, with its header above it.
            self.scroll = room.saturating_sub(starts.get(i).copied().unwrap_or(0).saturating_sub(3));
        }
        self.scroll = self.scroll.min(room);
        // Few messages sit at the bottom, by the box, as in any conversation.
        if lines.len() < height {
            lines.splice(0..0, std::iter::repeat_n(Line::default(), height - lines.len()));
        }
        let top = room - self.scroll;
        frame.render_widget(Paragraph::new(lines).scroll((top as u16, 0)), inner);
        if room > 0 {
            let mut state = ScrollbarState::new(room).position(top);
            let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(DIM)
                .thumb_style(Style::new().fg(ACCENT));
            frame.render_stateful_widget(bar, Rect { x: area.right() - 1, y: area.y + 1, width: 1, height: area.height.saturating_sub(2) }, &mut state);
        }
        if self.scroll > 0 {
            let below = Line::from(vec![Span::styled(format!(" {} ", t!("tui-lines-below", count = self.scroll)), Style::new().fg(ACCENT)), Span::styled(format!("{} ", t!("key-end")), STRONG)]);
            let width = below.width() as u16;
            frame.render_widget(below, Rect::new(area.x + area.width.saturating_sub(width) / 2, area.bottom() - 1, width.min(area.width), 1));
        }
    }

    /// How they can be reached, as the window says it.
    pub(super) fn reach(&self, c: &Conversation) -> (String, Style) {
        if c.note {
            return (format!("✎ {}", t!("note-here")), DIM);
        }
        let net = &self.network;
        if net.blocked() {
            return (if net.offline { t!("net-offline") } else { t!("net-blocked") }, Style::new().fg(Color::Red));
        }
        if net.percent < 100 {
            return (t!("connecting-percent", percent = net.percent), DIM);
        }
        match self.links.get(&c.name) {
            Some(LinkState::Open) => (format!("● {}", t!("link-open")), Style::new().fg(ACCENT)),
            Some(LinkState::Idle) => (format!("● {}", t!("link-idle")), Style::new().fg(ACCENT)),
            Some(LinkState::Trying) => (format!("{} {}", self.spinner(), t!("reaching", name = c.name)), Style::new().fg(ACCENT)),
            Some(LinkState::Unreachable) => (format!("○ {}", t!("link-away")), DIM),
            Some(LinkState::Older) => (t!("link-older", name = c.name), Style::new().fg(Color::Red)),
            Some(LinkState::Newer) => (t!("link-newer", name = c.name), Style::new().fg(Color::Red)),
            None => (format!("○ {}", t!("link-unknown")), DIM),
        }
    }

    /// The conversation: a rule between days, each run of messages under
    /// its sender's name and time, ours marked sent, delivered or read.
    /// The rows, and the row where each message starts.
    fn thread_lines(&self, c: &Conversation, width: usize) -> (Vec<Line<'static>>, Vec<usize>) {
        let mut lines = Vec::new();
        let mut starts = Vec::new();
        if c.messages.is_empty() {
            let text = if c.note { t!("note-empty") } else { t!("no-messages") };
            lines.extend(wrap_words(&text, width).into_iter().map(|row| Line::styled(row, DIM)));
        }
        let sending = self.sending(c);
        // Who set the timer, among the messages at its time.
        let mut timer_set = c.timer_set.as_ref();
        let timer_line = |lines: &mut Vec<Line<'static>>, text: &str| {
            lines.push(Line::default());
            lines.extend(wrap_words(&format!("⏱ {text}"), width).into_iter().map(|row| Line::styled(row, DIM)));
        };
        for (i, m) in c.messages.iter().enumerate() {
            if let Some((_, text)) = timer_set.filter(|(at, _)| *at <= m.time) {
                timer_line(&mut lines, text);
                timer_set = None;
            }
            let day = date(m.time);
            let previous = i.checked_sub(1).map(|j| &c.messages[j]);
            if previous.is_none_or(|p| date(p.time) != day) {
                let label = format!(" {} ", day_label(m.time));
                let side = width.saturating_sub(cols(&label)) / 2;
                lines.push(Line::default());
                lines.push(Line::styled(format!("{}{label}{}", "─".repeat(side), "─".repeat(side)), DIM));
            }
            if previous.is_none_or(|p| p.outgoing != m.outgoing || m.time.saturating_sub(p.time) > 5 * 60_000 || date(p.time) != day) {
                let (who, style) = if m.outgoing { (t!("tui-you"), STRONG) } else { (c.name.clone(), name_style(&c.name)) };
                lines.push(Line::default());
                lines.push(Line::from(vec![Span::styled(who, style), Span::styled(format!("  {}", clock(m.time)), DIM)]));
            }
            let mark = match (m.outgoing, m.delivered, m.read) {
                (false, ..) => None,
                // A note has no one to deliver to.
                (true, ..) if c.note => None,
                (true, false, _) if sending => Some(Span::styled(format!("{} {}", self.spinner(), t!("tui-sending")), Style::new().fg(ACCENT))),
                (true, false, _) => Some(Span::styled(t!("tui-pending"), DIM)),
                (true, true, false) => Some(Span::styled("✓✓", DIM)),
                (true, true, true) => Some(Span::styled(t!("tui-read-mark"), Style::new().fg(ACCENT))),
            };
            starts.push(lines.len());
            let mut rows: Vec<Line> = wrap_words(&m.text, width.saturating_sub(2)).into_iter().map(|r| Line::from(format!("  {r}"))).collect();
            if let (Some(mark), Some(last)) = (mark, rows.last_mut()) {
                // After the text, or under it when it does not fit.
                if last.width() + 2 + cols(&mark.content) <= width {
                    last.spans.push(Span::raw("  "));
                    last.spans.push(mark);
                } else {
                    rows.push(Line::from(vec![Span::raw("  "), mark]));
                }
            }
            lines.extend(rows);
        }
        if let Some((_, text)) = timer_set {
            timer_line(&mut lines, text);
        }
        if self.typing.as_ref().is_some_and(|t| t.0 == c.name) {
            lines.push(Line::default());
            lines.push(Line::from(vec![Span::styled(format!("{} ", self.spinner()), Style::new().fg(ACCENT)), Span::styled(t!("typing", name = c.name), DIM)]));
        }
        (lines, starts)
    }

    /// The box wraps by characters, the same way the cursor is placed.
    fn draw_input(&self, frame: &mut Frame, area: Rect, width: usize, rows: u16) {
        let block = self.pane(Focus::Input, Span::styled(format!(" {} ", t!("tui-message")), BOLD));
        let text = pad(block.inner(area));
        frame.render_widget(block, area);
        let (row, col) = cursor_at(&self.input, self.cursor, width);
        let first = row.saturating_sub(rows as usize - 1);
        if self.input.is_empty() {
            let hint = self.current().map_or_else(|| t!("tui-commands-hint"), |c| t!("write-to", name = c.name));
            frame.render_widget(Paragraph::new(Span::styled(hint, DIM)), text);
        } else {
            let visible: Vec<Line> = wrap(&self.input, width).into_iter().skip(first).map(Line::from).collect();
            frame.render_widget(Paragraph::new(visible), text);
        }
        if self.focus == Focus::Input && self.overlay.is_none() {
            frame.set_cursor_position(Position::new(text.x + col as u16, text.y + (row - first) as u16));
        }
    }

    /// The keys for the pane in use, or what just happened.
    fn draw_bar(&self, frame: &mut Frame, area: Rect) {
        if !self.status.is_empty() {
            frame.render_widget(Line::styled(format!(" {}", self.status), Style::new().fg(ACCENT)), area);
            return;
        }
        let newline = if self.keys_told_apart { t!("key-shift-enter") } else { "Ctrl+J".into() };
        let keys: Vec<(String, String)> = match self.focus {
            Focus::List => vec![
                ("↑↓".into(), t!("hint-choose")),
                (t!("key-enter"), t!("hint-write")),
                ("a".into(), t!("hint-add")),
                ("Tab".into(), t!("hint-pane")),
                ("v".into(), t!("hint-verify")),
                ("?".into(), t!("hint-help")),
                ("q".into(), t!("hint-quit")),
            ],
            Focus::Thread => vec![
                ("↑↓".into(), t!("help-scroll")),
                ("Tab".into(), t!("hint-pane")),
                ("Esc".into(), t!("hint-list")),
                ("?".into(), t!("hint-help")),
                ("q".into(), t!("hint-quit")),
            ],
            Focus::Input => vec![
                (t!("key-enter"), t!("help-send")),
                (newline, t!("hint-newline")),
                ("/".into(), t!("hint-commands")),
                ("Esc".into(), t!("hint-list")),
                ("Tab".into(), t!("hint-pane")),
                ("Ctrl+K".into(), t!("hint-conversations")),
            ],
        };
        // The least useful (the last) go first when the line is too long.
        let mut shown = keys.len();
        let line = loop {
            let mut line = keycaps(&keys[..shown]);
            line.spans.insert(0, Span::raw(" "));
            if line.width() <= area.width as usize || shown <= 1 {
                break line;
            }
            shown -= 1;
        };
        frame.render_widget(line, area);
    }

    /// A message for another conversation, for a few seconds.
    fn draw_notice(&self, frame: &mut Frame, thread: Rect) {
        let Some((from, text, _)) = &self.notice else { return };
        let room = thread.width.saturating_sub(6) as usize;
        let tail = "   Ctrl+K";
        let text = fit(text, room.saturating_sub(cols(from) + 2 + cols(tail))).trim_end().to_string();
        let line = Line::from(vec![Span::styled(format!("{from}  "), name_style(from)), Span::raw(text), Span::styled(tail, DIM)]);
        let width = (line.width() as u16 + 4).min(thread.width.saturating_sub(2));
        let rect = Rect::new(thread.right().saturating_sub(width + 1), thread.y + 1, width, 3);
        frame.render_widget(Clear, rect);
        let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(ACCENT)).padding(Padding::horizontal(1));
        frame.render_widget(Paragraph::new(line).block(block), rect);
    }

    /// The commands for what is typed, above the box. The window moves
    /// only when the selection reaches its edge.
    fn draw_menu(&mut self, frame: &mut Frame, input: Rect) {
        if self.focus != Focus::Input || self.overlay.is_some() {
            return;
        }
        let menu = matching(&self.input);
        if menu.is_empty() {
            return;
        }
        self.menu = self.menu.min(menu.len() - 1);
        if self.menu < self.menu_top {
            self.menu_top = self.menu;
        } else if self.menu >= self.menu_top + MENU_ROWS {
            self.menu_top = self.menu + 1 - MENU_ROWS;
        }
        self.menu_top = self.menu_top.min(menu.len().saturating_sub(MENU_ROWS));
        // Room for the descriptions: on a narrow screen only the chosen command
        // shows what follows its name, and a description is cut to what is left.
        let wide = input.width >= 70;
        let args_of = |c: &Command, on: bool| if wide || on { format!(" {}", c.args) } else { String::new() };
        let name_width = menu.iter().map(|c| cols(c.name) + cols(&args_of(c, wide))).max().unwrap_or(0) + 2;
        let lines: Vec<Line> = menu
            .iter()
            .enumerate()
            .skip(self.menu_top)
            .take(MENU_ROWS)
            .map(|(i, c): (usize, &Command)| {
                let on = i == self.menu;
                let name_style = if on { STRONG } else { Style::new() };
                let args = args_of(c, on);
                let used = cols(c.name) + cols(&args);
                let pad = " ".repeat(name_width.saturating_sub(used));
                let room = (input.width as usize).saturating_sub(6 + used.max(name_width));
                let about = if room >= 6 { fit(&c.about, room) } else { String::new() };
                Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, Style::new().fg(ACCENT)),
                    Span::styled(c.name.to_string(), name_style),
                    Span::styled(format!("{args}{pad}"), DIM),
                    Span::styled(about, DIM),
                ])
            })
            .collect();
        let width = (lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4).min(input.width);
        let height = lines.len() as u16 + 2;
        let rect = Rect::new(input.x, input.y.saturating_sub(height), width, height);
        let mut block = Block::bordered().border_type(BorderType::Rounded).border_style(DIM).padding(Padding::horizontal(1));
        if menu.len() > MENU_ROWS {
            block = block.title_bottom(Line::styled(format!(" {}/{} ", self.menu + 1, menu.len()), DIM).right_aligned());
        }
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn draw_overlay(&self, frame: &mut Frame, area: Rect) {
        match &self.overlay {
            None => {}
            Some(Overlay::Help(scroll)) => {
                let keys = keycaps(&[("↑↓", t!("help-scroll")), ("Esc", t!("hint-close"))]);
                popup_at(frame, &t!("tui-shortcuts"), self.help(frame.area().width), Some(keys), *scroll);
            }
            Some(Overlay::Search { query, selected }) => {
                let hits = self.hits(query);
                let mut lines = vec![Line::from(vec![Span::styled(format!("{} ", t!("tui-search")), DIM), Span::raw(query.clone()), Span::styled("▏", Style::new().fg(ACCENT))]), Line::default()];
                if let Some(c) = self.current() {
                    if hits.is_empty() && !query.is_empty() {
                        lines.push(Line::styled(t!("tui-no-match"), DIM));
                    }
                    // A window of what fits, around the one chosen.
                    let at = (*selected).min(hits.len().saturating_sub(1));
                    for (row, &i) in hits.iter().enumerate().skip(at.saturating_sub(5)).take(8) {
                        let m = &c.messages[i];
                        let who = if m.outgoing { t!("tui-you") } else { c.name.clone() };
                        let text: String = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
                        lines.push(Line::from(vec![
                            Span::styled(if row == at { "› " } else { "  " }, Style::new().fg(ACCENT)),
                            Span::styled(format!("{} ", short_time(m.time)), DIM),
                            Span::styled(fit(&who, 10), if m.outgoing { STRONG } else { name_style(&c.name) }),
                            Span::styled(fit(&text, 40).trim_end().to_string(), if row == at { Style::new() } else { DIM }),
                        ]));
                    }
                }
                let keys = keycaps(&[("↑↓", t!("hint-choose")), (t!("key-enter").as_str(), t!("hint-show")), ("Esc", t!("hint-close"))]);
                popup(frame, &t!("tui-search-title"), lines, Some(keys));
            }
            Some(Overlay::Picker { filter, selected }) => {
                let found = self.found(filter);
                let mut lines = vec![Line::from(vec![Span::styled(format!("{} ", t!("tui-go-to")), DIM), Span::raw(filter.clone()), Span::styled("▏", Style::new().fg(ACCENT))]), Line::default()];
                for (row, &i) in found.iter().enumerate() {
                    let c = &self.conversations[i];
                    let on = row == (*selected).min(found.len() - 1);
                    let reachable = self.reachable(&c.name);
                    let last = preview(c);
                    let mut spans = vec![
                        Span::styled(if on { "› " } else { "  " }, Style::new().fg(ACCENT)),
                        // As in the list: a pencil for the note, a dot for people.
                        if c.note { Span::styled("✎ ", Style::new().fg(ACCENT)) } else { Span::styled(if reachable { "● " } else { "○ " }, if reachable { Style::new().fg(ACCENT) } else { DIM }) },
                        Span::styled(fit(&c.name, 14), name_style(&c.name)),
                        Span::styled(fit(&last, 36).trim_end().to_string(), if on { Style::new() } else { DIM }),
                    ];
                    if c.unread > 0 {
                        spans.push(Span::styled(format!("  {}", c.unread), STRONG));
                    }
                    lines.push(Line::from(spans));
                }
                let keys = keycaps(&[("↑↓", t!("hint-choose")), (t!("key-enter").as_str(), t!("hint-open")), ("Esc", t!("hint-close"))]);
                popup(frame, &t!("tui-conversations"), lines, Some(keys));
            }
            Some(Overlay::Text { title, body }) => {
                let keys = keycaps(&[("↑↓", t!("help-scroll")), ("Esc", t!("hint-close"))]);
                self.overlay_scroll.set(popup_at(frame, title, body.iter().map(|l| Line::from(l.clone())).collect(), Some(keys), self.overlay_scroll.get()));
            }
            Some(Overlay::Recovery { body, .. }) => {
                let keys = keycaps(&[("↑↓", t!("help-scroll")), ("c", t!("hint-copy-phrase")), ("Esc", t!("hint-close"))]);
                self.overlay_scroll.set(popup_at(frame, &t!("recovery-title"), body.iter().map(|l| Line::from(l.clone())).collect(), Some(keys), self.overlay_scroll.get()));
            }
            Some(Overlay::Invite { title, help, link, qr }) => {
                let qr_width = qr.first().map_or(0, |l| l.chars().count());
                // Dark on light whatever the theme: not every phone reads
                // a code the other way round.
                let mut lines: Vec<Line> = if qr.len() + 12 <= area.height as usize && qr_width + 8 <= area.width as usize {
                    qr.iter().map(|l| Line::styled(l.clone(), Style::new().fg(Color::Black).bg(Color::White))).collect()
                } else {
                    vec![Line::styled(t!("tui-qr-too-small", width = qr_width + 8, height = qr.len() + 12), DIM)]
                };
                lines.extend([Line::default(), Line::styled(link.clone(), Style::new().fg(ACCENT)), Line::default(), Line::styled(help.clone(), DIM)]);
                let keys = keycaps(&[("↑↓", t!("help-scroll")), ("c", t!("hint-copy")), ("Esc", t!("hint-close"))]);
                self.overlay_scroll.set(popup_at(frame, title, lines, Some(keys), self.overlay_scroll.get()));
            }
            Some(Overlay::Confirm { name, number, selected }) => {
                let mut lines = vec![Line::from(t!("confirm-help")), Line::default()];
                lines.extend(number_lines(number).into_iter().map(|l| Line::styled(l, BOLD)));
                lines.extend([
                    Line::default(),
                    Line::styled(t!("confirm-warning", name = name), Style::new().fg(Color::Red).add_modifier(ratatui::style::Modifier::BOLD)),
                ]);
                lines.extend(answers(&[(t!("digits-match"), Style::new()), (t!("digits-differ"), Style::new())], *selected));
                popup(frame, &t!("confirm-title", name = name), lines, Some(answer_keys()));
            }
            Some(Overlay::Verify { name, number, selected }) => {
                let mut lines = vec![Line::from(t!("safety-help")), Line::default()];
                lines.extend(number_lines(number).into_iter().map(|l| Line::styled(l, BOLD)));
                lines.extend([
                    Line::default(),
                    Line::styled(t!("safety-warning", name = name), Style::new().fg(Color::Red).add_modifier(ratatui::style::Modifier::BOLD)),
                ]);
                lines.extend(answers(&[(t!("they-match"), Style::new()), (t!("not-now"), Style::new())], *selected));
                popup(frame, &t!("safety-label", name = name), lines, Some(answer_keys()));
            }
            Some(Overlay::Add { selected }) => {
                let lines = answers(&add_answers(), *selected);
                popup(frame, &t!("add-title"), lines, Some(answer_keys()));
            }
            Some(Overlay::Devices { devices, selected }) => {
                let mut lines: Vec<Line> = devices
                    .iter()
                    .map(|d| {
                        let what = [d.this.then(|| t!("device-this")), d.mailbox.then(|| t!("device-mailbox"))].into_iter().flatten().collect::<Vec<_>>().join(", ");
                        Line::from(vec![Span::styled(d.name.clone(), if d.this { BOLD } else { Style::new() }), Span::styled(if what.is_empty() { String::new() } else { format!("  {what}") }, DIM)])
                    })
                    .collect();
                let mut choices: Vec<(String, Style)> = devices.iter().filter(|d| !d.this).map(|d| (t!("device-remove", name = d.name.clone()), Style::new())).collect();
                choices.extend([(t!("device-add"), Style::new()), (t!("close"), Style::new())]);
                lines.extend(answers(&choices, *selected));
                popup(frame, &t!("devices-title"), lines, Some(answer_keys()));
            }
            Some(Overlay::RemoveDevice { name, selected, .. }) => {
                let mut lines = vec![Line::from(t!("device-remove-help", name = name.clone()))];
                lines.extend(answers(&[(t!("device-remove", name = name.clone()), Style::new().fg(Color::Red)), (t!("cancel"), Style::new())], *selected));
                popup(frame, &t!("device-remove-title", name = name.clone()), lines, Some(answer_keys()));
            }
            Some(Overlay::Remove { name, selected }) => {
                let mut lines = vec![Line::from(t!("remove-help", name = name))];
                lines.extend(answers(&remove_answers(name), *selected));
                popup(frame, &t!("remove-title", name = name), lines, Some(answer_keys()));
            }
            Some(Overlay::Info { name, selected }) => self.draw_info(frame, name, *selected),
            Some(Overlay::Looking { name, since }) => self.draw_looking(frame, name, *since),
            Some(Overlay::Settings(s)) => self.draw_settings(frame, s),
            Some(Overlay::Passphrase(p)) => self.draw_passphrase(frame, p),
        }
    }

    /// Every key, grouped by where it works, in two columns (one when the
    /// screen is too narrow for both).
    pub(super) fn help(&self, room: u16) -> Vec<Line<'static>> {
        let enter = t!("key-enter");
        let left: Vec<(String, Vec<(String, String)>)> = vec![
            (
                t!("tui-keys-everywhere"),
                vec![
                    ("Tab".into(), t!("help-next-pane")),
                    ("Ctrl+K".into(), t!("help-go-to")),
                    ("Ctrl+F".into(), t!("help-search")),
                    (format!("{} {}", t!("key-pgup"), t!("key-pgdn")), t!("help-scroll-thread")),
                    (format!("{}  {}", t!("key-home"), t!("key-end")), t!("help-top-bottom")),
                    ("Ctrl+C".into(), t!("help-quit")),
                ],
            ),
            (
                t!("tui-keys-list"),
                vec![
                    ("↑ ↓   j k".into(), t!("hint-choose")),
                    (enter.clone(), t!("help-write-to")),
                    ("v".into(), t!("hint-verify")),
                    ("a".into(), t!("help-add")),
                    ("?".into(), t!("help-help")),
                    ("q".into(), t!("hint-quit")),
                ],
            ),
        ];
        let right: Vec<(String, Vec<(String, String)>)> = vec![
            (t!("tui-keys-thread"), vec![("↑ ↓   j k".into(), t!("help-scroll")), ("Esc".into(), t!("help-back"))]),
            (
                t!("tui-message"),
                vec![
                    (enter, t!("help-send")),
                    (t!("key-shift-enter"), t!("help-newline")),
                    ("Ctrl+J".into(), t!("help-newline-anywhere")),
                    ("/".into(), t!("hint-commands")),
                    ("Esc".into(), t!("help-back")),
                ],
            ),
            (
                t!("tui-keys-mouse"),
                vec![(t!("key-wheel"), t!("help-scroll")), (t!("key-click"), t!("hint-choose")), (t!("key-shift-drag"), t!("help-select-text"))],
            ),
        ];
        let column = |groups: &[(String, Vec<(String, String)>)]| {
            let key_width = groups.iter().flat_map(|g| &g.1).map(|(k, _)| cols(k)).max().unwrap_or(0) + 3;
            let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
            for (i, (title, keys)) in groups.iter().enumerate() {
                if i > 0 {
                    lines.push(Vec::new());
                }
                lines.push(vec![Span::styled(title.clone(), BOLD)]);
                for (key, what) in keys {
                    lines.push(vec![Span::styled(format!("  {key}{}", " ".repeat(key_width - cols(key))), STRONG), Span::raw(what.clone())]);
                }
            }
            lines
        };
        let (left, right) = (column(&left), column(&right));
        let left_width = left.iter().map(|l| l.iter().map(|s| cols(&s.content)).sum::<usize>()).max().unwrap_or(0) + 4;
        let width_of = |c: &Vec<Vec<Span>>| c.iter().map(|l| l.iter().map(|s| cols(&s.content)).sum::<usize>()).max().unwrap_or(0);
        let narrow = left_width + width_of(&right) + 6 > room as usize;
        let (left, right) = if narrow { ([left, vec![Vec::new()], right].concat(), Vec::new()) } else { (left, right) };
        let mut lines: Vec<Line> = (0..left.len().max(right.len()))
            .map(|i| {
                let mut spans = left.get(i).cloned().unwrap_or_default();
                let used: usize = spans.iter().map(|s| cols(&s.content)).sum();
                spans.push(Span::raw(" ".repeat(left_width.saturating_sub(used))));
                spans.extend(right.get(i).cloned().unwrap_or_default());
                Line::from(spans)
            })
            .collect();
        lines.push(Line::default());
        lines.push(Line::styled(t!("tui-commands-hint"), DIM));
        lines
    }
}

/// Adding someone: what they have (or want) is the way in.
pub(super) fn add_answers() -> [(String, Style); 5] {
    [
        (t!("add-create-link"), Style::new()),
        (t!("add-create-code"), Style::new()),
        (t!("add-use-link"), Style::new()),
        (t!("add-use-code"), Style::new()),
        (t!("cancel"), Style::new()),
    ]
}

/// Removing someone: keep the conversation, delete it too, or not at all.
pub(super) fn remove_answers(name: &str) -> [(String, Style); 3] {
    [(t!("remove-name", name = name), Style::new()), (t!("tui-remove-wipe"), Style::new().fg(Color::Red)), (t!("tui-remove-no"), Style::new())]
}

/// Their last message, first line, as the list shows it.
fn preview(c: &Conversation) -> String {
    match c.messages.last() {
        Some(m) if m.outgoing => t!("preview-you", text = m.text.lines().next().unwrap_or_default()),
        Some(m) => m.text.lines().next().unwrap_or_default().to_string(),
        None => t!("no-messages"),
    }
}

/// One column of room on each side, inside a frame.
fn pad(r: Rect) -> Rect {
    Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r }
}

/// A framed window in the middle, as small as its content and never wider
/// than the screen (long lines wrap), keys at the bottom.
pub(super) fn popup(frame: &mut Frame, title: &str, lines: Vec<Line>, keys: Option<Line<'static>>) {
    popup_at(frame, title, lines, keys, 0);
}

/// The same, with the content scrolled by `scroll` lines when it is taller
/// than the screen: the keys stay where they are.
pub(super) fn popup_at(frame: &mut Frame, title: &str, mut lines: Vec<Line>, keys: Option<Line<'static>>, scroll: usize) -> usize {
    let area = frame.area();
    lines.insert(0, Line::default());
    // Text wraps at a width that reads well (a QR code still fits); lines
    // made of several pieces, like the columns of the help, keep theirs.
    let widest = lines.iter().chain(&keys).map(|l| if l.spans.len() > 1 { l.width() } else { l.width().min(80) }).max().unwrap_or(0);
    let width = (widest.max(cols(title) + 4) as u16 + 4).min(area.width);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    // Below the content: a blank row and the keys, then a spare row.
    let below: u16 = if keys.is_some() { 3 } else { 1 };
    let content = paragraph.line_count(width.saturating_sub(4)) as u16;
    let height = (content + below + 2).min(area.height);
    let rect = Rect { x: (area.width - width) / 2, y: (area.height - height) / 2, width, height };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(format!(" {title} "), STRONG))
        .padding(Padding::horizontal(1));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let body = Rect { height: inner.height.saturating_sub(below), ..inner };
    let hidden = content.saturating_sub(body.height);
    let top = (scroll.min(hidden as usize)) as u16;
    frame.render_widget(paragraph.scroll((top, 0)), body);
    if let Some(keys) = keys {
        frame.render_widget(fit_keys(keys, inner.width as usize), Rect { y: inner.y + inner.height.saturating_sub(2), height: 1, ..inner });
    }
    // More above or below: an arrow at the edge.
    let mark = |up: bool| Span::styled(if up { "▲" } else { "▼" }, DIM);
    if top > 0 {
        frame.render_widget(mark(true), Rect { x: rect.x + rect.width - 2, y: body.y, width: 1, height: 1 });
    }
    if top < hidden {
        frame.render_widget(mark(false), Rect { x: rect.x + rect.width - 2, y: body.y + body.height.saturating_sub(1), width: 1, height: 1 });
    }
    top as usize
}

/// Logical lines cut to `width` columns, by character: what the input box
/// shows, row by row.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
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

/// Display columns.
pub(super) fn cols(s: &str) -> usize {
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
pub(super) fn wrap_words(text: &str, width: usize) -> Vec<String> {
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

pub(super) fn clock(millis: u64) -> String {
    zoned(millis).map_or("--:--".into(), |z| z.strftime("%H:%M").to_string())
}

fn today() -> Option<jiff::Zoned> {
    zoned(now_millis())
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
pub(super) fn number_lines(number: &str) -> Vec<String> {
    let groups: Vec<String> = number.as_bytes().chunks(5).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
    groups.chunks(4).map(|row| format!("    {}", row.join("  "))).collect()
}

/// Digits three rows high, to read aloud from across the room: groups of
/// digits (`482-109-…`) three columns apart. Other characters are dropped.
pub(super) fn big_digits(code: &str) -> [String; 3] {
    const FONT: [[&str; 3]; 10] = [
        ["█▀█", "█ █", "▀▀▀"],
        [" ▀█", "  █", "  ▀"],
        ["▀▀█", "█▀▀", "▀▀▀"],
        ["▀▀█", " ▀█", "▀▀▀"],
        ["█ █", "▀▀█", "  ▀"],
        ["█▀▀", "▀▀█", "▀▀▀"],
        ["█▀▀", "█▀█", "▀▀▀"],
        ["▀▀█", "  █", "  ▀"],
        ["█▀█", "█▀█", "▀▀▀"],
        ["█▀█", "▀▀█", "▀▀▀"],
    ];
    std::array::from_fn(|row| {
        code.split('-')
            .map(|group| group.chars().filter_map(|c| c.to_digit(10)).map(|d| FONT[d as usize][row]).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("   ")
    })
}

pub fn groups(number: &str) -> String {
    number.as_bytes().chunks(5).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn big_digits_keep_the_groups_apart() {
        let rows = big_digits("482-10");
        assert_eq!(rows[0], "█ █ █▀█ ▀▀█    ▀█ █▀█");
        assert_eq!(rows[2], "  ▀ ▀▀▀ ▀▀▀     ▀ ▀▀▀");
        assert!(rows.iter().all(|r| r.chars().count() == rows[0].chars().count()));
        // A whole code: 4 groups of 3, 53 columns.
        assert_eq!(big_digits("482-109-375-512")[1].chars().count(), 53);
    }

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
    fn a_name_takes_the_middle_colour_of_its_rings() {
        // Whatever the hash gives, it is one of the window's middle rings.
        let Some(Color::Rgb(r, g, b)) = name_style("srv").fg else { panic!() };
        assert_eq!(format!("#{r:02X}{g:02X}{b:02X}"), nightpost_core::rings("srv")[1]);
    }

    /// What a screen of this size shows, row by row.
    fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect()
    }

    #[test]
    fn small_terminals_keep_the_name_and_whole_hints() {
        let mut app = App::new(Path::new("/nonexistent"), true);
        for name in ["Note", "carol-with-a-long-name"] {
            app.conversations.push(Conversation { name: name.into(), note: name == "Note", timer: 0, person: Default::default(), unread: 0, messages: Vec::new(), timer_set: None });
        }
        app.selected = 1;
        for (width, height) in [(60, 20), (80, 24), (100, 30)] {
            let rows = render(&mut app, width, height);
            let screen = rows.join("\n");
            assert!(screen.contains("carol-with-a-long-name"), "{width}x{height}: the name is cut\n{screen}");
            // The last row is hints, each of them whole: it ends on a full label.
            let bar = rows.last().unwrap().trim_end().to_string();
            let whole = ["send", "new line", "commands", "list", "pane", "conversations"];
            assert!(whole.iter().any(|w| bar.ends_with(w)), "{width}x{height}: the bar is cut: {bar:?}");
        }
        // Alone in the list on a narrow screen: the list, with how to add someone.
        app.focus = Focus::List;
        let rows = render(&mut app, 60, 20);
        assert!(rows.join("\n").contains("Conversations"));
    }
}
