//! Native committed-text / IME bridge. Raw control keys stay in KeyDown.
use super::*;
use std::ops::Range;
impl EntityInputHandler for TerminalPane {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16 = self.preedit.encode_utf16().collect::<Vec<_>>();
        let start = range.start.min(utf16.len());
        let end = range.end.min(utf16.len()).max(start);
        *actual = Some(start..end);
        Some(String::from_utf16_lossy(&utf16[start..end]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.preedit.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.preedit.is_empty()).then(|| 0..self.preedit.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit.clear();
        self.composition_target = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Terminal input is append-only: OS replacement ranges address only local
        // preedit, never text already sent to a remote program.
        self.preedit.clear();
        let captured = self.composition_target.take();
        if let Some(session) = &self.session
            && captured.is_none_or(|id| id == session.id)
        {
            session.input(text.as_bytes().to_vec());
        }
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preedit.is_empty() {
            self.composition_target = self.session.as_ref().map(|session| session.id);
        }
        self.preedit = text.to_string();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (col, row) = self
            .session
            .as_ref()?
            .state
            .lock()
            .unwrap()
            .terminal
            .cursor()?;
        Some(Bounds::new(
            point(
                bounds.origin.x + px(col as f32 * 8.4),
                bounds.origin.y + px(row as f32 * 20.),
            ),
            size(px(8.4), px(20.)),
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.preedit.encode_utf16().count())
    }
    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.preedit.encode_utf16().count())
    }
    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        self.session.as_ref().is_some_and(|s| s.terminal_running())
    }
}
