mod app;
mod editor;
mod theme;

use app::HelixLane;
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

actions!(helixlane, [Quit]);

fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let mut titlebar = TitleBar::title_bar_options();
        if let Some(position) = titlebar.traffic_light_position.as_mut() {
            // Preserve Kit's native-button alignment when increasing the toolbar height.
            position.y += (px(theme::TOOLBAR_HEIGHT) - gpui_kit::component::TITLE_BAR_HEIGHT) * 0.5;
        }

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1440.0), px(900.0)),
                cx,
            ))),
            window_min_size: Some(size(px(960.), px(640.))),
            titlebar: Some(TitlebarOptions {
                title: Some("HelixLane".into()),
                ..titlebar
            }),
            ..TitleBar::window_options()
        };

        open_window(options, cx, |window, cx| {
            cx.new(|cx| HelixLane::new(window, cx))
        })
        .expect("Failed to open the HelixLane desktop window");
        cx.activate(true);
    });
}
