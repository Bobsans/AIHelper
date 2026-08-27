//! Which repository, which API host, and which token.
//!
//! Everything a request needs before it can be built, and the only part of the
//! plugin that reads the environment: a git remote, the credential helper, the
//! two token variables.

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct RepoSlug {
    pub(crate) owner: String,
    pub(crate) repo: String,
}

impl RepoSlug {
    pub(crate) fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

#[derive(Debug)]
pub(crate) struct GithubContext {
    pub(crate) client: Client,
    pub(crate) api_url: String,
    pub(crate) token: Option<String>,
    pub(crate) repo: RepoSlug,
    pub(crate) remote_url: Option<String>,
}

pub(crate) fn github_context(
    args: &GithubConnectionArgs,
) -> Result<GithubContext, InvocationResponse> {
    let api_url = normalize_api_url(&args.api_url)?;
    let (repo, remote_url) = resolve_repo(args)?;
    let token = resolve_token(args, &api_url, remote_url.as_deref())?;
    let client = Client::builder()
        .timeout(Duration::from_secs(args.timeout_secs.max(1)))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| {
            InvocationResponse::error(
                "GITHUB_HTTP_FAILED",
                format!("failed to create HTTP client: {error}"),
            )
        })?;

    Ok(GithubContext {
        client,
        api_url,
        token,
        repo,
        remote_url,
    })
}

pub(crate) fn resolve_repo(
    args: &GithubConnectionArgs,
) -> Result<(RepoSlug, Option<String>), InvocationResponse> {
    if let Some(repo) = &args.repo {
        return parse_repo_slug(repo)
            .map(|slug| (slug, None))
            .ok_or_else(|| invalid_repo(repo));
    }

    let remote_url = git::remote_url(&args.remote, args.cwd.as_deref())?;
    parse_github_remote_url(&remote_url)
        .map(|slug| (slug, Some(remote_url.clone())))
        .ok_or_else(|| {
            InvocationResponse::error(
                "GITHUB_REPO_UNDETECTED",
                format!(
                    "could not detect GitHub owner/repo from remote '{}': {}",
                    args.remote, remote_url
                ),
            )
        })
}

pub(crate) fn parse_repo_slug(value: &str) -> Option<RepoSlug> {
    let normalized = value.trim().trim_end_matches(".git");
    let (owner, repo) = normalized.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(RepoSlug {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    })
}

pub(crate) fn parse_github_remote_url(remote: &str) -> Option<RepoSlug> {
    let trimmed = remote.trim();
    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        return parse_repo_slug(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        return parse_repo_slug(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("http://github.com/") {
        return parse_repo_slug(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://git@github.com/") {
        return parse_repo_slug(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("git+ssh://git@github.com/") {
        return parse_repo_slug(rest);
    }
    None
}

pub(crate) fn invalid_repo(value: &str) -> InvocationResponse {
    InvocationResponse::error(
        "INVALID_ARGUMENT",
        format!("--repo must use OWNER/REPO format, got '{value}'"),
    )
}

pub(crate) fn normalize_api_url(value: &str) -> Result<String, InvocationResponse> {
    let normalized = value.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--api-url must not be empty",
        ));
    }
    Ok(normalized)
}

pub(crate) fn resolve_token(
    args: &GithubConnectionArgs,
    api_url: &str,
    remote_url: Option<&str>,
) -> Result<Option<String>, InvocationResponse> {
    let explicit = args.token.clone().filter(|value| !value.trim().is_empty());
    credentials::resolve(api_url, explicit, &token_policy(args, api_url, remote_url))
        .map(|resolved| resolved.token)
        .map_err(|credentials::InsecureTokenTarget| insecure_token_target())
}

/// Where a GitHub token may come from when the caller supplied none.
pub(crate) fn token_policy<'a>(
    args: &GithubConnectionArgs,
    api_url: &str,
    remote_url: Option<&'a str>,
) -> credentials::TokenPolicy<'a> {
    credentials::TokenPolicy {
        // `api.github.com` and `github.com` are the same forge, so a credential
        // registered for either is registered for both.
        home_authorities: &[DEFAULT_API_AUTHORITY, "github.com"],
        env_vars: &["GITHUB_TOKEN", "GH_TOKEN"],
        credential_authority: args
            .use_git_credential
            .then(|| credential_authority(api_url))
            .flatten(),
        remote_url,
        helper_timeout: GIT_CREDENTIAL_TIMEOUT,
    }
}

pub(crate) fn insecure_token_target() -> InvocationResponse {
    InvocationResponse::error(
        "GITHUB_INSECURE_TOKEN_TARGET",
        "refusing to send a token to a cleartext --api-url; use https or a loopback host",
    )
}

/// Binds the credential helper lookup to the host that will receive the token,
/// so a redirected `--api-url` can never collect GitHub credentials.
pub(crate) fn credential_authority(api_url: &str) -> Option<String> {
    let authority = credentials::https_authority(api_url)?;
    Some(if authority == DEFAULT_API_AUTHORITY {
        "github.com".to_owned()
    } else {
        authority
    })
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    #[test]
    fn ambient_tokens_reach_only_github_loopback_or_the_detected_remote() {
        // Asserted through the policy the plugin actually builds, not a
        // restatement of it.
        let accepts = |authority: &str, remote: Option<&str>| {
            token_policy(&connection_args(DEFAULT_API_URL), DEFAULT_API_URL, remote)
                .accepts_ambient_credentials(authority)
        };
        let remote = Some("git@ghe.corp.example:owner/repo.git");

        assert!(accepts("api.github.com", None));
        assert!(accepts("github.com", None));
        assert!(accepts("127.0.0.1:8080", None));
        assert!(accepts("ghe.corp.example", remote));
        assert!(accepts(
            "ghe.corp.example",
            Some("https://ghe.corp.example/owner/repo.git")
        ));

        // A redirected --api-url is never trusted with an ambient credential.
        assert!(!accepts("attacker.example", None));
        assert!(!accepts("attacker.example", remote));
        assert!(!accepts("ghe.corp.example", None));
    }

    #[test]
    fn tokens_travel_only_over_https_or_loopback() {
        assert_eq!(
            credentials::secure_authority("https://ghe.corp.example/api/v3").as_deref(),
            Some("ghe.corp.example")
        );
        assert_eq!(
            credentials::secure_authority("http://127.0.0.1:8080").as_deref(),
            Some("127.0.0.1:8080")
        );
        assert_eq!(
            credentials::secure_authority("http://localhost:8080").as_deref(),
            Some("localhost:8080")
        );
        assert_eq!(
            credentials::secure_authority("http://ghe.corp.example"),
            None
        );
    }

    #[test]
    fn explicit_token_to_a_cleartext_host_is_refused() {
        let mut args = connection_args("http://ghe.corp.example");
        args.token = Some("explicit-token".to_owned());

        let error = resolve_token(&args, &args.api_url.clone(), None)
            .expect_err("a cleartext destination must not receive a token");

        assert_eq!(
            error.error_code.as_deref(),
            Some("GITHUB_INSECURE_TOKEN_TARGET")
        );
        assert!(!format!("{error:?}").contains("explicit-token"));
    }

    #[test]
    fn explicit_token_still_reaches_a_caller_chosen_https_host() {
        // The caller supplied both the credential and the destination, so only
        // the cleartext rule applies to it.
        let mut args = connection_args("https://ghe.corp.example/api/v3");
        args.token = Some("explicit-token".to_owned());

        let token = resolve_token(&args, &args.api_url.clone(), None).unwrap();

        assert_eq!(token.as_deref(), Some("explicit-token"));
    }

    fn connection_args(api_url: &str) -> GithubConnectionArgs {
        GithubConnectionArgs {
            repo: None,
            remote: DEFAULT_REMOTE.to_owned(),
            api_url: api_url.to_owned(),
            token: None,
            use_git_credential: false,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            cwd: None,
        }
    }

    #[test]
    fn credential_lookup_is_bound_to_the_api_url_host() {
        assert_eq!(
            credential_authority("https://api.github.com").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            credential_authority("https://ghe.corp.example/api/v3").as_deref(),
            Some("ghe.corp.example")
        );
        for redirected in [
            "https://attacker.example/api/v3",
            "https://api.github.com@attacker.example/api/v3",
        ] {
            assert_ne!(
                credential_authority(redirected).as_deref(),
                Some("github.com"),
                "{redirected}"
            );
        }
        assert_eq!(credential_authority("http://api.github.com"), None);
        assert_eq!(credential_authority("https://"), None);
    }

    #[test]
    fn credential_helper_timeout_kills_the_child() {
        if std::env::var_os("AH_GITHUB_TEST_CREDENTIAL_SLEEP").is_some() {
            thread::sleep(Duration::from_secs(1));
            return;
        }
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "context::tests::credential_helper_timeout_kills_the_child",
            ])
            .env("AH_GITHUB_TEST_CREDENTIAL_SLEEP", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        assert!(credentials::wait_for_credential_child(child, Duration::from_millis(20)).is_none());
    }

    #[test]
    fn parses_common_github_remotes() {
        assert_eq!(
            parse_github_remote_url("https://github.com/Bobsans/AIHelper.git")
                .expect("repo should parse")
                .full_name(),
            "Bobsans/AIHelper"
        );
        assert_eq!(
            parse_github_remote_url("git@github.com:Bobsans/AIHelper.git")
                .expect("repo should parse")
                .full_name(),
            "Bobsans/AIHelper"
        );
        assert_eq!(
            parse_github_remote_url("ssh://git@github.com/Bobsans/AIHelper.git")
                .expect("repo should parse")
                .full_name(),
            "Bobsans/AIHelper"
        );
    }

    #[test]
    fn rejects_non_github_remote() {
        assert!(parse_github_remote_url("https://gitlab.com/Bobsans/AIHelper.git").is_none());
    }
}
