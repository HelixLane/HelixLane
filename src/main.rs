mod app;
mod file_ui;
mod terminal_ui;
mod theme;
mod tmux_ui;

use app::HelixLane;
use gpui_kit::component::TitleBar;
use gpui_kit::*;

actions!(helixlane, [Quit]);

fn main() -> std::process::ExitCode {
    let args = match helixlane::launch::os_arguments() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("HelixLane: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let entry = match helixlane::launch::parse_native(&args) {
        Ok(e) => e,
        Err(error) => {
            eprintln!("HelixLane: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    drop(args);
    use helixlane::launch::{Entry, MAX_FRAME};
    let request = match entry {
        Entry::Help => {
            print!("{}", helixlane::launch::HELP);
            return std::process::ExitCode::SUCCESS;
        }
        Entry::Version => {
            println!("HelixLane {}", env!("CARGO_PKG_VERSION"));
            return std::process::ExitCode::SUCCESS;
        }
        Entry::Open(r) => Some(r),
        Entry::Gui => Some(helixlane::launch::OpenRequest::focus()),
        Entry::Stdin => {
            use std::io::Read;
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            if std::io::stdin()
                .take((MAX_FRAME + 1) as u64)
                .read_to_end(&mut bytes)
                .is_err()
            {
                eprintln!("HelixLane: Unable to read launch request / 无法读取启动请求");
                return std::process::ExitCode::from(2);
            }
            match helixlane::launch::decode(&bytes) {
                Ok(r) => Some(r),
                Err(error) => {
                    eprintln!("HelixLane: {error}");
                    return std::process::ExitCode::from(2);
                }
            }
        }
    };
    // Every entry path shares the protected primary-instance channel.
    if let Some(r) = &request {
        match helixlane::launch::ipc::forward(r) {
            Ok(true) => return std::process::ExitCode::SUCCESS,
            Ok(false) => {}
            Err(error) => {
                eprintln!("HelixLane: {error}");
                return std::process::ExitCode::from(2);
            }
        }
    }
    let server = match helixlane::launch::ipc::start() {
        Ok(server) => server,
        Err(error) => {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            if let Some(request) = &request {
                while std::time::Instant::now() < deadline {
                    match helixlane::launch::ipc::forward(request) {
                        Ok(true) => return std::process::ExitCode::SUCCESS,
                        Ok(false) => std::thread::sleep(std::time::Duration::from_millis(25)),
                        Err(_) => break,
                    }
                }
            }
            eprintln!("HelixLane: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let urls = server.sender();
    let application = application().with_assets(assets::Assets);
    application.on_open_urls(move |incoming| {
        for url in incoming
            .into_iter()
            .take(helixlane::launch::ipc::QUEUE_CAPACITY)
        {
            match helixlane::launch::parse_native(&[url]) {
                Ok(Entry::Open(r)) => {
                    if urls.try_send(r).is_err() {
                        eprintln!("HelixLane: Launch queue full / 启动队列已满");
                    }
                }
                _ => eprintln!("HelixLane: Invalid protocol URL / 协议 URL 无效；参数值已隐藏"),
            }
        }
    });
    application.run(move |cx| {
        gpui_shell::init(cx);
        init(cx);
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
            cx.new(|cx| {
                let mut app = HelixLane::new(window, cx);
                if let Some(request) = request {
                    app.dispatch_launch(request, window, cx);
                }
                app.listen_launch(server, window, cx);
                app
            })
        })
        .expect("Failed to open the HelixLane desktop window");
        cx.activate(true);
    });
    std::process::ExitCode::SUCCESS
}
