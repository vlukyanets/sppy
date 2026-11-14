//! The app's event log: chat, notices and errors; repeated notices collapse into one line.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Chat,
    Notice,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: LogKind,
    pub text: String,
    pub count: u32,
}

impl Entry {
    /// `Yaroslav resumed ×10` for collapsed repeats.
    pub fn display(&self) -> String {
        if self.count > 1 { format!("{} ×{}", self.text, self.count) } else { self.text.clone() }
    }
}

#[derive(Debug, Clone)]
pub struct EventLog {
    entries: VecDeque<Entry>,
    capacity: usize,
}

impl EventLog {
    pub fn new(capacity: usize) -> Self {
        EventLog { entries: VecDeque::new(), capacity }
    }

    pub fn push(&mut self, kind: LogKind, text: String) {
        if kind == LogKind::Notice
            && let Some(last) = self.entries.back_mut()
            && last.kind == LogKind::Notice
            && last.text == text
        {
            last.count += 1;
            return;
        }
        self.entries.push_back(Entry { kind, text, count: 1 });
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    /// Chat history received on join goes before everything else.
    pub fn push_history_front(&mut self, text: String) {
        if self.entries.len() < self.capacity {
            self.entries.push_front(Entry { kind: LogKind::Chat, text, count: 1 });
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn last_notice(&self) -> Option<&Entry> {
        self.entries.iter().rev().find(|e| e.kind == LogKind::Notice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_notices_collapse() {
        let mut log = EventLog::new(10);
        for _ in 0..10 {
            log.push(LogKind::Notice, "Yaroslav resumed".into());
        }
        log.push(LogKind::Chat, "bob: hi".into());
        log.push(LogKind::Notice, "Yaroslav resumed".into());
        let lines: Vec<String> = log.iter().map(|e| e.display()).collect();
        assert_eq!(lines, vec!["Yaroslav resumed ×10", "bob: hi", "Yaroslav resumed"]);
    }

    #[test]
    fn chat_lines_never_collapse() {
        let mut log = EventLog::new(10);
        log.push(LogKind::Chat, "bob: lol".into());
        log.push(LogKind::Chat, "bob: lol".into());
        assert_eq!(log.iter().count(), 2);
    }

    #[test]
    fn capacity_and_history() {
        let mut log = EventLog::new(3);
        for i in 0..5 {
            log.push(LogKind::Chat, format!("m{i}"));
        }
        log.push_history_front("old".into());
        let lines: Vec<String> = log.iter().map(|e| e.display()).collect();
        assert_eq!(lines, vec!["m2", "m3", "m4"], "the log is full: older history is dropped");
        assert_eq!(log.last_notice(), None);
    }
}
