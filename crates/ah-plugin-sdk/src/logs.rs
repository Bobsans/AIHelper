//! Reading a forge's build log without trusting how big it is.
//!
//! Both SCM plugins do the same thing to a log stream: read it a line at a
//! time against a byte budget, strip the terminal control sequences a runner
//! left in it, keep the lines the request asked for, and stop at the line
//! limit while saying that it stopped. Only the surrounding shapes differ -
//! GitHub unzips an archive and spends one budget across its entries, GitLab
//! reads a single trace - so what is shared is this loop and nothing else.
//!
//! The error cases are returned rather than rendered: each plugin owns its own
//! diagnostic codes and wording, and this module has no opinion about either.

use std::io::{BufRead, Read};

/// Which lines of a log the request asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct LineFilter<'a> {
    /// Keep only lines containing this, case-insensitively.
    pub grep: Option<&'a str>,
    /// Keep only lines that look like warnings. Takes precedence over `grep`,
    /// because the two commands that use it never pass both.
    pub warnings_only: bool,
    /// Stop after this many kept lines, reporting [`Scan::Truncated`].
    pub limit: Option<usize>,
}

/// How many more bytes one request may read, across however many streams it
/// takes to answer it.
#[derive(Debug, Clone, Copy)]
pub struct ByteBudget {
    remaining: usize,
}

impl ByteBudget {
    #[must_use]
    pub const fn new(bytes: usize) -> Self {
        Self { remaining: bytes }
    }

    /// Whether a response that declares this length can be read at all.
    ///
    /// A declared length over budget is refused before a single byte is read,
    /// which is the cheap half of the check; the loop enforces the same limit
    /// again for a response that declares nothing.
    #[must_use]
    pub const fn admits(&self, declared_length: Option<u64>) -> bool {
        match declared_length {
            Some(length) => length <= self.remaining as u64,
            None => true,
        }
    }

    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.remaining
    }
}

/// Why a scan stopped early.
#[derive(Debug)]
pub enum ScanError {
    /// The stream did not fit in the budget. The caller reports this as its own
    /// "too large" diagnostic, naming the flag the budget came from.
    BudgetExceeded,
    /// The stream could not be read.
    Read(std::io::Error),
}

/// Whether a scan saw the whole stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    Complete,
    /// The line limit was reached, so lines were left unread.
    Truncated,
}

impl Scan {
    #[must_use]
    pub const fn truncated(self) -> bool {
        matches!(self, Self::Truncated)
    }
}

/// Read `reader` line by line, keeping what `filter` selects.
///
/// `line` is handed the 1-based line number within this stream and the
/// ANSI-stripped text, and returns whatever the caller collects. Lines that are
/// not valid UTF-8 are skipped rather than failing the scan: one unreadable
/// line in a build log is not a reason to answer nothing.
///
/// # Errors
///
/// [`ScanError::BudgetExceeded`] when the stream is larger than `budget`
/// allows, and [`ScanError::Read`] when the stream itself fails.
pub fn scan_lines<R: BufRead, T>(
    reader: &mut R,
    budget: &mut ByteBudget,
    filter: &LineFilter<'_>,
    collected: &mut Vec<T>,
    mut line: impl FnMut(usize, String) -> T,
) -> Result<Scan, ScanError> {
    let limit = filter.limit.unwrap_or(usize::MAX);
    let needle = filter.grep.map(str::to_lowercase);
    let mut buffer = Vec::new();
    let mut number = 0usize;
    loop {
        buffer.clear();
        // One byte over the remaining budget is enough to prove the stream does
        // not fit, and stops this from reading a stream of any size.
        // `by_ref` so each iteration re-borrows the same stream rather than
        // consuming it.
        let read = reader
            .by_ref()
            .take(budget.remaining.saturating_add(1) as u64)
            .read_until(b'\n', &mut buffer)
            .map_err(ScanError::Read)?;
        if read == 0 {
            return Ok(Scan::Complete);
        }
        if read > budget.remaining {
            return Err(ScanError::BudgetExceeded);
        }
        budget.remaining -= read;

        number += 1;
        while buffer
            .last()
            .is_some_and(|byte| matches!(*byte, b'\n' | b'\r'))
        {
            buffer.pop();
        }
        let Ok(text) = std::str::from_utf8(&buffer) else {
            continue;
        };
        let text = crate::render::strip_ansi_sequences(text);
        let selected = if filter.warnings_only {
            is_warning_like(&text)
        } else if let Some(needle) = &needle {
            text.to_lowercase().contains(needle)
        } else {
            true
        };
        if !selected {
            continue;
        }
        if collected.len() == limit {
            return Ok(Scan::Truncated);
        }
        collected.push(line(number, text));
    }
}

/// Whether a log line is one the `warnings` commands report.
///
/// A heuristic over text a build tool wrote, so it is deliberately broad: the
/// commands using it are for reading, and a missed warning is worse than an
/// extra line.
#[must_use]
pub fn is_warning_like(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("warning")
        || lower.contains("deprecated")
        || lower.contains("deprecation")
        || lower.contains("will be removed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str, budget: usize, filter: LineFilter<'_>) -> (Vec<String>, Scan) {
        let mut budget = ByteBudget::new(budget);
        let mut collected = Vec::new();
        let scan = scan_lines(
            &mut text.as_bytes(),
            &mut budget,
            &filter,
            &mut collected,
            |number, text| format!("{number}:{text}"),
        )
        .expect("the stream fits");
        (collected, scan)
    }

    #[test]
    fn every_line_is_numbered_from_one_and_stripped_of_its_terminator() {
        let (lines, scan) = scan("first\r\nsecond\nthird", 1024, LineFilter::default());
        assert_eq!(lines, ["1:first", "2:second", "3:third"]);
        assert_eq!(scan, Scan::Complete);
    }

    #[test]
    fn terminal_control_sequences_are_stripped_before_matching() {
        let (lines, _) = scan(
            "\u{1b}[31mWARNING: red\u{1b}[0m\nplain\n",
            1024,
            LineFilter {
                warnings_only: true,
                ..LineFilter::default()
            },
        );
        assert_eq!(lines, ["1:WARNING: red"]);
    }

    #[test]
    fn grep_is_case_insensitive_and_warnings_only_wins() {
        let text = "Deprecated call\nnothing here\nWARN\n";
        let (lines, _) = scan(
            text,
            1024,
            LineFilter {
                grep: Some("NOTHING"),
                ..LineFilter::default()
            },
        );
        assert_eq!(lines, ["2:nothing here"]);

        let (lines, _) = scan(
            text,
            1024,
            LineFilter {
                grep: Some("nothing"),
                warnings_only: true,
                ..LineFilter::default()
            },
        );
        assert_eq!(lines, ["1:Deprecated call"]);
    }

    /// The limit counts kept lines, and reaching it is reported rather than
    /// looking like the end of the stream.
    #[test]
    fn the_line_limit_is_reported_as_truncation() {
        let (lines, scan) = scan(
            "a\nb\nc\n",
            1024,
            LineFilter {
                limit: Some(2),
                ..LineFilter::default()
            },
        );
        assert_eq!(lines, ["1:a", "2:b"]);
        assert_eq!(scan, Scan::Truncated);
        assert!(scan.truncated());
    }

    #[test]
    fn a_stream_over_budget_is_refused_rather_than_truncated() {
        let mut budget = ByteBudget::new(4);
        let mut collected: Vec<String> = Vec::new();
        let error = scan_lines(
            &mut "abcdefgh\n".as_bytes(),
            &mut budget,
            &LineFilter::default(),
            &mut collected,
            |_, text| text,
        )
        .expect_err("the stream does not fit");
        assert!(matches!(error, ScanError::BudgetExceeded));
    }

    /// One budget spans several streams, which is how the archive case spends
    /// a single `--max-expanded-bytes` across every entry.
    #[test]
    fn one_budget_is_shared_across_streams() {
        let mut budget = ByteBudget::new(12);
        let mut collected = Vec::new();
        for stream in ["aaaaa\n", "bbbbb\n"] {
            scan_lines(
                &mut stream.as_bytes(),
                &mut budget,
                &LineFilter::default(),
                &mut collected,
                |_, text| text,
            )
            .expect("both fit");
        }
        assert_eq!(collected, ["aaaaa", "bbbbb"]);
        assert_eq!(budget.remaining(), 0);

        let error = scan_lines(
            &mut "c\n".as_bytes(),
            &mut budget,
            &LineFilter::default(),
            &mut collected,
            |_, text| text,
        )
        .expect_err("the budget is spent");
        assert!(matches!(error, ScanError::BudgetExceeded));
    }

    #[test]
    fn a_declared_length_over_budget_is_refused_before_reading() {
        let budget = ByteBudget::new(10);
        assert!(budget.admits(None));
        assert!(budget.admits(Some(10)));
        assert!(!budget.admits(Some(11)));
    }

    #[test]
    fn a_line_that_is_not_utf8_is_skipped_rather_than_failing_the_scan() {
        let mut budget = ByteBudget::new(1024);
        let mut collected = Vec::new();
        let scan = scan_lines(
            &mut [b'o', b'k', b'\n', 0xff, b'\n', b'a', b'\n'].as_slice(),
            &mut budget,
            &LineFilter::default(),
            &mut collected,
            |number, text| format!("{number}:{text}"),
        )
        .expect("the stream fits");
        assert_eq!(collected, ["1:ok", "3:a"]);
        assert_eq!(scan, Scan::Complete);
    }

    #[test]
    fn warning_like_lines_are_the_ones_the_commands_report() {
        for line in [
            "WARNING: x",
            "note: `foo` is deprecated",
            "Deprecation notice",
            "this will be removed in 2.0",
        ] {
            assert!(is_warning_like(line), "{line}");
        }
        assert!(!is_warning_like("error: build failed"));
    }
}
