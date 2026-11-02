mod app;
mod app_assets;
mod application;
mod file_ui;
mod terminal_ui;
mod theme;
mod tmux_ui;

use gpui_kit::{App, KeyBinding, actions, application, init};

actions!(helixlane, [Quit]);

fn register_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    let quit = KeyBinding::load("secondary-q", Box::new(Quit), None, false, None, cx)
        .expect("Platform quit binding must parse");
    cx.bind_keys([quit]);
}

#[cfg(all(test, feature = "shell-tests"))]
mod startup_tests {
    use super::register_actions;
    use gpui_kit as gpui;
    use gpui_kit::{Keystroke, TestAppContext};
    #[gpui::test]
    fn startup_registers_platform_quit_binding(cx: &mut TestAppContext) {
        cx.update(|cx| {
            register_actions(cx);
            let key = Keystroke::parse_with_os("secondary-q", cx.platform_info().os).unwrap();
            assert_eq!(cx.all_bindings_for_input(&[key]).len(), 1);
        });
    }
}

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
    let application = application().with_assets(app_assets::AppAssets);
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
        register_actions(cx);
        let owner = application::ApplicationOwner::install(cx);
        owner.start(server, request, cx);
    });
    std::process::ExitCode::SUCCESS
}
