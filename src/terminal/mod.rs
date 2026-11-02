pub mod input;
use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{Config, TermMode, cell::Flags, test::TermSize},
    vte::ansi::{Color, Processor},
};
use std::sync::{Arc, Mutex};
#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<String>>>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            self.0.lock().unwrap().push(text);
        }
    }
}
#[derive(Clone)]
pub struct Cell {
    pub column: usize,
    pub wide: bool,
    pub selected: bool,
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub inverse: bool,
}
pub struct Terminal {
    term: Term<Listener>,
    parser: Processor,
    last_search: Option<(String, alacritty_terminal::term::search::Match)>,
    listener: Listener,
}
impl Terminal {
    pub fn new(columns: usize, rows: usize) -> Self {
        let listener = Listener::default();
        Self {
            term: Term::new(
                Config::default(),
                &TermSize::new(columns.max(2), rows.max(1)),
                listener.clone(),
            ),
            parser: Processor::new(),
            last_search: None,
            listener,
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.last_search = None;
    }
    pub fn replies(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.listener.0.lock().unwrap())
    }
    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.term.resize(TermSize::new(columns.max(2), rows.max(1)));
        self.last_search = None;
    }
    pub fn dimensions(&self) -> (usize, usize) {
        (self.term.columns(), self.term.screen_lines())
    }
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }
    pub fn application_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }
    pub fn select(&mut self, start: (usize, usize), end: (usize, usize)) {
        let point = |p: (usize, usize)| {
            Point::new(
                Line(p.1 as i32 - self.term.grid().display_offset() as i32),
                Column(p.0),
            )
        };
        let mut selection = Selection::new(SelectionType::Simple, point(start), Side::Left);
        selection.update(point(end), Side::Right);
        self.term.selection = Some(selection);
    }
    pub fn selection(&self) -> Option<String> {
        self.term.selection_to_string()
    }
    pub fn cells(&self) -> Vec<Vec<Cell>> {
        let mut rows = vec![Vec::new(); self.term.screen_lines()];
        let content = self.term.renderable_content();
        for indexed in content.display_iter {
            let row = (indexed.point.line.0 + content.display_offset as i32) as usize;
            if row >= rows.len() {
                continue;
            }
            let c = indexed.cell;
            if c.flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let mut text = c.c.to_string();
            if let Some(extra) = c.zerowidth() {
                text.extend(extra);
            }
            rows[row].push(Cell {
                column: indexed.point.column.0,
                wide: c.flags.contains(Flags::WIDE_CHAR),
                selected: content
                    .selection
                    .is_some_and(|range| range.contains(indexed.point)),
                text,
                fg: c.fg,
                bg: c.bg,
                bold: c.flags.contains(Flags::BOLD),
                inverse: c.flags.contains(Flags::INVERSE),
            });
        }
        rows
    }
    pub fn cursor(&self) -> Option<(usize, usize)> {
        let content = self.term.renderable_content();
        if !content.mode.contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        Some((
            content.cursor.point.column.0,
            (content.cursor.point.line.0 + content.display_offset as i32) as usize,
        ))
    }
    /// Literal, case-sensitive search through retained grid history, preserving
    /// combining marks and mapping UTF-8 positions back to wide terminal cells.
    pub fn find(&mut self, query: &str, backwards: bool) -> bool {
        if query.is_empty() || query.len() > 4096 {
            self.last_search = None;
            self.term.selection = None;
            return false;
        }
        let mut text = String::new();
        let mut positions = Vec::new();
        let grid = self.term.grid();
        for line in -(grid.history_size() as i32)..self.term.screen_lines() as i32 {
            for column in 0..self.term.columns() {
                let point = Point::new(Line(line), Column(column));
                let cell = &grid[point];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                positions.push((text.len(), point));
                text.push(cell.c);
                if let Some(extra) = cell.zerowidth() {
                    text.extend(extra);
                }
            }
            if !grid[Line(line)][Column(self.term.columns() - 1)]
                .flags
                .contains(Flags::WRAPLINE)
            {
                text.push('\n');
            }
        }
        let point_at = |offset| {
            positions[positions
                .partition_point(|(byte, _)| *byte <= offset)
                .saturating_sub(1)]
            .1
        };
        let matches: Vec<_> = text
            .match_indices(query)
            .map(|(start, _)| point_at(start)..=point_at(start + query.len() - 1))
            .collect();
        let previous = self
            .last_search
            .as_ref()
            .filter(|(q, _)| q == query)
            .map(|(_, range)| *range.start());
        let origin =
            previous.unwrap_or(Point::new(Line(-(grid.display_offset() as i32)), Column(0)));
        let found = if backwards {
            matches
                .iter()
                .rev()
                .find(|m| *m.start() < origin)
                .or_else(|| matches.last())
        } else {
            matches
                .iter()
                .find(|m| {
                    if previous.is_some() {
                        *m.start() > origin
                    } else {
                        *m.start() >= origin
                    }
                })
                .or_else(|| matches.first())
        };
        let Some(found) = found.cloned() else {
            self.last_search = None;
            self.term.selection = None;
            return false;
        };
        self.term.scroll_to_point(*found.start());
        let mut selection = Selection::new(SelectionType::Simple, *found.start(), Side::Left);
        selection.update(*found.end(), Side::Right);
        self.term.selection = Some(selection);
        self.last_search = Some((query.into(), found));
        true
    }
    pub fn text(&self) -> String {
        self.cells()
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| c.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_history_wrap_unicode_resize_and_literal_metacharacters() {
        let mut t = Terminal::new(20, 3);
        t.feed("old 界e\u{301}.* marker\r\n".as_bytes());
        for _ in 0..20 {
            t.feed(b"new output\r\n");
        }
        assert!(!t.text().contains("old"));
        assert!(t.find("界e\u{301}.*", false));
        assert_eq!(t.selection().as_deref(), Some("界e\u{301}.*"));
        assert!(t.text().contains("old"));
        assert!(t.find("old", true));
        t.resize(10, 3);
        assert!(t.find("marker", false));
        assert!(!t.find("absent", false));
        assert_eq!(t.selection(), None);
        assert!(!t.find("", false));
        let mut t = Terminal::new(8, 2);
        t.feed(b"123456needle");
        assert!(t.find("needle", false));
        assert_eq!(t.selection().as_deref(), Some("needle"));
        let mut t = Terminal::new(12, 2);
        t.feed(b"one hit\r\ntwo hit\r\nthree hit");
        assert!(t.find("hit", false));
        let first = t.last_search.as_ref().unwrap().1.clone();
        assert!(t.find("hit", false));
        let second = t.last_search.as_ref().unwrap().1.clone();
        assert_ne!(first, second);
        assert!(t.find("hit", true));
        assert_eq!(t.last_search.as_ref().unwrap().1, first);
    }
    #[test]
    fn ansi_cursor_erase_and_alternate_screen_are_interpreted() {
        let mut t = Terminal::new(40, 8);
        t.feed(b"hello\r\x1b[31mworld\x1b[0m");
        assert!(t.text().contains("world"));
        assert!(!t.text().contains("hello"));
        t.feed(b"\x1b[?1049hother screen");
        assert!(t.text().contains("other screen"));
        assert!(!t.text().contains("world"));
        t.feed(b"\x1b[?1049l");
        assert!(t.text().contains("world"));
        t.feed(b"\r\x1b[2Kcleared");
        assert!(!t.text().contains("world"));
    }
}
