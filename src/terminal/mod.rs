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
            listener,
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }
    pub fn replies(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.listener.0.lock().unwrap())
    }
    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.term.resize(TermSize::new(columns.max(2), rows.max(1)));
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
