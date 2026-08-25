//! Behavioural tests for the redaction engine.
//!
//! The important property is one-directional: a value that reaches a sink must
//! never contain a credential. The generated cases below assert exactly that
//! across every carrier shape the engine knows about, and the paired
//! "preserved" cases keep the engine from satisfying it by redacting
//! everything.

use ah_redact::{
    REDACTED, curl_contains_auth, is_authorization_header, is_sensitive_cli_flag,
    is_sensitive_name, sanitize_cli_argv, sanitize_string, sanitize_system_context, sanitize_value,
    url_contains_userinfo,
};
use serde_json::{Value, json};

/// Deterministic secrets: a seeded generator keeps failures reproducible while
/// still covering more shapes than a handful of literals would.
fn secrets(count: usize) -> Vec<String> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut generated = Vec::with_capacity(count);
    for index in 0..count {
        // Length 12..=27: long enough that a match cannot be coincidental.
        let length = 12 + index % 16;
        let mut secret = String::with_capacity(length);
        for _ in 0..length {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let pick = (state >> 33) as usize % ALPHABET.len();
            secret.push(ALPHABET[pick] as char);
        }
        generated.push(secret);
    }
    generated
}

const SENSITIVE_NAMES: &[&str] = &[
    "password",
    "token",
    "api_key",
    "apiKey",
    "secret",
    "clientSecret",
    "authorization",
    "bearer",
    "cookie",
    "access_token",
];

#[track_caller]
fn assert_hidden(context: &str, secret: &str, rendered: &str) {
    assert!(
        !rendered.contains(secret),
        "{context}: secret leaked into `{rendered}`"
    );
}

#[test]
fn json_values_under_sensitive_names_never_survive() {
    for secret in secrets(32) {
        for name in SENSITIVE_NAMES {
            let rendered = sanitize_value(json!({ *name: secret.clone() }), false, 0).to_string();
            assert_hidden(&format!("json {name}"), &secret, &rendered);

            let nested = sanitize_value(
                json!({"outer": {"inner": [{ *name: secret.clone() }]}}),
                false,
                0,
            )
            .to_string();
            assert_hidden(&format!("nested json {name}"), &secret, &nested);
        }
    }
}

#[test]
fn embedded_assignments_never_survive() {
    for secret in secrets(32) {
        for name in SENSITIVE_NAMES {
            for candidate in [
                format!("{name}={secret}"),
                format!("{name}: {secret}"),
                format!("request failed: {name}={secret} retrying"),
                format!("{name}=\"{secret}\""),
                format!("{name}='{secret}'"),
            ] {
                let rendered = sanitize_string(&candidate, false);
                assert_hidden(&format!("assignment {candidate}"), &secret, &rendered);
            }
        }
    }
}

#[test]
fn url_credentials_never_survive() {
    for secret in secrets(24) {
        for candidate in [
            format!("https://user:{secret}@example.test/path"),
            format!("https://{secret}@example.test/path"),
            format!("https://example.test/path?api_key={secret}"),
            format!("https://example.test/path?token={secret}&page=2"),
            format!("https://example.test/path?api%5Fkey={secret}#fragment"),
        ] {
            let rendered = sanitize_string(&candidate, false);
            assert_hidden(&format!("url {candidate}"), &secret, &rendered);
        }
    }
}

#[test]
fn curl_credentials_never_survive() {
    for secret in secrets(24) {
        for candidate in [
            format!("curl -u user:{secret} https://example.test"),
            format!("curl --user user:{secret} https://example.test"),
            format!("curl --user=user:{secret} https://example.test"),
            format!("curl -H \"Authorization: Bearer {secret}\" https://example.test"),
            format!("curl --header=\"Authorization: Bearer {secret}\" https://example.test"),
            format!("curl https://user:{secret}@example.test"),
        ] {
            let rendered = sanitize_string(&candidate, false);
            assert_hidden(&format!("curl {candidate}"), &secret, &rendered);
        }
    }
}

#[test]
fn cli_arguments_never_survive() {
    for secret in secrets(24) {
        for name in SENSITIVE_NAMES {
            let separate = sanitize_cli_argv(
                vec![format!("--{name}"), secret.clone(), "--json".to_owned()],
                false,
            )
            .to_string();
            assert_hidden(&format!("argv --{name} <value>"), &secret, &separate);

            let assigned = sanitize_cli_argv(vec![format!("--{name}={secret}")], false).to_string();
            assert_hidden(&format!("argv --{name}=<value>"), &secret, &assigned);
        }
    }
}

#[test]
fn system_context_argv_is_redacted_like_the_command_line() {
    for secret in secrets(8) {
        let context = json!({
            "argv": ["secrets", "add", "--token", secret.clone()],
            "component": "startup",
        });
        let rendered = sanitize_system_context(context, false).to_string();
        assert_hidden("system context argv", &secret, &rendered);
        assert!(
            rendered.contains("startup"),
            "non-sensitive context should survive: {rendered}"
        );
    }
}

#[test]
fn credential_identifiers_stay_redacted_even_in_unredacted_mode() {
    // `AH_LOG_UNREDACTED=1` is a debugging aid, not a bypass for the values the
    // engine considers always-sensitive.
    for secret in secrets(8) {
        let rendered =
            sanitize_cli_argv(vec!["--credential".to_owned(), secret.clone()], true).to_string();
        assert_hidden("unredacted mode credential id", &secret, &rendered);
    }
}

#[test]
fn ordinary_values_are_preserved() {
    let rendered = sanitize_value(
        json!({
            "command": "git.status",
            "duration_ms": 12,
            "branch": "main",
            "url": "https://example.test/path?page=2",
        }),
        false,
        0,
    );

    assert_eq!(rendered["command"], json!("git.status"));
    assert_eq!(rendered["duration_ms"], json!(12));
    assert_eq!(rendered["branch"], json!("main"));
    assert_eq!(
        rendered["url"],
        json!("https://example.test/path?page=2"),
        "a URL without credentials must survive intact"
    );
}

#[test]
fn truncation_marks_bounded_values() {
    let long = "a".repeat(ah_redact::MAX_STRING_BYTES * 2);
    let rendered = sanitize_string(&long, false);
    assert!(rendered.len() <= ah_redact::MAX_STRING_BYTES);
    assert!(rendered.ends_with(ah_redact::TRUNCATED));
}

#[test]
fn redaction_marker_is_used_consistently() {
    let rendered = sanitize_value(json!({"password": "hunter2"}), false, 0);
    assert_eq!(rendered["password"], json!(REDACTED));
}

#[test]
fn sensitive_name_classification() {
    for name in SENSITIVE_NAMES {
        assert!(is_sensitive_name(name), "{name} should be sensitive");
    }
    for name in ["branch", "duration_ms", "command", "page", "monkey"] {
        assert!(!is_sensitive_name(name), "{name} should not be sensitive");
    }

    assert!(is_sensitive_cli_flag("token"));
    assert!(!is_sensitive_cli_flag("json"));
}

#[test]
fn detectors_recognize_plaintext_credentials() {
    assert!(url_contains_userinfo("https://user:pass@example.test"));
    assert!(url_contains_userinfo("https://user@example.test/path"));
    assert!(!url_contains_userinfo("https://example.test/path"));
    assert!(!url_contains_userinfo("example.test/path@fragment"));

    assert!(is_authorization_header("Authorization: Bearer abc"));
    assert!(is_authorization_header("authorization:Bearer abc"));
    assert!(!is_authorization_header("Accept: application/json"));

    assert!(curl_contains_auth("curl -u user:pass https://example.test"));
    assert!(curl_contains_auth(
        "curl --user user:pass https://example.test"
    ));
    assert!(curl_contains_auth(
        "curl -H 'Authorization: Bearer abc' https://example.test"
    ));
    assert!(curl_contains_auth("curl https://user:pass@example.test"));
    assert!(!curl_contains_auth(
        "curl -H 'Accept: application/json' https://example.test"
    ));
}

#[test]
fn detectors_do_not_panic_on_hostile_input() {
    // The detectors gate requests, so a malformed value must be rejected or
    // accepted - never crash the process handling it.
    let hostile = [
        "",
        "://",
        "@",
        "https://",
        "https://@",
        "curl",
        "curl -u",
        "curl --header=",
        "curl 'unterminated",
        "\u{feff}curl -u a:b https://x",
        &"a".repeat(10_000),
    ];
    for value in hostile {
        let _ = url_contains_userinfo(value);
        let _ = is_authorization_header(value);
        let _ = curl_contains_auth(value);
        let _ = sanitize_string(value, false);
        let _ = sanitize_value(Value::String(value.to_owned()), false, 0);
    }
}
