use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};
use uuid::Uuid;

use crate::herdr::Herdr;

const KEY: &str = "prefix+m";
const COMMAND: &str = "shadowfax.ferry.open";
const DESCRIPTION: &str = "Move panes or tabs, or merge workspaces with Ferry";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Updated { backup: Option<PathBuf> },
    Unchanged,
}

pub fn install_from_environment() -> Result<InstallOutcome> {
    let path = config_path_from_environment()?;
    let outcome = install(&path)?;
    let herdr = Herdr::from_environment();
    match &outcome {
        InstallOutcome::Updated { .. } => {
            herdr.reload_config()?;
            herdr.notify("Ferry is bound to prefix+m")?;
        }
        InstallOutcome::Unchanged => {
            herdr.notify("Ferry keybinding is already installed")?;
        }
    }
    Ok(outcome)
}

pub fn config_path_from_environment() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HERDR_CONFIG_PATH") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("herdr/config.toml"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/herdr/config.toml"))
}

pub fn install(path: &Path) -> Result<InstallOutcome> {
    let target = resolve_target(path)?;
    let original = match fs::read_to_string(&target) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", target.display()))
        }
    };
    let mut document = match &original {
        Some(contents) => contents
            .parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {}", target.display()))?,
        None => DocumentMut::new(),
    };

    ensure_builtin_key_is_available(&document)?;
    let commands = commands_mut(&mut document)?;
    if commands.iter().any(|table| {
        table_string(table, "key") == Some(KEY) && table_string(table, "command") != Some(COMMAND)
    }) {
        bail!("{KEY} is already bound to another custom command");
    }
    let ferry_commands = commands
        .iter()
        .filter(|table| table_string(table, "command") == Some(COMMAND))
        .collect::<Vec<_>>();
    if ferry_commands.len() == 1 && is_desired(ferry_commands[0]) {
        return Ok(InstallOutcome::Unchanged);
    }

    commands.retain(|table| table_string(table, "command") != Some(COMMAND));
    commands.push(ferry_table());
    let rendered = document.to_string();
    if original.as_deref() == Some(rendered.as_str()) {
        return Ok(InstallOutcome::Unchanged);
    }

    let parent = target
        .parent()
        .context("Herdr config path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let backup = original
        .as_ref()
        .map(|contents| write_backup(&target, contents))
        .transpose()?;
    let mode = fs::metadata(&target)
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .unwrap_or(0o600);
    atomic_write(&target, rendered.as_bytes(), mode)?;
    Ok(InstallOutcome::Updated { backup })
}

fn resolve_target(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(target) => {
            if !fs::metadata(&target)?.is_file() {
                bail!("Herdr config is not a regular file");
            }
            Ok(target)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok() {
                bail!("Herdr config is a dangling symlink: {}", path.display());
            }
            Ok(path.to_path_buf())
        }
        Err(error) => Err(error).with_context(|| format!("failed to resolve {}", path.display())),
    }
}

fn ensure_builtin_key_is_available(document: &DocumentMut) -> Result<()> {
    let Some(keys) = document.get("keys").and_then(Item::as_table) else {
        return Ok(());
    };
    for (name, item) in keys.iter().filter(|(name, _)| *name != "command") {
        let occupied = item.as_str() == Some(KEY)
            || item
                .as_array()
                .is_some_and(|array| array.iter().any(|value| value.as_str() == Some(KEY)));
        if occupied {
            bail!("{KEY} is already assigned to keys.{name}");
        }
    }
    Ok(())
}

fn commands_mut(document: &mut DocumentMut) -> Result<&mut ArrayOfTables> {
    let keys = document
        .as_table_mut()
        .entry("keys")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .context("keys must be a TOML table")?;
    keys.entry("command")
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .context("keys.command must be an array of tables")
}

fn table_string<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(Item::as_str)
}

fn is_desired(table: &Table) -> bool {
    table.len() == 4
        && table_string(table, "key") == Some(KEY)
        && table_string(table, "type") == Some("plugin_action")
        && table_string(table, "command") == Some(COMMAND)
        && table_string(table, "description") == Some(DESCRIPTION)
}

fn ferry_table() -> Table {
    let mut table = Table::new();
    table.insert("key", value(KEY));
    table.insert("type", value("plugin_action"));
    table.insert("command", value(COMMAND));
    table.insert("description", value(DESCRIPTION));
    table
}

fn write_backup(target: &Path, contents: &str) -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis();
    let backup = target.with_file_name(format!(
        "{}.ferry-backup-{timestamp}-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config.toml"),
        Uuid::new_v4()
    ));
    let mode = fs::metadata(target)?.permissions().mode() & 0o777;
    write_new(&backup, contents.as_bytes(), mode)?;
    Ok(backup)
}

fn atomic_write(target: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let parent = target
        .parent()
        .context("target path has no parent directory")?;
    let temporary = parent.join(format!(".ferry-{}.tmp", Uuid::new_v4()));
    let result: Result<()> = (|| {
        write_new(&temporary, contents, mode)?;
        fs::rename(&temporary, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("failed to update {}", target.display()))
}

fn write_new(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::*;

    const ORIGINAL: &str = r#"[theme]
name = "dracula"

[keys]
prefix = "ctrl+a"

[[keys.command]]
key = "alt+i"
type = "plugin_action"
command = "shadowfax.scratch.toggle-nvim"
description = "Scratch"
"#;

    #[test]
    fn install_preserves_config_backs_up_and_is_idempotent() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, ORIGINAL).unwrap();

        let InstallOutcome::Updated {
            backup: Some(backup),
        } = install(&path).unwrap()
        else {
            panic!("expected update");
        };

        assert_eq!(fs::read_to_string(backup).unwrap(), ORIGINAL);
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("shadowfax.scratch.toggle-nvim"));
        assert!(updated.contains("shadowfax.ferry.open"));
        assert!(updated.contains(DESCRIPTION));
        assert_eq!(install(&path).unwrap(), InstallOutcome::Unchanged);
    }

    #[test]
    fn install_refuses_custom_and_builtin_conflicts() {
        let directory = tempdir().unwrap();
        let custom = directory.path().join("custom.toml");
        fs::write(
            &custom,
            r#"[[keys.command]]
key = "prefix+m"
type = "plugin_action"
command = "someone.else"
"#,
        )
        .unwrap();
        assert!(install(&custom).unwrap_err().to_string().contains("custom"));

        let builtin = directory.path().join("builtin.toml");
        fs::write(&builtin, "[keys]\nnew_tab = \"prefix+m\"\n").unwrap();
        assert!(install(&builtin)
            .unwrap_err()
            .to_string()
            .contains("keys.new_tab"));
    }

    #[test]
    fn install_rejects_dangling_config_symlinks() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        symlink(directory.path().join("missing"), &path).unwrap();

        assert!(install(&path)
            .unwrap_err()
            .to_string()
            .contains("dangling symlink"));
    }
}
