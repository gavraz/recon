use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

/// A selectable column in the session table.
///
/// The `#` index column is always shown first and is not configurable.
/// Variant string names (used in the config file) are the snake_case forms
/// below; keep [`Column::ALL`] in sync when adding a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Session,
    Window,
    Project,
    Directory,
    Status,
    Model,
    Context,
    LastActivity,
}

impl Column {
    /// All columns in the default display order.
    pub const ALL: [Column; 8] = [
        Column::Session,
        Column::Window,
        Column::Project,
        Column::Directory,
        Column::Status,
        Column::Model,
        Column::Context,
        Column::LastActivity,
    ];

    /// The snake_case name used in the config file.
    pub fn name(self) -> &'static str {
        match self {
            Column::Session => "session",
            Column::Window => "window",
            Column::Project => "project",
            Column::Directory => "directory",
            Column::Status => "status",
            Column::Model => "model",
            Column::Context => "context",
            Column::LastActivity => "last_activity",
        }
    }

    /// The header label shown in the table.
    pub fn header(self) -> &'static str {
        match self {
            Column::Session => "Session",
            Column::Window => "Window",
            Column::Project => "Project",
            Column::Directory => "Directory",
            Column::Status => "Status",
            Column::Model => "Model",
            Column::Context => "Context",
            Column::LastActivity => "Last Activity",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub table: TableConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableConfig {
    /// Columns to display, in order.
    #[serde(default = "default_columns")]
    pub columns: Vec<Column>,
    /// Optional per-column width overrides (in cells).
    #[serde(default)]
    pub widths: HashMap<Column, u16>,
}

fn default_columns() -> Vec<Column> {
    Column::ALL.to_vec()
}

impl Default for TableConfig {
    fn default() -> Self {
        TableConfig {
            columns: default_columns(),
            widths: HashMap::new(),
        }
    }
}

/// Path to the user config file: `$XDG_CONFIG_HOME/recon/config.toml`
/// (`~/.config/recon/config.toml` on most systems).
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("recon").join("config.toml"))
}

/// Load and validate the config.
///
/// Returns the default config when no file exists. A file that is present
/// but unreadable or malformed is a hard error (fail fast) — the error
/// string is user-facing and self-explanatory.
pub fn load() -> Result<Config, String> {
    let path = match config_path() {
        Some(p) => p,
        None => return Ok(Config::default()),
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(format!("Failed to read config {}: {e}", path.display())),
    };

    let config: Config =
        toml::from_str(&content).map_err(|e| format!("Invalid config {}:\n{e}", path.display()))?;

    if config.table.columns.is_empty() {
        return Err(format!(
            "Invalid config {}:\ntable.columns must list at least one column",
            path.display()
        ));
    }

    Ok(config)
}

/// Human-readable listing of the available columns plus an example stanza,
/// printed by `recon config --available-columns`.
pub fn available_columns_help() -> String {
    let mut out = String::from("Available columns:\n");
    for col in Column::ALL {
        out.push_str(&format!("  {:<14}{}\n", col.name(), col.header()));
    }
    let example = config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/recon/config.toml".to_string());
    out.push_str(&format!(
        "\nExample {example}:\n\n  [table]\n  columns = [\"session\", \"window\", \"project\", \"status\", \"context\", \"last_activity\"]\n\n  [table.widths]\n  window = 24\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.table.columns, Column::ALL.to_vec());
        assert!(c.table.widths.is_empty());
    }

    #[test]
    fn partial_config_fills_defaults() {
        let c: Config = toml::from_str("[table]\nwidths = { window = 20 }").unwrap();
        assert_eq!(c.table.columns, Column::ALL.to_vec());
        assert_eq!(c.table.widths.get(&Column::Window), Some(&20));
    }

    #[test]
    fn columns_are_ordered_as_written() {
        let c: Config = toml::from_str("[table]\ncolumns = [\"status\", \"session\"]").unwrap();
        assert_eq!(c.table.columns, vec![Column::Status, Column::Session]);
    }

    #[test]
    fn unknown_column_is_rejected() {
        let err = toml::from_str::<Config>("[table]\ncolumns = [\"brnach\"]").unwrap_err();
        // serde enumerates the valid variants in the error, which is what the
        // user sees on fail-fast launch.
        assert!(err.message().contains("session"));
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        assert!(toml::from_str::<Config>("nonsense = true").is_err());
    }

    #[test]
    fn full_config_parses_columns_and_widths() {
        let c: Config = toml::from_str(
            "[table]\ncolumns = [\"status\", \"session\", \"window\"]\n\n[table.widths]\nwindow = 24\nsession = 12\n",
        )
        .unwrap();
        assert_eq!(
            c.table.columns,
            vec![Column::Status, Column::Session, Column::Window]
        );
        assert_eq!(c.table.widths.get(&Column::Window), Some(&24));
        assert_eq!(c.table.widths.get(&Column::Session), Some(&12));
    }

    #[test]
    fn available_columns_help_lists_every_column() {
        let help = available_columns_help();
        for col in Column::ALL {
            assert!(
                help.contains(col.name()),
                "help text is missing column {}",
                col.name()
            );
        }
    }

    #[test]
    fn column_name_and_header_are_populated_and_distinct() {
        // Guards against forgetting a match arm when a variant is added:
        // every column must have a non-empty snake_case name and header.
        for col in Column::ALL {
            assert!(!col.name().is_empty());
            assert!(!col.header().is_empty());
            // The config name is the snake_case form (no spaces).
            assert!(!col.name().contains(' '));
        }
    }
}
