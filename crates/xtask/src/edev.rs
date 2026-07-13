//! Eguidev launch-target validation.

use std::{fs, path::Path, process::Command};

use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::{Error, Result};

/// Canonical Eguidev app command.
const EXPECTED_COMMAND: &[&str] = &[
    "cargo",
    "run",
    "-p",
    "hotki-app",
    "--features",
    "devtools",
    "--bin",
    "hotki-app",
    "--",
    "--dev-mcp",
    "--disable-event-tap",
    "--config",
    "examples/eguidev-demo.luau",
];

/// Validate that `.edev.toml` resolves to the current app package and fixture config.
pub fn validate(root_dir: &Path) -> Result<()> {
    println!("==> Eguidev launch target");
    let edev_path = root_dir.join(".edev.toml");
    let source = read_to_string(&edev_path)?;
    let command = app_command(&source)?;
    if !command
        .iter()
        .map(String::as_str)
        .eq(EXPECTED_COMMAND.iter().copied())
    {
        return Err(Error::Edev {
            message: format!(
                ".edev.toml app command is {:?}; expected {:?}",
                command, EXPECTED_COMMAND
            ),
        });
    }

    validate_app_target(root_dir)?;
    validate_fixture_config(root_dir)?;
    validate_documentation(root_dir)?;
    println!("validated .edev.toml: hotki-app + devtools");
    Ok(())
}

/// Parse the `[app].command` array from the Eguidev configuration.
fn app_command(source: &str) -> Result<Vec<String>> {
    let document = toml::from_str::<TomlValue>(source).map_err(|source| Error::Edev {
        message: format!("invalid .edev.toml: {source}"),
    })?;
    let command = document
        .get("app")
        .and_then(|app| app.get("command"))
        .and_then(TomlValue::as_array)
        .ok_or_else(|| Error::Edev {
            message: "missing [app].command array in .edev.toml".to_string(),
        })?;
    command
        .iter()
        .map(|token| {
            token
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| Error::Edev {
                    message: format!("non-string .edev.toml command entry: {token}"),
                })
        })
        .collect()
}

/// Confirm Cargo still resolves the package, feature, and binary target.
fn validate_app_target(root_dir: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root_dir)
        .output()
        .map_err(|source| Error::CommandStart {
            program: "cargo metadata".to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program: "cargo metadata".to_string(),
            status: output.status,
        });
    }
    let metadata =
        serde_json::from_slice::<JsonValue>(&output.stdout).map_err(|source| Error::Edev {
            message: format!("invalid cargo metadata output: {source}"),
        })?;
    let package = metadata
        .get("packages")
        .and_then(JsonValue::as_array)
        .and_then(|packages| {
            packages.iter().find(|package| {
                package.get("name").and_then(JsonValue::as_str) == Some("hotki-app")
            })
        })
        .ok_or_else(|| Error::Edev {
            message: "cargo metadata does not resolve package hotki-app".to_string(),
        })?;
    if package
        .get("features")
        .and_then(JsonValue::as_object)
        .is_none_or(|features| !features.contains_key("devtools"))
    {
        return Err(Error::Edev {
            message: "cargo metadata does not resolve hotki-app feature devtools".to_string(),
        });
    }
    let binary_exists = package
        .get("targets")
        .and_then(JsonValue::as_array)
        .is_some_and(|targets| {
            targets.iter().any(|target| {
                target.get("name").and_then(JsonValue::as_str) == Some("hotki-app")
                    && target
                        .get("kind")
                        .and_then(JsonValue::as_array)
                        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            })
        });
    if !binary_exists {
        return Err(Error::Edev {
            message: "cargo metadata does not resolve binary hotki-app".to_string(),
        });
    }

    let main_path = root_dir.join("crates/hotki-app/src/main.rs");
    let main = read_to_string(&main_path)?;
    for (needle, label) in [
        ("dev_mcp: bool", "--dev-mcp flag"),
        ("disable_event_tap: bool", "--disable-event-tap flag"),
        ("config: Option<PathBuf>", "--config flag"),
    ] {
        require_contains(&main, needle, label)?;
    }
    Ok(())
}

/// Confirm contributor documentation names the validated launch target and check.
fn validate_documentation(root_dir: &Path) -> Result<()> {
    let path = root_dir.join("DEV.md");
    let source = read_to_string(&path)?;
    require_contains(&source, "cargo xtask edev", "Eguidev target check")?;
    require_contains(
        &source,
        "launch command builds `hotki-app`",
        "Eguidev package documentation",
    )
}

/// Confirm the command's checked-in fixture config still exists.
fn validate_fixture_config(root_dir: &Path) -> Result<()> {
    let path = root_dir.join("examples/eguidev-demo.luau");
    if path.is_file() {
        Ok(())
    } else {
        Err(Error::Edev {
            message: format!("Eguidev fixture config is missing: {}", path.display()),
        })
    }
}

/// Read one validation input with path-aware diagnostics.
fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Require one static target marker.
fn require_contains(source: &str, needle: &str, label: &str) -> Result<()> {
    if source.contains(needle) {
        Ok(())
    } else {
        Err(Error::Edev {
            message: format!("{label} no longer resolves ({needle:?} not found)"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parser_accepts_normal_toml_layouts() -> Result<()> {
        let source = r#"
            [app]
            command = ["cargo", 'run'] # comments and mixed quotes are valid TOML
        "#;

        assert_eq!(app_command(source)?, ["cargo", "run"]);
        Ok(())
    }
}
