//! Append-only JSONL audit log: one record per tool call, allowed or denied.

use std::{fs, io::Write, path::PathBuf, sync::Mutex};

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct AuditRecord<'a> {
    pub ts: String,
    pub trace_id: String,
    pub agent: &'a str,
    pub tool: &'a str,
    pub args: &'a Value,
    /// `allow`, `deny`, or `deny-audit-only` (would be denied, ran in audit mode).
    pub decision: &'a str,
    pub policies: &'a [String],
    pub outcome: &'a str,
}

pub struct AuditLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn write(&self, rec: &AuditRecord) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let line = serde_json::to_string(rec).unwrap_or_default();
        match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{line}");
            }
            Err(e) => tracing::error!(error = %e, "failed to write audit log"),
        }
    }
}
