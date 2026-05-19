//! Shared Tokio runtime for all async discovery work.

use std::sync::OnceLock;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn worker_threads() -> usize {
    let parsed = std::env::var("DAFYDD_WORKER_THREADS").ok().and_then(|v| {
        v.parse::<usize>()
            .map_err(|e| {
                tracing::debug!(
                    value = %v,
                    error = %e,
                    "DAFYDD_WORKER_THREADS is set but could not be parsed; using default"
                );
            })
            .ok()
    });
    parsed.unwrap_or_else(|| {
        // TCP scanning is almost entirely I/O-bound; 2 workers saturate the
        // async scheduler without burning threads on context switches.
        // Serial spawn_blocking tasks get their own blocking thread pool.
        std::thread::available_parallelism()
            .map_or(2, std::num::NonZeroUsize::get)
            .min(2)
    })
}

fn thread_name() -> &'static str {
    static NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        Box::leak(
            std::env::var("DAFYDD_THREAD_NAME")
                .unwrap_or_else(|_| "dafydd-worker".to_string())
                .into_boxed_str(),
        )
    })
}

/// Initialise the process-wide Tokio runtime, returning an error if it fails.
///
/// Safe to call multiple times — subsequent calls are no-ops.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if the runtime cannot be built.
pub fn try_init_runtime() -> Result<(), std::io::Error> {
    if RUNTIME.get().is_none() {
        let threads = worker_threads();
        let name = thread_name();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(threads)
            .thread_name(name)
            .thread_stack_size(2 * 1024 * 1024)
            .enable_all()
            .build()?;
        // If another thread raced us, the runtime we built is simply dropped.
        let _ = RUNTIME.set(rt);
    }
    Ok(())
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
