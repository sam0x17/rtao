// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/logs.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Logging ladder and reload behavior verified through actual tracing events.

use std::sync::Arc;

use rtao_cli::logs::{LogPolicy, subscriber};
use rtao_cli::output::{MemorySink, Stream};
use tracing::Level;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn flag_ladder_and_environment_override_match_python() {
    for (verbosity, quiet, level) in [
        (0, false, Level::WARN),
        (0, true, Level::ERROR),
        (1, false, Level::INFO),
        (2, false, Level::DEBUG),
        (3, false, Level::DEBUG),
        (1, true, Level::INFO),
    ] {
        assert_eq!(LogPolicy::resolve(verbosity, quiet, None).level(), level);
    }
    for (environment, level, frames) in [
        (" error ", Level::ERROR, false),
        ("WARNING", Level::WARN, false),
        ("Info", Level::INFO, false),
        ("\u{1c}INFO\u{1f}", Level::INFO, false),
        ("debug", Level::DEBUG, false),
        (" TRACE ", Level::DEBUG, true),
    ] {
        let policy = LogPolicy::resolve(0, true, Some(environment));
        assert_eq!(policy.level(), level);
        assert_eq!(policy.raw_frames(), frames);
    }
    assert_eq!(
        LogPolicy::resolve(2, false, Some("bogus")),
        LogPolicy::resolve(2, false, None)
    );
}

fn emit() {
    tracing::error!(target:"rtao::client","fixture_error");
    tracing::warn!(target:"rtao::client","fixture_warning");
    tracing::info!(target:"rtao::client","fixture_info");
    tracing::debug!(target:"rtao::client","fixture_debug");
    tracing::debug!(target:"rtao::rpc::raw",frame="synthetic raw frame","fixture_raw");
    tracing::debug!(target:"websockets","fixture_websockets");
    tracing::error!(target:"unrelated_host","fixture_unrelated");
}

#[test]
fn each_ladder_level_filters_real_events_and_uses_only_stderr() -> TestResult {
    for (verbosity, quiet) in [(0, false), (0, true), (1, false), (2, false), (3, false)] {
        let sink = MemorySink::default();
        let (subscriber, _controller) = subscriber(
            LogPolicy::resolve(verbosity, quiet, None),
            Arc::new(sink.clone()),
        );
        tracing::subscriber::with_default(subscriber, emit);
        let text = sink.text(Stream::Stderr)?;
        assert!(text.contains("fixture_error"));
        assert_eq!(text.contains("fixture_warning"), !quiet);
        assert_eq!(text.contains("fixture_info"), verbosity >= 1);
        assert_eq!(text.contains("fixture_debug"), verbosity >= 2);
        assert_eq!(text.contains("fixture_raw"), verbosity >= 3);
        assert_eq!(text.contains("fixture_websockets"), verbosity >= 3);
        assert!(!text.contains("fixture_unrelated"));
        assert!(sink.bytes(Stream::Stdout)?.is_empty());
    }
    Ok(())
}

#[test]
fn reconfiguration_replaces_policy_without_duplicate_handlers() -> TestResult {
    let sink = MemorySink::default();
    let (subscriber, controller) =
        subscriber(LogPolicy::resolve(3, false, None), Arc::new(sink.clone()));
    tracing::subscriber::with_default(subscriber, || -> Result<(), Box<dyn std::error::Error>> {
        emit();
        controller.configure(LogPolicy::resolve(0, true, None))?;
        emit();
        controller.configure(LogPolicy::resolve(1, false, None))?;
        emit();
        Ok(())
    })?;
    let text = sink.text(Stream::Stderr)?;
    assert_eq!(text.matches("fixture_error").count(), 3);
    assert_eq!(text.matches("fixture_warning").count(), 2);
    assert_eq!(text.matches("fixture_info").count(), 2);
    assert_eq!(text.matches("fixture_debug").count(), 1);
    assert_eq!(text.matches("fixture_raw").count(), 1);
    assert_eq!(text.matches("fixture_websockets").count(), 1);
    Ok(())
}

#[test]
fn raw_frame_debug_is_independently_masked_at_debug_level() {
    let debug = LogPolicy::resolve(2, false, None);
    let trace = LogPolicy::resolve(3, false, None);
    assert!(debug.enabled("rtao_rpc::client", &Level::DEBUG));
    assert!(!debug.enabled("rtao::rpc::raw", &Level::DEBUG));
    assert!(!debug.enabled("bittensor.transport.raw_websocket", &Level::DEBUG));
    assert!(trace.enabled("rtao::rpc::raw", &Level::DEBUG));
    assert!(trace.enabled("bittensor.transport.raw_websocket", &Level::DEBUG));
    assert!(trace.enabled("websockets.client", &Level::DEBUG));
    assert!(!trace.enabled("other_library", &Level::ERROR));
}

#[test]
fn source_locations_follow_reconfiguration() -> TestResult {
    let sink = MemorySink::default();
    let (subscriber, controller) =
        subscriber(LogPolicy::resolve(1, false, None), Arc::new(sink.clone()));
    tracing::subscriber::with_default(subscriber, || -> Result<(), Box<dyn std::error::Error>> {
        tracing::info!(target:"rtao::client","without_source");
        controller.configure(LogPolicy::resolve(2, false, None))?;
        tracing::info!(target:"rtao::client","with_source");
        controller.configure(LogPolicy::resolve(0, false, None))?;
        tracing::warn!(target:"rtao::client","without_source_again");
        Ok(())
    })?;
    let text = sink.text(Stream::Stderr)?;
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(!lines[0].contains("logging_contract.rs"));
    assert!(lines[1].contains("logging_contract.rs"));
    assert!(!lines[2].contains("logging_contract.rs"));
    Ok(())
}
