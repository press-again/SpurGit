//! Application settings store.
//!
//! One JSON file under the platform config directory (`%APPDATA%\SpurGit` on
//! Windows, `~/Library/Application Support/SpurGit` on macOS, `~/.config/spurgit` elsewhere). It holds the selected theme, the
//! managed scan roots, and any unknown keys — unknown keys are preserved on
//! save so a setting written by another version is not destroyed.
//!
//! Reads are forgiving: a malformed file yields defaults plus a diagnostic
//! instead of an invisible failure. Writes are atomic (sibling temp file,
//! then rename), so a failed save leaves the previous readable file intact.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::accounts::Profile;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub theme: Option<String>,
    pub roots: Vec<String>,
    /// Repository tabs open at the last shutdown, in strip order.
    pub open_repos: Vec<String>,
    /// Path of the tab that was active, when it is still in `open_repos`.
    pub active_repo: Option<String>,
    /// Absolute Linux paths that discovery skips (workspace exclusions).
    pub exclude: Vec<String>,
    /// Pinned branches as `repo-path<US>branch` entries; shown above the
    /// branch tree. The unit separator keeps repo paths and branch names
    /// unambiguous without a nested structure.
    pub pinned_branches: Vec<String>,
    /// Local Branches sort order.
    pub branch_sort: BranchSort,
    /// Bounded auto-refresh of the workspace. Defaults to on.
    pub auto_refresh: bool,
    /// File-list column width in logical pixels. Defaults to 300,
    /// clamped to 220–800 on load and on drag.
    pub changes_list_w: f32,
    /// Repository sidebar width in logical pixels. Defaults to 200,
    /// clamped to 160–480 on load and on drag.
    pub sidebar_w: f32,
    /// Share of the history column the commit details panel takes.
    /// Defaults to half, clamped to 0.2–0.8.
    pub commit_panel_ratio: f32,
    /// Soft-wrap long diff lines instead of horizontal scrolling.
    /// Defaults to off.
    pub diff_wrap: bool,
    /// How long the bottom-left error alert stays before dismissing itself.
    pub alert_timeout: AlertTimeout,
    /// External client invocation: executable plus literal arguments; the
    /// repository path is appended as the final argument.
    pub external_client: Option<ExternalClient>,
    /// False until the first root is added or the first repository opens:
    /// the empty state keeps its welcome line and the
    /// character winks once. Defaulting to false means an existing install
    /// sees the welcome once after upgrading.
    pub welcomed: bool,
    /// Commit identities with their GitHub account; see [`crate::accounts`].
    pub profiles: Vec<Profile>,
    /// Label of the profile used where nothing more specific applies.
    pub default_profile: Option<String>,
    /// Profile picked per repository path; an empty label means "use Git's
    /// own config".
    pub repo_profiles: BTreeMap<String, String>,
    /// Keys written by other or newer versions; preserved on save.
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    /// Auto-refresh is opt-out, so the default is on (a derive would be off).
    fn default() -> Self {
        Self {
            theme: None,
            roots: Vec::new(),
            open_repos: Vec::new(),
            active_repo: None,
            exclude: Vec::new(),
            pinned_branches: Vec::new(),
            branch_sort: BranchSort::default(),
            auto_refresh: true,
            changes_list_w: default_changes_list_w(),
            sidebar_w: SIDEBAR_W_DEFAULT,
            commit_panel_ratio: COMMIT_PANEL_RATIO_DEFAULT,
            diff_wrap: false,
            alert_timeout: AlertTimeout::default(),
            external_client: None,
            welcomed: false,
            profiles: Vec::new(),
            default_profile: None,
            repo_profiles: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

/// Local Branches sort order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BranchSort {
    /// Alphabetical, folders first (existing behavior).
    #[default]
    Name,
    /// Most recently committed first.
    Recent,
}

impl BranchSort {
    /// Stable key used by settings.json and the sort chip.
    pub fn key(self) -> &'static str {
        match self {
            BranchSort::Name => "name",
            BranchSort::Recent => "recent",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "name" => Some(BranchSort::Name),
            "recent" => Some(BranchSort::Recent),
            _ => None,
        }
    }
}

/// Separator between the repo path and the branch name in a pin entry.
pub const PIN_SEPARATOR: char = '\u{1f}';

/// Default file-list column width, and its clamp bounds.
pub const CHANGES_LIST_W_DEFAULT: f32 = 300.0;
pub const CHANGES_LIST_W_MIN: f32 = 220.0;
pub const CHANGES_LIST_W_MAX: f32 = 800.0;

/// Default width for a fresh or malformed value.
pub fn default_changes_list_w() -> f32 {
    CHANGES_LIST_W_DEFAULT
}

/// Clamp a dragged or loaded width into the supported range.
pub fn clamp_changes_list_w(w: f32) -> f32 {
    if !w.is_finite() {
        return CHANGES_LIST_W_DEFAULT;
    }
    w.clamp(CHANGES_LIST_W_MIN, CHANGES_LIST_W_MAX)
}

/// Default sidebar width, and its clamp bounds.
pub const SIDEBAR_W_DEFAULT: f32 = 200.0;
pub const SIDEBAR_W_MIN: f32 = 160.0;
pub const SIDEBAR_W_MAX: f32 = 480.0;

pub fn clamp_sidebar_w(w: f32) -> f32 {
    if !w.is_finite() {
        return SIDEBAR_W_DEFAULT;
    }
    w.clamp(SIDEBAR_W_MIN, SIDEBAR_W_MAX)
}

/// Default commit-panel share of the history column, and its clamp bounds.
pub const COMMIT_PANEL_RATIO_DEFAULT: f32 = 0.5;
pub const COMMIT_PANEL_RATIO_MIN: f32 = 0.2;
pub const COMMIT_PANEL_RATIO_MAX: f32 = 0.8;

pub fn clamp_commit_panel_ratio(ratio: f32) -> f32 {
    if !ratio.is_finite() {
        return COMMIT_PANEL_RATIO_DEFAULT;
    }
    ratio.clamp(COMMIT_PANEL_RATIO_MIN, COMMIT_PANEL_RATIO_MAX)
}

/// Split a pin entry back into its repo path and branch name.
pub fn split_pin(entry: &str) -> Option<(&str, &str)> {
    entry.split_once(PIN_SEPARATOR)
}

/// Make a pin entry from a repo path and a branch name.
pub fn join_pin(repo: &str, branch: &str) -> String {
    format!("{repo}{PIN_SEPARATOR}{branch}")
}

/// How long the transient bottom-left error alert stays visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlertTimeout {
    #[default]
    ThreeSeconds,
    FiveSeconds,
    TenSeconds,
    /// Stays until its close button is clicked.
    Manual,
}

impl AlertTimeout {
    pub const ALL: [AlertTimeout; 4] = [
        AlertTimeout::ThreeSeconds,
        AlertTimeout::FiveSeconds,
        AlertTimeout::TenSeconds,
        AlertTimeout::Manual,
    ];

    /// Stable key used by settings.json and the settings control.
    pub fn key(self) -> &'static str {
        match self {
            AlertTimeout::ThreeSeconds => "3s",
            AlertTimeout::FiveSeconds => "5s",
            AlertTimeout::TenSeconds => "10s",
            AlertTimeout::Manual => "manual",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.key() == key)
    }

    /// Auto-dismiss delay; `None` for the click-to-remove mode.
    pub fn duration(self) -> Option<std::time::Duration> {
        match self {
            AlertTimeout::ThreeSeconds => Some(std::time::Duration::from_secs(3)),
            AlertTimeout::FiveSeconds => Some(std::time::Duration::from_secs(5)),
            AlertTimeout::TenSeconds => Some(std::time::Duration::from_secs(10)),
            AlertTimeout::Manual => None,
        }
    }
}

/// One WSL-aware external handoff: the executable and its literal arguments.
/// The repository path is appended last, never interpolated into a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalClient {
    pub program: String,
    pub args: Vec<String>,
}

impl ExternalClient {
    pub fn command_for(&self, path: &str) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args).arg(path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // A console client (e.g. wsl.exe) must not flash a cmd window when
            // the handoff is spawned from the GUI process.
            command.creation_flags(0x0800_0000);
        }
        command
    }
}

/// Platform config directory for SpurGit (`%APPDATA%\SpurGit` on Windows,
/// `~/Library/Application Support/SpurGit` on macOS, `~/.config/spurgit`
/// elsewhere).
pub fn data_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("SpurGit");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        return home.join("Library").join("Application Support").join("SpurGit");
    }
    home.join(".config").join("spurgit")
}

pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// Load settings, returning diagnostics for every problem instead of hiding
/// it. A missing file is normal; a malformed one yields defaults.
pub fn load_from(dir: &Path) -> (Settings, Vec<String>) {
    let path = settings_path(dir);
    match std::fs::read_to_string(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            (Settings::default(), Vec::new())
        }
        Err(err) => (
            Settings::default(),
            vec![format!("could not read {}: {err}", path.display())],
        ),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => settings_from_value(value),
            Err(err) => (
                Settings::default(),
                vec![format!("{} is not valid JSON: {err}", path.display())],
            ),
        },
    }
}

pub fn load() -> (Settings, Vec<String>) {
    load_from(&data_dir())
}

/// Atomic save: write a sibling temp file, then rename over the target.
/// On failure the previous file is untouched and the error is returned.
pub fn save_to(dir: &Path, settings: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let path = settings_path(dir);
    let tmp = dir.join("settings.json.tmp");
    let json = serde_json::to_string_pretty(&settings_to_value(settings))
        .map_err(|e| format!("could not encode settings: {e}"))?;
    std::fs::write(&tmp, json).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("could not replace {}: {e}", path.display()))
}

pub fn save(settings: &Settings) -> Result<(), String> {
    save_to(&data_dir(), settings)
}

pub fn theme_name() -> Option<String> {
    load().0.theme.filter(|name| !name.is_empty())
}

pub fn set_theme_name(name: &str) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.theme = Some(name.to_string());
    save(&settings)
}

pub fn set_roots(roots: &[String]) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.roots = roots.to_vec();
    save(&settings)
}

/// Persist the open repository tabs and which one is active.
pub fn set_open_repos(repos: &[String], active: Option<&str>) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.open_repos = repos.to_vec();
    settings.active_repo = active.map(str::to_string);
    save(&settings)
}

pub fn set_exclude(exclude: &[String]) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.exclude = exclude.to_vec();
    save(&settings)
}

/// Persist the pinned branches and the branch sort order.
pub fn set_pinned_branches(pinned: &[String]) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.pinned_branches = pinned.to_vec();
    save(&settings)
}

pub fn set_branch_sort(sort: BranchSort) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.branch_sort = sort;
    save(&settings)
}

pub fn set_auto_refresh(enabled: bool) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.auto_refresh = enabled;
    save(&settings)
}

/// Remember that the first-run welcome has been seen.
pub fn set_welcomed(welcomed: bool) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.welcomed = welcomed;
    save(&settings)
}

pub fn set_alert_timeout(mode: AlertTimeout) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.alert_timeout = mode;
    save(&settings)
}

pub fn set_changes_list_w(w: f32) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.changes_list_w = clamp_changes_list_w(w);
    save(&settings)
}

pub fn set_sidebar_w(w: f32) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.sidebar_w = clamp_sidebar_w(w);
    save(&settings)
}

pub fn set_commit_panel_ratio(ratio: f32) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.commit_panel_ratio = clamp_commit_panel_ratio(ratio);
    save(&settings)
}

pub fn set_diff_wrap(enabled: bool) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.diff_wrap = enabled;
    save(&settings)
}

/// Persist an external client configuration to settings.json.
#[allow(dead_code)]
pub fn set_external_client(client: Option<ExternalClient>) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.external_client = client;
    save(&settings)
}

/// Persist the profiles, the default one, and the per-repository picks.
pub fn set_accounts(
    profiles: &[Profile],
    default: Option<&str>,
    picks: &BTreeMap<String, String>,
) -> Result<(), String> {
    let (mut settings, _) = load();
    settings.profiles = profiles.to_vec();
    settings.default_profile = default.map(str::to_string);
    settings.repo_profiles = picks.clone();
    save(&settings)
}

fn settings_from_value(value: Value) -> (Settings, Vec<String>) {
    let mut diagnostics = Vec::new();
    let Value::Object(mut map) = value else {
        diagnostics.push("settings file is not a JSON object".to_string());
        return (Settings::default(), diagnostics);
    };
    let theme = match map.remove("theme") {
        None => None,
        Some(Value::String(name)) => Some(name),
        Some(other) => {
            diagnostics.push(format!("\"theme\" must be a string, found {other}"));
            None
        }
    };
    let roots = match map.remove("roots") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut roots = Vec::new();
            for item in items {
                match item {
                    Value::String(root) => roots.push(root),
                    other => diagnostics.push(format!(
                        "scan roots must be strings, found {other}"
                    )),
                }
            }
            roots
        }
        Some(other) => {
            diagnostics.push(format!("\"roots\" must be an array, found {other}"));
            Vec::new()
        }
    };
    let open_repos = match map.remove("open_repos") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut repos = Vec::new();
            for item in items {
                match item {
                    Value::String(path) if !path.is_empty() => repos.push(path),
                    other => diagnostics.push(format!(
                        "open repository tabs must be non-empty strings, found {other}"
                    )),
                }
            }
            repos
        }
        Some(other) => {
            diagnostics.push(format!("\"open_repos\" must be an array, found {other}"));
            Vec::new()
        }
    };
    let active_repo = match map.remove("active_repo") {
        None => None,
        Some(Value::String(path)) if !path.is_empty() => Some(path),
        Some(other) => {
            diagnostics.push(format!("\"active_repo\" must be a non-empty string, found {other}"));
            None
        }
    };
    let exclude = match map.remove("exclude") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut exclude = Vec::new();
            for item in items {
                match item {
                    Value::String(path) => exclude.push(path),
                    other => diagnostics
                        .push(format!("excluded paths must be strings, found {other}")),
                }
            }
            exclude
        }
        Some(other) => {
            diagnostics.push(format!("\"exclude\" must be an array, found {other}"));
            Vec::new()
        }
    };
    let pinned_branches = match map.remove("pinned_branches") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut pinned = Vec::new();
            for item in items {
                match item {
                    Value::String(entry) => pinned.push(entry),
                    other => diagnostics
                        .push(format!("pinned branches must be strings, found {other}")),
                }
            }
            pinned
        }
        Some(other) => {
            diagnostics.push(format!("\"pinned_branches\" must be an array, found {other}"));
            Vec::new()
        }
    };
    let branch_sort = match map.remove("branch_sort") {
        None => BranchSort::default(),
        Some(Value::String(key)) => match BranchSort::from_key(&key) {
            Some(sort) => sort,
            None => {
                diagnostics.push(format!(
                    "\"branch_sort\" must be one of \"name\", \"recent\", found \"{key}\""
                ));
                BranchSort::default()
            }
        },
        Some(other) => {
            diagnostics.push(format!("\"branch_sort\" must be a string, found {other}"));
            BranchSort::default()
        }
    };
    let auto_refresh = match map.remove("auto_refresh") {
        None => true,
        Some(Value::Bool(enabled)) => enabled,
        Some(other) => {
            diagnostics.push(format!("\"auto_refresh\" must be a boolean, found {other}"));
            true
        }
    };
    let mut number = |key: &str, default: f32, clamp: fn(f32) -> f32| match map.remove(key) {
        None => default,
        Some(value) => match value.as_f64() {
            Some(n) => clamp(n as f32),
            None => {
                diagnostics.push(format!("\"{key}\" must be a number, found {value}"));
                default
            }
        },
    };
    let changes_list_w = number("changes_list_w", CHANGES_LIST_W_DEFAULT, clamp_changes_list_w);
    let sidebar_w = number("sidebar_w", SIDEBAR_W_DEFAULT, clamp_sidebar_w);
    let commit_panel_ratio = number(
        "commit_panel_ratio",
        COMMIT_PANEL_RATIO_DEFAULT,
        clamp_commit_panel_ratio,
    );
    let diff_wrap = match map.remove("diff_wrap") {
        None => false,
        Some(Value::Bool(enabled)) => enabled,
        Some(other) => {
            diagnostics.push(format!("\"diff_wrap\" must be a boolean, found {other}"));
            false
        }
    };
    let welcomed = match map.remove("welcomed") {
        None => false,
        Some(Value::Bool(welcomed)) => welcomed,
        Some(other) => {
            diagnostics.push(format!("\"welcomed\" must be a boolean, found {other}"));
            false
        }
    };
    let alert_timeout = match map.remove("alert_timeout") {
        None => AlertTimeout::default(),
        Some(Value::String(key)) => match AlertTimeout::from_key(&key) {
            Some(mode) => mode,
            None => {
                diagnostics.push(format!(
                    "\"alert_timeout\" must be one of \"3s\", \"5s\", \"10s\", \"manual\", found \"{key}\""
                ));
                AlertTimeout::default()
            }
        },
        Some(other) => {
            diagnostics.push(format!("\"alert_timeout\" must be a string, found {other}"));
            AlertTimeout::default()
        }
    };
    let external_client = match map.remove("external_client") {
        None => None,
        Some(Value::Object(mut entry)) => {
            let program = match entry.remove("program") {
                Some(Value::String(program)) if !program.is_empty() => Some(program),
                Some(other) => {
                    diagnostics.push(format!(
                        "external client \"program\" must be a non-empty string, found {other}"
                    ));
                    None
                }
                None => {
                    diagnostics.push("external client has no \"program\"".to_string());
                    None
                }
            };
            let args = match entry.remove("args") {
                None => Vec::new(),
                Some(Value::Array(items)) => {
                    let mut args = Vec::new();
                    for item in items {
                        match item {
                            Value::String(arg) => args.push(arg),
                            other => diagnostics
                                .push(format!("external client arguments must be strings, found {other}")),
                        }
                    }
                    args
                }
                Some(other) => {
                    diagnostics.push(format!("external client \"args\" must be an array, found {other}"));
                    Vec::new()
                }
            };
            program.map(|program| ExternalClient { program, args })
        }
        Some(other) => {
            diagnostics.push(format!("\"external_client\" must be an object, found {other}"));
            None
        }
    };
    let profiles = match map.remove("profiles") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut profiles: Vec<Profile> = Vec::new();
            for item in items {
                let field = |key: &str| {
                    item.get(key)
                        .and_then(Value::as_str)
                        .map(|text| text.trim().to_string())
                        .unwrap_or_default()
                };
                let profile = Profile {
                    label: field("label"),
                    name: field("name"),
                    email: field("email"),
                    github: Some(field("github")).filter(|login| !login.is_empty()),
                };
                if profiles.iter().any(|known| known.label == profile.label) {
                    diagnostics.push(format!("profile \"{}\" is listed twice", profile.label));
                } else if let Err(issue) = profile.validate() {
                    diagnostics.push(format!(
                        "profile \"{}\" ignored: invalid {issue:?}",
                        profile.label
                    ));
                } else {
                    profiles.push(profile);
                }
            }
            profiles
        }
        Some(other) => {
            diagnostics.push(format!("\"profiles\" must be an array, found {other}"));
            Vec::new()
        }
    };
    let default_profile = match map.remove("default_profile") {
        None => None,
        Some(Value::String(label)) if profiles.iter().any(|profile| profile.label == label) => {
            Some(label)
        }
        Some(other) => {
            diagnostics.push(format!("\"default_profile\" is not a known profile: {other}"));
            None
        }
    };
    let repo_profiles = match map.remove("repo_profiles") {
        None => BTreeMap::new(),
        Some(Value::Object(entries)) => entries
            .into_iter()
            .filter_map(|(repo, label)| match label {
                Value::String(label) => Some((repo, label)),
                other => {
                    diagnostics.push(format!("\"repo_profiles\" values must be strings, found {other}"));
                    None
                }
            })
            .collect(),
        Some(other) => {
            diagnostics.push(format!("\"repo_profiles\" must be an object, found {other}"));
            BTreeMap::new()
        }
    };
    (
        Settings {
            theme,
            roots,
            open_repos,
            active_repo,
            exclude,
            pinned_branches,
            branch_sort,
            auto_refresh,
            changes_list_w,
            sidebar_w,
            commit_panel_ratio,
            diff_wrap,
            alert_timeout,
            external_client,
            welcomed,
            profiles,
            default_profile,
            repo_profiles,
            extra: map,
        },
        diagnostics,
    )
}

fn settings_to_value(settings: &Settings) -> Value {
    let mut map = settings.extra.clone();
    if let Some(theme) = &settings.theme {
        map.insert("theme".to_string(), Value::String(theme.clone()));
    } else {
        map.remove("theme");
    }
    map.insert(
        "roots".to_string(),
        Value::Array(settings.roots.iter().cloned().map(Value::String).collect()),
    );
    if !settings.open_repos.is_empty() {
        map.insert(
            "open_repos".to_string(),
            Value::Array(
                settings
                    .open_repos
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    } else {
        map.remove("open_repos");
    }
    match &settings.active_repo {
        Some(path) => {
            map.insert("active_repo".to_string(), Value::String(path.clone()));
        }
        None => {
            map.remove("active_repo");
        }
    }
    if !settings.exclude.is_empty() {
        map.insert(
            "exclude".to_string(),
            Value::Array(settings.exclude.iter().cloned().map(Value::String).collect()),
        );
    } else {
        map.remove("exclude");
    }
    if !settings.pinned_branches.is_empty() {
        map.insert(
            "pinned_branches".to_string(),
            Value::Array(
                settings
                    .pinned_branches
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    } else {
        map.remove("pinned_branches");
    }
    map.insert(
        "branch_sort".to_string(),
        Value::String(settings.branch_sort.key().to_string()),
    );
    map.insert("auto_refresh".to_string(), Value::Bool(settings.auto_refresh));
    for (key, value) in [
        ("changes_list_w", settings.changes_list_w),
        ("sidebar_w", settings.sidebar_w),
        ("commit_panel_ratio", settings.commit_panel_ratio),
    ] {
        // Rounded so an f32 ratio is not written as 0.20000000298…; always
        // finite (clamped on load and on save), so the number always exists.
        let value = (value as f64 * 1000.0).round() / 1000.0;
        if let Some(n) = serde_json::Number::from_f64(value) {
            map.insert(key.to_string(), Value::Number(n));
        }
    }
    map.insert("diff_wrap".to_string(), Value::Bool(settings.diff_wrap));
    map.insert("welcomed".to_string(), Value::Bool(settings.welcomed));
    map.insert(
        "alert_timeout".to_string(),
        Value::String(settings.alert_timeout.key().to_string()),
    );
    if settings.profiles.is_empty() {
        map.remove("profiles");
    } else {
        let profiles = settings.profiles.iter().map(|profile| {
            let mut entry = Map::new();
            entry.insert("label".to_string(), Value::String(profile.label.clone()));
            entry.insert("name".to_string(), Value::String(profile.name.clone()));
            entry.insert("email".to_string(), Value::String(profile.email.clone()));
            if let Some(login) = &profile.github {
                entry.insert("github".to_string(), Value::String(login.clone()));
            }
            Value::Object(entry)
        });
        map.insert("profiles".to_string(), Value::Array(profiles.collect()));
    }
    match &settings.default_profile {
        Some(label) => {
            map.insert("default_profile".to_string(), Value::String(label.clone()));
        }
        None => {
            map.remove("default_profile");
        }
    }
    if settings.repo_profiles.is_empty() {
        map.remove("repo_profiles");
    } else {
        let entries = settings
            .repo_profiles
            .iter()
            .map(|(repo, label)| (repo.clone(), Value::String(label.clone())))
            .collect();
        map.insert("repo_profiles".to_string(), Value::Object(entries));
    }
    match &settings.external_client {
        Some(client) => {
            let mut entry = Map::new();
            entry.insert("program".to_string(), Value::String(client.program.clone()));
            entry.insert(
                "args".to_string(),
                Value::Array(client.args.iter().cloned().map(Value::String).collect()),
            );
            map.insert("external_client".to_string(), Value::Object(entry));
        }
        None => {
            map.remove("external_client");
        }
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "spur-settings-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn sample() -> Settings {
        Settings {
            theme: Some("Spur Dark Custom".into()),
            roots: vec!["/home/me/dev".into(), "/tmp/a:b".into()],
            open_repos: vec!["/home/me/dev/app".into(), "/tmp/b".into()],
            active_repo: Some("/tmp/b".into()),
            exclude: vec!["/home/me/dev/scratch".into()],
            pinned_branches: vec![join_pin("/home/me/dev/app", "feature/x")],
            branch_sort: BranchSort::Recent,
            auto_refresh: true,
            changes_list_w: 320.0,
            sidebar_w: 240.0,
            commit_panel_ratio: 0.35,
            diff_wrap: true,
            alert_timeout: AlertTimeout::TenSeconds,
            external_client: Some(ExternalClient {
                program: "wsl.exe".into(),
                args: vec!["-d".into(), "Ubuntu-26.04".into(), "-e".into(), "code".into()],
            }),
            welcomed: true,
            profiles: vec![
                Profile {
                    label: "Work".into(),
                    name: "Ada Lovelace".into(),
                    email: "ada@example.com".into(),
                    github: Some("ada-work".into()),
                },
                Profile {
                    label: "Local".into(),
                    name: "Ada".into(),
                    email: "ada@home.example".into(),
                    github: None,
                },
            ],
            default_profile: Some("Local".into()),
            repo_profiles: [
                ("/home/me/dev/app".to_string(), "Work".to_string()),
                ("/home/me/dev/plain".to_string(), String::new()),
            ]
            .into_iter()
            .collect(),
            extra: [("future_key".to_string(), Value::from(7))]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn round_trips_and_preserves_unknown_keys() {
        let dir = temp_dir("roundtrip");
        save_to(&dir, &sample()).unwrap();
        let (loaded, diagnostics) = load_from(&dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(loaded, sample());

        // A later save must not drop the unknown key.
        let (mut edited, _) = load_from(&dir);
        edited.roots = vec!["/srv/repos".into()];
        save_to(&dir, &edited).unwrap();
        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        assert!(text.contains("future_key"), "{text}");
        let (reloaded, _) = load_from(&dir);
        assert_eq!(reloaded.roots, vec!["/srv/repos"]);
        assert_eq!(reloaded.theme.as_deref(), Some("Spur Dark Custom"));
    }

    #[test]
    fn save_replaces_an_existing_file() {
        let dir = temp_dir("replace");
        save_to(&dir, &sample()).unwrap();
        let mut second = sample();
        second.roots = vec!["/new".into()];
        save_to(&dir, &second).unwrap();
        let (loaded, _) = load_from(&dir);
        assert_eq!(loaded.roots, vec!["/new"]);
    }

    #[test]
    fn bad_profiles_are_dropped_with_a_diagnostic() {
        let dir = temp_dir("profiles");
        std::fs::write(
            settings_path(&dir),
            r#"{
                "profiles": [
                    {"label": "ok", "name": "Ada", "email": "ada@example.com", "github": "ada-1"},
                    {"label": "ok", "name": "Twin", "email": "twin@example.com"},
                    {"label": "no-mail", "name": "Ada", "email": "nope"},
                    {"label": "bad-login", "name": "Ada", "email": "a@b.co", "github": "-x"},
                    {"name": "No label", "email": "a@b.co"},
                    5
                ],
                "default_profile": "missing",
                "repo_profiles": {"/r": "ok", "/s": 3}
            }"#,
        )
        .unwrap();
        let (settings, diagnostics) = load_from(&dir);
        let labels: Vec<_> = settings.profiles.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["ok"], "{diagnostics:?}");
        assert_eq!(settings.profiles[0].github.as_deref(), Some("ada-1"));
        assert_eq!(settings.default_profile, None);
        assert_eq!(settings.repo_profiles.len(), 1);
        assert_eq!(diagnostics.len(), 7, "{diagnostics:?}");
    }

    #[test]
    fn malformed_files_are_reported_not_hidden() {
        let dir = temp_dir("malformed");
        std::fs::write(settings_path(&dir), "{ not json").unwrap();
        let (settings, diagnostics) = load_from(&dir);
        assert_eq!(settings, Settings::default());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].contains("not valid JSON"), "{diagnostics:?}");

        std::fs::write(settings_path(&dir), r#"{"theme": 5, "roots": "nope"}"#).unwrap();
        let (settings, diagnostics) = load_from(&dir);
        assert_eq!(settings.theme, None);
        assert!(settings.roots.is_empty());
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");

        std::fs::write(settings_path(&dir), r#"{"roots": ["/ok", 7]}"#).unwrap();
        let (settings, diagnostics) = load_from(&dir);
        assert_eq!(settings.roots, vec!["/ok"]);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    }

    #[test]
    fn pins_and_sort_round_trip_and_reject_bad_shapes() {
        let dir = temp_dir("pins");
        let settings = Settings {
            pinned_branches: vec![join_pin("/a", "main"), join_pin("/b", "x/y")],
            branch_sort: BranchSort::Recent,
            ..Settings::default()
        };
        save_to(&dir, &settings).unwrap();
        let (loaded, diagnostics) = load_from(&dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(loaded.pinned_branches, settings.pinned_branches);
        assert_eq!(loaded.branch_sort, BranchSort::Recent);
        assert_eq!(split_pin(&loaded.pinned_branches[1]), Some(("/b", "x/y")));

        // Empty pins clear the key; an unknown sort falls back with a note.
        save_to(&dir, &Settings::default()).unwrap();
        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        assert!(!text.contains("pinned_branches"), "{text}");

        std::fs::write(
            settings_path(&dir),
            r#"{"pinned_branches": "nope", "branch_sort": "fancy"}"#,
        )
        .unwrap();
        let (settings, diagnostics) = load_from(&dir);
        assert!(settings.pinned_branches.is_empty());
        assert_eq!(settings.branch_sort, BranchSort::Name);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    }

    #[test]
    fn open_repos_round_trip_through_the_store_and_reject_bad_shapes() {
        let dir = temp_dir("tabs");
        set_open_repos_to(&dir, &["/a".into(), "/b".into()], Some("/b"));
        let (settings, diagnostics) = load_from(&dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(settings.open_repos, vec!["/a", "/b"]);
        assert_eq!(settings.active_repo.as_deref(), Some("/b"));

        // Closing every tab clears the keys again.
        set_open_repos_to(&dir, &[], None);
        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        assert!(!text.contains("open_repos"), "{text}");
        assert!(!text.contains("active_repo"), "{text}");

        std::fs::write(
            settings_path(&dir),
            r#"{"open_repos": "nope", "active_repo": 5}"#,
        )
        .unwrap();
        let (settings, diagnostics) = load_from(&dir);
        assert!(settings.open_repos.is_empty());
        assert_eq!(settings.active_repo, None);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    }

    /// The public entry point writes to `%APPDATA%`; the test writes to a
    /// temp dir instead by round-tripping through the same value helpers.
    fn set_open_repos_to(dir: &Path, repos: &[String], active: Option<&str>) {
        save_to(
            dir,
            &Settings {
                open_repos: repos.to_vec(),
                active_repo: active.map(str::to_string),
                ..Settings::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn a_failed_save_leaves_the_previous_file_untouched() {
        let dir = temp_dir("failsave");
        save_to(&dir, &sample()).unwrap();
        let before = std::fs::read(settings_path(&dir)).unwrap();

        // A real filesystem failure at the temp-write step: the temp path is
        // a directory, so writing the temp file cannot succeed.
        std::fs::create_dir(dir.join("settings.json.tmp")).unwrap();
        let mut edited = sample();
        edited.roots = vec!["/lost".into()];
        assert!(save_to(&dir, &edited).is_err());

        let after = std::fs::read(settings_path(&dir)).unwrap();
        assert_eq!(before, after, "failed save changed the previous file");
    }

    #[test]
    fn missing_file_is_a_clean_default() {
        let dir = temp_dir("missing");
        let (settings, diagnostics) = load_from(&dir);
        assert_eq!(settings, Settings::default());
        assert!(settings.auto_refresh, "auto-refresh defaults to on");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn external_client_and_auto_refresh_round_trip_and_reject_bad_shapes() {
        let dir = temp_dir("client");
        save_to(&dir, &sample()).unwrap();
        let (loaded, diagnostics) = load_from(&dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let client = loaded.external_client.expect("client");
        assert_eq!(client.program, "wsl.exe");
        assert_eq!(client.args.len(), 4);
        let command = client.command_for("/home/me/dev");
        assert_eq!(command.get_program().to_string_lossy(), "wsl.exe");
        assert_eq!(
            command.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            vec!["-d", "Ubuntu-26.04", "-e", "code", "/home/me/dev"]
        );

        std::fs::write(
            settings_path(&dir),
            r#"{"auto_refresh": "yes", "external_client": {"args": ["x"]}, "alert_timeout": "soon", "welcomed": "sure"}"#,
        )
        .unwrap();
        let (settings, diagnostics) = load_from(&dir);
        assert!(settings.auto_refresh, "malformed flag falls back to on");
        assert_eq!(settings.external_client, None);
        assert_eq!(
            settings.alert_timeout,
            AlertTimeout::ThreeSeconds,
            "malformed alert timeout falls back to the default"
        );
        assert!(!settings.welcomed, "a malformed welcome flag greets again");
        assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
    }

    #[test]
    fn alert_timeout_keys_round_trip() {
        for mode in AlertTimeout::ALL {
            assert_eq!(AlertTimeout::from_key(mode.key()), Some(mode));
        }
        assert_eq!(AlertTimeout::from_key("1m"), None);
        assert_eq!(AlertTimeout::default().duration().map(|d| d.as_secs()), Some(3));
        assert!(AlertTimeout::Manual.duration().is_none());
    }

    #[test]
    fn list_width_clamps_and_wrap_round_trips() {
        assert_eq!(clamp_changes_list_w(300.0), 300.0);
        assert_eq!(clamp_changes_list_w(50.0), CHANGES_LIST_W_MIN);
        assert_eq!(clamp_changes_list_w(5000.0), CHANGES_LIST_W_MAX);
        assert_eq!(
            clamp_changes_list_w(f32::NAN),
            CHANGES_LIST_W_DEFAULT,
            "NaN must not reach settings.json"
        );

        assert_eq!(clamp_sidebar_w(50.0), SIDEBAR_W_MIN);
        assert_eq!(clamp_sidebar_w(f32::INFINITY), SIDEBAR_W_DEFAULT);
        assert_eq!(clamp_commit_panel_ratio(0.95), COMMIT_PANEL_RATIO_MAX);
        assert_eq!(clamp_commit_panel_ratio(f32::NAN), COMMIT_PANEL_RATIO_DEFAULT);

        let dir = temp_dir("listwrap");
        let (mut settings, _) = load_from(&dir);
        settings.changes_list_w = 50.0;
        settings.diff_wrap = true;
        save_to(&dir, &settings).unwrap();
        let (loaded, diagnostics) = load_from(&dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(loaded.changes_list_w, CHANGES_LIST_W_MIN);
        assert!(loaded.diff_wrap);

        std::fs::write(
            settings_path(&dir),
            r#"{"changes_list_w": "wide", "diff_wrap": "yes"}"#,
        )
        .unwrap();
        let (settings, diagnostics) = load_from(&dir);
        assert_eq!(settings.changes_list_w, CHANGES_LIST_W_DEFAULT);
        assert_eq!(settings.sidebar_w, SIDEBAR_W_DEFAULT);
        assert!(!settings.diff_wrap);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    }
}

/// The configured external handoff must be WSL-aware: a Windows client cannot
/// open a raw Linux path. Ignored by default; run with
/// `cargo test -- --ignored`.
#[cfg(all(test, windows))]
mod wsl_tests {
    use super::*;
    use crate::process::wsl_support as wsl;

    #[test]
    #[ignore = "requires a WSL distro"]
    fn external_client_handoff_is_wsl_aware() {
        wsl::watchdog(120);
        let dir = wsl::temp_dir("client");
        let repo = format!("{dir}/repo");
        wsl::must(&["git", "init", "-q", "-b", "main", &repo]);

        // The selected handoff bridges through the distro: `wsl.exe -e test -d`.
        let client = ExternalClient {
            program: "wsl.exe".into(),
            args: vec![
                "-d".into(),
                crate::git::distro().into(),
                "-e".into(),
                "test".into(),
                "-d".into(),
            ],
        };
        let output = client.command_for(&repo).output().expect("spawn handoff");
        assert!(
            output.status.success(),
            "WSL-aware handoff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut missing = client.command_for(&format!("{dir}/missing"));
        assert!(!missing.output().expect("spawn handoff").status.success());

        // Negative control: a blind Windows client given the raw Linux path
        // must fail, which is what the WSL bridge exists to avoid.
        let blind = ExternalClient {
            program: "cmd.exe".into(),
            args: vec!["/c".into(), "dir".into()],
        };
        assert!(
            !blind.command_for(&repo).output().expect("spawn blind").status.success(),
            "a Windows client accepted a raw Linux path"
        );
    }
}
