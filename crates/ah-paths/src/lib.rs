//! Where AIHelper's files live.
//!
//! Six subsystems used to answer this independently — the host config, the
//! postgres plugin, the updater's two entry points, the AI targets and the
//! managed service — with different fallback orders, so a user who set
//! `AH_CONFIG_DIR` got different degrees of respect from each. The postgres
//! plugin's version was a character-for-character copy of the host's, which is
//! the only reason those two agreed.
//!
//! The per-platform policy here is *data*, not `cfg`. One `cfg` picks the
//! layout; every branch of every layout is then reachable from a test on any
//! machine, which is what the inline `cfg` blocks made impossible.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The directory conventions of one operating system family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `%APPDATA%` and `%LOCALAPPDATA%`, application name capitalised.
    Windows,
    /// `~/Library`, application name capitalised.
    MacOs,
    /// The XDG base directory specification, application name lowercased.
    Xdg,
}

impl Layout {
    /// The layout of the machine this was compiled for.
    ///
    /// The only `cfg` in the crate. Everything below it takes the layout as an
    /// argument, so a test can ask what any platform would answer.
    #[must_use]
    pub fn host() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            Self::Xdg
        }
    }
}

/// Where a resolved directory came from.
///
/// Recorded rather than inferred: the host joins a *relative* override onto the
/// request directory, and it can only know an override was in play by being
/// told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The named environment variable asked for this path.
    Environment(&'static str),
    /// Nobody asked; this is where the platform puts it.
    Platform,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub source: Source,
}

/// Why a directory could not be resolved.
///
/// Carries the variable rather than a message: each caller has its own error
/// type and its own wording, and this crate exists to unify the *policy*, not
/// to rewrite six error strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    /// The variable that was consulted and found unusable.
    pub variable: &'static str,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Set, but to nothing.
    Empty,
    /// Not set, or set to nothing, and there is no further fallback.
    Unresolved,
}

impl Error {
    fn empty(variable: &'static str) -> Self {
        Self {
            variable,
            kind: ErrorKind::Empty,
        }
    }

    fn unresolved(variable: &'static str) -> Self {
        Self {
            variable,
            kind: ErrorKind::Unresolved,
        }
    }
}

/// The environment a resolution reads.
///
/// A parameter rather than `std::env` so that two tests asking about two
/// platforms cannot race each other over process-global variables.
pub trait Environment {
    fn get(&self, name: &str) -> Option<OsString>;
}

/// The real process environment.
pub struct ProcessEnvironment;

impl Environment for ProcessEnvironment {
    fn get(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }
}

const CONFIG_OVERRIDE: &str = "AH_CONFIG_DIR";
const CACHE_OVERRIDE: &str = "AH_CACHE_DIR";
const HOME: &str = "HOME";

/// The configuration directory, honouring `AH_CONFIG_DIR`.
///
/// # Errors
///
/// [`Error`] when the override is set to nothing, or when no override is set
/// and the platform's own variable is missing.
pub fn config_dir(layout: Layout, environment: &dyn Environment) -> Result<Resolved, Error> {
    match override_dir(CONFIG_OVERRIDE, environment)? {
        Some(resolved) => Ok(resolved),
        None => Ok(Resolved {
            path: platform_config_dir(layout, environment)?,
            source: Source::Platform,
        }),
    }
}

/// The cache directory, honouring `AH_CACHE_DIR`.
///
/// # Errors
///
/// As [`config_dir`].
pub fn cache_dir(layout: Layout, environment: &dyn Environment) -> Result<Resolved, Error> {
    match override_dir(CACHE_OVERRIDE, environment)? {
        Some(resolved) => Ok(resolved),
        None => Ok(Resolved {
            path: platform_cache_dir(layout, environment)?,
            source: Source::Platform,
        }),
    }
}

/// Where the platform puts AIHelper's configuration, ignoring any override.
///
/// The updater resolves its state root this way on purpose: a helper from one
/// release hands off to an `ah` from another, and an `AH_CONFIG_DIR` set for one
/// command must not move the transaction the other is trying to finish.
///
/// # Errors
///
/// [`Error`] naming the platform variable that could not be read.
pub fn platform_config_dir(
    layout: Layout,
    environment: &dyn Environment,
) -> Result<PathBuf, Error> {
    match layout {
        Layout::Windows => Ok(non_empty("APPDATA", environment)?.join("AIHelper")),
        Layout::MacOs => Ok(non_empty(HOME, environment)?
            .join("Library")
            .join("Application Support")
            .join("AIHelper")),
        Layout::Xdg => match non_empty("XDG_CONFIG_HOME", environment) {
            Ok(base) => Ok(base.join("aihelper")),
            Err(_) => Ok(non_empty(HOME, environment)?
                .join(".config")
                .join("aihelper")),
        },
    }
}

/// Where the platform puts AIHelper's cache, ignoring any override.
///
/// # Errors
///
/// As [`platform_config_dir`].
pub fn platform_cache_dir(layout: Layout, environment: &dyn Environment) -> Result<PathBuf, Error> {
    match layout {
        Layout::Windows => Ok(non_empty("LOCALAPPDATA", environment)?.join("AIHelper")),
        Layout::MacOs => Ok(non_empty(HOME, environment)?
            .join("Library")
            .join("Caches")
            .join("AIHelper")),
        Layout::Xdg => match non_empty("XDG_CACHE_HOME", environment) {
            Ok(base) => Ok(base.join("aihelper")),
            Err(_) => Ok(non_empty(HOME, environment)?
                .join(".cache")
                .join("aihelper")),
        },
    }
}

/// The user's home directory.
///
/// Windows reads `USERPROFILE` and does *not* fall back to `HOME`: a shell like
/// MSYS sets `HOME` to a path inside its own tree, and an agent configuration
/// written there would be invisible to the agent.
///
/// # Errors
///
/// [`Error`] naming [`home_variable`] when it is unset or empty.
pub fn home_dir(layout: Layout, environment: &dyn Environment) -> Result<PathBuf, Error> {
    non_empty(home_variable(layout), environment)
}

/// The variable a home directory comes from on this layout.
///
/// Exposed because callers word their own message around the name.
#[must_use]
pub fn home_variable(layout: Layout) -> &'static str {
    if layout == Layout::Windows {
        "USERPROFILE"
    } else {
        HOME
    }
}

fn override_dir(
    variable: &'static str,
    environment: &dyn Environment,
) -> Result<Option<Resolved>, Error> {
    let Some(value) = environment.get(variable) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() {
        return Err(Error::empty(variable));
    }
    Ok(Some(Resolved {
        path,
        source: Source::Environment(variable),
    }))
}

fn non_empty(variable: &'static str, environment: &dyn Environment) -> Result<PathBuf, Error> {
    environment
        .get(variable)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| Error::unresolved(variable))
}

/// Join a relative path onto a base, leaving an absolute one alone.
///
/// Here because several callers of this crate need it and each had written its
/// own.
#[must_use]
pub fn rebase(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment built from pairs, so a test states exactly what is set.
    struct Fake(Vec<(&'static str, &'static str)>);

    impl Environment for Fake {
        fn get(&self, name: &str) -> Option<OsString> {
            self.0
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(*value))
        }
    }

    /// One row of the table: a platform, what its environment holds, and the
    /// directory that should come out.
    type Case = (
        Layout,
        &'static [(&'static str, &'static str)],
        &'static str,
    );

    fn env(pairs: &[(&'static str, &'static str)]) -> Fake {
        Fake(pairs.to_vec())
    }

    /// Every platform's configuration directory, checked from any machine.
    ///
    /// These paths used to sit behind `cfg`, so on any given run two of the
    /// three were not compiled, let alone tested.
    #[test]
    fn each_layout_places_configuration_where_its_platform_expects() {
        let cases: [Case; 4] = [
            (
                Layout::Windows,
                &[("APPDATA", "C:/Users/x/AppData/Roaming")],
                "C:/Users/x/AppData/Roaming/AIHelper",
            ),
            (
                Layout::MacOs,
                &[("HOME", "/Users/x")],
                "/Users/x/Library/Application Support/AIHelper",
            ),
            (
                Layout::Xdg,
                &[("XDG_CONFIG_HOME", "/home/x/.config")],
                "/home/x/.config/aihelper",
            ),
            (
                Layout::Xdg,
                &[("HOME", "/home/x")],
                "/home/x/.config/aihelper",
            ),
        ];
        for (layout, pairs, expected) in cases {
            let found =
                platform_config_dir(layout, &env(pairs)).expect("resolution should succeed");
            assert_eq!(found, PathBuf::from(expected), "{layout:?} {pairs:?}");
        }
    }

    #[test]
    fn each_layout_places_the_cache_where_its_platform_expects() {
        let cases: [Case; 4] = [
            (
                Layout::Windows,
                &[("LOCALAPPDATA", "C:/Users/x/AppData/Local")],
                "C:/Users/x/AppData/Local/AIHelper",
            ),
            (
                Layout::MacOs,
                &[("HOME", "/Users/x")],
                "/Users/x/Library/Caches/AIHelper",
            ),
            (
                Layout::Xdg,
                &[("XDG_CACHE_HOME", "/home/x/.cache")],
                "/home/x/.cache/aihelper",
            ),
            (
                Layout::Xdg,
                &[("HOME", "/home/x")],
                "/home/x/.cache/aihelper",
            ),
        ];
        for (layout, pairs, expected) in cases {
            let found = platform_cache_dir(layout, &env(pairs)).expect("resolution should succeed");
            assert_eq!(found, PathBuf::from(expected), "{layout:?} {pairs:?}");
        }
    }

    /// An empty `XDG_*` variable falls through to `HOME` rather than resolving
    /// to a bare `aihelper` in whatever directory the process sits in.
    #[test]
    fn an_empty_xdg_variable_falls_through_to_home() {
        let environment = env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/x")]);
        assert_eq!(
            platform_config_dir(Layout::Xdg, &environment).expect("resolution should succeed"),
            PathBuf::from("/home/x/.config/aihelper")
        );
    }

    /// The override wins over the platform, and says so.
    #[test]
    fn the_override_wins_and_records_that_it_did() {
        let environment = env(&[("AH_CONFIG_DIR", "/somewhere/else"), ("HOME", "/home/x")]);
        assert_eq!(
            config_dir(Layout::Xdg, &environment).expect("resolution should succeed"),
            Resolved {
                path: PathBuf::from("/somewhere/else"),
                source: Source::Environment("AH_CONFIG_DIR"),
            }
        );
    }

    /// An override set to nothing is an error, not a fallback.
    ///
    /// Silently falling back would put files somewhere the user did not ask
    /// for while they believe they redirected them.
    #[test]
    fn an_empty_override_is_refused() {
        type Resolve = fn(Layout, &dyn Environment) -> Result<Resolved, Error>;
        for (variable, resolve) in [
            ("AH_CONFIG_DIR", config_dir as Resolve),
            ("AH_CACHE_DIR", cache_dir as Resolve),
        ] {
            let environment = env(&[(variable, ""), ("HOME", "/home/x")]);
            assert_eq!(
                resolve(Layout::Xdg, &environment),
                Err(Error {
                    variable,
                    kind: ErrorKind::Empty
                })
            );
        }
    }

    /// The platform form ignores the override, because the updater's state must
    /// stay where the previous release left it.
    #[test]
    fn the_platform_form_ignores_the_override() {
        let environment = env(&[("AH_CONFIG_DIR", "/somewhere/else"), ("HOME", "/home/x")]);
        assert_eq!(
            platform_config_dir(Layout::Xdg, &environment).expect("resolution should succeed"),
            PathBuf::from("/home/x/.config/aihelper")
        );
    }

    #[test]
    fn an_unresolvable_directory_names_the_variable_it_wanted() {
        assert_eq!(
            platform_config_dir(Layout::Windows, &env(&[])),
            Err(Error {
                variable: "APPDATA",
                kind: ErrorKind::Unresolved
            })
        );
        assert_eq!(
            platform_config_dir(Layout::Xdg, &env(&[])),
            Err(Error {
                variable: "HOME",
                kind: ErrorKind::Unresolved
            })
        );
    }

    /// Windows reads `USERPROFILE` and ignores a `HOME` a shell may have set;
    /// everywhere else reads `HOME` and ignores `USERPROFILE`.
    #[test]
    fn the_home_directory_follows_the_platform() {
        let both = [("USERPROFILE", "C:/Users/x"), ("HOME", "/msys/x")];
        assert_eq!(
            home_dir(Layout::Windows, &env(&both)).expect("resolution should succeed"),
            PathBuf::from("C:/Users/x")
        );
        assert_eq!(
            home_dir(Layout::Xdg, &env(&both)).expect("resolution should succeed"),
            PathBuf::from("/msys/x")
        );
        assert_eq!(
            home_dir(Layout::Windows, &env(&[("HOME", "/msys/x")])),
            Err(Error {
                variable: "USERPROFILE",
                kind: ErrorKind::Unresolved
            })
        );
    }

    #[test]
    fn rebasing_leaves_an_absolute_path_alone() {
        let base = Path::new("/base");
        assert_eq!(
            rebase(base, Path::new("child")),
            PathBuf::from("/base/child")
        );
        let absolute = if cfg!(windows) {
            r"C:\elsewhere"
        } else {
            "/elsewhere"
        };
        assert_eq!(rebase(base, Path::new(absolute)), PathBuf::from(absolute));
    }
}
