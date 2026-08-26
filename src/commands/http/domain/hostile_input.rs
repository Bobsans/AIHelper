//! Deterministic generated input for the parsers that read untrusted strings.
//!
//! `cargo-fuzz` is the right long-term tool for these two parsers, but it needs
//! a nightly toolchain and a CI lane of its own, and the roadmap defers it for
//! that reason. This is the interim: a fixed-seed generator over an alphabet
//! chosen to hit the delimiters each parser slices on, run in the ordinary test
//! suite on stable.
//!
//! Fixed seeds matter. A failure has to reproduce for whoever reads the report,
//! and a test that finds a different bug on every run is a test nobody can act
//! on.

/// xorshift64*, so the corpus is identical on every machine and every run.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        // Zero is the one state xorshift cannot leave.
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % bound as u64).unwrap_or(0)
    }

    /// A string of up to `max_len` characters drawn from `alphabet`.
    pub(super) fn string(&mut self, alphabet: &[char], max_len: usize) -> String {
        let len = self.below(max_len + 1);
        (0..len)
            .map(|_| alphabet[self.below(alphabet.len())])
            .collect()
    }
}

/// Every character the JSON path parser gives meaning to, plus the ones that
/// break its assumptions: multi-byte characters next to the byte offsets it
/// slices on, and digits long enough to overflow a `usize`.
pub(super) const PATH_ALPHABET: &[char] = &[
    '.', '[', ']', '"', '\'', '\\', '/', '*', '$', '@', '-', ' ', '\t', '\n', '\0', '0', '1', '9',
    'a', 'Z', '_', 'é', '中', '𝄞',
];

/// The shell-ish surface of a copied `curl` command line: quoting, escapes,
/// continuations, and flag punctuation.
pub(super) const CURL_ALPHABET: &[char] = &[
    ' ', '\t', '\n', '\r', '\\', '"', '\'', '-', '=', ':', '/', '?', '&', '%', '@', '{', '}', '$',
    ';', '|', '0', '9', 'a', 'X', 'H', 'd', 'é', '中',
];
