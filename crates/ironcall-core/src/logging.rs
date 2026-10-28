//! Logging setup with redaction (PLAN.md §3.1, §6.3).
//!
//! Log fields whose names look sensitive (see [`redact::is_sensitive_name`]) are written
//! as [`redact::MASK`]. This is a safety net: code should still never log secrets, and
//! secret values should use `secrecy` types so `Debug` output is redacted too.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::field::MakeExt;
use tracing_subscriber::fmt::format::{FormatFields, Writer, debug_fn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

use crate::redact;

/// Field formatter that masks values of sensitive-looking fields.
pub fn redacting_fields() -> impl for<'w> FormatFields<'w> + Send + Sync + 'static {
    debug_fn(
        |w: &mut Writer<'_>, field: &tracing::field::Field, value: &dyn std::fmt::Debug| {
            if field.name() == "message" {
                write!(w, "{value:?}")
            } else if redact::is_sensitive_name(field.name()) {
                write!(w, "{}={}", field.name(), redact::MASK)
            } else {
                write!(w, "{}={value:?}", field.name())
            }
        },
    )
    .delimited(" ")
}

/// Logs panics (scrubbed of secrets, with a backtrace) instead of printing them raw.
///
/// The default hook would write the message to stderr as is, and panic messages can
/// include `Debug` output of values that should never be logged (PLAN.md §6.3).
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(non-text panic payload)".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let message = redact::scrub(&payload);
        tracing::error!(%location, %backtrace, "panic: {message}");
        eprintln!("IronCall panicked at {location}: {message}");
    }));
}

/// Starts logging to daily-rolling files in `log_dir` and to stderr.
///
/// Keep the returned guard alive for the life of the process, or buffered log lines are lost.
/// The filter defaults to `info` and can be overridden with `IRONCALL_LOG`.
pub fn init(log_dir: &Path) -> std::io::Result<WorkerGuard> {
    std::fs::create_dir_all(log_dir)?;
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("ironcall")
        .filename_suffix("log")
        .max_log_files(14)
        .build(log_dir)
        .map_err(std::io::Error::other)?;
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("IRONCALL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .fmt_fields(redacting_fields())
                .with_writer(std::io::stderr),
        )
        .with(
            fmt::layer()
                .fmt_fields(redacting_fields())
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .try_init()
        .map_err(std::io::Error::other)?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn sensitive_fields_are_masked_in_log_output() {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .fmt_fields(redacting_fields())
            .with_writer(capture.clone())
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                password = "hunter2",
                access_token = "abc.def",
                user_id = 42,
                "sending"
            );
        });

        let out = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(out.contains("sending"), "{out}");
        assert!(out.contains("user_id=42"), "{out}");
        assert!(out.contains(&format!("password={}", redact::MASK)), "{out}");
        assert!(!out.contains("hunter2"), "{out}");
        assert!(!out.contains("abc.def"), "{out}");
    }
}
