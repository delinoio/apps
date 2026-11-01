// SPDX-License-Identifier: Apache-2.0
//! Process environment snapshot and OS directories. Tests construct settings from explicit
//! variables so that they never read or change the real user environment.
use std::{collections::BTreeMap, path::PathBuf};
pub const DEFAULT_API_URL: &str = "https://anydash.api.delino.io";
pub const APP_ID: &str = "io.delino.anydash";
#[derive(Clone, Debug, Default)]
pub struct Settings {
    vars: BTreeMap<String, String>,
    pub user_home: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    pub data: PathBuf,
    pub config: PathBuf,
    pub logs: PathBuf,
    pub runtime: PathBuf,
}
impl Settings {
    #[must_use]
    pub fn from_env() -> Self {
        let vars: BTreeMap<String, String> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        let home = vars
            .get(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .cloned()
            .unwrap_or_default();
        Self {
            vars,
            user_home: PathBuf::from(home),
        }
    }
    #[must_use]
    pub fn with_vars(user_home: PathBuf, vars: &[(&str, &str)]) -> Self {
        Self {
            vars: vars
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            user_home,
        }
    }
    /// A non-empty variable value.
    #[must_use]
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
    pub fn set_var(&mut self, name: &str, value: &str) {
        self.vars.insert(name.to_owned(), value.to_owned());
    }
    fn path(&self, name: &str) -> Option<PathBuf> {
        self.var(name).map(PathBuf::from)
    }
    #[must_use]
    pub fn api_url(&self) -> String {
        self.var("ANYDASH_API_URL")
            .unwrap_or(DEFAULT_API_URL)
            .trim_end_matches('/')
            .to_owned()
    }
    /// Color is disabled by `NO_COLOR` or `ANYDASH_LOG_COLOR=never`.
    #[must_use]
    pub fn color_disabled(&self) -> bool {
        self.vars.contains_key("NO_COLOR") || self.var("ANYDASH_LOG_COLOR") == Some("never")
    }
    #[must_use]
    pub fn dirs(&self) -> Dirs {
        if let Some(home) = self.path("ANYDASH_HOME") {
            let data = home.join("data");
            return Dirs {
                runtime: data.clone(),
                data,
                config: home.join("config"),
                logs: home.join("logs"),
            };
        }
        let home = &self.user_home;
        if cfg!(target_os = "macos") {
            let library = home.join("Library");
            let data = library.join("Application Support").join(APP_ID);
            Dirs {
                runtime: data.clone(),
                data,
                config: library.join("Preferences").join(APP_ID),
                logs: library.join("Logs").join(APP_ID),
            }
        } else if cfg!(windows) {
            let local = self
                .path("LOCALAPPDATA")
                .unwrap_or_else(|| home.join("AppData").join("Local"))
                .join("Delino")
                .join("AnyDash");
            let roaming = self
                .path("APPDATA")
                .unwrap_or_else(|| home.join("AppData").join("Roaming"));
            let data = local.join("Data");
            Dirs {
                runtime: data.clone(),
                data,
                config: roaming.join("Delino").join("AnyDash"),
                logs: local.join("Logs"),
            }
        } else {
            let data = self
                .path("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local").join("share"))
                .join("anydash");
            Dirs {
                runtime: self
                    .path("XDG_RUNTIME_DIR")
                    .map_or_else(|| data.clone(), |r| r.join("anydash")),
                data,
                config: self.config_home().join("anydash"),
                logs: self
                    .path("XDG_STATE_HOME")
                    .unwrap_or_else(|| home.join(".local").join("state"))
                    .join("anydash")
                    .join("logs"),
            }
        }
    }
    /// `$XDG_CONFIG_HOME`, defaulting to `~/.config`.
    #[must_use]
    pub fn config_home(&self) -> PathBuf {
        self.path("XDG_CONFIG_HOME")
            .unwrap_or_else(|| self.user_home.join(".config"))
    }
    /// `$CODEX_HOME`, defaulting to `~/.codex`.
    #[must_use]
    pub fn codex_home(&self) -> (PathBuf, bool) {
        self.path("CODEX_HOME")
            .map_or_else(|| (self.user_home.join(".codex"), false), |p| (p, true))
    }
    /// Claude Code configuration directories: each comma-separated `CLAUDE_CONFIG_DIR` entry, or
    /// `~/.claude` and `$XDG_CONFIG_HOME/claude` when it is unset. The flag marks environment
    /// origins.
    #[must_use]
    pub fn claude_dirs(&self) -> Vec<(PathBuf, bool)> {
        self.var("CLAUDE_CONFIG_DIR").map_or_else(
            || {
                vec![
                    (self.user_home.join(".claude"), false),
                    (
                        self.config_home().join("claude"),
                        self.vars.contains_key("XDG_CONFIG_HOME"),
                    ),
                ]
            },
            |v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| (PathBuf::from(s), true))
                    .collect()
            },
        )
    }
}
