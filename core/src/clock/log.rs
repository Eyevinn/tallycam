//! In-memory log like chrony's `log measurements statistics tracking
//! selection`: one text line per record, column layout close to chrony's
//! log files, plus discipline events. A platform driver may also append the
//! records to a file.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogKind {
    Measurements,
    Statistics,
    Tracking,
    Selection,
    /// Discipline events: source selected, sync lost, step, leap, path change.
    Event,
}

impl LogKind {
    pub fn name(self) -> &'static str {
        match self {
            LogKind::Measurements => "measurements",
            LogKind::Statistics => "statistics",
            LogKind::Tracking => "tracking",
            LogKind::Selection => "selection",
            LogKind::Event => "event",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "measurements" => LogKind::Measurements,
            "statistics" => LogKind::Statistics,
            "tracking" => LogKind::Tracking,
            "selection" => LogKind::Selection,
            "event" | "events" => LogKind::Event,
            _ => return None,
        })
    }

    /// The column header chrony writes at the top of each log file.
    pub fn header(self) -> &'static str {
        match self {
            LogKind::Measurements => {
                "Date (UTC) Time     IP Address   L St 123 567 ABCD  LP RP Score    Offset  Peer del. Peer disp.  Root del. Root disp. Refid"
            }
            LogKind::Statistics => {
                "Date (UTC) Time     IP Address    Std dev'n Est offset  Offset sd  Diff freq   Est skew  Stress  Ns  Bs  Nr  Asym"
            }
            LogKind::Tracking => {
                "Date (UTC) Time     IP Address   St   Freq ppm   Skew ppm     Offset L Co  Offset sd Rem. corr. Root delay Root disp. Max. error"
            }
            LogKind::Selection => "Date (UTC) Time     IP Address   S EOpts Reach Score   Last sam Low end    High end",
            LogKind::Event => "Date (UTC) Time     Event",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogRecord {
    /// Increasing sequence number, for incremental fetches and file sinks.
    pub seq: u64,
    pub kind: LogKind,
    pub line: String,
}

#[derive(Debug, Clone)]
pub struct ClockLog {
    records: VecDeque<LogRecord>,
    capacity: usize,
    next_seq: u64,
}

impl ClockLog {
    pub fn new(capacity: usize) -> Self {
        ClockLog {
            records: VecDeque::with_capacity(capacity.min(1024)),
            capacity: capacity.max(1),
            next_seq: 1,
        }
    }

    pub fn push(&mut self, kind: LogKind, line: String) {
        if self.records.len() == self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(LogRecord {
            seq: self.next_seq,
            kind,
            line,
        });
        self.next_seq += 1;
    }

    /// Records with `seq > after`, oldest first.
    pub fn since(&self, after: u64) -> impl Iterator<Item = &LogRecord> {
        self.records.iter().filter(move |r| r.seq > after)
    }

    /// The newest `limit` lines of the given kinds (all kinds when empty),
    /// oldest first, each prefixed with its kind.
    pub fn tail(&self, kinds: &[LogKind], limit: usize) -> Vec<String> {
        let mut out: Vec<String> = self
            .records
            .iter()
            .rev()
            .filter(|r| kinds.is_empty() || kinds.contains(&r.kind))
            .take(limit)
            .map(|r| format!("{} {}", r.kind.name(), r.line))
            .collect();
        out.reverse();
        out
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_and_filters() {
        let mut log = ClockLog::new(3);
        log.push(LogKind::Tracking, "a".into());
        log.push(LogKind::Selection, "b".into());
        log.push(LogKind::Tracking, "c".into());
        log.push(LogKind::Event, "d".into());
        assert_eq!(log.len(), 3);
        assert_eq!(
            log.tail(&[], 10),
            vec!["selection b", "tracking c", "event d"]
        );
        assert_eq!(log.tail(&[LogKind::Tracking], 10), vec!["tracking c"]);
        assert_eq!(
            log.since(3).map(|r| r.line.as_str()).collect::<Vec<_>>(),
            vec!["d"]
        );
        assert_eq!(LogKind::parse("statistics"), Some(LogKind::Statistics));
    }
}
