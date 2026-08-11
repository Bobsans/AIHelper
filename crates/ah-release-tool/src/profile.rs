use ah_release_manifest::FilePurpose;

pub const PLUGIN_DOMAINS: &[&str] = &["github", "gitlab", "ollama", "postgres"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseProfile {
    pub asset_name: &'static str,
    pub target: &'static str,
    pub architecture: &'static str,
    pub executable: &'static str,
    pub service_worker: Option<&'static str>,
    pub update_helper: Option<&'static str>,
    pub plugin_suffix: &'static str,
}

impl ReleaseProfile {
    pub fn managed_paths(&self) -> Vec<(String, FilePurpose)> {
        let mut paths = Vec::with_capacity(3 + PLUGIN_DOMAINS.len());
        paths.push((self.executable.to_owned(), FilePurpose::Executable));
        if let Some(service_worker) = self.service_worker {
            paths.push((service_worker.to_owned(), FilePurpose::Support));
        }
        if let Some(update_helper) = self.update_helper {
            paths.push((update_helper.to_owned(), FilePurpose::UpdateHelper));
        }
        paths.extend(PLUGIN_DOMAINS.iter().map(|domain| {
            (
                format!("plugins/ah-plugin-{domain}{}", self.plugin_suffix),
                FilePurpose::Plugin,
            )
        }));
        paths.sort_by(|left, right| left.0.cmp(&right.0));
        paths
    }
}

pub const RELEASE_PROFILES: &[ReleaseProfile] = &[
    ReleaseProfile {
        asset_name: "ah-linux-x64.zip",
        target: "x86_64-unknown-linux-gnu",
        architecture: "x86_64",
        executable: "ah",
        service_worker: None,
        update_helper: None,
        plugin_suffix: ".so",
    },
    ReleaseProfile {
        asset_name: "ah-macos-arm64.zip",
        target: "aarch64-apple-darwin",
        architecture: "aarch64",
        executable: "ah",
        service_worker: None,
        update_helper: None,
        plugin_suffix: ".dylib",
    },
    ReleaseProfile {
        asset_name: "ah-windows-x64.zip",
        target: "x86_64-pc-windows-msvc",
        architecture: "x86_64",
        executable: "ah.exe",
        service_worker: Some("ah-mcp-service.exe"),
        update_helper: Some("ah-update-helper.exe"),
        plugin_suffix: ".dll",
    },
];
