//! Tempera Android's stable, agent-facing control contracts.
//!
//! The CLI, daemon, MCP server, dashboard and each target backend use the
//! same command and response types. The Android companion is an optional
//! acceleration backend; ADB/UIAutomator remains the independent fallback.

pub mod adb;
pub mod avd;
pub mod benchmark;
pub mod bridge;
pub mod command;
pub mod config;
pub mod daemon;
pub mod dashboard;
pub mod error;
pub mod evals;
pub mod mcp;
pub mod model;
pub mod session;

pub use command::{execute, CommandRequest, CommandResponse};
pub use error::{AndroidError, Result};
pub use model::{ActionReceiptV1, ActionV1, SessionV1, SnapshotV1};
