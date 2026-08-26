//! The live status screen: one row per target, redrawn in place as each
//! probe finishes.
//!
//! Separated from the install logic because it owns a terminal - cursor
//! control, frame clipping, column widths - and the logic owns none of that.
//! The logic reports [`LiveEvent`]s; this consumes them.

use std::{
    borrow::Cow,
    io::{self, Write},
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{
    error::AppError,
    output::{TextFormatter, TextStyle},
};

use super::targets::{self, Scope, Target, Transport};

use super::install::*;
use super::output::{status_action_label, status_action_style};

pub(super) struct LiveTarget {
    target: &'static Target,
    cli: Option<(Option<&'static str>, bool)>,
    scopes: Vec<LiveScope>,
    legacy_server: Option<&'static str>,
    error: bool,
}

pub(super) struct LiveScope {
    scope: Scope,
    mcp_supported: bool,
    mcp: Option<ScopedMcpReport>,
    rules_supported: bool,
    rules: Option<ScopedRulesReport>,
}

impl LiveTarget {
    fn new(target: &'static Target, in_project: bool) -> Self {
        Self {
            target,
            cli: None,
            scopes: status_scopes(target, in_project)
                .into_iter()
                .map(|scope| LiveScope {
                    scope,
                    mcp_supported: target.supports_status_mcp(scope),
                    mcp: None,
                    rules_supported: target.supports_status_rules(scope),
                    rules: None,
                })
                .collect(),
            legacy_server: None,
            error: false,
        }
    }

    fn apply(&mut self, progress: StatusProgress) {
        match progress {
            StatusProgress::Cli { cli, available } => self.cli = Some((cli, available)),
            StatusProgress::Mcp { scope, report } => {
                self.scopes
                    .iter_mut()
                    .find(|status| status.scope == scope)
                    .expect("status scope should be preallocated")
                    .mcp = Some(report)
            }
            StatusProgress::Rules { scope, report } => {
                self.scopes
                    .iter_mut()
                    .find(|status| status.scope == scope)
                    .expect("status scope should be preallocated")
                    .rules = Some(report)
            }
        }
    }

    fn complete(&mut self, result: &Result<TargetStatus, AppError>) {
        match result {
            Ok(status) => self.legacy_server = status.legacy_server,
            Err(_) => self.error = true,
        }
    }
}

pub(super) enum LiveEvent {
    Progress(usize, StatusProgress),
    Complete(usize, Result<TargetStatus, AppError>),
}

pub(super) struct CursorGuard;

impl Drop for CursorGuard {
    fn drop(&mut self) {
        let mut output = io::stdout();
        let _ = write!(output, "\u{1b}[?25h");
        let _ = output.flush();
    }
}

/// One scope of one target: `mcp` and `rules` side by side under a heading.
pub(super) struct LiveRow {
    target: usize,
    scope: &'static str,
    mcp: String,
    rules: String,
}

pub(super) const UNSUPPORTED: &str = "not supported";

/// Widest cell a finished `mcp` check can produce, reserved before the first
/// frame so the `rules` column never shifts as spinners become results.
pub(super) fn mcp_width() -> usize {
    let transport = [Transport::Stdio, Transport::Http]
        .into_iter()
        .map(|transport| transport.as_str().len())
        .max()
        .unwrap_or(0);
    [
        StatusAction::Installed,
        StatusAction::NotPresent,
        StatusAction::Unknown,
    ]
    .into_iter()
    .map(|action| {
        let label = status_action_label(action).len();
        // Only a registration carries a transport in parentheses.
        match action {
            StatusAction::Installed => label + " (".len() + transport + ")".len(),
            _ => label,
        }
    })
    .chain([UNSUPPORTED.len()])
    .max()
    .unwrap_or(UNSUPPORTED.len())
}

pub(super) fn render_live_lines(
    targets: &[LiveTarget],
    spinner: &str,
    formatter: TextFormatter,
) -> Vec<String> {
    let error = formatter.paint(TextStyle::Error, "error");
    let unsupported = formatter.paint(TextStyle::Muted, UNSUPPORTED);
    let headings = targets.iter().map(|target| {
        let cli = match target.cli {
            Some((Some(cli), true)) => {
                formatter.paint(TextStyle::Success, format!("`{cli}` available"))
            }
            Some((Some(cli), false)) => {
                formatter.paint(TextStyle::Warning, format!("`{cli}` missing"))
            }
            Some((None, _)) => formatter.paint(TextStyle::Muted, "config file"),
            None if target.error => error.clone(),
            None => spinner.to_owned(),
        };
        // A legacy registration belongs to the target rather than to one of its
        // scopes, and would widen the mcp column it used to sit in.
        let legacy = target
            .legacy_server
            .map(|name| formatter.paint(TextStyle::Warning, format!(", legacy `{name}` present")))
            .unwrap_or_default();
        format!(
            "{} ({cli}{legacy})",
            formatter.paint(TextStyle::Heading, target.target.name),
        )
    });
    let error = error.as_str();
    let unsupported = unsupported.as_str();
    let rows: Vec<LiveRow> = targets
        .iter()
        .enumerate()
        .flat_map(|(index, target)| {
            let pending = move || {
                if target.error {
                    error.to_owned()
                } else {
                    spinner.to_owned()
                }
            };
            target.scopes.iter().map(move |scope| {
                let mcp = if !scope.mcp_supported {
                    unsupported.to_owned()
                } else if let Some(mcp) = &scope.mcp {
                    let details: Vec<String> = mcp
                        .transport
                        .map(|transport| transport.as_str().to_owned())
                        .into_iter()
                        .chain(mcp.detail.clone())
                        .collect();
                    state_cell(mcp.action, &details, formatter)
                } else {
                    pending()
                };
                let rules = if !scope.rules_supported {
                    unsupported.to_owned()
                } else if let Some(rules) = &scope.rules {
                    state_cell(rules.action, rules.detail.as_slice(), formatter)
                } else {
                    pending()
                };
                LiveRow {
                    target: index,
                    scope: scope.scope.as_str(),
                    mcp,
                    rules,
                }
            })
        })
        .collect();
    let scope_width = rows.iter().map(|row| row.scope.len()).max().unwrap_or(0);
    let mcp_width = mcp_width();
    let separator = formatter.paint(TextStyle::Muted, "::");
    let mut lines = Vec::with_capacity(targets.len() + rows.len());
    for (index, heading) in headings.enumerate() {
        lines.push(heading);
        for row in rows.iter().filter(|row| row.target == index) {
            lines.push(format!(
                "  {} {separator} mcp {} rules {}",
                pad(&formatter.paint(TextStyle::Key, row.scope), scope_width),
                pad(&row.mcp, mcp_width),
                row.rules,
            ));
        }
    }
    lines
}

pub(super) fn state_cell(
    action: StatusAction,
    details: &[String],
    formatter: TextFormatter,
) -> String {
    let state = formatter.paint(status_action_style(action), status_action_label(action));
    if details.is_empty() {
        return state;
    }
    format!("{state} ({})", details.join(", "))
}

/// Pads to a visible width, which `{:<width$}` cannot do once a cell carries
/// colour escapes.
pub(super) fn pad(cell: &str, width: usize) -> String {
    let padding = width.saturating_sub(console::measure_text_width(cell));
    format!("{cell}{:padding$}", "")
}

/// A live block redrawn in place, kept inside the terminal it lands in.
pub(super) struct LiveScreen {
    /// Widest column a line may occupy before it wraps and desynchronises the
    /// cursor arithmetic below.
    width: Option<usize>,
    /// Tallest block that may be animated: once the block scrolls, its first row
    /// is out of reach, `\u{1b}[<n>A` clamps at the top of the screen and every
    /// frame is appended instead of replacing the previous one.
    height: Option<usize>,
    drawn: usize,
}

impl LiveScreen {
    fn new() -> Self {
        let size = console::Term::stdout().size_checked();
        Self {
            // Terminals differ on whether a line filling the last column wraps
            // immediately, so keep one column and one row spare.
            width: size.map(|(_, width)| usize::from(width).saturating_sub(1)),
            height: size.map(|(height, _)| usize::from(height).saturating_sub(1)),
            drawn: 0,
        }
    }

    /// One animation frame, clipped to the visible viewport.
    fn draw(&mut self, output: &mut impl Write, lines: &[String]) -> io::Result<()> {
        let visible = clip_height(lines, self.height);
        self.rewind(output)?;
        write_live_lines(output, &visible, self.width)?;
        self.drawn = visible.len();
        output.flush()
    }

    /// The finished report, in full: the clipped frames are erased first so the
    /// rows the viewport could not hold are not left behind as a stale copy.
    fn finish(&mut self, output: &mut impl Write, lines: &[String]) -> io::Result<()> {
        self.rewind(output)?;
        if self.drawn > 0 {
            write!(output, "\r\u{1b}[J")?;
        }
        write_live_lines(output, lines, self.width)?;
        self.drawn = lines.len();
        output.flush()
    }

    fn rewind(&self, output: &mut impl Write) -> io::Result<()> {
        if self.drawn > 0 {
            write!(output, "\u{1b}[{}A", self.drawn)?;
        }
        Ok(())
    }
}

/// Keeps the block inside `height` rows, replacing the rows that do not fit with
/// a count of what the finished report will add.
pub(super) fn clip_height(lines: &[String], height: Option<usize>) -> Vec<String> {
    let Some(height) = height.filter(|height| lines.len() > *height) else {
        return lines.to_vec();
    };
    let kept = height.saturating_sub(1);
    let mut visible = lines[..kept].to_vec();
    if height > 0 {
        visible.push(format!("… {} more", lines.len() - kept));
    }
    visible
}

pub(super) fn write_live_lines(
    output: &mut impl Write,
    lines: &[String],
    width: Option<usize>,
) -> io::Result<()> {
    for line in lines {
        let line = match width {
            Some(width) => console::truncate_str(line, width, ""),
            None => Cow::Borrowed(line.as_str()),
        };
        // Overwrite in place and clear the tail: clearing first would blank the
        // row for a frame and read as flicker.
        write!(output, "\r{line}\u{1b}[K\n")?;
    }
    Ok(())
}

pub(super) fn emit_live_status(request: StatusRequest) -> Result<(), AppError> {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const FRAME_INTERVAL: Duration = Duration::from_millis(80);

    let selected: Vec<&Target> = match request.target {
        Some(name) => vec![targets::find(&name)?],
        None => targets::TARGETS.iter().collect(),
    };
    let cwd = project_root()?;
    let project = status_project_root(&cwd);
    let in_project = project.is_some();
    let root = project.as_deref().unwrap_or(&cwd);
    let mut targets = selected
        .iter()
        .map(|target| LiveTarget::new(target, in_project))
        .collect::<Vec<_>>();
    let mut results = std::iter::repeat_with(|| None)
        .take(selected.len())
        .collect::<Vec<Option<Result<TargetStatus, AppError>>>>();
    let formatter = TextFormatter::stdout();
    let mut output = io::stdout();
    let started = Instant::now();
    let frame = |elapsed: Duration| {
        FRAMES[(elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as usize % FRAMES.len()]
    };
    let mut screen = LiveScreen::new();
    write!(output, "\u{1b}[?25l").map_err(status_render_error)?;
    let _cursor = CursorGuard;
    screen
        .draw(
            &mut output,
            &render_live_lines(&targets, FRAMES[0], formatter),
        )
        .map_err(status_render_error)?;

    std::thread::scope(|scope| -> Result<(), AppError> {
        let (sender, receiver) = mpsc::channel();
        for (index, target) in selected.iter().copied().enumerate() {
            let sender = sender.clone();
            let root = &root;
            scope.spawn(move || {
                let result = status_target(target, root, in_project, |progress| {
                    let _ = sender.send(LiveEvent::Progress(index, progress));
                });
                let _ = sender.send(LiveEvent::Complete(index, result));
            });
        }
        drop(sender);

        let mut completed = 0;
        while completed < selected.len() {
            match receiver.recv_timeout(FRAME_INTERVAL) {
                Ok(LiveEvent::Progress(index, progress)) => targets[index].apply(progress),
                Ok(LiveEvent::Complete(index, result)) => {
                    targets[index].complete(&result);
                    if results[index].is_none() {
                        completed += 1;
                    }
                    results[index] = Some(result);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AppError::external(
                        "AI_STATUS_WORKER_FAILED",
                        "an AI status worker stopped without returning a result",
                    ));
                }
            }
            screen
                .draw(
                    &mut output,
                    &render_live_lines(&targets, frame(started.elapsed()), formatter),
                )
                .map_err(status_render_error)?;
        }
        Ok(())
    })?;

    screen
        .finish(
            &mut output,
            &render_live_lines(&targets, FRAMES[0], formatter),
        )
        .map_err(status_render_error)?;

    results
        .into_iter()
        .map(|result| {
            result.unwrap_or_else(|| {
                Err(AppError::external(
                    "AI_STATUS_WORKER_FAILED",
                    "an AI status worker stopped without returning a result",
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(())
}

pub(super) fn status_render_error(source: io::Error) -> AppError {
    AppError::external(
        "AI_STATUS_RENDER_FAILED",
        format!("unable to render AI status: {source}"),
    )
}

#[cfg(test)]
mod tests {
    use ah_plugin_api::TextFormatter;

    use super::*;
    use crate::ai::install::{StatusAction, StatusProgress};

    #[test]
    fn pending_status_renders_each_supported_scope() {
        let targets = [LiveTarget::new(
            crate::ai::targets::find("cursor").unwrap(),
            true,
        )];

        assert_eq!(
            render_live_lines(&targets, "*", TextFormatter::with_color(false)),
            vec![
                "cursor (*)",
                "  user    :: mcp *                 rules *",
                "  project :: mcp *                 rules *",
            ]
        );
    }

    #[test]
    fn live_status_replaces_ready_slots_without_waiting_for_mcp() {
        let mut target = LiveTarget::new(crate::ai::targets::find("cursor").unwrap(), true);
        target.apply(StatusProgress::Cli {
            cli: None,
            available: true,
        });
        target.apply(StatusProgress::Rules {
            scope: Scope::User,
            report: super::ScopedRulesReport {
                action: super::StatusAction::Unknown,
                path: None,
                detail: Some("Cursor settings".to_owned()),
            },
        });

        assert_eq!(
            render_live_lines(&[target], "*", TextFormatter::with_color(false)),
            vec![
                "cursor (config file)",
                "  user    :: mcp *                 rules unknown (Cursor settings)",
                "  project :: mcp *                 rules *",
            ]
        );
    }

    #[test]
    fn live_status_replaces_the_mcp_slot_when_its_probe_finishes() {
        let mut target = LiveTarget::new(crate::ai::targets::find("cursor").unwrap(), true);
        target.apply(StatusProgress::Mcp {
            scope: Scope::Project,
            report: ScopedMcpReport {
                action: StatusAction::Installed,
                registrar: "file",
                scope: Scope::Project,
                transport: Some(Transport::Http),
                url: Some("http://127.0.0.1:8787/mcp".to_owned()),
                path: Some("opencode.json".to_owned()),
                detail: None,
            },
        });

        assert_eq!(
            render_live_lines(&[target], "*", TextFormatter::with_color(false))[2],
            "  project :: mcp installed (http)  rules *"
        );
    }

    #[test]
    fn the_rules_column_does_not_move_when_a_spinner_becomes_a_result() {
        let formatter = TextFormatter::with_color(false);
        let mut target = LiveTarget::new(crate::ai::targets::find("cursor").unwrap(), true);
        let pending = render_live_lines(std::slice::from_ref(&target), "*", formatter);
        target.apply(StatusProgress::Mcp {
            scope: Scope::User,
            report: ScopedMcpReport {
                action: StatusAction::Installed,
                registrar: "file",
                scope: Scope::User,
                transport: Some(Transport::Stdio),
                url: None,
                path: None,
                detail: None,
            },
        });

        let column = |lines: &[String]| {
            lines[1..]
                .iter()
                .map(|line| line.find("rules ").expect("every row has a rules cell"))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            column(&pending),
            column(&render_live_lines(&[target], "*", formatter))
        );
    }

    #[test]
    fn the_first_frame_is_written_where_the_cursor_already_is() {
        let mut output = Vec::new();

        screen(None, None)
            .draw(&mut output, &["one".to_owned(), "two".to_owned()])
            .unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\rone\u{1b}[K\n\rtwo\u{1b}[K\n"
        );
    }

    #[test]
    fn every_later_frame_rewinds_over_the_rows_it_drew() {
        let mut output = Vec::new();
        let mut screen = screen(None, None);
        let lines = ["one".to_owned(), "two".to_owned()];

        screen.draw(&mut io::sink(), &lines).unwrap();
        screen.draw(&mut output, &lines).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\u{1b}[2A\rone\u{1b}[K\n\rtwo\u{1b}[K\n"
        );
    }

    #[test]
    fn lines_are_clipped_to_the_terminal_width_ansi_aside() {
        let mut output = Vec::new();

        screen(Some(9), None)
            .draw(&mut output, &["\u{1b}[1msome very long line".to_owned()])
            .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(
            rendered.contains("some very") && !rendered.contains("long"),
            "{rendered:?} should keep nine visible columns"
        );
    }

    #[test]
    fn a_block_taller_than_the_terminal_is_animated_within_the_viewport() {
        let lines: Vec<String> = (0..8).map(|row| format!("row {row}")).collect();

        assert_eq!(
            super::clip_height(&lines, Some(3)),
            vec!["row 0", "row 1", "… 6 more"]
        );
        assert_eq!(super::clip_height(&lines, Some(8)), lines);
        assert_eq!(super::clip_height(&lines, None), lines);
    }

    #[test]
    fn the_finished_report_erases_the_clipped_frames_before_printing_in_full() {
        let mut output = Vec::new();
        let mut screen = screen(None, Some(2));
        let lines = ["one".to_owned(), "two".to_owned(), "three".to_owned()];

        screen.draw(&mut io::sink(), &lines).unwrap();
        screen.finish(&mut output, &lines).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\u{1b}[2A\r\u{1b}[J\rone\u{1b}[K\n\rtwo\u{1b}[K\n\rthree\u{1b}[K\n"
        );
    }

    fn screen(width: Option<usize>, height: Option<usize>) -> LiveScreen {
        LiveScreen {
            width,
            height,
            drawn: 0,
        }
    }
}
