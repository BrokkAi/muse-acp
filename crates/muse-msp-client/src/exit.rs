//! How a `muse serve` process ended.

use std::fmt;
use std::process::ExitStatus;

/// The reason a `muse serve` child ended. The names and retry posture follow
/// MSP §2.11; stderr is evidence attached to the surrounding classification,
/// never an input to this mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    CleanShutdown,
    UnhandledError,
    UsageError,
    ConfigError,
    LeaseUnavailable,
    SdkSurfaceUnavailable,
    Crash,
}

/// A process exit observed after the child was successfully spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitClassification {
    pub kind: ExitKind,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// The bounded stderr tail captured before the exit, verbatim. It may
    /// contain credentials or workspace text; never log it unreviewed.
    pub stderr_tail: String,
}

impl ExitClassification {
    /// Whether an automatic durable-host relaunch can reasonably help.
    pub fn retryable(&self) -> bool {
        matches!(self.kind, ExitKind::UnhandledError | ExitKind::Crash)
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            ExitKind::CleanShutdown => "cleanShutdown",
            ExitKind::UnhandledError => "unhandledError",
            ExitKind::UsageError => "usageError",
            ExitKind::ConfigError => "configError",
            ExitKind::LeaseUnavailable => "leaseUnavailable",
            ExitKind::SdkSurfaceUnavailable => "sdkSurfaceUnavailable",
            ExitKind::Crash => "crash",
        }
    }

    pub fn retry_advice(&self) -> &'static str {
        match self.kind {
            ExitKind::CleanShutdown => "none",
            ExitKind::UnhandledError | ExitKind::Crash => "retry",
            ExitKind::UsageError | ExitKind::SdkSurfaceUnavailable => "never",
            ExitKind::ConfigError => "fix-config",
            ExitKind::LeaseUnavailable => "after-lease-release",
        }
    }

    /// Machine-readable support lines: one classification line, then the
    /// stderr tail line by line under the same prefix.
    pub fn support_lines(&self, prefix: &str) -> Vec<String> {
        let exit = self
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string());
        let signal = self
            .signal
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string());
        let mut lines = vec![format!(
            "{prefix} kind={} exit-code={} signal={} retry={}",
            self.kind_name(),
            exit,
            signal,
            self.retry_advice()
        )];
        if self.stderr_tail.is_empty() {
            lines.push(format!("{prefix} stderr-tail=(empty)"));
        } else {
            for line in self.stderr_tail.lines() {
                lines.push(format!("{prefix} stderr-tail {line}"));
            }
        }
        lines
    }
}

impl fmt::Display for ExitClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ExitKind::CleanShutdown => f.write_str("muse serve shut down cleanly (exit code 0)"),
            ExitKind::UnhandledError => {
                f.write_str("muse serve stopped with an unhandled error (exit code 1)")
            }
            ExitKind::UsageError => f.write_str("muse serve rejected its arguments (exit code 2)"),
            ExitKind::ConfigError => {
                f.write_str("muse serve rejected its configuration (exit code 3)")
            }
            ExitKind::LeaseUnavailable => f.write_str(
                "muse serve could not start because another client holds its session lease (exit code 4)",
            ),
            ExitKind::SdkSurfaceUnavailable => f.write_str(
                "muse serve is unavailable because the Muse SDK surface is switched off (exit code 5)",
            ),
            ExitKind::Crash => match (self.exit_code, self.signal) {
                (Some(code), _) => write!(f, "muse serve crashed with exit code {code}"),
                (_, Some(signal)) => write!(f, "muse serve was terminated by signal {signal}"),
                _ => f.write_str("muse serve stopped unexpectedly"),
            },
        }
    }
}

/// Classify an exit from its parts. Exit codes 0 to 5 are MSP's documented
/// outcomes; any other code, or a signal, is a crash.
pub fn classify_exit_parts(
    exit_code: Option<i32>,
    signal: Option<i32>,
    stderr_tail: String,
) -> ExitClassification {
    let kind = match exit_code {
        Some(0) => ExitKind::CleanShutdown,
        Some(1) => ExitKind::UnhandledError,
        Some(2) => ExitKind::UsageError,
        Some(3) => ExitKind::ConfigError,
        Some(4) => ExitKind::LeaseUnavailable,
        Some(5) => ExitKind::SdkSurfaceUnavailable,
        _ => ExitKind::Crash,
    };
    ExitClassification {
        kind,
        exit_code,
        signal,
        stderr_tail,
    }
}

fn status_parts(status: &ExitStatus) -> (Option<i32>, Option<i32>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        (status.code(), status.signal())
    }
    #[cfg(not(unix))]
    {
        (status.code(), None)
    }
}

pub(crate) fn classify_status(status: &ExitStatus, stderr_tail: String) -> ExitClassification {
    let (exit_code, signal) = status_parts(status);
    classify_exit_parts(exit_code, signal, stderr_tail)
}

#[cfg(test)]
mod tests {
    use super::{ExitKind, classify_exit_parts};

    #[test]
    fn exit_codes_are_classified_without_consulting_stderr() {
        for (code, kind, retryable, advice) in [
            (0, ExitKind::CleanShutdown, false, "none"),
            (1, ExitKind::UnhandledError, true, "retry"),
            (2, ExitKind::UsageError, false, "never"),
            (3, ExitKind::ConfigError, false, "fix-config"),
            (4, ExitKind::LeaseUnavailable, false, "after-lease-release"),
            (5, ExitKind::SdkSurfaceUnavailable, false, "never"),
            (6, ExitKind::Crash, true, "retry"),
            (-1, ExitKind::Crash, true, "retry"),
        ] {
            // The stderr text names a different outcome on purpose.
            let exit = classify_exit_parts(Some(code), None, "exit code 5 clean shutdown".into());
            assert_eq!(exit.kind, kind, "{code}");
            assert_eq!(exit.retryable(), retryable, "{code}");
            assert_eq!(exit.retry_advice(), advice, "{code}");
        }
        let signal = classify_exit_parts(None, Some(9), "clean shutdown".into());
        assert_eq!(signal.kind, ExitKind::Crash);
        assert!(signal.retryable());
        assert_eq!(signal.to_string(), "muse serve was terminated by signal 9");
    }

    #[test]
    fn support_lines_include_exit_code_and_stderr_tail() {
        let exit = classify_exit_parts(Some(5), None, "serve gate is off\nsecond".into());
        let lines = exit.support_lines("support");
        assert_eq!(
            lines[0],
            "support kind=sdkSurfaceUnavailable exit-code=5 signal=none retry=never"
        );
        assert_eq!(lines[1], "support stderr-tail serve gate is off");
        assert_eq!(lines[2], "support stderr-tail second");
        let quiet = classify_exit_parts(Some(0), None, String::new()).support_lines("p");
        assert_eq!(quiet[1], "p stderr-tail=(empty)");
    }
}
