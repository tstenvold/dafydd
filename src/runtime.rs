//! Shared Tokio runtime for all async discovery work.

use std::sync::OnceLock;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn worker_threads() -> usize {
    std::env::var("DAFYDD_WORKER_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4)
}

fn thread_name() -> String {
    std::env::var("DAFYDD_THREAD_NAME").unwrap_or_else(|_| "dafydd-worker".to_string())
}

/// Returns the process-wide Tokio multi-thread runtime.
///
/// Initialised on first call and reused for the lifetime of the process.
///
/// # Panics
/// Panics if the runtime cannot be built (should never occur in practice).
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        let threads = worker_threads();
        let name = thread_name();

        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(threads)
            .thread_name(name)
            .thread_stack_size(2 * 1024 * 1024)
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("failed to build Tokio runtime with {threads} threads: {e}"))
    })
}

/// Returns the Tokio runtime handle for cancellation support.
#[must_use]
pub fn runtime_handle() -> tokio::runtime::Handle {
    runtime().handle().clone()
}
