#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
fn main() -> std::process::ExitCode {
    helixlane::launch::attach_parent_console();
    if let Err(error) = helixlane::launch::run_proxy(
        helixlane::launch::parse_putty,
        helixlane::launch::PUTTY_HELP,
    ) {
        eprintln!(
            "HelixLane launcher: {}",
            helixlane::launch::diagnostic(&error)
        );
        return std::process::ExitCode::from(2);
    }
    std::process::ExitCode::SUCCESS
}
