//! Which project, which host, which API URLs, and which token.
//!
//! Everything a request needs before it can be built. GitLab differs from
//! GitHub here: the host is read from the git remote unless the caller names
//! the project, so a self-managed instance needs no override - and the token is
//! bound to the one authority it was resolved for, because the REST and GraphQL
//! URLs can be different hosts.

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct ProjectRef {
    pub(crate) value: String,
}

impl ProjectRef {
    pub(crate) fn encoded(&self) -> String {
        urlencoding::encode(&self.value).into_owned()
    }
}

#[derive(Debug)]
pub(crate) struct GitlabContext {
    pub(crate) client: Client,
    pub(crate) host: String,
    pub(crate) api_url: String,
    pub(crate) graphql_url: String,
    pub(crate) token: Option<String>,
    /// The one authority `token` may be sent to; see `authorized_token`.
    pub(crate) token_authority: Option<String>,
    pub(crate) project: ProjectRef,
    pub(crate) remote_url: Option<String>,
}

pub(crate) fn gitlab_context(
    args: &GitlabConnectionArgs,
) -> Result<GitlabContext, InvocationResponse> {
    let (host, project, remote_url) = resolve_host_and_project(args)?;
    let api_url = normalize_api_url(args.api_url.as_deref(), &host)?;
    let graphql_url = normalize_graphql_url(args.graphql_url.as_deref(), &api_url, &host)?;
    let (token, token_authority) =
        resolve_token(args, &api_url, &graphql_url, remote_url.as_deref())?;
    let client = Client::builder()
        .timeout(Duration::from_secs(args.timeout_secs.max(1)))
        .build()
        .map_err(|error| {
            InvocationResponse::error(
                "GITLAB_HTTP_FAILED",
                format!("failed to create HTTP client: {error}"),
            )
        })?;

    Ok(GitlabContext {
        client,
        host,
        api_url,
        graphql_url,
        token,
        token_authority,
        project,
        remote_url,
    })
}

/// Detection is what pins the host: an unset `--host` starts as `gitlab.com`,
/// and a remote pointing at a self-hosted instance names the real one instead of
/// failing and making the caller repeat it on every command.
pub(crate) fn resolve_host_and_project(
    args: &GitlabConnectionArgs,
) -> Result<(String, ProjectRef, Option<String>), InvocationResponse> {
    let host = normalize_host(args.host.as_deref().unwrap_or(DEFAULT_HOST))?;
    let error = match resolve_project(args, &host) {
        Ok((project, remote_url)) => return Ok((host, project, remote_url)),
        Err(error) => error,
    };
    if args.host.is_some() || args.project.is_some() {
        return Err(error);
    }
    let Some(remote_url) = git::remote_url(&args.remote, args.cwd.as_deref()).ok() else {
        return Err(error);
    };
    let Some(host) = remote_host(&remote_url).and_then(|host| normalize_host(&host).ok()) else {
        return Err(error);
    };
    match parse_gitlab_remote_url(&remote_url, &host) {
        Some(project) => Ok((host, project, Some(remote_url))),
        None => Err(error),
    }
}

/// The host a git remote itself names.
pub(crate) fn remote_host(remote: &str) -> Option<String> {
    let trimmed = remote.trim();
    for scheme in ["https://", "http://"] {
        if let Some(authority) = credentials::url_authority(trimmed, scheme) {
            return Some(format!("{scheme}{authority}"));
        }
    }
    // An ssh or scp-like remote carries an ssh port that says nothing about the
    // web endpoint, so only the host survives.
    let authority = credentials::remote_authority(trimmed)?;
    let host = authority.split(':').next().unwrap_or(&authority);
    (!host.is_empty()).then(|| format!("https://{host}"))
}

pub(crate) fn resolve_project(
    args: &GitlabConnectionArgs,
    host: &str,
) -> Result<(ProjectRef, Option<String>), InvocationResponse> {
    if let Some(project) = &args.project {
        return parse_project_ref(project)
            .map(|project| (project, None))
            .ok_or_else(|| invalid_project(project));
    }

    let remote_url = git::remote_url(&args.remote, args.cwd.as_deref())?;
    parse_gitlab_remote_url(&remote_url, host)
        .map(|project| (project, Some(remote_url.clone())))
        .ok_or_else(|| {
            InvocationResponse::error(
                "GITLAB_PROJECT_UNDETECTED",
                format!(
                    "could not detect GitLab project from remote '{}' for host '{}': {}",
                    args.remote, host, remote_url
                ),
            )
        })
}

pub(crate) fn parse_project_ref(value: &str) -> Option<ProjectRef> {
    let normalized = value.trim().trim_matches('/').trim_end_matches(".git");
    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(ProjectRef {
            value: normalized.to_owned(),
        });
    }
    if normalized.split('/').count() < 2 || normalized.split('/').any(str::is_empty) {
        return None;
    }
    Some(ProjectRef {
        value: normalized.to_owned(),
    })
}

pub(crate) fn parse_gitlab_remote_url(remote: &str, host: &str) -> Option<ProjectRef> {
    let host_authority = host_authority(host)?;
    let trimmed = remote.trim();

    if let Some(rest) = trimmed.strip_prefix("git@") {
        let (remote_host, path) = rest.split_once(':')?;
        if remote_host.eq_ignore_ascii_case(&host_authority) {
            return parse_project_ref(path);
        }
    }

    for prefix in ["https://", "http://"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let (remote_host, path) = rest.split_once('/')?;
            if remote_host.eq_ignore_ascii_case(&host_authority) {
                return parse_project_ref(path);
            }
        }
    }

    if let Some(rest) = trimmed.strip_prefix("ssh://git@") {
        let (_, path) = split_authority_and_path(rest)?;
        if authority_matches_host(rest, &host_authority) {
            return parse_project_ref(path);
        }
    }

    None
}

pub(crate) fn split_authority_and_path(value: &str) -> Option<(&str, &str)> {
    let slash = value.find('/')?;
    Some((&value[..slash], &value[slash + 1..]))
}

pub(crate) fn authority_matches_host(value: &str, expected_host: &str) -> bool {
    let Some((authority, _)) = split_authority_and_path(value) else {
        return false;
    };
    let host_without_port = authority.split(':').next().unwrap_or(authority);
    host_without_port.eq_ignore_ascii_case(expected_host)
}

pub(crate) fn invalid_project(value: &str) -> InvocationResponse {
    InvocationResponse::error(
        "INVALID_ARGUMENT",
        format!("--project must use group/project path or numeric id, got '{value}'"),
    )
}

pub(crate) fn normalize_host(value: &str) -> Result<String, InvocationResponse> {
    let normalized = value.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--host must not be empty",
        ));
    }
    if !normalized.starts_with("https://") && !normalized.starts_with("http://") {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--host must start with http:// or https://",
        ));
    }
    Ok(normalized)
}

pub(crate) fn normalize_api_url(
    value: Option<&str>,
    host: &str,
) -> Result<String, InvocationResponse> {
    let raw = value
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{host}/api/v4"));
    let normalized = raw.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--api-url must not be empty",
        ));
    }
    Ok(normalized)
}

pub(crate) fn normalize_graphql_url(
    explicit_graphql_url: Option<&str>,
    api_url: &str,
    host: &str,
) -> Result<String, InvocationResponse> {
    let graphql_url = if let Some(value) = explicit_graphql_url {
        value.to_owned()
    } else if let Some(prefix) = api_url.strip_suffix("/api/v4") {
        format!("{prefix}/api/graphql")
    } else {
        format!("{host}/api/graphql")
    };
    let normalized = graphql_url.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "GraphQL URL must not be empty",
        ));
    }
    Ok(normalized)
}

pub(crate) fn host_authority(host: &str) -> Option<String> {
    let without_scheme = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))?;
    Some(without_scheme.split('/').next()?.to_owned())
}

/// Resolves the token together with the single authority it is allowed to reach.
/// `authorized_token` re-checks that authority at every send, so a `--graphql-url`
/// pointing elsewhere never receives it.
pub(crate) fn resolve_token(
    args: &GitlabConnectionArgs,
    api_url: &str,
    graphql_url: &str,
    remote_url: Option<&str>,
) -> Result<(Option<String>, Option<String>), InvocationResponse> {
    let explicit = args.token.clone().filter(|value| !value.trim().is_empty());
    let policy = token_policy(args, api_url, graphql_url, remote_url);
    credentials::resolve(api_url, explicit, &policy)
        .map(|resolved| (resolved.token, resolved.authority))
        .map_err(|credentials::InsecureTokenTarget| insecure_token_target())
}

/// Where a GitLab token may come from when the caller supplied none.
pub(crate) fn token_policy<'a>(
    args: &GitlabConnectionArgs,
    api_url: &str,
    graphql_url: &str,
    remote_url: Option<&'a str>,
) -> credentials::TokenPolicy<'a> {
    credentials::TokenPolicy {
        home_authorities: &[DEFAULT_HOST_AUTHORITY],
        env_vars: &["GITLAB_TOKEN", "GL_TOKEN"],
        credential_authority: args
            .use_git_credential
            .then(|| credential_authority(api_url, graphql_url))
            .flatten(),
        remote_url,
        helper_timeout: GIT_CREDENTIAL_TIMEOUT,
    }
}

/// Attaches the token only to the authority it was resolved for.
pub(crate) fn authorized_token<'a>(context: &'a GitlabContext, url: &str) -> Option<&'a str> {
    let token = context.token.as_deref()?;
    let authority = context.token_authority.as_deref()?;
    (credentials::secure_authority(url)? == authority).then_some(token)
}

pub(crate) fn insecure_token_target() -> InvocationResponse {
    InvocationResponse::error(
        "GITLAB_INSECURE_TOKEN_TARGET",
        "refusing to send a token to a cleartext --api-url; use https or a loopback host",
    )
}

/// Binds the credential helper lookup to the hosts that will receive the token,
/// so a redirected `--api-url` or `--graphql-url` can never collect credentials.
pub(crate) fn credential_authority(api_url: &str, graphql_url: &str) -> Option<String> {
    let authority = credentials::https_authority(api_url)?;
    (credentials::https_authority(graphql_url)? == authority).then_some(authority)
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    #[test]
    fn ambient_tokens_reach_only_gitlab_loopback_or_the_detected_remote() {
        // Asserted through the policy the plugin actually builds, not a
        // restatement of it.
        let accepts = |authority: &str, remote: Option<&str>| {
            token_policy(
                &connection_args(),
                "https://gitlab.com/api/v4",
                "https://gitlab.com/api/graphql",
                remote,
            )
            .accepts_ambient_credentials(authority)
        };
        let remote = Some("git@gitlab.corp.example:group/project.git");

        assert!(accepts("gitlab.com", None));
        assert!(accepts("127.0.0.1:8080", None));
        assert!(accepts("gitlab.corp.example", remote));

        assert!(!accepts("attacker.example", None));
        assert!(!accepts("attacker.example", remote));
        assert!(!accepts("gitlab.corp.example", None));
    }

    #[test]
    fn tokens_travel_only_over_https_or_loopback() {
        assert_eq!(
            credentials::secure_authority("https://gitlab.corp.example/api/v4").as_deref(),
            Some("gitlab.corp.example")
        );
        assert_eq!(
            credentials::secure_authority("http://127.0.0.1:8080/api/v4").as_deref(),
            Some("127.0.0.1:8080")
        );
        assert_eq!(
            credentials::secure_authority("http://gitlab.corp.example/api/v4"),
            None
        );
    }

    #[test]
    fn the_token_is_withheld_from_a_graphql_url_on_another_host() {
        let context = context_with_token(
            "https://gitlab.com/api/v4",
            "https://attacker.example/api/graphql",
        );

        assert_eq!(
            authorized_token(&context, &context.api_url),
            Some("private-token")
        );
        assert_eq!(authorized_token(&context, &context.graphql_url), None);
    }

    fn connection_args() -> GitlabConnectionArgs {
        GitlabConnectionArgs {
            project: None,
            remote: DEFAULT_REMOTE.to_owned(),
            host: None,
            api_url: None,
            graphql_url: None,
            token: None,
            use_git_credential: false,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            cwd: None,
        }
    }

    fn context_with_token(api_url: &str, graphql_url: &str) -> GitlabContext {
        GitlabContext {
            client: Client::builder().build().unwrap(),
            host: DEFAULT_HOST.to_owned(),
            api_url: api_url.to_owned(),
            graphql_url: graphql_url.to_owned(),
            token: Some("private-token".to_owned()),
            token_authority: credentials::secure_authority(api_url),
            project: ProjectRef {
                value: "group/project".to_owned(),
            },
            remote_url: None,
        }
    }

    #[test]
    fn credential_lookup_requires_one_https_authority_for_both_endpoints() {
        assert_eq!(
            credential_authority(
                "https://gitlab.com/api/v4",
                "https://gitlab.com/api/graphql"
            )
            .as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(
            credential_authority(
                "https://gitlab.corp.example/api/v4",
                "https://gitlab.corp.example/api/graphql"
            )
            .as_deref(),
            Some("gitlab.corp.example")
        );
        // A redirected endpoint must never collect the other endpoint's credential.
        assert_eq!(
            credential_authority(
                "https://gitlab.com/api/v4",
                "https://attacker.example/api/graphql"
            ),
            None
        );
        assert_eq!(
            credential_authority(
                "https://gitlab.com@attacker.example/api/v4",
                "https://gitlab.com/api/graphql"
            ),
            None
        );
        assert_eq!(
            credential_authority("http://gitlab.com/api/v4", "http://gitlab.com/api/graphql"),
            None
        );
    }

    #[test]
    fn credential_helper_timeout_kills_the_child() {
        if std::env::var_os("AH_GITLAB_TEST_CREDENTIAL_SLEEP").is_some() {
            thread::sleep(Duration::from_secs(1));
            return;
        }
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "context::tests::credential_helper_timeout_kills_the_child",
            ])
            .env("AH_GITLAB_TEST_CREDENTIAL_SLEEP", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        assert!(credentials::wait_for_credential_child(child, Duration::from_millis(20)).is_none());
    }

    #[test]
    fn parses_gitlab_project_refs() {
        assert_eq!(
            parse_project_ref("group/subgroup/tool.git")
                .expect("project should parse")
                .value,
            "group/subgroup/tool"
        );
        assert_eq!(
            parse_project_ref("123")
                .expect("project id should parse")
                .value,
            "123"
        );
        assert!(parse_project_ref("single").is_none());
    }

    #[test]
    fn parses_common_gitlab_remotes_for_custom_host() {
        let host = "https://gitlab.example.com";
        assert_eq!(
            parse_gitlab_remote_url("https://gitlab.example.com/group/tool.git", host)
                .expect("project should parse")
                .value,
            "group/tool"
        );
        assert_eq!(
            parse_gitlab_remote_url("git@gitlab.example.com:group/subgroup/tool.git", host)
                .expect("project should parse")
                .value,
            "group/subgroup/tool"
        );
        assert_eq!(
            parse_gitlab_remote_url("ssh://git@gitlab.example.com:2222/group/tool.git", host)
                .expect("project should parse")
                .value,
            "group/tool"
        );
    }

    #[test]
    fn rejects_non_matching_host_remote() {
        assert!(
            parse_gitlab_remote_url(
                "https://gitlab.other.example.com/group/tool.git",
                "https://gitlab.example.com"
            )
            .is_none()
        );
    }

    #[test]
    fn a_remote_names_the_host_when_none_was_given() {
        assert_eq!(
            remote_host("https://gitlab.uco.co.il/fixdigital/lms.git").as_deref(),
            Some("https://gitlab.uco.co.il")
        );
        assert_eq!(
            remote_host("http://gitlab.internal:8080/group/tool.git").as_deref(),
            Some("http://gitlab.internal:8080")
        );
        // An ssh port says nothing about the web endpoint.
        assert_eq!(
            remote_host("ssh://git@gitlab.example.com:2222/group/tool.git").as_deref(),
            Some("https://gitlab.example.com")
        );
        assert_eq!(
            remote_host("git@gitlab.example.com:group/tool.git").as_deref(),
            Some("https://gitlab.example.com")
        );
        assert_eq!(remote_host("   ").as_deref(), None);
    }
}
