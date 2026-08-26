//! What `gitlab` publishes to the command catalog: one descriptor per
//! command, with its schemas, declared effects, risk and secret slot.
//!
//! Declaration, not logic - which is why it outgrew the dispatch it used to
//! share a file with.

use super::*;

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        PLUGIN_NAME,
        DOMAIN,
        vec![
            project_descriptor(),
            releases_descriptor(),
            release_get_descriptor(),
            release_create_descriptor(),
            issues_descriptor(),
            issue_view_descriptor(),
            issue_create_descriptor(),
            issue_update_descriptor(),
            issue_close_descriptor(),
            issue_comment_descriptor(),
            issue_comments_descriptor(),
            pipelines_descriptor(),
            pipeline_get_descriptor(),
            pipeline_wait_descriptor(),
            pipeline_jobs_descriptor(),
            job_trace_descriptor(false),
            job_trace_descriptor(true),
        ],
    )
}

pub(super) fn project_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.project",
        "Inspect GitLab project",
        "Detect the GitLab project and return remote plus API metadata.",
        input_schema_for::<Wire<NoArgs>>(),
        output_schema_for::<ProjectOutput>("gitlab.project"),
        read_effects(
            "May run Git project detection and sends a read request to the configured API URL; a supplied token is sent to that host.",
        ),
    )
}

pub(super) fn releases_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.releases",
        "List GitLab releases",
        "List project releases with the shared result limit.",
        input_schema_for::<Wire<NoArgs>>(),
        output_schema_for::<ReleasesOutput>("gitlab.releases"),
        read_effects("Reads release metadata and assets from the configured GitLab API."),
    )
}

pub(super) fn release_get_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.release.get",
        "Get GitLab release",
        "Return one GitLab release by tag.",
        input_schema_for::<Wire<TagArgs>>(),
        output_schema_for::<ReleaseOutput>("gitlab.release.get"),
        read_effects("Reads release metadata and asset links from the configured GitLab API."),
    )
}

pub(super) fn release_create_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.release.create",
        "Create GitLab release",
        "Create a project release with optional description and target reference. Use description or description_file, not both.",
        input_schema_for::<Wire<CreateReleaseArgs>>(),
        output_schema_for::<ReleaseOutput>("gitlab.release.create"),
        write_effects(
            "Creates a persistent release and may create a tag; description files are read from the execution cwd.",
        ),
    )
}

pub(super) fn issues_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.issues",
        "List GitLab issues",
        "List project issues with state, label, author, assignee, date, and search filters.",
        input_schema_for::<Wire<IssuesArgs>>(),
        output_schema_for::<IssuesOutput>("gitlab.issues"),
        read_effects(
            "Reads issue metadata from the configured GitLab API and may expose private project data.",
        ),
    )
}

pub(super) fn issue_view_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.issue.view",
        "View GitLab issue",
        "Return one issue, optionally with comments, designs, and warnings.",
        input_schema_for::<Wire<IssueViewArgs>>(),
        // Hand-written: `gitlab.issue.view` returns one of two payloads
        // depending on `full`, so no single type describes it. Deriving would
        // need an untagged enum, whose `anyOf` is not the published `oneOf`.
        json!({
            "type": "object",
            "oneOf": [
                item_output("gitlab.issue.view", "issue"),
                top_output(
                    "gitlab.issue.view",
                    &[
                        ("project", string_schema()),
                        ("iid", positive_integer_schema()),
                        ("full", json!({"const": true})),
                        ("issue", external_object_schema()),
                        ("comment_count", nonnegative_integer_schema()),
                        ("comments", external_array_schema()),
                        ("design_count", nonnegative_integer_schema()),
                        ("designs", external_array_schema()),
                        ("warnings", json!({"type": "array", "items": string_schema()}))
                    ]
                )
            ]
        }),
        read_effects(
            "Reads issue metadata and, with full=true, comments plus design information through REST and GraphQL.",
        ),
    )
}

pub(super) fn issue_create_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.issue.create",
        "Create GitLab issue",
        "Create a project issue with optional description, labels, and assignees. Use description or description_file, not both.",
        input_schema_for::<Wire<CreateIssueArgs>>(),
        output_schema_for::<IssueOutput>("gitlab.issue.create"),
        write_effects(
            "Creates a persistent issue and may notify project participants; description files are read from the execution cwd.",
        ),
    )
}

pub(super) fn issue_update_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.issue.update",
        "Update GitLab issue",
        "Update one or more fields on a project issue. At least one update field is required; use description or description_file, not both.",
        input_schema_for::<Wire<UpdateIssueArgs>>(),
        output_schema_for::<IssueOutput>("gitlab.issue.update"),
        write_effects(
            "Mutates a persistent issue and may change workflow state or notify participants.",
        ),
    )
}

pub(super) fn issue_close_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.issue.close",
        "Close GitLab issue",
        "Close an issue, optionally adding a comment first. Use comment or comment_file, not both.",
        input_schema_for::<Wire<CloseIssueArgs>>(),
        output_schema_for::<IssueOutput>("gitlab.issue.close"),
        write_effects("May create a note, closes a persistent issue, and may notify participants."),
    )
}

pub(super) fn issue_comment_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.issue.comment",
        "Comment on GitLab issue",
        "Create a note on one project issue. Exactly one of body or body_file is required.",
        input_schema_for::<Wire<CommentIssueArgs>>(),
        output_schema_for::<IssueNoteOutput>("gitlab.issue.comment"),
        write_effects("Creates a persistent issue note and may notify project participants."),
    )
}

pub(super) fn issue_comments_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.issue.comments",
        "List GitLab issue comments",
        "List notes for one project issue with the shared result limit.",
        input_schema_for::<Wire<IssueIidArgs>>(),
        output_schema_for::<IssueNotesOutput>("gitlab.issue.comments"),
        read_effects("Reads issue notes and author metadata from the configured GitLab API."),
    )
}

pub(super) fn pipelines_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.pipelines",
        "List GitLab pipelines",
        "List project pipelines with an optional branch filter.",
        input_schema_for::<Wire<PipelinesArgs>>(),
        output_schema_for::<PipelinesOutput>("gitlab.pipelines"),
        read_effects(
            "Reads pipeline status, commit SHA, references, and URLs from the configured GitLab API.",
        ),
    )
}

pub(super) fn pipeline_get_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.pipeline.get",
        "Get GitLab pipeline",
        "Return one project pipeline by id.",
        input_schema_for::<Wire<PipelineIdArgs>>(),
        output_schema_for::<PipelineOutput>("gitlab.pipeline.get"),
        read_effects(
            "Reads one pipeline and its commit/status metadata from the configured GitLab API.",
        ),
    )
}

pub(super) fn pipeline_wait_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.pipeline.wait",
        "Wait for GitLab pipeline",
        "Poll a pipeline until completion, timeout, or cancellation. pipeline_id is a pipeline id from gitlab.pipelines, not a job id and not a merge request iid.",
        input_schema_for::<Wire<WaitPipelineArgs>>(),
        output_schema_for::<WaitPipelineOutput>("gitlab.pipeline.wait"),
        read_effects("Repeatedly reads external pipeline state and may consume API rate limits."),
    )
    .with_example(CommandExample::new(
        "Wait for a pipeline of a project named outright, which needs no working directory",
        json!({"project": "group/tool", "pipeline_id": 101919, "interval_secs": 30}),
    ))
}

pub(super) fn pipeline_jobs_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.pipeline.jobs",
        "List GitLab pipeline jobs",
        "List jobs belonging to one project pipeline.",
        input_schema_for::<Wire<PipelineIdArgs>>(),
        output_schema_for::<JobsOutput>("gitlab.pipeline.jobs"),
        read_effects(
            "Reads job names, stages, statuses, timestamps, and URLs from the configured GitLab API.",
        ),
    )
}

pub(super) fn job_trace_descriptor(warnings: bool) -> CommandDescriptor {
    let id = if warnings {
        "gitlab.job.warnings"
    } else {
        "gitlab.job.trace"
    };
    descriptor(
        id,
        if warnings {
            "Extract GitLab job warnings"
        } else {
            "Read GitLab job trace"
        },
        if warnings {
            "Extract warning-like lines from one job trace."
        } else {
            "Read or filter one job trace."
        },
        if warnings {
            input_schema_for::<Wire<JobTraceReadArgs>>()
        } else {
            input_schema_for::<Wire<JobTraceArgs>>()
        },
        output_schema_for::<TraceOutput>(id),
        read_effects("Downloads a job trace that may contain secrets or untrusted build output."),
    )
}

/// Every command in this domain talks to the API, so all of them accept the
/// vault token slot.
pub(super) fn token_slot() -> SecretSlot {
    SecretSlot::optional("token", ["gitlab-token"], "GitLab API token.")
}

pub(super) fn read_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        true,
        false,
        true,
        true,
        vec![
            CommandEffect::NetworkRead,
            CommandEffect::ExternalRead,
            CommandEffect::ConfigurationRead,
            CommandEffect::ProcessSpawn,
        ],
        RiskLevel::Medium,
        impact,
        Reversibility::Yes,
    )
}

pub(super) fn write_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        false,
        false,
        false,
        true,
        vec![
            CommandEffect::NetworkWrite,
            CommandEffect::ExternalWrite,
            CommandEffect::ConfigurationRead,
            CommandEffect::FilesystemRead,
            CommandEffect::ProcessSpawn,
        ],
        RiskLevel::High,
        impact,
        Reversibility::Unknown,
    )
}

pub(super) fn string_schema() -> Value {
    json!({"type": "string"})
}

pub(super) fn positive_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 1})
}

pub(super) fn nonnegative_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 0})
}

pub(super) fn external_object_schema() -> Value {
    json!({"type": "object", "additionalProperties": true})
}

pub(super) fn external_array_schema() -> Value {
    json!({"type": "array", "items": external_object_schema()})
}

pub(super) fn top_output(command: &str, fields: &[(&str, Value)]) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "command".to_owned(),
        json!({"type": "string", "const": command}),
    );
    let mut required = vec!["command"];
    for (name, schema) in fields {
        properties.insert((*name).to_owned(), schema.clone());
        required.push(name);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

pub(super) fn item_output(command: &str, item: &str) -> Value {
    top_output(
        command,
        &[
            ("project", string_schema()),
            (item, external_object_schema()),
        ],
    )
}
