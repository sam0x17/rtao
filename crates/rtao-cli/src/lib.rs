//! Bittensor CLI machinery, shared by generated and hand-written commands.
#![deny(missing_docs)]

/// Extensible command tree and hidden compatibility aliases.
pub mod application;
/// Read-only shared books and public account-reference resolution.
pub mod books;
/// Name resolution inside raw calls and nested intent JSON.
pub mod call_names;
/// Command context, connection configuration and submission discipline.
pub mod context;
/// Process status and safe user diagnostics.
pub mod error;
/// Global flag tiers and config precedence.
pub mod globals;
/// Declarative generated-intent prompt policies.
pub mod intent_prompts;
/// Diagnostic logging policy.
pub mod logs;
/// Explicit native macOS password and Keychain support.
pub mod macos_password;
/// Internal semantic multisig dispatch preserving the original intent contract.
pub mod multisig;
/// JSON data and human renderers.
pub mod output;
/// Injectable shell-free process execution.
pub mod process;
/// Terminal prompts and missing-input handling.
pub mod prompt;
/// Root fund pricing, public positions and shared presentation helpers.
pub mod root_helpers;
/// Argv-secret warnings and clipboard handling.
pub mod secrets;

/// Raw metadata-checked calls and multisig approval workflows.
pub mod call;

/// Hand-written command families and catalog discovery.
pub mod commands;
