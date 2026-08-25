use std::{
    fmt::Display,
    io::{self, Write},
    sync::{Arc, Mutex},
};

pub use ah_plugin_api::{TextFormatter, TextStyle};
use serde::Serialize;

use crate::{cli::GlobalOptions, error::AppError};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum OutputMode {
    Text,
    Json,
}

pub(crate) fn render_semantic_count(
    label: &str,
    value: usize,
    non_zero_style: TextStyle,
    formatter: TextFormatter,
) -> String {
    formatter.paint(
        if value == 0 {
            TextStyle::Muted
        } else {
            non_zero_style
        },
        format!("{label}={value}"),
    )
}

pub(crate) fn git_status_style(status: &str) -> TextStyle {
    let normalized = status.trim();
    if normalized.contains('U') || normalized.contains('D') || normalized == "AA" {
        TextStyle::Error
    } else if normalized == "??" || normalized.contains('M') || normalized.contains('?') {
        TextStyle::Warning
    } else if normalized.contains('R') || normalized.contains('C') {
        TextStyle::Key
    } else if normalized.contains('A') {
        TextStyle::Success
    } else {
        TextStyle::Muted
    }
}

pub fn emit_warning(message: impl Display) {
    eprintln!("{}", render_warning_line(TextFormatter::stderr(), message));
}

pub fn emit_muted_stderr(message: impl Display) {
    eprintln!(
        "{}",
        TextFormatter::stderr().paint(TextStyle::Muted, message)
    );
}

/// Everything a command prints goes through one sink.
///
/// Adapters used to call `println!`, which meant three things had to be
/// remembered at every call site: whether `--quiet` was set, whether the caller
/// asked for JSON, and whether the stream was a terminal. `--quiet` in
/// particular was re-checked by hand at the top of every `emit` function, so a
/// new one silently opted out of it.
///
/// Here those are properties of the sink: [`Emitter::value`] renders text or
/// JSON according to the mode, writes nothing at all when quiet, and hands the
/// text renderer a formatter that already knows whether the stream is a
/// terminal. Tests write into buffers instead of into the process, so asserting
/// on output no longer needs a subprocess.
pub struct Emitter {
    out: Box<dyn Write>,
    err: Box<dyn Write>,
    mode: OutputMode,
    quiet: bool,
    out_color: TextFormatter,
    err_color: TextFormatter,
}

impl Emitter {
    /// The process streams, coloured according to whether each is a terminal.
    pub fn stdio(options: &GlobalOptions) -> Self {
        Self {
            out: Box::new(io::stdout()),
            err: Box::new(io::stderr()),
            mode: options.output,
            quiet: options.quiet,
            out_color: TextFormatter::stdout(),
            err_color: TextFormatter::stderr(),
        }
    }

    /// Buffers instead of the process streams, with colour off so assertions
    /// read as plain text.
    pub fn capture(options: &GlobalOptions) -> (Self, Captured) {
        let captured = Captured::default();
        let emitter = Self {
            out: Box::new(captured.out.clone()),
            err: Box::new(captured.err.clone()),
            mode: options.output,
            quiet: options.quiet,
            out_color: TextFormatter::with_color(false),
            err_color: TextFormatter::with_color(false),
        };
        (emitter, captured)
    }

    pub fn mode(&self) -> OutputMode {
        self.mode
    }

    pub fn is_text(&self) -> bool {
        self.mode == OutputMode::Text
    }

    /// The stdout formatter, for a renderer that has to run before
    /// [`Emitter::value`] is called.
    pub fn formatter(&self) -> TextFormatter {
        self.out_color
    }

    /// Emit one command result: `text` in text mode, `json` serialized in JSON
    /// mode, nothing at all when quiet.
    ///
    /// `text` is a closure so a renderer that costs something does not run when
    /// its output would be discarded.
    ///
    /// # Errors
    ///
    /// [`AppError`] if the payload will not serialize, or the stream will not
    /// accept the bytes.
    pub fn value<T, F>(&mut self, json: &T, text: F) -> Result<(), AppError>
    where
        T: Serialize + ?Sized,
        F: FnOnce(TextFormatter) -> String,
    {
        if self.quiet {
            return Ok(());
        }
        match self.mode {
            OutputMode::Text => {
                let rendered = text(self.out_color);
                // A renderer with nothing to say writes nothing, not a blank line.
                if rendered.is_empty() {
                    return Ok(());
                }
                self.write_line(&rendered)
            }
            OutputMode::Json => {
                let rendered = serde_json::to_string_pretty(json)?;
                self.write_line(&rendered)
            }
        }
    }

    /// Emit already-rendered text, in text mode only.
    ///
    /// For output with no JSON counterpart, such as a diff the caller receives
    /// verbatim.
    ///
    /// # Errors
    ///
    /// [`AppError`] if the stream will not accept the bytes.
    pub fn line(&mut self, text: impl Display) -> Result<(), AppError> {
        if self.quiet || !self.is_text() {
            return Ok(());
        }
        self.write_line(text)
    }

    /// A warning on stderr, suppressed by `--quiet` like every other output.
    ///
    /// Warnings are best-effort: a stderr that will not accept one must not
    /// fail the command that produced it.
    pub fn warning(&mut self, message: impl Display) {
        if self.quiet {
            return;
        }
        let line = render_warning_line(self.err_color, message);
        let _ = writeln!(self.err, "{line}");
    }

    /// Emit output whose format the command chose itself, rather than the
    /// global text/JSON mode - an `--report junit` document, say. Still subject
    /// to `--quiet`.
    ///
    /// # Errors
    ///
    /// Whatever `render` returns, or [`AppError`] if the stream will not accept
    /// the bytes.
    pub fn report<F>(&mut self, render: F) -> Result<(), AppError>
    where
        F: FnOnce(TextFormatter) -> Result<String, AppError>,
    {
        if self.quiet {
            return Ok(());
        }
        let rendered = render(self.out_color)?;
        self.write_line(rendered)
    }

    /// The stderr formatter, for a caller that renders a block itself.
    pub fn err_formatter(&self) -> TextFormatter {
        self.err_color
    }

    /// Write captured child output to stdout verbatim, adding no newline.
    ///
    /// A subprocess's bytes are its own; this must not reformat them.
    ///
    /// # Errors
    ///
    /// [`AppError`] if the stream will not accept the bytes.
    pub fn raw(&mut self, text: &str) -> Result<(), AppError> {
        if self.quiet || !self.is_text() || text.is_empty() {
            return Ok(());
        }
        write!(self.out, "{text}").map_err(write_failed)
    }

    /// Write captured child output to stderr verbatim, best-effort.
    pub fn raw_err(&mut self, text: &str) {
        if self.quiet || !self.is_text() || text.is_empty() {
            return;
        }
        let _ = write!(self.err, "{text}");
    }

    /// A warning that accompanies text output only. In JSON mode the payload
    /// already carries the fact - `truncated: true` and the like - so repeating
    /// it on stderr would be noise a machine reader has to filter.
    pub fn text_warning(&mut self, message: impl Display) {
        if self.is_text() {
            self.warning(message);
        }
    }

    /// A de-emphasized note on stderr, best-effort like [`Emitter::warning`].
    pub fn muted(&mut self, message: impl Display) {
        if self.quiet {
            return;
        }
        let line = self.err_color.paint(TextStyle::Muted, message);
        let _ = writeln!(self.err, "{line}");
    }

    fn write_line(&mut self, text: impl Display) -> Result<(), AppError> {
        writeln!(self.out, "{text}").map_err(write_failed)
    }
}

fn write_failed(error: io::Error) -> AppError {
    AppError::external(
        "OUTPUT_WRITE_FAILED",
        format!("failed to write output: {error}"),
    )
}

/// What an [`Emitter::capture`] emitter wrote.
#[derive(Clone, Default)]
pub struct Captured {
    out: SharedBuffer,
    err: SharedBuffer,
}

impl Captured {
    pub fn stdout(&self) -> String {
        self.out.contents()
    }

    pub fn stderr(&self) -> String {
        self.err.contents()
    }
}

/// A buffer the emitter can own while the test still reads it.
#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("buffer lock should not be poisoned"))
            .into_owned()
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("buffer lock should not be poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn render_warning_line(formatter: TextFormatter, message: impl Display) -> String {
    format!(
        "{} {message}",
        formatter.paint(TextStyle::Warning, "warning:")
    )
}

#[cfg(test)]
mod tests {
    use super::{
        TextFormatter, TextStyle, git_status_style, render_semantic_count, render_warning_line,
    };

    #[test]
    fn semantic_count_uses_muted_zero_and_requested_non_zero_style() {
        let formatter = TextFormatter::with_color(true);

        assert_eq!(
            render_semantic_count("changed", 0, TextStyle::Warning, formatter),
            "\u{1b}[2mchanged=0\u{1b}[0m"
        );
        assert_eq!(
            render_semantic_count("changed", 2, TextStyle::Warning, formatter),
            "\u{1b}[33mchanged=2\u{1b}[0m"
        );
    }

    #[test]
    fn git_status_style_maps_common_states() {
        assert_eq!(git_status_style("A "), TextStyle::Success);
        assert_eq!(git_status_style(" M"), TextStyle::Warning);
        assert_eq!(git_status_style("??"), TextStyle::Warning);
        assert_eq!(git_status_style("D "), TextStyle::Error);
        assert_eq!(git_status_style("UU"), TextStyle::Error);
        assert_eq!(git_status_style("AA"), TextStyle::Error);
        assert_eq!(git_status_style("R100"), TextStyle::Key);
    }

    #[test]
    fn warning_renderer_preserves_plain_contract() {
        assert_eq!(
            render_warning_line(TextFormatter::with_color(false), "output truncated"),
            "warning: output truncated"
        );
    }

    #[test]
    fn warning_renderer_styles_only_the_label() {
        assert_eq!(
            render_warning_line(TextFormatter::with_color(true), "output truncated"),
            "\u{1b}[33mwarning:\u{1b}[0m output truncated"
        );
    }
}
