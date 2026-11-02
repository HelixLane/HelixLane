fn main() -> std::process::ExitCode {
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
