//! Property tests for the redaction engine.
//!
//! `redaction.rs` next to this file drives the carriers the engine knows about
//! with deterministic secrets: a token under a sensitive key, userinfo in a
//! URL, a `-u` flag on a command line. Those are the cases somebody thought of.
//!
//! This file is for the ones nobody thought of. `proptest` generates the
//! *structure* - arbitrary nesting, arbitrary key names, arbitrary strings,
//! arrays of objects of arrays - and asserts the properties that have to hold
//! for every shape rather than for a listed one. When a property fails, the
//! generated case is shrunk to the smallest input that still fails and written
//! to `tests/properties.proptest-regressions`, so the failure becomes a
//! permanent test rather than a report nobody can reproduce.
//!
//! The bounds matter as much as the redaction. A sink that receives an
//! attacker-shaped payload has to receive a *bounded* one: the engine truncates
//! long strings, drops entries past a limit and stops at a depth, and a
//! recursive walk that respects none of those is a way to exhaust memory
//! through the log.

use ah_redact::{
    MAX_COLLECTION_ENTRIES, MAX_DEPTH, MAX_STRING_BYTES, REDACTED, TRUNCATED, sanitize_cli_argv,
    sanitize_string, sanitize_value,
};
use proptest::prelude::*;
use serde_json::{Map, Value};

/// The key names the engine treats as sensitive, as a strategy.
///
/// Case is varied deliberately: the classifier is meant to be
/// case-insensitive, and a property over `Token`/`TOKEN`/`token` is the cheap
/// way to say so for every carrier at once.
fn sensitive_key() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("password"),
        Just("Token"),
        Just("API_KEY"),
        Just("apiKey"),
        Just("secret"),
        Just("clientSecret"),
        Just("Authorization"),
        Just("cookie"),
        Just("access_token"),
        Just("refresh_token"),
    ]
    .prop_map(str::to_owned)
}

/// A credential long enough that finding it in the output cannot be a
/// coincidence, and alphanumeric so it survives any escaping unchanged.
fn secret() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{16,40}"
}

/// An arbitrary JSON value, nested deeper than the engine's own depth limit.
///
/// The leaves include the values a sanitiser can trip over - an empty string, a
/// huge number, a string with a NUL and multi-byte characters in it - because
/// the engine slices strings by byte offset.
fn arbitrary_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>()
            .prop_filter("JSON has no NaN or infinity", |value| value.is_finite())
            .prop_map(Value::from),
        ".{0,64}".prop_map(Value::String),
        Just(Value::String(String::new())),
        Just(Value::String("\u{0}中\u{feff}𝄞".to_owned())),
    ];
    // Deeper than MAX_DEPTH on purpose: the property below is that the engine
    // stops, and it cannot be checked with input that never reaches the limit.
    leaf.prop_recursive(12, 256, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::hash_map("[a-zA-Z_]{1,12}", inner, 0..8)
                .prop_map(|entries| { Value::Object(entries.into_iter().collect::<Map<_, _>>()) }),
        ]
    })
}

/// Every string in `value`, at any depth.
fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        Value::Object(entries) => entries.values().for_each(|entry| strings(entry, out)),
        _ => {}
    }
}

/// How deep `value` nests, counting the outermost container as depth 1.
fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(entries) => 1 + entries.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// The largest array or object in `value`.
fn widest(value: &Value) -> usize {
    match value {
        Value::Array(items) => items
            .iter()
            .map(widest)
            .chain(std::iter::once(items.len()))
            .max()
            .unwrap_or(0),
        Value::Object(entries) => entries
            .values()
            .map(widest)
            .chain(std::iter::once(entries.len()))
            .max()
            .unwrap_or(0),
        _ => 0,
    }
}

/// Wrap `inner` in `nesting` layers, alternating object and array, so a secret
/// can be planted at an arbitrary depth without generating the path by hand.
fn bury(inner: Value, nesting: usize) -> Value {
    let mut value = inner;
    for level in 0..nesting {
        value = if level % 2 == 0 {
            Value::Array(vec![value])
        } else {
            let mut entries = Map::new();
            entries.insert(format!("level{level}"), value);
            Value::Object(entries)
        };
    }
    value
}

proptest! {
    /// The one-directional property, over structure rather than over a list of
    /// carriers: a credential under a sensitive key never reaches the output,
    /// however deeply it is buried and whatever surrounds it.
    ///
    /// `nesting` stays under `MAX_DEPTH`, because past the limit the engine
    /// replaces the subtree wholesale - which also hides the secret, but for a
    /// different reason, and a property that both branches satisfy would not
    /// distinguish them.
    #[test]
    fn a_credential_under_a_sensitive_key_never_survives_any_nesting(
        key in sensitive_key(),
        secret in secret(),
        nesting in 0_usize..MAX_DEPTH - 1,
    ) {
        let mut entries = Map::new();
        entries.insert(key, Value::String(secret.clone()));
        let payload = bury(Value::Object(entries), nesting);

        let rendered = sanitize_value(payload, false, 0).to_string();

        prop_assert!(
            !rendered.contains(&secret),
            "the secret survived {nesting} levels of nesting: {rendered}"
        );
    }

    /// A sensitive key next to ordinary data redacts the one value, not the
    /// object: an engine that satisfies the property above by emptying
    /// everything would be useless and would still pass it.
    #[test]
    fn redaction_is_targeted_rather_than_wholesale(
        key in sensitive_key(),
        secret in secret(),
        marker in "[a-z]{8,16}",
    ) {
        let mut entries = Map::new();
        entries.insert(key, Value::String(secret.clone()));
        entries.insert("command".to_owned(), Value::String(marker.clone()));

        let rendered = sanitize_value(Value::Object(entries), false, 0).to_string();

        prop_assert!(!rendered.contains(&secret), "{rendered}");
        prop_assert!(
            rendered.contains(&marker),
            "an ordinary value was redacted too: {rendered}"
        );
        prop_assert!(rendered.contains(REDACTED), "{rendered}");
    }

    /// Whatever shape arrives, what leaves is bounded. A sink that can be made
    /// to receive an unbounded payload is a way to exhaust memory through the
    /// log, so this is a security property rather than a tidiness one.
    #[test]
    fn the_output_is_bounded_whatever_the_input_shape(value in arbitrary_json()) {
        let sanitized = sanitize_value(value, false, 0);

        prop_assert!(
            depth(&sanitized) <= MAX_DEPTH + 1,
            "depth {} exceeds the limit",
            depth(&sanitized)
        );
        prop_assert!(
            widest(&sanitized) <= MAX_COLLECTION_ENTRIES + 1,
            "a collection of {} entries exceeds the limit",
            widest(&sanitized)
        );
        let mut collected = Vec::new();
        strings(&sanitized, &mut collected);
        for text in collected {
            prop_assert!(
                text.len() <= MAX_STRING_BYTES + TRUNCATED.len(),
                "a string of {} bytes exceeds the limit",
                text.len()
            );
        }
    }

    /// `sanitize_string` slices by byte offset, so a truncation that lands
    /// inside a multi-byte character would panic. The generator emits
    /// characters of every UTF-8 width at a length that straddles the limit.
    #[test]
    fn truncation_never_splits_a_character(
        text in prop::collection::vec(
            prop_oneof![Just('a'), Just('é'), Just('中'), Just('𝄞')],
            MAX_STRING_BYTES / 4..MAX_STRING_BYTES * 2 / 3,
        ).prop_map(|chars| chars.into_iter().collect::<String>()),
    ) {
        let bounded = sanitize_string(&text, false);

        // Reaching here at all is the assertion - a bad slice panics - but the
        // bound is worth stating too.
        prop_assert!(bounded.len() <= MAX_STRING_BYTES + TRUNCATED.len());
    }

    /// The command line is the carrier where the secret and its flag are two
    /// separate arguments, so the engine has to remember the previous one.
    /// Ordinary arguments on either side must not move it off the value.
    #[test]
    fn a_secret_after_a_sensitive_flag_never_survives(
        flag in prop_oneof![
            Just("--token"), Just("--password"), Just("--secret"),
            Just("-u"), Just("--api-key"), Just("--authorization"),
        ],
        secret in secret(),
        before in prop::collection::vec("[a-z]{1,8}", 0..4),
        after in prop::collection::vec("[a-z]{1,8}", 0..4),
    ) {
        let mut argv = vec!["ah".to_owned()];
        argv.extend(before);
        argv.push(flag.to_owned());
        argv.push(secret.clone());
        argv.extend(after);

        let rendered = sanitize_cli_argv(argv, false).to_string();

        prop_assert!(!rendered.contains(&secret), "{rendered}");
    }

    /// `--token=value` is the same secret in one argument instead of two.
    #[test]
    fn a_secret_joined_to_its_flag_never_survives(
        flag in prop_oneof![
            Just("--token"), Just("--password"), Just("--secret"), Just("--api-key"),
        ],
        secret in secret(),
    ) {
        let argv = vec!["ah".to_owned(), format!("{flag}={secret}")];

        let rendered = sanitize_cli_argv(argv, false).to_string();

        prop_assert!(!rendered.contains(&secret), "{rendered}");
    }

    /// Nothing the engine is handed may crash the process handling it: it runs
    /// on the path that reports a failure, so a panic here turns a diagnostic
    /// into a second failure with no diagnostic at all.
    #[test]
    fn no_input_shape_panics(value in arbitrary_json()) {
        let _ = sanitize_value(value.clone(), false, 0);
        let _ = sanitize_value(value, true, 0);
    }
}
