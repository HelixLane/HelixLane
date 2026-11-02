//! Per-hop authentication. Prompt responses and agent signing stay out of persistence.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HopIdentity {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub index: usize,
    pub total: usize,
}
impl HopIdentity {
    pub fn label(&self) -> String {
        format!(
            "{}@{}:{} · {}/{}",
            self.username,
            self.host,
            self.port,
            self.index + 1,
            self.total
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthPrompt {
    pub hop: HopIdentity,
    pub instructions: String,
    pub prompts: Vec<(String, bool)>,
}
impl Session {
    pub fn answer_auth(&self, answers: Vec<Secret>) {
        if let Some(reply) = self.auth_reply.lock().unwrap().take() {
            let _ = reply.send(answers);
        }
    }
    async fn ask_auth(&self, prompt: AuthPrompt) -> Result<Vec<Secret>, String> {
        let count = prompt.prompts.len();
        let (tx, rx) = oneshot::channel();
        *self.auth_reply.lock().unwrap() = Some(tx);
        {
            let mut state = self.state.lock().unwrap();
            state.authentication = Some(prompt);
            state.revision += 1;
        }
        let answer = tokio::time::timeout(Duration::from_secs(120), rx).await;
        self.auth_reply.lock().unwrap().take();
        {
            let mut state = self.state.lock().unwrap();
            state.authentication = None;
            state.revision += 1;
        }
        let answer = answer
            .map_err(|_| "Authentication prompt timed out")?
            .map_err(|_| "Authentication cancelled")?;
        if answer.len() != count {
            return Err("Authentication response count mismatch".into());
        }
        Ok(answer)
    }
}
pub(super) async fn authenticate(
    session: &Session,
    ssh: &mut client::Handle<Handler>,
    profile: &ConnectionProfile,
    hop: HopIdentity,
    mut secret: Secret,
    remember: bool,
    prompt_missing: bool,
) -> Result<(), String> {
    session.status(Status::Authenticating);
    let uses_secret = matches!(
        profile.auth,
        AuthMethod::Password | AuthMethod::PrivateKey { .. }
    );
    if uses_secret
        && secret.expose().is_empty()
        && let Some(reference) = profile.credential_ref.clone()
    {
        secret = tokio::task::spawn_blocking(move || credentials::load(&reference))
            .await
            .map_err(|_| "Keychain task failed")??;
    } else if prompt_missing
        && matches!(profile.auth, AuthMethod::Password)
        && secret.expose().is_empty()
    {
        secret = session
            .ask_auth(AuthPrompt {
                hop: hop.clone(),
                instructions: "Jump host authentication / 跳板主机认证".into(),
                prompts: vec![("Password / 密码".into(), false)],
            })
            .await?
            .remove(0);
    }
    let success = match &profile.auth {
        AuthMethod::Password => ssh
            .authenticate_password(&profile.username, secret.expose())
            .await
            .map_err(|_| "Authentication transport failed")?
            .success(),
        AuthMethod::None => ssh
            .authenticate_none(&profile.username)
            .await
            .map_err(|_| "Authentication transport failed")?
            .success(),
        AuthMethod::PrivateKey { path } => {
            let path = path.clone();
            let passphrase = Secret::new(secret.expose().to_string());
            let key = tokio::task::spawn_blocking(move || {
                russh::keys::load_secret_key(
                    path,
                    if passphrase.expose().is_empty() {
                        None
                    } else {
                        Some(passphrase.expose())
                    },
                )
            })
            .await
            .map_err(|_| "Private-key task failed")?
            .map_err(|_| "Cannot read private key or incorrect passphrase")?;
            let hash = ssh
                .best_supported_rsa_hash()
                .await
                .map_err(|_| "Key negotiation failed")?
                .flatten();
            ssh.authenticate_publickey(
                &profile.username,
                PrivateKeyWithHashAlg::new(Arc::new(key), hash),
            )
            .await
            .map_err(|_| "Authentication transport failed")?
            .success()
        }
        AuthMethod::KeyboardInteractive => {
            use client::KeyboardInteractiveAuthResponse as Response;
            let mut response = ssh
                .authenticate_keyboard_interactive_start(&profile.username, None)
                .await
                .map_err(|_| "Interactive authentication failed")?;
            let mut rounds = 0;
            loop {
                match response {
                    Response::Success => break true,
                    Response::Failure { .. } => break false,
                    Response::InfoRequest {
                        name,
                        instructions,
                        prompts,
                    } => {
                        rounds += 1;
                        if rounds > 16
                            || prompts.len() > 16
                            || instructions.len() + name.len() > 16384
                            || prompts.iter().any(|p| p.prompt.len() > 4096)
                        {
                            return Err("Interactive authentication limits exceeded".into());
                        }
                        let answers = if prompts.is_empty() {
                            vec![]
                        } else {
                            session
                                .ask_auth(AuthPrompt {
                                    hop: hop.clone(),
                                    instructions: format!("{name}\n{instructions}"),
                                    prompts: prompts
                                        .into_iter()
                                        .map(|p| (p.prompt, p.echo))
                                        .collect(),
                                })
                                .await?
                        };
                        response = ssh
                            .authenticate_keyboard_interactive_respond(
                                answers.iter().map(|s| s.expose().to_string()).collect(),
                            )
                            .await
                            .map_err(|_| "Interactive authentication failed")?;
                    }
                }
            }
        }
        AuthMethod::Agent => {
            #[cfg(unix)]
            let mut agent = russh::keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|_| "SSH agent unavailable")?;
            #[cfg(windows)]
            let mut agent = russh::keys::agent::client::AgentClient::connect_named_pipe(
                r"\\.\pipe\openssh-ssh-agent",
            )
            .await
            .map_err(|_| "SSH agent unavailable")?;
            #[cfg(any(unix, windows))]
            {
                authenticate_agent(ssh, &profile.username, &mut agent).await?
            }
            #[cfg(not(any(unix, windows)))]
            {
                return Err("SSH agent is unavailable on this platform".into());
            }
        }
    };
    if !success {
        return Err("Authentication rejected".into());
    }
    if remember && uses_secret {
        let reference = profile
            .credential_ref
            .clone()
            .ok_or("Missing keychain reference")?;
        tokio::task::spawn_blocking(move || credentials::save(&reference, &secret))
            .await
            .map_err(|_| "Keychain task failed")??;
    }
    Ok(())
}
pub(super) async fn authenticate_agent<
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
>(
    ssh: &mut client::Handle<Handler>,
    user: &str,
    agent: &mut russh::keys::agent::client::AgentClient<S>,
) -> Result<bool, String> {
    let identities = agent
        .request_identities()
        .await
        .map_err(|_| "SSH agent identity request failed")?;
    if identities.len() > 64 {
        return Err("SSH agent supplied too many identities".into());
    }
    let hash = ssh
        .best_supported_rsa_hash()
        .await
        .map_err(|_| "Key negotiation failed")?
        .flatten();
    for identity in identities {
        if ssh
            .authenticate_publickey_with(user, identity.public_key().into_owned(), hash, agent)
            .await
            .map_err(|_| "SSH agent signing failed")?
            .success()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
