//! The panic hook logs a scrubbed message. Its own test binary, because it replaces the
//! process-wide panic hook and logger.

use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

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
fn panics_are_logged_without_secrets() {
    let capture = Capture::default();
    tracing_subscriber::fmt()
        .fmt_fields(ironcall_core::logging::redacting_fields())
        .with_writer(capture.clone())
        .with_ansi(false)
        .init();
    ironcall_core::logging::install_panic_hook();

    let result = std::panic::catch_unwind(|| {
        panic!(
            "request failed: Authorization: Bearer sk-live-123456 for https://bob:pw123@api.test/?api_key=k9"
        );
    });
    assert!(result.is_err());

    let out = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(out.contains("panic: request failed"), "{out}");
    assert!(out.contains("panic_hook.rs"), "location is logged: {out}");
    for secret in ["sk-live-123456", "pw123", "k9"] {
        assert!(!out.contains(secret), "{secret} leaked: {out}");
    }
}
