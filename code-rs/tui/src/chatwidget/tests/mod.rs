    use super::*;
    use super::{
        CAPTURE_AUTO_TURN_COMMIT_STUB,
        GIT_DIFF_NAME_ONLY_BETWEEN_STUB,
    };
    use crate::app_event::AppEvent;
    use crate::bottom_pane::panes::auto_coordinator::AutoCoordinatorViewModel;
    use crate::bottom_pane::settings_pages::mcp::McpServerRow;
    use crate::chatwidget::message::UserMessage;
    use crate::chatwidget::smoke_helpers::{enter_test_runtime_guard, ChatWidgetHarness};
    use crate::history_cell::{self, ExploreAggregationCell, HistoryCellType};
    use code_auto_drive_core::{
    AutoContinueMode,
    AutoRunPhase,
    AutoRunSummary,
    TurnComplexity,
    TurnMode,
    AUTO_RESOLVE_MAX_REVIEW_ATTEMPTS,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use code_core::config_types::AutoResolveAttemptLimit;
    use code_core::history::state::{
    AssistantStreamDelta,
    AssistantStreamState,
    HistoryId,
    HistoryRecord,
    HistorySnapshot,
    HistoryState,
    InlineSpan,
    MessageLine,
    MessageLineKind,
    OrderKeySnapshot,
    PlainMessageKind,
    PlainMessageRole,
    PlainMessageState,
    TextEmphasis,
    TextTone,
    };
    use code_core::parse_command::ParsedCommand;
    use code_core::protocol::OrderMeta;
    use code_core::config_types::{McpServerConfig, McpServerTransportConfig};
    use code_core::protocol::{
    AskForApproval,
    AgentMessageDeltaEvent,
    AgentMessageEvent,
    AgentStatusUpdateEvent,
    ErrorEvent,
    Event,
    EventMsg,
    ExecCommandBeginEvent,
    McpAuthStatus,
    McpServerFailure,
    McpServerFailurePhase,
    TaskCompleteEvent,
    };
    use code_core::protocol::AgentInfo as CoreAgentInfo;
    use code_protocol::protocol::{ReviewTarget, TurnAbortedEvent, TurnAbortReason};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::process::Command;
    use tempfile::tempdir;
    use std::sync::Arc;
    use std::path::PathBuf;

    struct PathGuard {
        original_path: Option<std::ffi::OsString>,
    }

    impl Drop for PathGuard {
        fn drop(&mut self) {
            if let Some(path) = self.original_path.take() {
                unsafe { std::env::set_var("PATH", path) };
            } else {
                unsafe { std::env::remove_var("PATH") };
            }
        }
    }

    fn prepend_fake_executable_to_path(name: &str) -> (tempfile::TempDir, PathGuard) {
        let temp_dir = tempfile::tempdir().expect("temporary bin directory");
        let executable_name = if cfg!(target_os = "windows") {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let executable_path = temp_dir.path().join(executable_name);
        std::fs::write(&executable_path, b"#!/bin/sh\nexit 0\n").expect("fake executable");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&executable_path)
                .expect("fake executable metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable_path, permissions)
                .expect("fake executable permissions");
        }

        let original_path = std::env::var_os("PATH");
        let mut paths = vec![temp_dir.path().to_path_buf()];
        if let Some(path) = original_path.as_ref() {
            paths.extend(std::env::split_paths(path));
        }
        let joined_path = std::env::join_paths(paths).expect("joined PATH");
        unsafe { std::env::set_var("PATH", joined_path) };

        (temp_dir, PathGuard { original_path })
    }

    include!("harness.rs");
    include!("review.rs");
    include!("ordering.rs");
    include!("hooks.rs");
    include!("autodrive.rs");
    include!("streaming.rs");
    include!("apps.rs");
