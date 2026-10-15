// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/logs.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Application logging on stderr with Python's verbosity ladder.
//!
//! Library diagnostics never become CLI data. A reloadable filter avoids
//! stacked handlers on repeated initialization, and raw websocket frame DEBUG
//! events remain opt-in even when ordinary SDK DEBUG diagnostics are enabled.

use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tracing::{Level, Metadata, Subscriber};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, format::Writer};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, Registry, reload};

use crate::output::{OutputSink, ProcessSink, Stream};

/// The upstream environment override, also recognized by rtao.
pub const ENV_VAR: &str = "BITTENSOR_LOG";

/// Diagnostic filtering derived from flags and the optional environment value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogPolicy {
    verbosity: u8,
    quiet: bool,
}

impl LogPolicy {
    /// Resolve exactly the Python environment override ladder.
    ///
    /// Whitespace and ASCII casing are ignored; unknown values leave flags
    /// intact. Explicit positive verbosity takes precedence over `--quiet`,
    /// matching `setup_logging`'s `if verbosity <= 0` branch.
    pub fn resolve(verbosity: u8, quiet: bool, environment: Option<&str>) -> Self {
        match environment
            .map(crate::prompt::python_strip)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("error") => Self {
                verbosity: 0,
                quiet: true,
            },
            Some("warning") => Self {
                verbosity: 0,
                quiet: false,
            },
            Some("info") => Self {
                verbosity: 1,
                quiet: false,
            },
            Some("debug") => Self {
                verbosity: 2,
                quiet: false,
            },
            Some("trace") => Self {
                verbosity: 3,
                quiet: false,
            },
            _ => Self { verbosity, quiet },
        }
    }

    /// Effective inherited SDK level.
    pub fn level(self) -> Level {
        match self.verbosity {
            0 if self.quiet => Level::ERROR,
            0 => Level::WARN,
            1 => Level::INFO,
            _ => Level::DEBUG,
        }
    }

    /// Whether raw websocket frame DEBUG diagnostics are enabled.
    pub fn raw_frames(self) -> bool {
        self.verbosity >= 3
    }

    /// Whether verbose source-location diagnostics are requested.
    pub fn source_locations(self) -> bool {
        self.verbosity >= 2
    }

    /// Select SDK and opted-in websocket diagnostics without ambient root logs.
    pub fn enabled(self, target: &str, level: &Level) -> bool {
        if target == "rtao::rpc::raw"
            || target == "bittensor.transport.raw_websocket"
            || target.starts_with("bittensor.transport.raw_websocket.")
        {
            return *level
                <= if self.raw_frames() {
                    Level::DEBUG
                } else {
                    Level::INFO
                };
        }
        if target == "websockets"
            || target.starts_with("websockets.")
            || target.starts_with("websockets::")
        {
            return self.raw_frames() && *level <= Level::DEBUG;
        }
        let sdk = target == "rtao"
            || target.starts_with("rtao::")
            || target.starts_with("rtao_")
            || target == "bittensor"
            || target.starts_with("bittensor.");
        sdk && *level <= self.level()
    }
}

#[derive(Clone, Copy, Debug)]
struct DiagnosticFilter(LogPolicy);

impl<S: Subscriber> Layer<S> for DiagnosticFilter {
    fn enabled(&self, metadata: &Metadata<'_>, _context: Context<'_, S>) -> bool {
        self.0.enabled(metadata.target(), metadata.level())
    }

    fn register_callsite(
        &self,
        _metadata: &'static Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        // The reload handle invalidates cached filtering when flags change.
        // Dynamic interest also admits event-specific target/level checks.
        tracing::subscriber::Interest::sometimes()
    }

    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        Some(tracing::metadata::LevelFilter::DEBUG)
    }
}

/// Reload controller for an installed subscriber.
#[derive(Clone)]
pub struct LoggingController {
    filter: reload::Handle<DiagnosticFilter, Registry>,
    source_locations: Arc<AtomicBool>,
}

impl LoggingController {
    /// Replace filtering, without adding another subscriber or writer.
    pub fn configure(&self, policy: LogPolicy) -> Result<(), LoggingError> {
        self.filter
            .reload(DiagnosticFilter(policy))
            .map_err(|error| LoggingError(error.to_string()))?;
        self.source_locations
            .store(policy.source_locations(), Ordering::Relaxed);
        Ok(())
    }
}

struct DiagnosticFormat(Arc<AtomicBool>);

impl<S, N> FormatEvent<S, N> for DiagnosticFormat
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        let locations = self.0.load(Ordering::Relaxed);
        tracing_subscriber::fmt::format()
            .without_time()
            .with_target(false)
            .with_file(locations)
            .with_line_number(locations)
            .format_event(context, writer, event)
    }
}

/// CLI logging setup failed or a previously installed subscriber disappeared.
#[derive(Clone, Debug)]
pub struct LoggingError(String);

impl fmt::Display for LoggingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LoggingError {}

#[derive(Clone)]
struct SinkWriter(Arc<dyn OutputSink>);

impl Write for SinkWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(Stream::Stderr, bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Build a scoped subscriber with injected stderr; no global state changes.
///
/// Keep the returned controller alive if filtering will be reconfigured. This
/// API also lets embedding callers install logging in their own tracing scope.
pub fn subscriber(
    policy: LogPolicy,
    sink: Arc<dyn OutputSink>,
) -> (impl Subscriber + Send + Sync, LoggingController) {
    let (filter, handle) = reload::Layer::new(DiagnosticFilter(policy));
    let source_locations = Arc::new(AtomicBool::new(policy.source_locations()));
    let format = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .event_format(DiagnosticFormat(source_locations.clone()))
        .with_writer(move || SinkWriter(sink.clone()));
    (
        Registry::default().with(filter).with(format),
        LoggingController {
            filter: handle,
            source_locations,
        },
    )
}

static INSTALLED: OnceLock<Mutex<Option<LoggingController>>> = OnceLock::new();

/// Set up process logging exactly once; later calls replace the effective policy.
///
/// Existing host tracing subscribers are preserved: conflicting first setup
/// returns an error instead of silently replacing or duplicating host logging.
pub fn setup_logging(verbosity: u8, quiet: bool) -> Result<(), LoggingError> {
    let environment = std::env::var(ENV_VAR).ok();
    let policy = LogPolicy::resolve(verbosity, quiet, environment.as_deref());
    let lock = INSTALLED.get_or_init(|| Mutex::new(None));
    let mut installed = lock
        .lock()
        .map_err(|_| LoggingError("CLI logging initialization poisoned".into()))?;
    if let Some(controller) = installed.as_ref() {
        return controller.configure(policy);
    }
    let (subscriber, controller) = subscriber(policy, Arc::new(ProcessSink));
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|error| LoggingError(error.to_string()))?;
    *installed = Some(controller);
    Ok(())
}
