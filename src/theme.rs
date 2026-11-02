use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::*;
use helixlane::settings::ThemePreference;
pub const TOOLBAR_HEIGHT: f32 = 44.;
#[derive(Clone, Copy)]
pub struct Palette {
    pub frame: u32,
    pub panel: u32,
    pub editor: u32,
    pub hover: u32,
    pub selected: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub border: u32,
    pub warning: u32,
}
impl Palette {
    pub fn current(cx: &App) -> Self {
        if cx.theme().is_dark() {
            Self {
                frame: 0x292a2e,
                panel: 0x202125,
                editor: 0x1e1f22,
                hover: 0x35373d,
                selected: 0x34415e,
                text: 0xcdd0d8,
                muted: 0x969ba8,
                accent: 0x88aaff,
                border: 0x3e4149,
                warning: 0xd9b476,
            }
        } else {
            Self {
                frame: 0xe9eaed,
                panel: 0xf9f9fb,
                editor: 0xffffff,
                hover: 0xe4e6ec,
                selected: 0xdce7ff,
                text: 0x252830,
                muted: 0x626978,
                accent: 0x315eaf,
                border: 0xd1d5df,
                warning: 0x8a5c16,
            }
        }
    }
}
pub fn apply(mode: ThemePreference, window: Option<&mut Window>, cx: &mut App) {
    match mode {
        ThemePreference::System => Theme::sync_system_appearance(window, cx),
        ThemePreference::Dark => Theme::change(ThemeMode::Dark, window, cx),
        ThemePreference::Light => Theme::change(ThemeMode::Light, window, cx),
    }
}
pub fn row() -> Div {
    div().flex().items_center()
}
pub fn island(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .min_h_0()
        .rounded(px(10.))
        .overflow_hidden()
        .bg(rgb(p.panel))
}
pub fn icon(name: assets::IconName) -> gpui_kit::component::Icon {
    gpui_kit::component::Icon::new(name).size(px(16.))
}
