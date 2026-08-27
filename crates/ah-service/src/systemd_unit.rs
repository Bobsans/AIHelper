//! The systemd projection: a [`ServiceSpec`] seen as a user unit, and the
//! property-level comparison that decides whether the unit on disk is still
//! the one we wrote.
//!
//! Data, not `systemctl`, so it compiles and is tested on every platform -
//! `systemd_scheduler` is the half that runs the commands. That split is what
//! `windows_task` and `windows_scheduler` do, for the same reason: the policy
//! is worth testing on the machine you happen to be sitting at.
//!
//! **The unit file is the registration.** Windows hands its task definition to
//! the Task Scheduler and reads it back through COM; systemd reads a file we
//! own, so the readback is that file - located through the `FragmentPath` the
//! manager reports, which is what proves the manager loaded *ours*. Two things
//! the file cannot say are read from the manager instead: whether the unit is
//! enabled, and whether the manager's copy is stale (`NeedDaemonReload`).

use std::path::{Path, PathBuf};

use ah_error::AppError;

use crate::{
    model::{DriftEntry, TaskMarker},
    spec::{RestartPolicy, ServiceId, ServiceSpec, StartPolicy},
};

/// One unit per account, because a *user* manager is already per-account -
/// unlike a scheduled task, which is machine-global and so carries the SID in
/// its name.
pub const UNIT_NAME: &str = "aihelper-managed-mcp.service";

/// The section holding AIHelper's ownership marker. systemd ignores a section
/// whose name starts with `X-` entirely, so this is ours to read and nothing
/// else's to complain about.
pub const MARKER_SECTION: &str = "X-AIHelper";

pub const MARKER_KEY: &str = "Marker";

pub const DESCRIPTION: &str = "AIHelper managed MCP server";

/// `StartPolicy::AtLogon` for a user manager: the unit is wanted by the target
/// that comes up when the account's manager starts.
pub const WANTED_BY: &str = "default.target";

/// systemd's own spelling of "no limit".
const NO_RUNTIME_LIMIT: &str = "infinity";

/// The identity of the managed service as systemd keeps it.
pub fn service_id(owner: String) -> ServiceId {
    ServiceId {
        owner,
        path: UNIT_NAME.to_owned(),
    }
}

/// Where the account's user manager reads unit files from.
///
/// This follows systemd's rule rather than AIHelper's, `$XDG_CONFIG_HOME` and
/// all: it locates *systemd's* directory, and a unit written anywhere else is
/// a unit the manager will not see. AIHelper's own state directory is the
/// opposite case and deliberately ignores the environment - see
/// [`crate::paths::ServicePaths::discover`].
pub fn unit_directory() -> Result<PathBuf, AppError> {
    let configuration = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        Some(_) | None => PathBuf::from(std::env::var_os("HOME").ok_or_else(|| {
            AppError::external(
                "MCP_SERVICE_PATH_INVALID",
                "neither XDG_CONFIG_HOME nor HOME is set, so systemd's unit directory \
                 cannot be located",
            )
        })?)
        .join(".config"),
    };
    if !configuration.is_absolute() {
        return Err(AppError::external(
            "MCP_SERVICE_PATH_INVALID",
            format!(
                "systemd's configuration directory '{}' is not absolute",
                configuration.display()
            ),
        ));
    }
    Ok(configuration.join("systemd").join("user"))
}

/// One user unit, as written and as read back.
///
/// Every value is the text systemd stores, not a parsed form of it, so the
/// comparison below is over the same bytes the file holds. The two fields the
/// file cannot carry come from the manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitSpec {
    pub unit_name: String,
    pub marker: TaskMarker,
    pub description: String,
    pub service_type: String,
    pub exec_start: String,
    pub working_directory: String,
    pub restart: String,
    pub runtime_max_sec: String,
    pub wanted_by: String,
    /// Whether the manager will start the unit when the account's manager comes
    /// up. `systemctl enable` writes a symlink; the unit file cannot say it.
    pub enabled: bool,
    /// Whether the manager's loaded copy differs from the file. The Windows
    /// side has no equivalent because the Task Scheduler holds only one copy.
    pub needs_daemon_reload: bool,
}

impl From<&ServiceSpec> for UnitSpec {
    fn from(spec: &ServiceSpec) -> Self {
        let mut command = vec![spec.executable.to_string_lossy().into_owned()];
        command.extend(spec.arguments.iter().cloned());
        Self {
            unit_name: spec.id.path.clone(),
            marker: spec.marker.clone(),
            description: DESCRIPTION.to_owned(),
            // `exec`, not `simple`: the unit is started once the binary has
            // been executed, and readiness is AIHelper's own HTTP probe rather
            // than anything systemd could observe.
            service_type: "exec".to_owned(),
            exec_start: command
                .iter()
                .map(|value| unit_argument(value))
                .collect::<Vec<_>>()
                .join(" "),
            working_directory: unit_argument(&spec.working_directory.to_string_lossy()),
            restart: match spec.restart {
                RestartPolicy::Never => "no".to_owned(),
            },
            runtime_max_sec: match spec.resource.execution_time {
                None => NO_RUNTIME_LIMIT.to_owned(),
                Some(limit) => limit.as_secs().to_string(),
            },
            wanted_by: match spec.start {
                StartPolicy::AtLogon => WANTED_BY.to_owned(),
            },
            enabled: true,
            needs_daemon_reload: false,
        }
    }
}

impl UnitSpec {
    /// The unit file's contents.
    ///
    /// # Errors
    ///
    /// [`AppError`] when the ownership marker cannot be serialized.
    pub fn render(&self) -> Result<String, AppError> {
        let marker = serde_json::to_string(&self.marker)?;
        Ok(format!(
            "[Unit]\n\
             Description={description}\n\
             \n\
             [{MARKER_SECTION}]\n\
             {MARKER_KEY}={marker}\n\
             \n\
             [Service]\n\
             Type={service_type}\n\
             ExecStart={exec_start}\n\
             WorkingDirectory={working_directory}\n\
             Restart={restart}\n\
             RuntimeMaxSec={runtime_max_sec}\n\
             \n\
             [Install]\n\
             WantedBy={wanted_by}\n",
            description = self.description,
            service_type = self.service_type,
            exec_start = self.exec_start,
            working_directory = self.working_directory,
            restart = self.restart,
            runtime_max_sec = self.runtime_max_sec,
            wanted_by = self.wanted_by,
        ))
    }

    /// The unit as the file says it is, or `None` when the file is not one of
    /// ours - no ownership marker, or one that does not parse.
    ///
    /// `enabled` and `needs_daemon_reload` are the manager's answers, not the
    /// file's, so they are supplied by the caller.
    #[must_use]
    pub fn parse(
        text: &str,
        unit_name: &str,
        enabled: bool,
        needs_daemon_reload: bool,
    ) -> Option<Self> {
        let mut section = String::new();
        let mut values: Vec<(String, String, String)> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_owned();
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                values.push((section.clone(), key.trim().to_owned(), value.to_owned()));
            }
        }
        let value = |section: &str, key: &str| {
            values
                .iter()
                .find(|(candidate_section, candidate_key, _)| {
                    candidate_section == section && candidate_key == key
                })
                .map(|(_, _, value)| value.clone())
        };
        let marker =
            serde_json::from_str::<TaskMarker>(&value(MARKER_SECTION, MARKER_KEY)?).ok()?;
        marker.validate().ok()?;
        Some(Self {
            unit_name: unit_name.to_owned(),
            marker,
            description: value("Unit", "Description").unwrap_or_default(),
            service_type: value("Service", "Type").unwrap_or_default(),
            exec_start: value("Service", "ExecStart").unwrap_or_default(),
            working_directory: value("Service", "WorkingDirectory").unwrap_or_default(),
            restart: value("Service", "Restart").unwrap_or_default(),
            runtime_max_sec: value("Service", "RuntimeMaxSec").unwrap_or_default(),
            wanted_by: value("Install", "WantedBy").unwrap_or_default(),
            enabled,
            needs_daemon_reload,
        })
    }
}

/// One `ExecStart` word, quoted the way systemd expects.
///
/// `%` is a specifier to systemd *inside quotes as well*, so it is doubled
/// unconditionally; everything else only needs quoting when the value could
/// otherwise be re-split or read as syntax.
fn unit_argument(value: &str) -> String {
    let escaped = value.replace('%', "%%");
    let needs_quotes = escaped.is_empty()
        || escaped
            .chars()
            .any(|character| character.is_whitespace() || "\"'\\;$".contains(character));
    if !needs_quotes {
        return escaped;
    }
    let quoted = escaped.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{quoted}\"")
}

pub fn unit_path(directory: &Path, unit_name: &str) -> PathBuf {
    directory.join(unit_name)
}

pub fn semantic_drift(desired: &UnitSpec, observed: &UnitSpec) -> Vec<DriftEntry> {
    let mut drift = Vec::new();
    compare(
        &mut drift,
        "unit.name",
        &desired.unit_name,
        &observed.unit_name,
    );
    compare_value(
        &mut drift,
        "registration.data.schema_version",
        desired.marker.schema_version,
        observed.marker.schema_version,
    );
    compare(
        &mut drift,
        "registration.data.owner",
        &desired.marker.owner,
        &observed.marker.owner,
    );
    compare(
        &mut drift,
        "registration.data.kind",
        &desired.marker.kind,
        &observed.marker.kind,
    );
    compare(
        &mut drift,
        "registration.data.service_id",
        &desired.marker.service_id.to_string(),
        &observed.marker.service_id.to_string(),
    );
    compare(
        &mut drift,
        "registration.data.configuration_id",
        &desired.marker.configuration_id.to_string(),
        &observed.marker.configuration_id.to_string(),
    );
    compare(
        &mut drift,
        "registration.data.definition_path",
        &desired.marker.definition_path.to_string_lossy(),
        &observed.marker.definition_path.to_string_lossy(),
    );
    compare(
        &mut drift,
        "unit.description",
        &desired.description,
        &observed.description,
    );
    compare(
        &mut drift,
        "service.type",
        &desired.service_type,
        &observed.service_type,
    );
    compare(
        &mut drift,
        "service.exec_start",
        &desired.exec_start,
        &observed.exec_start,
    );
    compare(
        &mut drift,
        "service.working_directory",
        &desired.working_directory,
        &observed.working_directory,
    );
    compare(
        &mut drift,
        "service.restart",
        &desired.restart,
        &observed.restart,
    );
    compare(
        &mut drift,
        "service.runtime_max_sec",
        &desired.runtime_max_sec,
        &observed.runtime_max_sec,
    );
    compare(
        &mut drift,
        "install.wanted_by",
        &desired.wanted_by,
        &observed.wanted_by,
    );
    compare_value(
        &mut drift,
        "unit_file.enabled",
        desired.enabled,
        observed.enabled,
    );
    compare_value(
        &mut drift,
        "unit_file.needs_daemon_reload",
        desired.needs_daemon_reload,
        observed.needs_daemon_reload,
    );
    drift.sort_by(|left, right| left.field.cmp(&right.field));
    drift
}

fn compare(drift: &mut Vec<DriftEntry>, field: &str, expected: &str, actual: &str) {
    if expected != actual {
        drift.push(DriftEntry::mismatch(field, expected, actual));
    }
}

fn compare_value<T: std::fmt::Debug + PartialEq>(
    drift: &mut Vec<DriftEntry>,
    field: &str,
    expected: T,
    actual: T,
) {
    if expected != actual {
        drift.push(DriftEntry::mismatch(
            field,
            format!("{expected:?}").to_lowercase(),
            format!("{actual:?}").to_lowercase(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// The projection is data, so it is tested everywhere - but a marker only
    /// validates with an absolute definition path, and what counts as absolute
    /// is the host's business.
    fn platform_path(value: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("C:{}", value.replace('/', "\\")))
        } else {
            PathBuf::from(value)
        }
    }

    fn spec(definition: &str, executable: &str, working_directory: &str) -> ServiceSpec {
        let marker = TaskMarker {
            schema_version: 1,
            owner: "AIHelper".to_owned(),
            kind: "managed_mcp".to_owned(),
            service_id: Uuid::nil(),
            configuration_id: Uuid::nil(),
            definition_path: platform_path(definition),
        };
        ServiceSpec::managed_mcp(
            service_id("1000".to_owned()),
            marker,
            platform_path(executable),
            platform_path(working_directory),
        )
    }

    fn desired() -> UnitSpec {
        UnitSpec::from(&spec(
            "/state/definitions/0.json",
            "/opt/aihelper/ah",
            "/work",
        ))
    }

    /// The exact bytes of a unit only matter where systemd reads them, so the
    /// byte-level assertions use real POSIX paths rather than a host-shaped
    /// stand-in. The properties that hold everywhere are tested below.
    #[cfg(unix)]
    #[test]
    fn the_unit_says_what_the_spec_asked_for() {
        let unit = desired();
        assert_eq!(unit.unit_name, UNIT_NAME);
        assert_eq!(
            unit.exec_start,
            "/opt/aihelper/ah mcp serve --transport http --managed-config /state/definitions/0.json"
        );
        assert_eq!(unit.working_directory, "/work");
        assert_eq!(unit.service_type, "exec");
        assert_eq!(unit.restart, "no");
        assert_eq!(unit.runtime_max_sec, "infinity");
        assert_eq!(unit.wanted_by, WANTED_BY);
        assert!(unit.enabled);
        assert!(!unit.needs_daemon_reload);
    }

    #[cfg(unix)]
    #[test]
    fn paths_that_would_be_re_read_as_syntax_are_quoted() {
        let unit = UnitSpec::from(&spec(
            "/state/def with space.json",
            "/opt/ah 50%/ah",
            "/work dir",
        ));
        assert_eq!(
            unit.exec_start,
            "\"/opt/ah 50%%/ah\" mcp serve --transport http --managed-config \"/state/def with space.json\""
        );
        assert_eq!(unit.working_directory, "\"/work dir\"");
    }

    /// systemd reads `%` as a specifier even inside quotes, and splits on
    /// whitespace outside them.
    #[test]
    fn a_word_is_quoted_only_when_it_could_be_re_read_as_syntax() {
        assert_eq!(unit_argument("plain"), "plain");
        assert_eq!(unit_argument("--managed-config"), "--managed-config");
        assert_eq!(unit_argument("with space"), "\"with space\"");
        assert_eq!(unit_argument("50%"), "50%%");
        assert_eq!(unit_argument(r"a\b"), r#""a\\b""#);
        assert_eq!(unit_argument("say\"hi\""), "\"say\\\"hi\\\"\"");
        assert_eq!(unit_argument("semi;colon"), "\"semi;colon\"");
        assert_eq!(unit_argument(""), "\"\"");
    }

    /// The unit that is written has to read back as the same unit whatever the
    /// host spells its paths like - otherwise every install would report drift
    /// against itself.
    #[test]
    fn a_rendered_unit_reads_back_unchanged() {
        for unit in [
            desired(),
            UnitSpec::from(&spec(
                "/state/def with space.json",
                "/opt/ah 50%/ah",
                "/work dir",
            )),
        ] {
            let text = unit.render().expect("the marker serializes");
            let parsed = UnitSpec::parse(&text, UNIT_NAME, true, false).expect("the unit is ours");
            assert_eq!(parsed, unit);
            assert!(semantic_drift(&unit, &parsed).is_empty());
        }
    }

    #[test]
    fn a_unit_without_our_marker_is_not_ours() {
        let text = "[Unit]\nDescription=someone else\n\n[Service]\nExecStart=/bin/true\n";
        assert!(UnitSpec::parse(text, UNIT_NAME, true, false).is_none());

        let broken = format!("[{MARKER_SECTION}]\n{MARKER_KEY}=not json\n");
        assert!(UnitSpec::parse(&broken, UNIT_NAME, true, false).is_none());
    }

    #[test]
    fn drift_is_property_level_and_sorted() {
        let expected = desired();
        let mut actual = expected.clone();
        actual.restart = "always".to_owned();
        actual.enabled = false;
        actual.needs_daemon_reload = true;
        let drift = semantic_drift(&expected, &actual)
            .into_iter()
            .map(|entry| entry.field)
            .collect::<Vec<_>>();
        assert_eq!(
            drift,
            [
                "service.restart",
                "unit_file.enabled",
                "unit_file.needs_daemon_reload"
            ]
        );
    }

    /// A manager whose loaded copy is stale is running something other than
    /// what the file says, which is drift even though the file is right.
    #[test]
    fn a_stale_manager_is_drift_on_its_own() {
        let expected = desired();
        let mut actual = expected.clone();
        actual.needs_daemon_reload = true;
        assert_eq!(semantic_drift(&expected, &actual).len(), 1);
    }
}
