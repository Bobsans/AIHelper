//! A blocking JSON API client with uniform error mapping.
//!
//! Three plugins had written the same twenty lines: build the URL, send, map a
//! transport failure, check the status, read and bound the error body, decode.
//! What differs between them is data — the service name that appears in
//! messages, the three error codes, the headers, and how a token is attached —
//! so that is what [`JsonApi`] takes.
//!
//! [`Authorization`] carries the authority a token is bound to, not just the
//! token, because a plugin that resolved a token for one host has to re-check
//! every URL against that host. Leaving the check outside this module would make
//! it the caller's problem again, which is how it gets forgotten.

use ah_plugin_api::InvocationResponse;
use reqwest::{
    Method,
    blocking::{Client, RequestBuilder, Response},
};
use serde::{Serialize, de::DeserializeOwned};

use crate::render::truncate_for_error;

/// The error codes a plugin has frozen for the three ways an API call fails.
#[derive(Debug, Clone, Copy)]
pub struct ApiErrorCodes {
    /// The request never completed.
    pub transport: &'static str,
    /// The service answered, with a failure status.
    pub status: &'static str,
    /// The service answered successfully with a body that would not decode.
    pub decode: &'static str,
}

/// How a token is carried.
#[derive(Debug, Clone, Copy)]
pub enum AuthScheme<'a> {
    /// `Authorization: Bearer <token>`.
    Bearer(&'a str),
    /// A service-specific header, such as GitLab's `PRIVATE-TOKEN`.
    Header { name: &'a str, token: &'a str },
}

/// A token together with the authority it may reach.
#[derive(Debug, Clone, Copy)]
pub struct Authorization<'a> {
    pub scheme: AuthScheme<'a>,
    /// When set, the token is attached only to URLs on this authority — so a
    /// second URL the caller configured, or a redirected one, never receives a
    /// token resolved for somewhere else. `None` attaches it to every request
    /// this client builds, which is correct only when every URL is derived from
    /// `base_url`.
    pub authority: Option<&'a str>,
}

impl Authorization<'_> {
    fn apply(&self, request: RequestBuilder, url: &str) -> RequestBuilder {
        if let Some(authority) = self.authority
            && crate::credentials::secure_authority(url).as_deref() != Some(authority)
        {
            return request;
        }
        match self.scheme {
            AuthScheme::Bearer(token) => request.bearer_auth(token),
            AuthScheme::Header { name, token } => request.header(name, token),
        }
    }
}

/// A JSON API rooted at `base_url`.
pub struct JsonApi<'a> {
    pub client: &'a Client,
    /// Prefixed to every request path; must not end in a separator.
    pub base_url: &'a str,
    /// The service name as it appears in error messages.
    pub service: &'a str,
    pub codes: ApiErrorCodes,
    /// Headers sent with every request.
    pub headers: &'a [(&'a str, &'a str)],
    /// `None` sends no credentials.
    pub authorize: Option<Authorization<'a>>,
    /// How many characters of a failure body are quoted back in the error.
    pub error_body_chars: usize,
}

impl JsonApi<'_> {
    /// Send a request and decode a successful response.
    ///
    /// # Errors
    ///
    /// One of the three [`ApiErrorCodes`], according to how far the call got.
    pub fn json<T, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, InvocationResponse>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        let response = self.send(method, path, body)?;
        response.json::<T>().map_err(|error| {
            InvocationResponse::error(
                self.codes.decode,
                format!(
                    "failed to decode {} response for '{path}': {error}",
                    self.service
                ),
            )
        })
    }

    /// Send a request and return the response only if the status is a success,
    /// for a caller that reads the body itself or discards it.
    ///
    /// # Errors
    ///
    /// [`ApiErrorCodes::transport`] or [`ApiErrorCodes::status`].
    pub fn send<B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<Response, InvocationResponse>
    where
        B: Serialize,
    {
        let url = format!("{}{path}", self.base_url);
        let mut request = self.client.request(method, &url);
        for (name, value) in self.headers {
            request = request.header(*name, *value);
        }
        if let Some(authorize) = &self.authorize {
            request = authorize.apply(request, &url);
        }
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().map_err(|error| {
            InvocationResponse::error(
                self.codes.transport,
                format!("request to '{url}' failed: {error}"),
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .unwrap_or_else(|_| "<failed to read response body>".to_owned());
            return Err(InvocationResponse::error(
                self.codes.status,
                format!(
                    "{} returned HTTP {status} for '{url}': {}",
                    self.service,
                    truncate_for_error(&body, self.error_body_chars)
                ),
            ));
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_token_reaches_only_its_own_authority() {
        let bound = Authorization {
            scheme: AuthScheme::Header {
                name: "PRIVATE-TOKEN",
                token: "secret",
            },
            authority: Some("gitlab.com"),
        };
        let client = Client::builder().build().expect("client should build");
        let sent = |url: &str| {
            bound
                .apply(client.request(Method::GET, url), url)
                .build()
                .expect("request should build")
                .headers()
                .contains_key("PRIVATE-TOKEN")
        };

        assert!(sent("https://gitlab.com/api/v4/projects"));
        assert!(!sent("https://attacker.example/api/graphql"));
        // Userinfo does not make an authority, and cleartext off-box is not a
        // token target at all.
        assert!(!sent("https://gitlab.com@attacker.example/api/v4"));
        assert!(!sent("http://gitlab.com/api/v4"));
    }

    #[test]
    fn an_unbound_token_reaches_every_url_the_client_builds() {
        let unbound = Authorization {
            scheme: AuthScheme::Bearer("secret"),
            authority: None,
        };
        let client = Client::builder().build().expect("client should build");
        let url = "https://ghe.corp.example/api/v3/repos";

        assert!(
            unbound
                .apply(client.request(Method::GET, url), url)
                .build()
                .expect("request should build")
                .headers()
                .contains_key("authorization")
        );
    }
}
