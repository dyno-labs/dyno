//! The phone's log, through its syslog relay: the kernel lines where jetsam, the part of iOS that
//! frees memory, kills a process, or says that a process went over its memory limit.
//!
//! A kill names the process and its pid in one of two forms:
//!
//! ```text
//! memorystatus: killing process 15036 [com.apple.WebKit.WebContent] in high band FOREGROUND (100) - memorystatus_available_pages: 53907
//! memorystatus: killing_highwater_process pid 15036 [com.apple.WebKit.WebContent] (highwater 100 10s rf:- type:app) 2371666KB - ...
//! ```
//!
//! A process over its limit is `memorystatus: <name> [<pid>] exceeded mem limit: ActiveSoft 1536
//! MB (non-fatal)`, or an `ActiveHard` limit that is fatal. `killing_idle_process` lines are not
//! kills that memory caused: Safari's idle spare pages exit that way.

use serde::Serialize;

/// Where the kernel's memory lines start.
const MEMORYSTATUS: &str = "memorystatus: ";

/// What a memory line says happened to a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvent {
    /// Jetsam killed it.
    Killed,
    /// It went over its memory limit. A fatal limit kills it.
    OverLimit { fatal: bool },
}

/// One kernel memory line about a process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryLine {
    pub pid: u32,
    pub process: String,
    pub event: MemoryEvent,
    /// The kernel's own words, from `memorystatus:` to the end of the line.
    pub text: String,
}

impl MemoryLine {
    /// Whether the process died of it.
    pub fn is_kill(&self) -> bool {
        matches!(
            self.event,
            MemoryEvent::Killed | MemoryEvent::OverLimit { fatal: true }
        )
    }

    /// The kernel's words up to the first ` - `, which leaves out the page counts.
    pub fn short_text(&self) -> &str {
        self.text
            .split_once(" - ")
            .map_or(self.text.as_str(), |(text, _)| text)
    }
}

/// The memory line in a line of the log. None for any other line.
pub fn memory_line(line: &str) -> Option<MemoryLine> {
    let start = line.find(MEMORYSTATUS)?;
    let text = line[start..].lines().next()?.trim_end();
    let said = &text[MEMORYSTATUS.len()..];
    let (pid, process, event) = match said.strip_prefix("killing") {
        Some(kill) => {
            // `killing process 1 [name]`, or `killing_<kind> pid 1 [name]`.
            let (kind, rest) = kill.split_once(' ')?;
            if kind == "_idle_process" {
                return None;
            }
            let (before, bracketed) = rest.split_once('[')?;
            let pid = before.split_whitespace().last()?.parse().ok()?;
            let (process, _) = bracketed.split_once(']')?;
            (pid, process, MemoryEvent::Killed)
        }
        None => {
            let (who, limit) = said.split_once(" exceeded mem limit: ")?;
            let (process, pid) = who.rsplit_once(" [")?;
            let pid = pid.strip_suffix(']')?.parse().ok()?;
            let fatal = limit.contains("(fatal)");
            (pid, process, MemoryEvent::OverLimit { fatal })
        }
    };
    Some(MemoryLine {
        pid,
        process: process.to_owned(),
        event,
        text: text.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KILLED: &str = "Nov 10 21:50:01 Dev-iPhone kernel[0] <Notice>: memorystatus: killing \
        process 15036 [com.apple.WebKit.WebContent] in high band FOREGROUND (100) - \
        memorystatus_available_pages: 53907";

    #[test]
    fn a_kill_names_the_pid_and_the_process_and_not_the_phone() {
        let line = memory_line(KILLED).unwrap();
        assert_eq!(line.pid, 15036);
        assert_eq!(line.process, "com.apple.WebKit.WebContent");
        assert_eq!(line.event, MemoryEvent::Killed);
        assert!(line.is_kill());
        assert!(line.text.starts_with("memorystatus: killing process 15036"));
        assert!(!line.text.contains("Dev-iPhone"));
        assert_eq!(
            line.short_text(),
            "memorystatus: killing process 15036 [com.apple.WebKit.WebContent] in high band \
             FOREGROUND (100)"
        );
    }

    #[test]
    fn a_highwater_kill_gives_its_pid_after_the_word_pid() {
        let line = memory_line(
            "kernel[0] <Notice>: memorystatus: killing_highwater_process pid 15036 \
             [com.apple.WebKit.WebContent] (highwater 100 10s rf:- type:app) 2371666KB - \
             memorystatus_available_pages: 41000",
        )
        .unwrap();
        assert_eq!(line.pid, 15036);
        assert_eq!(line.event, MemoryEvent::Killed);
    }

    #[test]
    fn an_idle_exit_is_not_a_kill() {
        assert_eq!(
            memory_line(
                "kernel[0] <Notice>: memorystatus: killing_idle_process pid 15102 \
                 [com.apple.WebKit.WebContent] (long-idle-exit 0 100s rf:-) 20480KB"
            ),
            None
        );
    }

    #[test]
    fn a_soft_limit_is_not_fatal_and_a_hard_one_is() {
        let soft = memory_line(
            "kernel[0] <Notice>: memorystatus: com.apple.WebKit.WebContent [15036] exceeded \
             mem limit: ActiveSoft 1536 MB (non-fatal)",
        )
        .unwrap();
        assert_eq!(soft.pid, 15036);
        assert_eq!(soft.process, "com.apple.WebKit.WebContent");
        assert_eq!(soft.event, MemoryEvent::OverLimit { fatal: false });
        assert!(!soft.is_kill());
        let hard = memory_line(
            "memorystatus: com.apple.WebKit.WebContent [99] exceeded mem limit: ActiveHard \
             2048 MB (fatal)",
        )
        .unwrap();
        assert!(hard.is_kill());
    }

    #[test]
    fn other_lines_are_not_memory_lines() {
        for line in [
            "kernel[0] <Notice>: memorystatus_perform_idle_demotion() timed out stuck process \
             5562 [ctkd], moving to idle band",
            "MobileSafari(WebKit)[300] <Notice>: WebPageProxy::processDidTerminate: (pid \
             15036), reason=Crash",
            "memorystatus: killing process",
            "memorystatus: killing process abc [name]",
            "",
        ] {
            assert_eq!(memory_line(line), None, "{line}");
        }
    }

    #[test]
    fn a_line_ends_at_its_newline() {
        let line = memory_line(&format!("{KILLED}\nthe next line")).unwrap();
        assert!(line.text.ends_with("53907"));
    }
}
