//! Where a token may be sent, and where an unattended token may come from.
//!
//! This is the code that must not diverge between plugins, so it lives here
//! once. The GitHub and GitLab plugins each carried a byte-identical copy of
//! every function below; the only genuinely per-forge parts are which hosts are
//! "home", which environment variables are consulted, and which authority the
//! Git credential helper is bound to. Those are the fields of [`TokenPolicy`].
//!
//! Two rules are enforced here rather than left to callers:
//!
//! - A token is never sent over cleartext unless the target is loopback, which
//!   cannot leave the machine.
//! - *Ambient* credentials — environment variables and the Git credential
//!   helper, which the user did not type for this call — reach only the forge's
//!   own hosts, the detected remote's host, or loopback. A redirected API URL
//!   therefore cannot collect them; that case needs an explicit token.

use std::{
    env,
    io::{Read, Write},
    process::{Child, Output, Stdio},
    time::{Duration, Instant},
};

use ah_plugin_api::noninteractive_command;

/// How long the Git credential helper may run before it is killed.
pub const DEFAULT_HELPER_TIMEOUT: Duration = Duration::from_secs(5);

/// The per-forge facts [`resolve`] needs; everything else about the policy is
/// the same for every forge.
#[derive(Debug, Clone)]
pub struct TokenPolicy<'a> {
    /// Authorities that accept ambient credentials unconditionally, because they
    /// are the forge itself.
    pub home_authorities: &'a [&'a str],
    /// Environment variables consulted in order, ahead of the credential helper.
    pub env_vars: &'a [&'a str],
    /// The authority the Git credential helper is bound to. `None` skips the
    /// helper, which is also how a caller turns it off.
    pub credential_authority: Option<String>,
    /// The detected Git remote, whose host also accepts ambient credentials.
    pub remote_url: Option<&'a str>,
    /// How long the helper may run. Use [`DEFAULT_HELPER_TIMEOUT`].
    pub helper_timeout: Duration,
}

/// The target refuses tokens: a cleartext URL that is not loopback. Callers turn
/// this into their own error code, which is frozen per plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsecureTokenTarget;

/// A token and the authority it was resolved for. A caller that attaches the
/// token per-request compares that authority against the request URL, so a token
/// resolved for one host is never sent to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedToken {
    pub token: Option<String>,
    /// `None` only when the target refuses tokens entirely.
    pub authority: Option<String>,
}

impl TokenPolicy<'_> {
    /// Whether `authority` may receive credentials the caller did not type for
    /// this call. Exposed so a plugin can test its own policy rather than a
    /// restatement of it.
    pub fn accepts_ambient_credentials(&self, authority: &str) -> bool {
        if self.home_authorities.contains(&authority) || is_loopback_authority(authority) {
            return true;
        }
        self.remote_url
            .and_then(remote_authority)
            .is_some_and(|remote| remote == authority)
    }
}

impl ResolvedToken {
    fn none() -> Self {
        Self {
            token: None,
            authority: None,
        }
    }
}

/// Resolve the token for `api_url`, preferring `explicit` over anything ambient.
///
/// # Errors
///
/// Returns [`InsecureTokenTarget`] when `explicit` is set and `api_url` is
/// cleartext off-box. Without an explicit token that case is not an error, it
/// just yields no token: the caller did not ask to authenticate, so there is
/// nothing to leak.
pub fn resolve(
    api_url: &str,
    explicit: Option<String>,
    policy: &TokenPolicy<'_>,
) -> Result<ResolvedToken, InsecureTokenTarget> {
    let Some(authority) = secure_authority(api_url) else {
        return match explicit {
            Some(_) => Err(InsecureTokenTarget),
            None => Ok(ResolvedToken::none()),
        };
    };
    if let Some(token) = explicit {
        return Ok(ResolvedToken {
            token: Some(token),
            authority: Some(authority),
        });
    }
    if !policy.accepts_ambient_credentials(&authority) {
        return Ok(ResolvedToken::none());
    }

    let token = policy
        .env_vars
        .iter()
        .find_map(|name| env_token(name))
        .or_else(|| {
            let host = policy.credential_authority.as_deref()?;
            git_credential_token(host, policy.helper_timeout)
        });
    Ok(ResolvedToken {
        token,
        authority: Some(authority),
    })
}

/// The authority a caller-supplied token may reach: https anywhere, or cleartext
/// only on loopback.
pub fn secure_authority(url: &str) -> Option<String> {
    https_authority(url).or_else(|| {
        let authority = url_authority(url, "http://")?;
        is_loopback_authority(&authority).then_some(authority)
    })
}

pub fn https_authority(url: &str) -> Option<String> {
    url_authority(url, "https://")
}

/// The host of `url` under `scheme`, lowercased, with any userinfo dropped —
/// `https://api.example.com@attacker.test/` is `attacker.test`, not
/// `api.example.com`.
pub fn url_authority(url: &str, scheme: &str) -> Option<String> {
    let remainder = url.trim().strip_prefix(scheme)?;
    let authority = remainder.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

pub fn is_loopback_authority(authority: &str) -> bool {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next() == Some("::1");
    }
    matches!(
        authority.split(':').next().unwrap_or(authority),
        "127.0.0.1" | "localhost"
    )
}

/// Extracts the host from either URL syntax or the scp-like `git@host:path` form.
pub fn remote_authority(remote: &str) -> Option<String> {
    let trimmed = remote.trim();
    for scheme in ["https://", "http://", "ssh://", "git+ssh://", "git://"] {
        if let Some(authority) = url_authority(trimmed, scheme) {
            return Some(authority);
        }
    }
    let (authority, _) = trimmed.split_once(':')?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!host.is_empty() && !host.contains('/')).then(|| host.to_ascii_lowercase())
}

/// A non-empty, trimmed environment variable.
pub fn env_token(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Ask `git credential fill` for the https password registered for `host`.
///
/// Prompting is disabled, so a helper that would need the user answers nothing
/// rather than blocking an unattended run.
pub fn git_credential_token(host: &str, timeout: Duration) -> Option<String> {
    let mut child = noninteractive_command("git")
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let written = match child.stdin.take() {
        Some(mut stdin) => stdin
            .write_all(format!("protocol=https\nhost={host}\n\n").as_bytes())
            .is_ok(),
        None => false,
    };
    let output = wait_for_credential_child(child, timeout)?;
    if !written || !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("password=") {
            let token = value.trim().to_owned();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    None
}

/// Wait for a credential helper, killing it at `timeout`.
///
/// A helper that hangs must not hang the command, and one that writes more than
/// a pipe buffer must not deadlock waiting for a reader.
pub fn wait_for_credential_child(mut child: Child, timeout: Duration) -> Option<Output> {
    let deadline = Instant::now().checked_add(timeout)?;
    let stdout = child.stdout.take();
    std::thread::scope(|scope| {
        // Drains stdout while polling so a chatty helper cannot fill the pipe and stall.
        let reader = scope.spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stdout) = stdout {
                let _ = stdout.read_to_end(&mut buffer);
            }
            buffer
        });
        while child.try_wait().ok()?.is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Some(Output {
            status: child.wait().ok()?,
            stdout: reader.join().ok()?,
            stderr: Vec::new(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB: TokenPolicy<'static> = TokenPolicy {
        home_authorities: &["api.github.com", "github.com"],
        env_vars: &[],
        credential_authority: None,
        remote_url: None,
        helper_timeout: DEFAULT_HELPER_TIMEOUT,
    };

    #[test]
    fn userinfo_never_becomes_the_authority() {
        assert_eq!(
            url_authority("https://api.github.com@attacker.example/api/v3", "https://").as_deref(),
            Some("attacker.example")
        );
        assert_eq!(
            remote_authority("https://github.com@attacker.example/o/r.git").as_deref(),
            Some("attacker.example")
        );
    }

    #[test]
    fn cleartext_is_a_token_target_only_on_loopback() {
        assert_eq!(
            secure_authority("http://127.0.0.1:8080/api").as_deref(),
            Some("127.0.0.1:8080")
        );
        assert_eq!(
            secure_authority("http://[::1]:8080/api").as_deref(),
            Some("[::1]:8080")
        );
        assert_eq!(secure_authority("http://ghe.corp.example/api"), None);
    }

    #[test]
    fn an_explicit_token_over_cleartext_fails_instead_of_leaking() {
        assert_eq!(
            resolve(
                "http://ghe.corp.example/api",
                Some("token-sentinel".to_owned()),
                &GITHUB,
            ),
            Err(InsecureTokenTarget)
        );
    }

    #[test]
    fn cleartext_without_a_token_is_not_an_error() {
        assert_eq!(
            resolve("http://ghe.corp.example/api", None, &GITHUB),
            Ok(ResolvedToken::none())
        );
    }

    #[test]
    fn an_explicit_token_reaches_any_https_host() {
        assert_eq!(
            resolve(
                "https://ghe.corp.example/api/v3",
                Some("token-sentinel".to_owned()),
                &GITHUB,
            ),
            Ok(ResolvedToken {
                token: Some("token-sentinel".to_owned()),
                authority: Some("ghe.corp.example".to_owned()),
            })
        );
    }

    #[test]
    fn a_redirected_url_collects_no_ambient_credentials() {
        // The helper is bound to a host, and an unrelated host is not an ambient
        // target at all, so neither source is even consulted.
        let policy = TokenPolicy {
            credential_authority: Some("attacker.example".to_owned()),
            ..GITHUB
        };
        assert_eq!(
            resolve("https://attacker.example/api/v3", None, &policy),
            Ok(ResolvedToken::none())
        );
    }

    #[test]
    fn the_detected_remote_host_accepts_ambient_credentials() {
        let policy = TokenPolicy {
            remote_url: Some("git@ghe.corp.example:owner/repo.git"),
            ..GITHUB
        };
        assert!(policy.accepts_ambient_credentials("ghe.corp.example"));
        assert!(!policy.accepts_ambient_credentials("other.corp.example"));
    }

    #[test]
    fn loopback_always_accepts_ambient_credentials() {
        for authority in ["127.0.0.1:8080", "localhost", "[::1]:8080"] {
            assert!(GITHUB.accepts_ambient_credentials(authority), "{authority}");
        }
    }

    #[test]
    fn a_hanging_helper_is_killed_at_the_timeout() {
        // A `git credential fill` with no stdin closed and no helper configured
        // still exits; to observe the kill path, wait on a child that will not.
        let child = noninteractive_command("git")
            .args(["credential", "fill"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(child) = child else {
            // No git on this machine; the timeout path has nothing to observe.
            return;
        };
        assert!(wait_for_credential_child(child, Duration::from_millis(20)).is_none());
    }
}
