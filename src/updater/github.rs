use std::io::{Read, Write};
use std::time::Duration;

use ah_updater_core::{
    DiscoveredReleaseV1, GitHubReleaseDtoV1, ReleaseAssetV1, UpdaterError, UpdaterErrorCode,
    WINDOWS_X64_TARGET, select_highest_stable_release,
};
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, CONTENT_LENGTH, LOCATION, USER_AGENT};
use reqwest::{StatusCode, Url};

use super::check::ReleaseCheckSource;

const GITHUB_RELEASES_API_ROOT: &str = "https://api.github.com/repos/Bobsans/AIHelper";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const DEFAULT_PAGE_SIZE: usize = 100;
const DEFAULT_MAX_PAGES: usize = 5;
const DEFAULT_MAX_RELEASES: usize = 500;
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ASSET_REDIRECTS: usize = 3;

#[derive(Debug, Clone, Copy)]
struct DiscoveryLimits {
    page_size: usize,
    max_pages: usize,
    max_releases: usize,
    max_response_bytes: u64,
    timeout: Duration,
}

impl DiscoveryLimits {
    const fn production() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            max_pages: DEFAULT_MAX_PAGES,
            max_releases: DEFAULT_MAX_RELEASES,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GitHubReleaseClient {
    client: Client,
    api_root: String,
    limits: DiscoveryLimits,
}

impl GitHubReleaseClient {
    pub(crate) fn new() -> Result<Self, UpdaterError> {
        Self::with_config(
            GITHUB_RELEASES_API_ROOT.to_owned(),
            DiscoveryLimits::production(),
        )
    }

    fn with_config(api_root: String, limits: DiscoveryLimits) -> Result<Self, UpdaterError> {
        let client = Client::builder()
            .connect_timeout(limits.timeout.min(Duration::from_secs(5)))
            .timeout(limits.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| network("failed to initialize GitHub Releases client"))?;
        Ok(Self {
            client,
            api_root,
            limits,
        })
    }

    pub(crate) fn discover(&self) -> Result<DiscoveredReleaseV1, UpdaterError> {
        let releases = self.list_releases()?;
        select_highest_stable_release(&releases, WINDOWS_X64_TARGET)
    }

    pub(crate) fn download_asset(&self, asset: &ReleaseAssetV1) -> Result<Vec<u8>, UpdaterError> {
        let mut bytes = Vec::with_capacity(usize::try_from(asset.size).unwrap_or(0));
        self.download_asset_to(asset, &mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn download_asset_to(
        &self,
        asset: &ReleaseAssetV1,
        output: &mut dyn Write,
    ) -> Result<(), UpdaterError> {
        let mut url = Url::parse(&asset.api_url)
            .map_err(|_| network("GitHub release asset URL is invalid"))?;
        for redirect_count in 0..=MAX_ASSET_REDIRECTS {
            let response = self
                .client
                .get(url.clone())
                .header(ACCEPT, "application/octet-stream")
                .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
                .header(
                    USER_AGENT,
                    format!("AIHelper/{} updater", env!("CARGO_PKG_VERSION")),
                )
                .send()
                .map_err(map_request_error)?;
            if is_redirect(response.status()) {
                if redirect_count == MAX_ASSET_REDIRECTS {
                    return Err(network("GitHub release asset exceeded redirect bounds"));
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| network("GitHub release asset redirect is missing Location"))?;
                let next = url
                    .join(location)
                    .map_err(|_| network("GitHub release asset redirect URL is invalid"))?;
                validate_asset_redirect(&next)?;
                url = next;
                continue;
            }
            write_exact_response(response, asset.size, output)?;
            return Ok(());
        }
        Err(network("GitHub release asset exceeded redirect bounds"))
    }

    fn list_releases(&self) -> Result<Vec<GitHubReleaseDtoV1>, UpdaterError> {
        let mut releases = Vec::new();
        for page_number in 1..=self.limits.max_pages {
            let url = format!(
                "{}/releases?per_page={}&page={page_number}",
                self.api_root, self.limits.page_size
            );
            let response = self
                .client
                .get(url)
                .header(ACCEPT, GITHUB_ACCEPT)
                .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
                .header(
                    USER_AGENT,
                    format!("AIHelper/{} updater", env!("CARGO_PKG_VERSION")),
                )
                .send()
                .map_err(map_request_error)?;
            let bytes = read_bounded_response(response, self.limits.max_response_bytes)?;
            let page = serde_json::from_slice::<Vec<GitHubReleaseDtoV1>>(&bytes).map_err(|_| {
                release_contract("GitHub Releases response is not valid release JSON")
            })?;
            if page.len() > self.limits.page_size {
                return Err(release_contract(
                    "GitHub Releases response exceeded the requested page size",
                ));
            }
            if releases.len() + page.len() > self.limits.max_releases {
                return Err(release_contract(
                    "GitHub release listing exceeded the release-count bound",
                ));
            }
            let complete = page.len() < self.limits.page_size;
            releases.extend(page);
            if complete {
                return Ok(releases);
            }
        }
        Err(release_contract(
            "GitHub release listing exceeded pagination bounds before completeness was proven",
        ))
    }
}

fn write_exact_response(
    mut response: Response,
    expected_bytes: u64,
    output: &mut dyn Write,
) -> Result<(), UpdaterError> {
    if response.status() != StatusCode::OK {
        return Err(network_with_status(response.status().as_u16()));
    }
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length != expected_bytes)
    {
        return Err(network(
            "GitHub release asset size differs from its declaration",
        ));
    }

    let mut remaining = expected_bytes;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded download chunk fits usize");
        let read = response
            .read(&mut buffer[..requested])
            .map_err(|_| network("GitHub release asset body was incomplete"))?;
        if read == 0 {
            return Err(network(
                "GitHub release asset size differs from its declaration",
            ));
        }
        output
            .write_all(&buffer[..read])
            .map_err(|_| candidate("failed to write downloaded release asset"))?;
        remaining -= read as u64;
    }
    let mut extra = [0_u8; 1];
    if response
        .read(&mut extra)
        .map_err(|_| network("GitHub release asset body was incomplete"))?
        != 0
    {
        return Err(network(
            "GitHub release asset size differs from its declaration",
        ));
    }
    Ok(())
}

impl ReleaseCheckSource for GitHubReleaseClient {
    fn discover(&self) -> Result<DiscoveredReleaseV1, UpdaterError> {
        self.discover()
    }

    fn download(&self, asset: &ReleaseAssetV1) -> Result<Vec<u8>, UpdaterError> {
        self.download_asset(asset)
    }
}

fn read_bounded_response(
    mut response: Response,
    maximum_bytes: u64,
) -> Result<Vec<u8>, UpdaterError> {
    if response.status() != StatusCode::OK {
        return Err(network_with_status(response.status().as_u16()));
    }
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(network("GitHub Releases response exceeded the byte bound"));
    }

    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(maximum_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| network("GitHub Releases response body was incomplete"))?;
    if bytes.len() as u64 > maximum_bytes {
        return Err(network("GitHub Releases response exceeded the byte bound"));
    }
    Ok(bytes)
}

fn map_request_error(error: reqwest::Error) -> UpdaterError {
    if error.is_timeout() {
        network("GitHub Releases request timed out")
    } else {
        network("GitHub Releases request failed")
    }
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn validate_asset_redirect(url: &Url) -> Result<(), UpdaterError> {
    let allowed_host = matches!(
        url.host_str(),
        Some(
            "github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com"
        )
    );
    if url.scheme() != "https"
        || !allowed_host
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(network(
            "GitHub release asset redirect must use an approved HTTPS host",
        ));
    }
    Ok(())
}

fn network(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Network, detail)
}

fn network_with_status(status: u16) -> UpdaterError {
    UpdaterError::new(
        UpdaterErrorCode::Network,
        format!("GitHub Releases request returned HTTP status {status}"),
    )
}

fn candidate(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Candidate, detail)
}

fn release_contract(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::ReleaseContract, detail)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use ah_updater_core::DETACHED_SIGNATURE_BYTES;
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn discovers_paginated_release_with_required_headers() {
        let server = MockServer::spawn(vec![
            MockResponse::json(json!([
                release_json(1, "v1.1.0", false, false),
                release_json(9, "v9.0.0", true, false)
            ])),
            MockResponse::json(json!([release_json(2, "v1.2.0", false, false)])),
        ]);
        let client = test_client(&server, limits(2, 3, 5, 64 * 1024, 1_000));

        let selected = client.discover().unwrap();
        let requests = server.finish();

        assert_eq!(selected.tag, "v1.2.0");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("GET /releases?per_page=2&page=1 HTTP/1.1"));
        let headers = requests[0].to_ascii_lowercase();
        assert!(headers.contains("accept: application/vnd.github+json"));
        assert!(headers.contains("x-github-api-version: 2022-11-28"));
        assert!(headers.contains("user-agent: aihelper/1.1.0 updater"));
    }

    #[test]
    fn fails_closed_when_pagination_bound_cannot_prove_completeness() {
        let server = MockServer::spawn(vec![MockResponse::json(json!([release_json(
            1, "v1.1.0", false, false
        )]))]);
        let client = test_client(&server, limits(1, 1, 1, 64 * 1024, 1_000));

        let error = client.discover().unwrap_err();
        server.finish();

        assert_eq!(error.code(), UpdaterErrorCode::ReleaseContract);
        assert!(error.detail().contains("pagination bounds"));
    }

    #[test]
    fn rejects_redirects_and_rate_limit_statuses() {
        for status in ["302 Found", "403 Forbidden", "429 Too Many Requests"] {
            let server = MockServer::spawn(vec![MockResponse::status(status)]);
            let client = test_client(&server, limits(1, 1, 1, 64 * 1024, 1_000));

            let error = client.discover().unwrap_err();
            server.finish();

            assert_eq!(error.code(), UpdaterErrorCode::Network);
            assert!(error.detail().contains("HTTP status"));
        }
    }

    #[test]
    fn rejects_oversized_and_partial_response_bodies() {
        let oversized = MockServer::spawn(vec![
            MockResponse::json(json!([])).with_declared_length(65 * 1024),
        ]);
        let client = test_client(&oversized, limits(1, 1, 1, 64 * 1024, 1_000));
        let error = client.discover().unwrap_err();
        oversized.finish();
        assert_eq!(error.code(), UpdaterErrorCode::Network);
        assert!(error.detail().contains("byte bound"));

        let partial = MockServer::spawn(vec![
            MockResponse::json(json!([])).declare_additional_bytes(10),
        ]);
        let client = test_client(&partial, limits(1, 1, 1, 64 * 1024, 1_000));
        let error = client.discover().unwrap_err();
        partial.finish();
        assert_eq!(error.code(), UpdaterErrorCode::Network);
        assert!(error.detail().contains("incomplete"));
    }

    #[test]
    fn reports_request_timeout_without_response_content() {
        let server = MockServer::spawn(vec![
            MockResponse::json(json!([])).with_delay(Duration::from_millis(150)),
        ]);
        let client = test_client(&server, limits(1, 1, 1, 64 * 1024, 30));

        let error = client.discover().unwrap_err();
        server.finish();

        assert_eq!(error.code(), UpdaterErrorCode::Network);
        assert_eq!(error.detail(), "GitHub Releases request timed out");
    }

    #[test]
    fn downloads_exact_asset_and_rejects_insecure_redirect() {
        let server = MockServer::spawn(vec![MockResponse::bytes(vec![b'x'; 86])]);
        let client = test_client(&server, limits(1, 1, 1, 64 * 1024, 1_000));
        let asset = test_asset(&server, 86);
        assert_eq!(client.download_asset(&asset).unwrap(), vec![b'x'; 86]);
        server.finish();

        for actual_size in [85, 87] {
            let bounded = MockServer::spawn(vec![MockResponse::bytes(vec![b'x'; actual_size])]);
            let client = test_client(&bounded, limits(1, 1, 1, 64 * 1024, 1_000));
            let error = client
                .download_asset(&test_asset(&bounded, 86))
                .unwrap_err();
            bounded.finish();
            assert_eq!(error.code(), UpdaterErrorCode::Network);
            assert!(error.detail().contains("size differs"));
        }

        let redirect_server =
            MockServer::spawn(vec![MockResponse::redirect("http://example.invalid/asset")]);
        let client = test_client(&redirect_server, limits(1, 1, 1, 64 * 1024, 1_000));
        let error = client
            .download_asset(&test_asset(&redirect_server, 86))
            .unwrap_err();
        redirect_server.finish();
        assert_eq!(error.code(), UpdaterErrorCode::Network);
        assert!(error.detail().contains("approved HTTPS host"));
    }

    fn test_client(server: &MockServer, limits: DiscoveryLimits) -> GitHubReleaseClient {
        GitHubReleaseClient::with_config(server.url.clone(), limits).unwrap()
    }

    const fn limits(
        page_size: usize,
        max_pages: usize,
        max_releases: usize,
        max_response_bytes: u64,
        timeout_ms: u64,
    ) -> DiscoveryLimits {
        DiscoveryLimits {
            page_size,
            max_pages,
            max_releases,
            max_response_bytes,
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    fn release_json(id: u64, tag: &str, draft: bool, prerelease: bool) -> Value {
        let names = [
            WINDOWS_X64_TARGET.archive_name,
            WINDOWS_X64_TARGET.manifest_name,
            WINDOWS_X64_TARGET.signature_name,
        ];
        let assets = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                let asset_id = id * 10 + index as u64 + 1;
                let size = if name == WINDOWS_X64_TARGET.signature_name {
                    DETACHED_SIGNATURE_BYTES as u64
                } else {
                    100
                };
                json!({
                    "id": asset_id,
                    "name": name,
                    "state": "uploaded",
                    "size": size,
                    "url": format!("https://api.github.com/repos/Bobsans/AIHelper/releases/assets/{asset_id}"),
                    "browser_download_url": format!("https://github.com/Bobsans/AIHelper/releases/download/{tag}/{name}")
                })
            })
            .collect::<Vec<_>>();
        json!({
            "id": id,
            "tag_name": tag,
            "draft": draft,
            "prerelease": prerelease,
            "assets": assets
        })
    }

    fn test_asset(server: &MockServer, size: u64) -> ReleaseAssetV1 {
        ReleaseAssetV1 {
            id: 1,
            name: "asset".to_owned(),
            size,
            api_url: format!("{}/asset", server.url),
            browser_download_url: "https://github.com/asset".to_owned(),
        }
    }

    struct MockServer {
        url: String,
        handle: thread::JoinHandle<Vec<String>>,
    }

    impl MockServer {
        fn spawn(responses: Vec<MockResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let handle = thread::spawn(move || {
                let mut requests = Vec::new();
                for response in responses {
                    let (mut stream, _) = listener.accept().unwrap();
                    requests.push(read_request(&mut stream));
                    if !response.delay.is_zero() {
                        thread::sleep(response.delay);
                    }
                    let declared_length = response.declared_length.unwrap_or(response.body.len());
                    let headers = format!(
                        "HTTP/1.1 {}\r\nContent-Length: {declared_length}\r\nConnection: close\r\n{}\r\n",
                        response.status, response.headers
                    );
                    let _ = stream.write_all(headers.as_bytes());
                    let _ = stream.write_all(&response.body);
                }
                requests
            });
            Self {
                url: format!("http://{address}"),
                handle,
            }
        }

        fn finish(self) -> Vec<String> {
            self.handle.join().unwrap()
        }
    }

    struct MockResponse {
        status: &'static str,
        headers: String,
        body: Vec<u8>,
        declared_length: Option<usize>,
        delay: Duration,
    }

    impl MockResponse {
        fn json(value: Value) -> Self {
            Self {
                status: "200 OK",
                headers: "Content-Type: application/json\r\n".to_owned(),
                body: serde_json::to_vec(&value).unwrap(),
                declared_length: None,
                delay: Duration::ZERO,
            }
        }

        fn status(status: &'static str) -> Self {
            Self {
                status,
                headers: String::new(),
                body: Vec::new(),
                declared_length: None,
                delay: Duration::ZERO,
            }
        }

        fn bytes(body: Vec<u8>) -> Self {
            Self {
                status: "200 OK",
                headers: String::new(),
                body,
                declared_length: None,
                delay: Duration::ZERO,
            }
        }

        fn redirect(location: &str) -> Self {
            Self {
                status: "302 Found",
                headers: format!("Location: {location}\r\n"),
                body: Vec::new(),
                declared_length: None,
                delay: Duration::ZERO,
            }
        }

        fn with_declared_length(mut self, declared_length: usize) -> Self {
            self.declared_length = Some(declared_length);
            self
        }

        fn declare_additional_bytes(mut self, additional: usize) -> Self {
            self.declared_length = Some(self.body.len() + additional);
            self
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }
    }

    fn read_request(stream: &mut impl Read) -> String {
        let mut bytes = Vec::new();
        let mut byte = [0_u8; 1];
        while bytes.len() < 16 * 1024 && stream.read(&mut byte).unwrap_or(0) == 1 {
            bytes.push(byte[0]);
            if bytes.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }
}
