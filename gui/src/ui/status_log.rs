//! Ported from Beamer unchanged. A capped, oldest-evicted log rendered in the
//! run page, so a run leaves a readable trail without unbounded growth.

use dioxus::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// The stylesheet's per-level class on `.log-entry`.
    pub fn class(&self) -> &'static str {
        match self {
            LogLevel::Info => "log-level-info",
            LogLevel::Warn => "log-level-warn",
            LogLevel::Error => "log-level-error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusEntry {
    pub time: String,
    pub level: LogLevel,
    pub message: String,
}

/// A long script-generation run emits an assistant turn every few seconds;
/// this bounds what the pane holds without bounding what tracing records.
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct StatusLog {
    pub entries: Vec<StatusEntry>,
}

impl StatusLog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, level: LogLevel, message: impl Into<String>) {
        let now = chrono::Local::now().format("%H:%M:%S").to_string();
        self.entries.push(StatusEntry {
            time: now,
            level,
            message: message.into(),
        });
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for StatusLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Push an entry into a signal from async code.
pub fn log_status(log: &mut Signal<StatusLog>, level: LogLevel, message: impl Into<String>) {
    log.write().push(level, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_evicts_the_oldest_entry_rather_than_growing_without_bound() {
        let mut log = StatusLog::new();
        for i in 0..(MAX_ENTRIES + 10) {
            log.push(LogLevel::Info, format!("line {i}"));
        }
        assert_eq!(log.entries.len(), MAX_ENTRIES);
        assert_eq!(
            log.entries[0].message, "line 10",
            "the first ten must have been evicted from the front"
        );
    }

    #[test]
    fn the_newest_entry_is_last_so_the_pane_reads_top_to_bottom() {
        let mut log = StatusLog::new();
        log.push(LogLevel::Info, "first");
        log.push(LogLevel::Warn, "second");
        assert_eq!(log.entries.last().unwrap().message, "second");
    }
}
