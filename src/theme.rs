use gpui_kit::*;

pub const TOOLBAR_HEIGHT: f32 = 44.;

pub const FRAME: u32 = 0x292a2e;
pub const PANEL: u32 = 0x202125;
pub const EDITOR: u32 = 0x1e1f22;
pub const HOVER: u32 = 0x35373d;
pub const SELECTED: u32 = 0x34415e;
pub const TEXT: u32 = 0xcdd0d8;
pub const MUTED: u32 = 0x858994;
pub const ACCENT: u32 = 0x88aaff;
pub const GREEN: u32 = 0x83bd93;
pub const ORANGE: u32 = 0xd69a73;

pub fn row() -> Div {
    div().flex().items_center()
}

pub fn island() -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .min_h_0()
        .rounded(px(10.))
        .overflow_hidden()
        .bg(rgb(PANEL))
}
pub fn icon(name: assets::IconName) -> gpui_kit::component::Icon {
    gpui_kit::component::Icon::new(name).size(px(16.))
}
