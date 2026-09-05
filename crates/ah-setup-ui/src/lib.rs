//! The browser-facing pages for entering a secret, and the response headers
//! that harden them.
//!
//! This is a web application, not an MCP concern. It lived inside the MCP
//! protocol adapter, which meant its Content-Security-Policy, nonce and
//! `no-store` handling were reviewed as part of protocol changes. Here they are
//! one small crate with their own tests.
//!
//! The transport still owns routing and the local-request policy; this crate
//! owns what a browser is shown and what headers it is shown under.

use std::collections::BTreeMap;

use axum::{
    http::{
        HeaderMap, HeaderValue,
        header::{ACCEPT, CACHE_CONTROL, CONTENT_SECURITY_POLICY, REFERRER_POLICY},
    },
    response::Response,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretSetupRequest {
    Create {
        id: String,
        kind: String,
        label: Option<String>,
        description: Option<String>,
    },
    Edit {
        id: String,
        label: Option<String>,
        description: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretSetupField {
    pub name: &'static str,
    pub label: &'static str,
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretSetupForm {
    pub id: String,
    pub kind: String,
    pub fields: Vec<SecretSetupField>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretSetupMetadata {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct SecretSetupError {
    pub code: &'static str,
    pub message: &'static str,
}

impl SecretSetupError {
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

pub trait SecretSetupService: Send + Sync {
    fn issue(&self, request: SecretSetupRequest) -> Result<String, SecretSetupError>;

    fn form(&self, capability: &str) -> Result<SecretSetupForm, SecretSetupError>;

    fn submit(
        &self,
        capability: &str,
        values: BTreeMap<String, String>,
    ) -> Result<SecretSetupMetadata, SecretSetupError>;
}

pub fn no_store(response: Response) -> Response {
    no_store_with_referrer_policy(response, "no-referrer")
}

/// The setup pages are served with `same-origin` rather than `no-referrer`: under
/// `no-referrer` a browser sends `Origin: null` on the form POST, which the local
/// HTTP policy rejects. The page loads no third-party resources, so `same-origin`
/// keeps the capability out of every referrer that leaves this server.
///
/// The nonce-based policy pins the page to its own inline style and script and
/// blocks every outbound load, so nothing on a secret entry page can reach out.
pub fn no_store_form(mut response: Response, nonce: &str) -> Response {
    let policy = format!(
        "default-src 'none'; style-src 'nonce-{nonce}'; script-src 'nonce-{nonce}'; form-action 'self'; base-uri 'none'"
    );
    if let Ok(value) = HeaderValue::from_str(&policy) {
        response
            .headers_mut()
            .insert(CONTENT_SECURITY_POLICY, value);
    }
    no_store_with_referrer_policy(response, "same-origin")
}

/// Single-use nonce so the inline style and script survive the page's own CSP.
pub fn page_nonce() -> String {
    Uuid::new_v4().simple().to_string()
}

pub fn no_store_with_referrer_policy(mut response: Response, policy: &'static str) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(REFERRER_POLICY, HeaderValue::from_static(policy));
    response
}

const SETUP_PAGE_STYLE: &str = "\
*,::before,::after{box-sizing:border-box}\
:root{color-scheme:light dark;\
--bg:#f4f5f7;--card:#fff;--ink:#1b1f24;--muted:#5b6570;--line:#d8dde3;\
--field:#fff;--accent:#2b6cb0;--accent-ink:#fff;--ok:#1f7a4d;--ok-bg:#e8f5ee}\
@media (prefers-color-scheme:dark){:root{\
--bg:#15181d;--card:#1e232a;--ink:#e8ecf1;--muted:#9aa5b1;--line:#333b45;\
--field:#161a20;--accent:#4a90d9;--accent-ink:#0f1216;--ok:#63d19b;--ok-bg:#163024}}\
body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;\
padding:24px;background:var(--bg);color:var(--ink);\
font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
main{width:100%;max-width:26rem;background:var(--card);border:1px solid var(--line);\
border-radius:12px;padding:28px}\
h1{margin:0 0 4px;font-size:1.25rem;letter-spacing:-.01em}\
.kind{display:inline-block;margin-bottom:20px;padding:2px 8px;border-radius:999px;\
background:var(--bg);border:1px solid var(--line);color:var(--muted);\
font-size:.75rem;font-family:ui-monospace,SFMono-Regular,Consolas,monospace}\
label{display:block;margin-bottom:16px;font-size:.8125rem;font-weight:600;color:var(--muted)}\
input,textarea{display:block;width:100%;margin-top:6px;padding:9px 11px;\
border:1px solid var(--line);border-radius:8px;background:var(--field);color:var(--ink);\
font:inherit}\
textarea{min-height:8rem;resize:vertical;font-family:ui-monospace,SFMono-Regular,Consolas,monospace;\
font-size:.8125rem}\
input:focus,textarea:focus{outline:2px solid var(--accent);outline-offset:1px;border-color:transparent}\
.optional{font-weight:400;text-transform:none}\
button{width:100%;padding:10px 16px;border:0;border-radius:8px;\
background:var(--accent);color:var(--accent-ink);font:inherit;font-weight:600;cursor:pointer}\
button:hover{filter:brightness(1.08)}\
button:focus-visible{outline:2px solid var(--ink);outline-offset:2px}\
.done{display:flex;align-items:center;gap:10px;margin-bottom:16px;padding:10px 12px;\
border-radius:8px;background:var(--ok-bg);color:var(--ok);font-weight:600}\
.done svg{flex:none}\
dl{margin:0 0 20px;display:grid;grid-template-columns:auto 1fr;gap:6px 16px;font-size:.875rem}\
dt{color:var(--muted)}\
dd{margin:0;font-family:ui-monospace,SFMono-Regular,Consolas,monospace;word-break:break-all}\
.hint{margin:14px 0 0;font-size:.8125rem;color:var(--muted);text-align:center}\
[hidden]{display:none}";

pub fn setup_page(nonce: &str, title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"referrer\" content=\"same-origin\"><title>{}</title>\
<style nonce=\"{}\">{SETUP_PAGE_STYLE}</style></head><body><main>{body}</main></body></html>",
        html_escape(title),
        html_escape(nonce),
    )
}

pub fn render_secret_setup_form(form: &SecretSetupForm, nonce: &str) -> String {
    let fields = form
        .fields
        .iter()
        .map(|field| {
            let required = if field.optional { "" } else { " required" };
            let label = if field.optional {
                format!(
                    "{} <span class=\"optional\">(optional)</span>",
                    html_escape(field.label)
                )
            } else {
                html_escape(field.label)
            };
            if field.name == "private_key" {
                format!(
                    "<label>{label}<textarea name=\"{}\" autocomplete=\"off\" spellcheck=\"false\"{required}></textarea></label>",
                    html_escape(field.name),
                )
            } else {
                let (input_type, autocomplete) =
                    if matches!(
                        field.name,
                        "username" | "password" | "passphrase" | "token"
                    ) {
                        ("password", "new-password")
                    } else {
                        ("text", "off")
                    };
                format!(
                    "<label>{label}<input type=\"{input_type}\" name=\"{}\" autocomplete=\"{autocomplete}\"{required}></label>",
                    html_escape(field.name),
                )
            }
        })
        .collect::<String>();
    setup_page(
        nonce,
        "AIHelper secret setup",
        &format!(
            "<h1>Set up {}</h1><span class=\"kind\">{}</span>\
<form method=\"post\">{fields}<button type=\"submit\">Save</button></form>",
            html_escape(&form.id),
            html_escape(&form.kind),
        ),
    )
}

pub fn render_secret_setup_success(metadata: &SecretSetupMetadata, nonce: &str) -> String {
    let description = metadata
        .description
        .as_deref()
        .map_or_else(String::new, |description| {
            format!("<dt>Description</dt><dd>{}</dd>", html_escape(description))
        });
    setup_page(
        nonce,
        "AIHelper secret saved",
        &format!(
            "<p class=\"done\">\
<svg width=\"18\" height=\"18\" viewBox=\"0 0 18 18\" fill=\"none\" stroke=\"currentColor\" \
stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">\
<path d=\"M3.5 9.5l3.5 3.5 7.5-8\"/></svg>Secret saved</p>\
<h1>{}</h1><span class=\"kind\">{}</span>\
<dl><dt>Label</dt><dd>{}</dd>{description}</dl>\
<button type=\"button\" id=\"close\">Close</button>\
<p class=\"hint\" id=\"hint\" hidden>This tab can be closed now.</p>\
<script nonce=\"{}\">\
document.getElementById('close').addEventListener('click',function(){{\
window.close();\
document.getElementById('hint').hidden=false;\
}});\
</script>",
            html_escape(&metadata.id),
            html_escape(&metadata.kind),
            html_escape(&metadata.label),
            html_escape(nonce),
        ),
    )
}

/// Browsers submitting the form get the confirmation page; every other caller
/// keeps the documented redacted-metadata JSON.
pub fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

pub fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::ACCEPT};

    use super::*;

    #[test]
    fn private_key_setup_field_uses_a_multiline_textarea() {
        let html = render_secret_setup_form(&ssh_key_form(), "test-nonce");

        assert!(html.contains("<textarea name=\"private_key\""));
        assert!(!html.contains("type=\"password\" name=\"private_key\""));
        assert!(html.contains("<input type=\"password\" name=\"passphrase\""));
        assert!(html.contains("(optional)"));
    }

    #[test]
    fn postgres_connection_fields_use_text_inputs_and_password_stays_hidden() {
        let html = render_secret_setup_form(
            &SecretSetupForm {
                id: "app-db".to_owned(),
                kind: "postgres".to_owned(),
                fields: vec![
                    SecretSetupField {
                        name: "host",
                        label: "PostgreSQL host",
                        optional: false,
                    },
                    SecretSetupField {
                        name: "user",
                        label: "PostgreSQL user",
                        optional: false,
                    },
                    SecretSetupField {
                        name: "password",
                        label: "PostgreSQL password",
                        optional: false,
                    },
                ],
            },
            "test-nonce",
        );

        assert!(html.contains("<input type=\"text\" name=\"host\""));
        assert!(html.contains("<input type=\"text\" name=\"user\""));
        assert!(html.contains("<input type=\"password\" name=\"password\""));
    }

    #[test]
    fn http_basic_username_remains_hidden() {
        let html = render_secret_setup_form(
            &SecretSetupForm {
                id: "service-api".to_owned(),
                kind: "http-basic".to_owned(),
                fields: vec![
                    SecretSetupField {
                        name: "username",
                        label: "HTTP basic username",
                        optional: false,
                    },
                    SecretSetupField {
                        name: "password",
                        label: "HTTP basic password",
                        optional: false,
                    },
                ],
            },
            "test-nonce",
        );

        assert!(html.contains("<input type=\"password\" name=\"username\""));
        assert!(html.contains("<input type=\"password\" name=\"password\""));
    }

    #[test]
    fn setup_pages_carry_the_nonce_on_every_inline_block() {
        let form = render_secret_setup_form(&ssh_key_form(), "form-nonce");
        let success = render_secret_setup_success(
            &SecretSetupMetadata {
                id: "deployment-key".to_owned(),
                kind: "ssh-key".to_owned(),
                label: "Deployment key".to_owned(),
                description: None,
            },
            "success-nonce",
        );

        assert!(form.contains("<style nonce=\"form-nonce\">"));
        assert!(!form.contains("<script"));
        assert!(success.contains("<style nonce=\"success-nonce\">"));
        assert!(success.contains("<script nonce=\"success-nonce\">"));
    }

    #[test]
    fn success_page_confirms_the_secret_and_offers_a_close_button() {
        let html = render_secret_setup_success(
            &SecretSetupMetadata {
                id: "deployment-key".to_owned(),
                kind: "ssh-key".to_owned(),
                label: "Deployment <key>".to_owned(),
                description: Some("Release runner".to_owned()),
            },
            "test-nonce",
        );

        assert!(html.contains("Secret saved"));
        assert!(html.contains("id=\"close\""));
        assert!(html.contains("window.close()"));
        assert!(html.contains("Release runner"));
        // Metadata is escaped, and the page never offers a way back to the form.
        assert!(html.contains("Deployment &lt;key&gt;"));
        assert!(!html.contains("<form"));
    }

    #[test]
    fn only_browsers_receive_the_confirmation_page() {
        let mut html_headers = HeaderMap::new();
        html_headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/html,application/xhtml+xml"),
        );
        let mut json_headers = HeaderMap::new();
        json_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        assert!(wants_html(&html_headers));
        assert!(!wants_html(&json_headers));
        assert!(!wants_html(&HeaderMap::new()));
    }

    fn ssh_key_form() -> SecretSetupForm {
        SecretSetupForm {
            id: "deployment-key".to_owned(),
            kind: "ssh-key".to_owned(),
            fields: vec![
                SecretSetupField {
                    name: "private_key",
                    label: "SSH private key",
                    optional: false,
                },
                SecretSetupField {
                    name: "passphrase",
                    label: "SSH key passphrase",
                    optional: true,
                },
            ],
        }
    }
}
