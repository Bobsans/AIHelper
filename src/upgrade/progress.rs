//! Incremental text output. Only the active row is animated, so completed
//! stages remain in scrollback even in a small terminal.

use std::{io::IsTerminal, sync::mpsc, time::Duration};

use ah_output::{Emitter, GlobalOptions, OutputMode, TextStyle};
use ah_updater::{UpgradeProgress, check::UpgradeOutcome, request::UpgradeRequest};

use crate::error::AppError;

const FRAME_INTERVAL: Duration = Duration::from_millis(80);
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

enum Event {
    Progress(UpgradeProgress),
    Finished(bool),
}

pub(super) fn execute(
    request: UpgradeRequest,
    options: GlobalOptions,
    run: impl FnOnce(&dyn Fn(UpgradeProgress)) -> Result<UpgradeOutcome, AppError>,
) -> Result<(), AppError> {
    if options.quiet || options.output == OutputMode::Json {
        return super::render::outcome(&run(&|_| {})?, options);
    }

    let outcome = std::thread::scope(|scope| -> Result<UpgradeOutcome, AppError> {
        let (sender, receiver) = mpsc::channel();
        let render_options = &options;
        let renderer = scope.spawn(move || {
            let terminal = std::io::stdout().is_terminal();
            let width = console::Term::stdout()
                .size_checked()
                .map(|(_, width)| usize::from(width).saturating_sub(1));
            let mut screen = ProgressScreen {
                emitter: Emitter::stdio(render_options),
                terminal,
                width,
                current: None,
                frame: 0,
                heading: match request {
                    UpgradeRequest::Rollback => "AIHelper rollback",
                    UpgradeRequest::Check => "AIHelper update check",
                    _ => "AIHelper update",
                },
                header_shown: false,
            };
            loop {
                match receiver.recv_timeout(FRAME_INTERVAL) {
                    Ok(Event::Progress(progress)) => screen.apply(progress)?,
                    Ok(Event::Finished(success)) => return screen.complete(success),
                    Err(mpsc::RecvTimeoutError::Timeout) => screen.tick()?,
                    // Unwinding the updater must not leave the renderer waiting.
                    Err(mpsc::RecvTimeoutError::Disconnected) => return screen.complete(false),
                }
            }
        });
        let result = run(&|progress| {
            let _ = sender.send(Event::Progress(progress));
        });
        let _ = sender.send(Event::Finished(result.is_ok()));
        drop(sender);
        let rendered = renderer.join().expect("upgrade renderer panicked");
        // Preserve the update error if reporting also failed. A broken output
        // stream must not interrupt service restoration or helper handoff.
        let outcome = result?;
        rendered?;
        Ok(outcome)
    })?;
    super::render::outcome(&outcome, options)
}

struct ProgressScreen {
    emitter: Emitter,
    terminal: bool,
    width: Option<usize>,
    current: Option<&'static str>,
    frame: usize,
    heading: &'static str,
    header_shown: bool,
}

impl ProgressScreen {
    fn apply(&mut self, progress: UpgradeProgress) -> Result<(), AppError> {
        if let UpgradeProgress::Version { current, selected } = progress {
            self.clear_row()?;
            let formatter = self.emitter.formatter();
            self.emitter.line(format!(
                "{} · {} → {}\n",
                formatter.paint(TextStyle::Heading, self.heading),
                formatter.paint(TextStyle::Key, current),
                formatter.paint(TextStyle::Key, selected),
            ))?;
            self.header_shown = true;
            return self.finish(true);
        }
        self.finish(true)?;
        let label = match progress {
            UpgradeProgress::Checking => "Checking for updates",
            UpgradeProgress::CheckingBackup => "Verifying backup",
            UpgradeProgress::Downloading => "Downloading update",
            UpgradeProgress::Verifying => "Verifying update",
            UpgradeProgress::Testing => "Testing the new version",
            UpgradeProgress::Preparing => "Preparing installation",
            UpgradeProgress::StoppingMcp => "Stopping the MCP service",
            UpgradeProgress::Launching => "Starting the installer",
            UpgradeProgress::Version { .. } => unreachable!(),
        };
        self.current = Some(label);
        self.frame = 0;
        if self.terminal {
            self.tick()
        } else if matches!(
            progress,
            UpgradeProgress::Checking | UpgradeProgress::CheckingBackup
        ) {
            Ok(())
        } else {
            let mark = self.emitter.formatter().paint(TextStyle::Key, "…");
            self.emitter.line(format!("  {mark} {label}…"))
        }
    }

    fn tick(&mut self) -> Result<(), AppError> {
        if self.terminal
            && let Some(label) = self.current
        {
            let mark = self
                .emitter
                .formatter()
                .paint(TextStyle::Key, FRAMES[self.frame % FRAMES.len()]);
            let line = format!("  {mark} {label}…");
            self.frame += 1;
            self.replace_row(&line)?;
        }
        Ok(())
    }

    fn finish(&mut self, success: bool) -> Result<(), AppError> {
        if let Some(label) = self.current.take() {
            let (style, mark) = if success {
                (TextStyle::Success, "✓")
            } else {
                (TextStyle::Error, "✗")
            };
            let mark = self.emitter.formatter().paint(style, mark);
            let line = format!("  {mark} {label}");
            if self.terminal {
                self.replace_row(&line)?;
                self.emitter.line("")?;
            } else {
                self.emitter.line(line)?;
            }
        }
        Ok(())
    }

    fn complete(&mut self, success: bool) -> Result<(), AppError> {
        if success && self.current == Some("Checking for updates") {
            self.clear_row()?;
            self.current = None;
            return Ok(());
        }
        if !success && !self.header_shown {
            self.clear_row()?;
            self.emitter.line(
                self.emitter
                    .formatter()
                    .paint(TextStyle::Heading, self.heading),
            )?;
        }
        self.finish(success)
    }

    fn clear_row(&mut self) -> Result<(), AppError> {
        if self.terminal && self.current.is_some() {
            self.replace_row("")?;
        }
        Ok(())
    }

    fn replace_row(&mut self, line: &str) -> Result<(), AppError> {
        let line = console::truncate_str(line, self.width.unwrap_or(80), "");
        self.emitter.raw(&format!("\r{line}\u{1b}[K"))?;
        self.emitter.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(terminal: bool) -> (ProgressScreen, ah_output::Captured) {
        let (emitter, captured) = Emitter::capture(&GlobalOptions {
            output: OutputMode::Text,
            quiet: false,
            limit: None,
            cwd: None,
        });
        (
            ProgressScreen {
                emitter,
                terminal,
                width: Some(80),
                current: None,
                frame: 0,
                heading: "AIHelper update",
                header_shown: false,
            },
            captured,
        )
    }

    #[test]
    fn redirected_output_reports_stages_and_failure_without_ansi() {
        let (mut screen, captured) = screen(false);
        screen.apply(UpgradeProgress::Checking).unwrap();
        screen
            .apply(UpgradeProgress::Version {
                current: "2.0.0".into(),
                selected: "2.0.1".into(),
            })
            .unwrap();
        screen.apply(UpgradeProgress::Downloading).unwrap();
        screen.tick().unwrap();
        screen.finish(false).unwrap();
        assert_eq!(
            captured.stdout(),
            "AIHelper update · 2.0.0 → 2.0.1\n\n  ✓ Checking for updates\n  … Downloading update…\n  ✗ Downloading update\n"
        );
    }

    #[test]
    fn terminal_animates_only_current_row_and_keeps_finished_stages() {
        let (mut screen, captured) = screen(true);
        screen.apply(UpgradeProgress::Downloading).unwrap();
        screen.tick().unwrap();
        screen.apply(UpgradeProgress::Verifying).unwrap();
        screen.finish(true).unwrap();
        let output = captured.stdout();
        assert!(output.contains("⠋ Downloading update"));
        assert!(output.contains("⠙ Downloading update"));
        assert!(output.contains("\r  ✓ Downloading update\u{1b}[K\n"));
        assert!(output.ends_with("\r  ✓ Verifying update\u{1b}[K\n"));
        assert!(!output.contains("[?25l"));
    }

    #[test]
    fn active_row_is_clipped_to_avoid_wrapping() {
        let (mut screen, captured) = screen(true);
        screen.width = Some(8);
        screen.apply(UpgradeProgress::Downloading).unwrap();
        assert_eq!(captured.stdout(), "\r  ⠋ Down\u{1b}[K");
    }

    #[test]
    fn successful_check_leaves_only_the_final_result() {
        let (mut plain_screen, captured) = screen(false);
        plain_screen.apply(UpgradeProgress::Checking).unwrap();
        plain_screen.complete(true).unwrap();
        assert_eq!(captured.stdout(), "");

        let (mut screen, captured) = screen(true);
        screen.apply(UpgradeProgress::Checking).unwrap();
        screen.complete(true).unwrap();
        assert!(captured.stdout().ends_with("\r\u{1b}[K"));
        assert!(!captured.stdout().contains("✓"));
    }
}
