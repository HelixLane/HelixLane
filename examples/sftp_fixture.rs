//! Run explicitly: cargo run --no-default-features --example sftp_fixture
//! Passwordless `fixture` account, loopback only. Approve the printed host key in the UI.
//! OpenSSH SFTP backend, not full sshd. The temporary root is an initial directory, NOT a sandbox.
#[path = "support/sftp_server.rs"]
mod fixture;
use russh::server::Server as _;
#[tokio::main]
async fn main() {
    let root = std::env::temp_dir().join(format!(
        "helixlane-sftp-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("fixture-file.txt"), "OpenSSH SFTP backend\n").unwrap();
    let key = fixture::key();
    println!(
        "HELIXLANE_FIXTURE_PUBLIC_KEY={}",
        key.public_key().to_openssh().unwrap()
    );
    println!(
        "HELIXLANE_FIXTURE_FINGERPRINT={}",
        key.public_key().fingerprint(russh::keys::HashAlg::Sha256)
    );
    let port = std::env::var("HELIXLANE_FIXTURE_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    println!(
        "HELIXLANE_FIXTURE_PORT={}",
        listener.local_addr().unwrap().port()
    );
    println!("HELIXLANE_FIXTURE_ROOT={}", root.display());
    println!(
        "HELIXLANE_FIXTURE_USER=fixture (authentication None; explicit host-key trust required)"
    );
    let config = std::sync::Arc::new(russh::server::Config {
        keys: vec![key],
        auth_rejection_time: std::time::Duration::ZERO,
        ..Default::default()
    });
    fixture::Fixture::new(root)
        .run_on_socket(config, &listener)
        .await
        .unwrap();
}
