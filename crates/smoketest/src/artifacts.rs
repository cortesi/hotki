use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::json;

use crate::{
    error::{Error, Result},
    suite::{Budget, StageDurationsOptional},
};

/// Write configured/actual budget metadata for a case and return the emitted path.
pub fn write_budget_report(
    case: &str,
    budget: &Budget,
    actual: &StageDurationsOptional,
    output_dir: &Path,
) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)?;
    let path = output_dir.join(format!("{case}.budget.json"));
    let payload = json!({
        "case": case,
        "configured": {
            "setup_ms": budget.setup_ms,
            "action_ms": budget.action_ms,
            "settle_ms": budget.settle_ms,
        },
        "actual": actual,
    });
    let mut file = File::create(&path)?;
    serde_json::to_writer_pretty(&mut file, &payload)
        .map_err(|e| Error::InvalidState(format!("failed to serialize budget: {e}")))?;
    file.write_all(b"\n")?;
    Ok(path)
}
