use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use camoufox_juggler::{
    ConsoleMessage, JugglerBrowser, JugglerPage, NetworkEvent, NetworkRequest,
    NetworkRequestFailed, NetworkRequestFinished, NetworkResponseInfo,
    launch_prepared_with_juggler,
};
use rmcp::model::RequestId as McpRequestId;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};

use crate::camoufox_runtime::{
    BrowserRuntime, browser_profile_dir, prepare_browser_launch, validate_screenshot,
};

pub(crate) use rustwright_agent::{
    BrowserError, BrowserOp, BrowserOutput, BrowserResult, FillField, FillFieldKind, RegexSpec,
    ScreenshotType, TabAction,
};
use rustwright_agent::{ConsoleLevel as RequestedConsoleLevel, NetworkPart};
#[cfg(test)]
pub(crate) use rustwright_agent::{
    production_form_fill_safe_later_failure_result, production_form_fill_unknown_outcome_result,
    production_type_submit_safe_failure_result, production_type_submit_unknown_outcome_result,
};

const DEFAULT_TOOL_TIMEOUT_MS: u64 = 60_000;
const MIN_TOOL_TIMEOUT_MS: u64 = 1_000;
const MAX_TOOL_TIMEOUT_MS: u64 = 600_000;
const COMMAND_QUEUE_CAPACITY: usize = 64;
const MAX_FILE_INPUTS: usize = 50;
const MAX_FILE_INPUT_BYTES: u64 = 20 * 1024 * 1024;
const MAX_FILE_INPUT_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const SNAPSHOT_SCRIPT: &str = include_str!("snapshot.js");

pub(crate) struct BrowserActor {
    sender: mpsc::UnboundedSender<ActorCommand>,
    cancellations: Arc<Mutex<HashMap<String, Arc<CommandCancellation>>>>,
    queued: Arc<AtomicUsize>,
    default_timeout: Duration,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

enum ActorCommand {
    Execute(ActorRequest),
    Shutdown,
}

struct ActorRequest {
    key: String,
    op: ActorOperation,
    cancellation: Arc<CommandCancellation>,
    deadline: Instant,
    timeout_ms: u64,
    reply: oneshot::Sender<ActorResult>,
}

enum ActorOperation {
    Agent(BrowserOp),
    Embedded(EmbeddedBrowserOp),
}

pub(crate) enum EmbeddedBrowserOp {
    PageInfo,
    MouseMove { x: f64, y: f64 },
    MouseDown { x: f64, y: f64 },
    MouseUp { x: f64, y: f64 },
    Click { x: f64, y: f64 },
    TypeText(String),
    Scroll { dx: f64, dy: f64 },
    Tabs,
    NewTab(String),
    SelectTarget(String),
    CloseTarget(String),
    Cookies,
    SetCookies(Vec<Value>),
}

pub(crate) enum EmbeddedBrowserOutput {
    None,
    Value(Value),
    Tabs(Vec<EmbeddedTab>),
    TargetId(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct EmbeddedTab {
    pub index: usize,
    pub target_id: String,
    pub title: String,
    pub url: String,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum CancellationReason {
    Active = 0,
    Cancelled = 1,
    Deadline = 2,
}

struct CommandCancellation {
    reason: AtomicU8,
    changed: Notify,
}

impl CommandCancellation {
    fn new() -> Self {
        Self {
            reason: AtomicU8::new(CancellationReason::Active as u8),
            changed: Notify::new(),
        }
    }

    fn cancel(&self, reason: CancellationReason) -> bool {
        if self
            .reason
            .compare_exchange(
                CancellationReason::Active as u8,
                reason as u8,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_err()
        {
            return false;
        }
        self.changed.notify_waiters();
        true
    }

    fn reason(&self) -> CancellationReason {
        match self.reason.load(Ordering::SeqCst) {
            1 => CancellationReason::Cancelled,
            2 => CancellationReason::Deadline,
            _ => CancellationReason::Active,
        }
    }

    async fn wait(&self) {
        loop {
            if self.reason() != CancellationReason::Active {
                return;
            }
            self.changed.notified().await;
        }
    }

    fn error(&self, timeout_ms: u64) -> BrowserError {
        match self.reason() {
            CancellationReason::Deadline => BrowserError::Timeout(timeout_ms),
            _ => BrowserError::Cancelled,
        }
    }
}

impl BrowserActor {
    pub(crate) fn spawn() -> Self {
        let default_timeout = tool_timeout_from_env();
        let workspace = env::var_os("RUSTWRIGHT_MCP_WORKSPACE").map(PathBuf::from);
        let cancellations = Arc::new(Mutex::new(HashMap::new()));
        let actor_cancellations = Arc::clone(&cancellations);
        let queued = Arc::new(AtomicUsize::new(0));
        let actor_queued = Arc::clone(&queued);
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
            .name("camoufox-juggler-actor".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create Camoufox actor runtime");
                runtime.block_on(async move {
                    let mut state = BrowserState::new(workspace);
                    while let Some(command) = receiver.recv().await {
                        match command {
                            ActorCommand::Execute(request) => {
                                actor_queued.fetch_sub(1, Ordering::SeqCst);
                                let result = execute_request(&mut state, &request).await;
                                actor_cancellations.lock().unwrap().remove(&request.key);
                                let _ = request.reply.send(result);
                            }
                            ActorCommand::Shutdown => break,
                        }
                    }
                    state.close().await;
                });
            })
            .expect("failed to spawn Camoufox browser actor");
        Self {
            sender,
            cancellations,
            queued,
            default_timeout,
            thread: Mutex::new(Some(thread)),
        }
    }

    pub(crate) async fn execute(&self, request_id: McpRequestId, op: BrowserOp) -> BrowserResult {
        match self
            .execute_operation(request_id, ActorOperation::Agent(op))
            .await?
        {
            ActorOutput::Agent(output) => Ok(output),
            ActorOutput::Embedded(_) => Err(BrowserError::Stopped),
        }
    }

    pub(crate) async fn execute_embedded(
        &self,
        request_id: McpRequestId,
        op: EmbeddedBrowserOp,
    ) -> Result<EmbeddedBrowserOutput, BrowserError> {
        match self
            .execute_operation(request_id, ActorOperation::Embedded(op))
            .await?
        {
            ActorOutput::Embedded(output) => Ok(output),
            ActorOutput::Agent(_) => Err(BrowserError::Stopped),
        }
    }

    async fn execute_operation(
        &self,
        request_id: McpRequestId,
        op: ActorOperation,
    ) -> Result<ActorOutput, BrowserError> {
        if self.queued.fetch_add(1, Ordering::SeqCst) >= COMMAND_QUEUE_CAPACITY {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            return Err(BrowserError::Busy);
        }
        let key = request_key(&request_id);
        let timeout_ms = duration_millis(self.default_timeout);
        let cancellation = Arc::new(CommandCancellation::new());
        self.cancellations
            .lock()
            .unwrap()
            .insert(key.clone(), Arc::clone(&cancellation));
        let (reply, response) = oneshot::channel();
        let request = ActorRequest {
            key: key.clone(),
            op,
            cancellation,
            deadline: Instant::now() + self.default_timeout,
            timeout_ms,
            reply,
        };
        if self.sender.send(ActorCommand::Execute(request)).is_err() {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            self.cancellations.lock().unwrap().remove(&key);
            return Err(BrowserError::Stopped);
        }
        response.await.unwrap_or(Err(BrowserError::Stopped))
    }

    pub(crate) fn cancel(&self, request_id: &McpRequestId) -> bool {
        self.cancellations
            .lock()
            .unwrap()
            .get(&request_key(request_id))
            .is_some_and(|cancellation| cancellation.cancel(CancellationReason::Cancelled))
    }
}

impl Drop for BrowserActor {
    fn drop(&mut self) {
        let _ = self.sender.send(ActorCommand::Shutdown);
        if let Ok(thread) = self.thread.get_mut()
            && let Some(thread) = thread.take()
            && thread.join().is_err()
        {
            eprintln!("Camoufox browser actor panicked during shutdown");
        }
    }
}

enum ActorOutput {
    Agent(BrowserOutput),
    Embedded(EmbeddedBrowserOutput),
}

type ActorResult = Result<ActorOutput, BrowserError>;

async fn execute_request(state: &mut BrowserState, request: &ActorRequest) -> ActorResult {
    if request.cancellation.reason() != CancellationReason::Active {
        return Err(request.cancellation.error(request.timeout_ms));
    }
    let request_started = Instant::now();
    let remaining = request.deadline.saturating_duration_since(request_started);
    if remaining.is_zero() {
        request.cancellation.cancel(CancellationReason::Deadline);
        return Err(BrowserError::Timeout(request.timeout_ms));
    }
    let runtime_prepared = match &request.op {
        ActorOperation::Agent(op) => state.prepare_browser_runtime(op)?,
        ActorOperation::Embedded(_) => {
            if state.runtime.is_some() || state.browser.is_some() {
                false
            } else {
                state.runtime = Some(
                    BrowserRuntime::discover()
                        .map_err(|error| BrowserError::Message(error.to_string()))?,
                );
                true
            }
        }
    };
    if request.cancellation.reason() != CancellationReason::Active {
        return Err(request.cancellation.error(request.timeout_ms));
    }
    let operation_deadline = if runtime_prepared {
        browser_operation_deadline(Instant::now(), Duration::from_millis(request.timeout_ms))
    } else {
        request.deadline
    };
    let remaining = operation_deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        request.cancellation.cancel(CancellationReason::Deadline);
        return Err(BrowserError::Timeout(request.timeout_ms));
    }
    let deadline = tokio::time::sleep(remaining);
    tokio::pin!(deadline);
    let operation = async {
        match &request.op {
            ActorOperation::Agent(op) => state.execute(op).await.map(ActorOutput::Agent),
            ActorOperation::Embedded(op) => state
                .execute_embedded(op)
                .await
                .map(ActorOutput::Embedded),
        }
    };
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => result,
        () = request.cancellation.wait() => Err(request.cancellation.error(request.timeout_ms)),
        () = &mut deadline => {
            request.cancellation.cancel(CancellationReason::Deadline);
            Err(BrowserError::Timeout(request.timeout_ms))
        }
    }
}

fn browser_operation_deadline(setup_finished: Instant, timeout: Duration) -> Instant {
    setup_finished + timeout
}

fn request_key(request_id: &McpRequestId) -> String {
    match request_id {
        McpRequestId::Number(value) => format!("n:{value}"),
        McpRequestId::String(value) => format!("s:{value}"),
    }
}

fn tool_timeout_from_env() -> Duration {
    let timeout_ms = env::var("RUSTWRIGHT_MCP_TOOL_TIMEOUT_MS")
        .ok()
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TOOL_TIMEOUT_MS)
        .clamp(MIN_TOOL_TIMEOUT_MS, MAX_TOOL_TIMEOUT_MS);
    Duration::from_millis(timeout_ms)
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[derive(Clone)]
struct PageSlot {
    page: Arc<JugglerPage>,
}

struct AttachedPage {
    session_id: String,
    target_id: String,
    browser_context_id: Option<String>,
}

#[derive(Clone, Serialize)]
struct StoredNetworkRecord {
    index: u64,
    target_id: String,
    request: NetworkRequest,
    response: Option<NetworkResponseInfo>,
    finished: Option<NetworkRequestFinished>,
    failed: Option<NetworkRequestFailed>,
}

struct BrowserState {
    runtime: Option<BrowserRuntime>,
    browser: Option<JugglerBrowser>,
    attached_targets: Option<mpsc::UnboundedReceiver<camoufox_juggler::protocol::Event>>,
    pages: Vec<PageSlot>,
    active: usize,
    next_ref: u64,
    current_refs: HashSet<String>,
    console: Arc<Mutex<Vec<ConsoleMessage>>>,
    network: Arc<Mutex<Vec<StoredNetworkRecord>>>,
    workspace: Option<PathBuf>,
}

impl BrowserState {
    fn new(workspace: Option<PathBuf>) -> Self {
        Self {
            runtime: None,
            browser: None,
            attached_targets: None,
            pages: Vec::new(),
            active: 0,
            next_ref: 1,
            current_refs: HashSet::new(),
            console: Arc::new(Mutex::new(Vec::new())),
            network: Arc::new(Mutex::new(Vec::new())),
            workspace,
        }
    }

    fn prepare_browser_runtime(&mut self, op: &BrowserOp) -> Result<bool, BrowserError> {
        if self.runtime.is_some() || self.browser.is_some() || !browser_operation_needs_runtime(op)
        {
            return Ok(false);
        }
        self.runtime = Some(
            BrowserRuntime::discover().map_err(|error| BrowserError::Message(error.to_string()))?,
        );
        Ok(true)
    }

    async fn execute(&mut self, op: &BrowserOp) -> BrowserResult {
        self.sync_attached_pages().await?;
        match op {
            BrowserOp::Navigate(url) => {
                let page = self.ensure_page().await?;
                page.goto(url).await.map_err(|error| browser_error("navigation failed", error))?;
                self.reset_refs();
                self.snapshot(None, None, false, None).await.map(BrowserOutput::Text)
            }
            BrowserOp::NavigateBack => self.history_move(-1).await.map(BrowserOutput::Text),
            BrowserOp::NavigateForward => self.history_move(1).await.map(BrowserOutput::Text),
            BrowserOp::Reload => {
                let page = self.ensure_page().await?;
                page.reload().await.map_err(|error| browser_error("reload failed", error))?;
                page.pump(Duration::from_millis(500)).await;
                self.reset_refs();
                self.snapshot(None, None, false, None).await.map(BrowserOutput::Text)
            }
            BrowserOp::Resize { width, height } => {
                let page = self.ensure_page().await?;
                page.set_viewport(*width, *height)
                    .await
                    .map_err(|error| browser_error("viewport resize failed", error))?;
                self.reset_refs();
                self.snapshot(None, None, false, None).await.map(BrowserOutput::Text)
            }
            BrowserOp::Snapshot { target, depth, boxes } => self
                .snapshot(target.as_deref(), *depth, *boxes, None)
                .await
                .map(BrowserOutput::Text),
            BrowserOp::SnapshotLimited { max_items } => {
                let snapshot = self.snapshot(None, None, false, None).await?;
                Ok(BrowserOutput::Text(
                    snapshot.lines().take(*max_items).collect::<Vec<_>>().join("\n"),
                ))
            }
            BrowserOp::Find { text, regex } => self.find(text.as_deref(), regex.as_ref()).await,
            BrowserOp::Click { target, double_click } => {
                let selector = self.consume_ref_selector(target)?;
                self.click(&selector, *double_click).await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::ClickSelector { selector, double_click } => {
                self.reset_refs();
                self.click(selector, *double_click).await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::ScrollTarget(target) => {
                let selector = self.consume_ref_selector(target)?;
                self.evaluate(&format!(
                    "(() => {{ const el = document.querySelector({}); if (!el) throw new Error('target not found'); el.scrollIntoView({{block:'center', inline:'center'}}); return true; }})()",
                    json!(selector)
                ))
                .await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::ScrollViewport(delta_y) => {
                self.reset_refs();
                self.evaluate(&format!("window.scrollBy(0, {})", json!(delta_y)))
                    .await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::Type { target, text, submit, slowly, clear } => {
                self.type_text(target, text, *submit, *slowly, *clear).await
            }
            BrowserOp::FillSelector { selector, text } => {
                self.reset_refs();
                let sensitive = self.fill_text(selector, text).await?;
                self.write_result(sensitive).await
            }
            BrowserOp::SelectOption { target, values } => {
                let selector = self.consume_ref_selector(target)?;
                self.select_option(&selector, values).await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::FillForm(fields) => self.fill_form(fields).await,
            BrowserOp::Hover(target) => {
                let page = self.ensure_page().await?;
                let selector = self.consume_ref_selector(target)?;
                let (x, y) = element_center(&page, &selector).await?;
                page.mouse_move(x, y).await.map_err(|error| browser_error("hover failed", error))?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::PressKey { target, key } => {
                let page = self.ensure_page().await?;
                if let Some(target) = target {
                    let selector = self.consume_ref_selector(target)?;
                    focus_selector(&page, &selector).await?;
                } else {
                    self.reset_refs();
                }
                page.press_key(key).await.map_err(|error| browser_error("key press failed", error))?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::Drag { start_target, end_target, .. } => {
                self.drag(start_target, end_target).await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::Drop { target, paths, data } => {
                self.drop_data(target, paths, data).await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::ConsoleMessages { level, all, filename } => {
                self.console_messages(*level, *all, filename.as_deref())
            }
            BrowserOp::NetworkRequests { include_static, filter, filename } => {
                self.network_requests(*include_static, filter.as_deref(), filename.as_deref())
            }
            BrowserOp::NetworkRequest { index, part, filename } => {
                self.network_request(*index, *part, filename.as_deref()).await
            }
            BrowserOp::Tabs { action, index, url } => {
                self.tabs(*action, *index, url.as_deref()).await.map(BrowserOutput::Text)
            }
            BrowserOp::HandleDialog { accept, prompt_text } => {
                self.reset_refs();
                self.handle_dialog(*accept, prompt_text.as_deref()).await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::FileUpload(paths) => {
                self.reset_refs();
                self.file_upload(paths).await?;
                self.post_action_snapshot().await.map(BrowserOutput::Text)
            }
            BrowserOp::WaitFor { time_seconds, text, text_gone, timeout_ms } => self
                .wait_for(*time_seconds, text.as_deref(), text_gone.as_deref(), *timeout_ms)
                .await
                .map(BrowserOutput::Text),
            BrowserOp::GetText { selector, max_chars } => self
                .get_text(selector, *max_chars, false)
                .await
                .map(BrowserOutput::Text),
            BrowserOp::GetTextStrict { selector, max_chars } => self
                .get_text(selector, *max_chars, true)
                .await
                .map(BrowserOutput::Text),
            BrowserOp::GetTextRef { target, max_chars } => {
                let selector = self.current_ref_selector(target)?;
                self.get_text(&selector, *max_chars, true)
                    .await
                    .map(BrowserOutput::Text)
            }
            BrowserOp::Evaluate { function, target } => self.evaluate_function(function, target.as_deref()).await,
            BrowserOp::EvaluateWire { expression } => {
                let value = self.evaluate(expression).await?;
                self.reset_refs();
                Ok(BrowserOutput::Text(json_text(&value)))
            }
            BrowserOp::PageInfo => self.page_info().await.map(BrowserOutput::Text),
            BrowserOp::Status => Ok(BrowserOutput::Text(
                json!({
                    "backend": "camoufox-juggler",
                    "browserApi": {
                        "protocol": "juggler-pipe",
                        "driver": "camoufox-juggler",
                        "transport": "native-rust"
                    },
                    "running": self.browser.is_some(),
                    "pid": self.browser.as_ref().and_then(JugglerBrowser::id),
                    "camouflags": self.browser.as_ref().map(|browser| browser.prepared.config.len()).unwrap_or(0),
                    "tabs": self.pages.len(),
                    "activeTab": self.pages.get(self.active).map(|_| self.active),
                })
                .to_string(),
            )),
            BrowserOp::TakeScreenshot { full_page, image_type } => {
                self.take_screenshot(*full_page, *image_type).await
            }
            BrowserOp::Close => {
                self.close().await;
                Ok(BrowserOutput::Text("Browser closed.".to_owned()))
            }
        }
    }

    async fn execute_embedded(
        &mut self,
        op: &EmbeddedBrowserOp,
    ) -> Result<EmbeddedBrowserOutput, BrowserError> {
        self.sync_attached_pages().await?;
        match op {
            EmbeddedBrowserOp::PageInfo => {
                let value = self
                    .evaluate("({url: location.href, title: document.title, width: innerWidth, height: innerHeight, readyState: document.readyState})")
                    .await?;
                Ok(EmbeddedBrowserOutput::Value(value))
            }
            EmbeddedBrowserOp::MouseMove { x, y } => {
                self.ensure_page()
                    .await?
                    .mouse_move(*x, *y)
                    .await
                    .map_err(|error| browser_error("mouse move failed", error))?;
                Ok(EmbeddedBrowserOutput::None)
            }
            EmbeddedBrowserOp::MouseDown { x, y } => {
                self.ensure_page()
                    .await?
                    .mouse_down(
                        *x,
                        *y,
                        camoufox_juggler::MouseButton::Left,
                        camoufox_juggler::Modifiers::NONE,
                        1,
                    )
                    .await
                    .map_err(|error| browser_error("mouse down failed", error))?;
                Ok(EmbeddedBrowserOutput::None)
            }
            EmbeddedBrowserOp::MouseUp { x, y } => {
                self.ensure_page()
                    .await?
                    .mouse_up(
                        *x,
                        *y,
                        camoufox_juggler::MouseButton::Left,
                        camoufox_juggler::Modifiers::NONE,
                        1,
                    )
                    .await
                    .map_err(|error| browser_error("mouse up failed", error))?;
                Ok(EmbeddedBrowserOutput::None)
            }
            EmbeddedBrowserOp::Click { x, y } => {
                self.ensure_page()
                    .await?
                    .click(*x, *y)
                    .await
                    .map_err(|error| browser_error("mouse click failed", error))?;
                Ok(EmbeddedBrowserOutput::None)
            }
            EmbeddedBrowserOp::TypeText(text) => {
                self.ensure_page()
                    .await?
                    .type_text(text)
                    .await
                    .map_err(|error| browser_error("text input failed", error))?;
                Ok(EmbeddedBrowserOutput::None)
            }
            EmbeddedBrowserOp::Scroll { dx, dy } => {
                self.evaluate(&format!("window.scrollBy({}, {})", json!(dx), json!(dy)))
                    .await?;
                Ok(EmbeddedBrowserOutput::None)
            }
            EmbeddedBrowserOp::Tabs => {
                self.pages.retain(|slot| !slot.page.is_detached());
                let mut tabs = Vec::with_capacity(self.pages.len());
                for (index, slot) in self.pages.iter().enumerate() {
                    let title = slot
                        .page
                        .evaluate("document.title")
                        .await
                        .ok()
                        .and_then(|value| value.as_str().map(ToOwned::to_owned))
                        .unwrap_or_default();
                    tabs.push(EmbeddedTab {
                        index,
                        target_id: slot.page.target_id.clone(),
                        title,
                        url: slot.page.url().unwrap_or_default(),
                        active: index == self.active,
                    });
                }
                Ok(EmbeddedBrowserOutput::Tabs(tabs))
            }
            EmbeddedBrowserOp::NewTab(url) => {
                let page = self
                    .ensure_browser()
                    .await?
                    .new_page()
                    .await
                    .map_err(|error| browser_error("new tab failed", error))?;
                let target_id = page.target_id.clone();
                self.attach_page(Arc::clone(&page));
                self.pages.push(PageSlot {
                    page: Arc::clone(&page),
                });
                self.active = self.pages.len() - 1;
                if !url.is_empty() {
                    page.goto(url)
                        .await
                        .map_err(|error| browser_error("new tab navigation failed", error))?;
                }
                self.reset_refs();
                Ok(EmbeddedBrowserOutput::TargetId(target_id))
            }
            EmbeddedBrowserOp::SelectTarget(target_id) => {
                let index = self
                    .pages
                    .iter()
                    .position(|slot| slot.page.target_id == *target_id)
                    .ok_or_else(|| {
                        BrowserError::Message(format!("unknown browser target {target_id}"))
                    })?;
                self.pages[index]
                    .page
                    .bring_to_front()
                    .await
                    .map_err(|error| browser_error("target selection failed", error))?;
                self.active = index;
                self.reset_refs();
                Ok(EmbeddedBrowserOutput::None)
            }
            EmbeddedBrowserOp::CloseTarget(target_id) => {
                let index = self
                    .pages
                    .iter()
                    .position(|slot| slot.page.target_id == *target_id)
                    .ok_or_else(|| {
                        BrowserError::Message(format!("unknown browser target {target_id}"))
                    })?;
                self.pages[index]
                    .page
                    .close()
                    .await
                    .map_err(|error| browser_error("target close failed", error))?;
                self.pages.remove(index);
                self.active = self.active.min(self.pages.len().saturating_sub(1));
                self.reset_refs();
                Ok(EmbeddedBrowserOutput::None)
            }
            EmbeddedBrowserOp::Cookies => {
                let cookies = self
                    .ensure_page()
                    .await?
                    .cookies()
                    .await
                    .map_err(|error| browser_error("cookie read failed", error))?;
                Ok(EmbeddedBrowserOutput::Value(Value::Array(cookies)))
            }
            EmbeddedBrowserOp::SetCookies(cookies) => {
                self.ensure_page()
                    .await?
                    .set_cookies(cookies)
                    .await
                    .map_err(|error| browser_error("cookie update failed", error))?;
                Ok(EmbeddedBrowserOutput::None)
            }
        }
    }

    async fn ensure_browser(&mut self) -> Result<&mut JugglerBrowser, BrowserError> {
        if self.browser.is_none() {
            if self.runtime.is_none() {
                self.runtime = Some(
                    BrowserRuntime::discover()
                        .map_err(|error| BrowserError::Message(error.to_string()))?,
                );
            }
            let runtime = self.runtime.as_ref().ok_or(BrowserError::Stopped)?;
            let launch = prepare_browser_launch(runtime, Some(browser_profile_dir()))
                .await
                .map_err(|error| BrowserError::Message(error.to_string()))?;
            let browser = launch_prepared_with_juggler(&launch.options, launch.prepared)
                .await
                .map_err(|error| browser_error("Camoufox Juggler launch failed", error))?;
            self.attached_targets = Some(browser.attached_targets());
            self.browser = Some(browser);
        }
        self.browser.as_mut().ok_or(BrowserError::Stopped)
    }

    async fn ensure_page(&mut self) -> Result<Arc<JugglerPage>, BrowserError> {
        self.sync_attached_pages().await?;
        self.pages.retain(|slot| !slot.page.is_detached());
        if self.pages.is_empty() {
            let page = self
                .ensure_browser()
                .await?
                .new_page()
                .await
                .map_err(|error| browser_error("failed to create Camoufox page", error))?;
            self.attach_page(Arc::clone(&page));
            self.pages.push(PageSlot { page });
            self.active = 0;
        } else if self.active >= self.pages.len() {
            self.active = self.pages.len() - 1;
        }
        Ok(Arc::clone(&self.pages[self.active].page))
    }

    fn attach_page(&self, page: Arc<JugglerPage>) {
        let mut console_events = page.console_messages();
        let console = Arc::clone(&self.console);
        tokio::spawn(async move {
            while let Some(message) = console_events.next().await {
                let mut records = console.lock().unwrap();
                records.push(message);
                if records.len() > 2_000 {
                    records.drain(..500);
                }
            }
        });

        let target_id = page.target_id.clone();
        let mut network_events = page.network_events();
        let network = Arc::clone(&self.network);
        tokio::spawn(async move {
            loop {
                match network_events.next().await {
                    Ok(Some(event)) => record_network_event(&network, &target_id, event),
                    Ok(None) | Err(_) => break,
                }
            }
        });

        tokio::spawn(async move {
            while !page.is_detached() {
                page.pump(Duration::from_millis(100)).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
    }

    async fn sync_attached_pages(&mut self) -> Result<(), BrowserError> {
        let mut attachments = Vec::new();
        if let Some(events) = self.attached_targets.as_mut() {
            loop {
                match events.try_recv() {
                    Ok(event) => {
                        if let Some(attachment) = attached_page(&event) {
                            attachments.push(attachment);
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        self.attached_targets = None;
                        break;
                    }
                }
            }
        }
        if attachments.is_empty() {
            return Ok(());
        }
        let connection = self
            .browser
            .as_ref()
            .ok_or(BrowserError::Stopped)?
            .connection();
        for attachment in attachments {
            if self
                .pages
                .iter()
                .any(|slot| slot.page.target_id == attachment.target_id)
            {
                continue;
            }
            let page_events = connection.subscribe(&attachment.session_id);
            let page = JugglerPage::new(
                Arc::clone(&connection),
                attachment.session_id,
                attachment.target_id,
                attachment.browser_context_id,
                page_events,
            );
            self.attach_page(Arc::clone(&page));
            self.pages.push(PageSlot { page });
        }
        Ok(())
    }

    fn reset_refs(&mut self) {
        self.current_refs.clear();
    }

    fn current_ref_selector(&self, target: &str) -> Result<String, BrowserError> {
        if !self.current_refs.contains(target) {
            return Err(BrowserError::Message(format!(
                "unknown or stale ref {target}; call browser_snapshot and use its latest refs"
            )));
        }
        ref_selector(target)
    }

    fn consume_ref_selector(&mut self, target: &str) -> Result<String, BrowserError> {
        let selector = self.current_ref_selector(target)?;
        self.reset_refs();
        Ok(selector)
    }

    fn consume_ref_pair(
        &mut self,
        start_target: &str,
        end_target: &str,
    ) -> Result<(String, String), BrowserError> {
        let start = self.current_ref_selector(start_target)?;
        let end = self.current_ref_selector(end_target)?;
        self.reset_refs();
        Ok((start, end))
    }

    async fn evaluate(&mut self, expression: &str) -> Result<Value, BrowserError> {
        self.ensure_page()
            .await?
            .evaluate(expression)
            .await
            .map_err(|error| browser_error("page evaluation failed", error))
    }

    async fn snapshot(
        &mut self,
        target: Option<&str>,
        depth: Option<u32>,
        boxes: bool,
        find: Option<Value>,
    ) -> Result<String, BrowserError> {
        let target = match target {
            Some(target) => Some(self.current_ref_selector(target)?),
            None => None,
        };
        let input = json!({
            "startRef": self.next_ref.max(1),
            "target": target,
            "maxDepth": depth,
            "boxes": boxes,
            "mask": "••••••",
            "find": find,
        });
        let expression = format!("({SNAPSHOT_SCRIPT})({input})");
        let value = self.evaluate(&expression).await?;
        let next_ref = value
            .get("nextRef")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                BrowserError::Message(format!("snapshot returned no nextRef: {value}"))
            })?;
        self.current_refs = value
            .get("refs")
            .and_then(Value::as_array)
            .map(|refs| {
                refs.iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        self.next_ref = next_ref;
        if find.is_some() {
            return render_find(&value);
        }
        value
            .get("outline")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| BrowserError::Message(format!("snapshot returned no outline: {value}")))
    }

    async fn post_action_snapshot(&mut self) -> Result<String, BrowserError> {
        self.sync_attached_pages().await?;
        let page = self.ensure_page().await?;
        if let Some(dialog) = page.dialogs().first() {
            return Ok(format!(
                "Pending {} dialog: {}\nUse browser_handle_dialog before continuing.",
                dialog.kind, dialog.message
            ));
        }
        self.snapshot(None, None, false, None).await
    }

    async fn find(&mut self, text: Option<&str>, regex: Option<&RegexSpec>) -> BrowserResult {
        let query = if let Some(text) = text {
            json!({"kind": "text", "value": text})
        } else if let Some(regex) = regex {
            json!({"kind": "regex", "pattern": regex.pattern, "flags": regex.flags})
        } else {
            return Err(BrowserError::Message(
                "find requires text or regex".to_owned(),
            ));
        };
        self.snapshot(None, None, false, Some(query))
            .await
            .map(BrowserOutput::Text)
    }

    async fn click(&mut self, selector: &str, double_click: bool) -> Result<(), BrowserError> {
        let page = self.ensure_page().await?;
        let (x, y) = element_center(&page, selector).await?;
        let result = if double_click {
            page.double_click(x, y).await
        } else {
            page.click(x, y).await
        };
        result.map_err(|error| browser_error("click failed", error))
    }

    async fn history_move(&mut self, delta: i32) -> Result<String, BrowserError> {
        let page = self.ensure_page().await?;
        let length = page
            .evaluate("history.length")
            .await
            .map_err(|error| browser_error("history inspection failed", error))?
            .as_u64()
            .unwrap_or(0);
        if length <= 1 {
            return Err(BrowserError::Message(if delta < 0 {
                "no back history".to_owned()
            } else {
                "no forward history".to_owned()
            }));
        }
        page.evaluate(&format!("history.go({delta})"))
            .await
            .map_err(|error| browser_error("history navigation failed", error))?;
        page.pump(Duration::from_secs(2)).await;
        self.reset_refs();
        self.snapshot(None, None, false, None).await
    }

    async fn type_text(
        &mut self,
        target: &str,
        text: &str,
        submit: bool,
        slowly: bool,
        clear: bool,
    ) -> BrowserResult {
        let selector = self.consume_ref_selector(target)?;
        let page = self.ensure_page().await?;
        let sensitive = is_password(&page, &selector).await?;
        focus_selector(&page, &selector).await?;
        if clear {
            set_text_value(&page, &selector, "").await?;
        }
        if slowly {
            for character in text.chars() {
                page.type_text(&character.to_string())
                    .await
                    .map_err(|error| browser_error("typing failed", error))?;
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        } else {
            page.type_text(text)
                .await
                .map_err(|error| browser_error("typing failed", error))?;
        }
        if submit {
            page.press_key("Enter")
                .await
                .map_err(|error| browser_error("submit key failed", error))?;
        }
        self.write_result(sensitive).await
    }

    async fn fill_text(&mut self, selector: &str, text: &str) -> Result<bool, BrowserError> {
        let page = self.ensure_page().await?;
        let sensitive = is_password(&page, selector).await?;
        set_text_value(&page, selector, text).await?;
        Ok(sensitive)
    }

    async fn write_result(&mut self, sensitive: bool) -> BrowserResult {
        if sensitive {
            Ok(BrowserOutput::Text(
                "Sensitive field updated; post-action snapshot omitted to prevent credential disclosure."
                    .to_owned(),
            ))
        } else {
            self.post_action_snapshot().await.map(BrowserOutput::Text)
        }
    }

    async fn select_option(
        &mut self,
        selector: &str,
        values: &[String],
    ) -> Result<(), BrowserError> {
        let page = self.ensure_page().await?;
        let expression = format!(
            "(() => {{ const el = document.querySelector({selector}); if (!(el instanceof HTMLSelectElement)) throw new Error('target is not a select'); const values = new Set({values}); for (const option of el.options) option.selected = values.has(option.value) || values.has(option.label); el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()",
            selector = json!(selector),
            values = json!(values),
        );
        page.evaluate(&expression)
            .await
            .map_err(|error| browser_error("select option failed", error))?;
        Ok(())
    }

    async fn fill_form(&mut self, fields: &[FillField]) -> BrowserResult {
        for field in fields {
            self.current_ref_selector(&field.target)?;
        }
        self.reset_refs();
        let mut sensitive = false;
        for field in fields {
            let selector = ref_selector(&field.target)?;
            match field.kind {
                FillFieldKind::Textbox => {
                    sensitive |= self.fill_text(&selector, &field.value).await?;
                }
                FillFieldKind::Checkbox | FillFieldKind::Radio => {
                    let checked = parse_bool(&field.value).ok_or_else(|| {
                        BrowserError::Message(format!("{} value must be true or false", field.name))
                    })?;
                    self.evaluate(&format!(
                        "(() => {{ const el = document.querySelector({}); if (!(el instanceof HTMLInputElement)) throw new Error('target is not an input'); if (el.checked !== {}) el.click(); return el.checked; }})()",
                        json!(selector),
                        checked
                    ))
                    .await?;
                }
                FillFieldKind::Combobox => {
                    self.select_option(&selector, std::slice::from_ref(&field.value))
                        .await?;
                }
                FillFieldKind::Slider => {
                    self.evaluate(&format!(
                        "(() => {{ const el = document.querySelector({}); if (!(el instanceof HTMLInputElement)) throw new Error('target is not an input'); el.value = {}; el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); return el.value; }})()",
                        json!(selector),
                        json!(field.value)
                    ))
                    .await?;
                }
            }
        }
        self.write_result(sensitive).await
    }

    async fn drag(&mut self, start: &str, end: &str) -> Result<(), BrowserError> {
        let page = self.ensure_page().await?;
        let (start_selector, end_selector) = self.consume_ref_pair(start, end)?;
        let (start_x, start_y) = element_center(&page, &start_selector).await?;
        let (end_x, end_y) = element_center(&page, &end_selector).await?;
        page.mouse_move(start_x, start_y)
            .await
            .map_err(|error| browser_error("drag start failed", error))?;
        page.mouse_down(
            start_x,
            start_y,
            camoufox_juggler::MouseButton::Left,
            camoufox_juggler::Modifiers::NONE,
            1,
        )
        .await
        .map_err(|error| browser_error("drag mouse down failed", error))?;
        page.mouse_move(end_x, end_y)
            .await
            .map_err(|error| browser_error("drag move failed", error))?;
        page.mouse_up(
            end_x,
            end_y,
            camoufox_juggler::MouseButton::Left,
            camoufox_juggler::Modifiers::NONE,
            1,
        )
        .await
        .map_err(|error| browser_error("drag mouse up failed", error))
    }

    async fn drop_data(
        &mut self,
        target: &str,
        paths: &[String],
        data: &[(String, String)],
    ) -> Result<(), BrowserError> {
        let selector = self.consume_ref_selector(target)?;
        if paths.len() > MAX_FILE_INPUTS {
            return Err(BrowserError::Message(format!(
                "drop accepts at most {MAX_FILE_INPUTS} files"
            )));
        }
        let mut files = Vec::new();
        let mut total_bytes = 0;
        for raw in paths {
            let path = self.resolve_input_path(raw)?;
            let metadata = fs::metadata(&path).map_err(|error| {
                BrowserError::Message(format!("cannot inspect {}: {error}", path.display()))
            })?;
            if metadata.len() > MAX_FILE_INPUT_BYTES {
                return Err(BrowserError::Message(format!(
                    "drop file exceeds {} bytes: {}",
                    MAX_FILE_INPUT_BYTES,
                    path.display()
                )));
            }
            total_bytes = accumulate_drop_bytes(total_bytes, metadata.len())?;
            let bytes = fs::read(&path).map_err(|error| {
                BrowserError::Message(format!("cannot read {}: {error}", path.display()))
            })?;
            files.push(json!({
                "name": path.file_name().and_then(|name| name.to_str()).unwrap_or("file"),
                "mime": "application/octet-stream",
                "base64": base64::engine::general_purpose::STANDARD.encode(bytes),
            }));
        }
        let expression = format!(
            "(async () => {{ const target = document.querySelector({selector}); if (!target) throw new Error('drop target not found'); const input = {input}; const transfer = new DataTransfer(); for (const entry of input.files) {{ const binary = atob(entry.base64); const bytes = new Uint8Array(binary.length); for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i); transfer.items.add(new File([bytes], entry.name, {{type: entry.mime, lastModified: 0}})); }} for (const [mime, value] of input.data) transfer.setData(mime, value); const bounds = target.getBoundingClientRect(); const options = {{bubbles:true, cancelable:true, composed:true, clientX:bounds.left + bounds.width/2, clientY:bounds.top + bounds.height/2, dataTransfer:transfer}}; for (const type of ['dragenter','dragover','drop']) target.dispatchEvent(new DragEvent(type, options)); await new Promise(resolve => setTimeout(resolve, 0)); return true; }})()",
            selector = json!(selector),
            input = json!({"files": files, "data": data}),
        );
        self.evaluate(&expression).await?;
        Ok(())
    }

    fn console_messages(
        &self,
        minimum: RequestedConsoleLevel,
        all: bool,
        filename: Option<&str>,
    ) -> BrowserResult {
        let records = self.console.lock().unwrap();
        let start = if all {
            0
        } else {
            records.len().saturating_sub(100)
        };
        let messages = records[start..]
            .iter()
            .filter(|message| console_level_matches(minimum, message.level))
            .cloned()
            .collect::<Vec<_>>();
        let text = serde_json::to_string_pretty(&messages).map_err(|error| {
            BrowserError::Message(format!("console serialization failed: {error}"))
        })?;
        self.output_or_file(filename, &text, "console messages")
    }

    fn network_requests(
        &self,
        include_static: bool,
        filter: Option<&str>,
        filename: Option<&str>,
    ) -> BrowserResult {
        let filter = filter.map(str::to_lowercase);
        let records = self
            .network
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                (include_static || !is_static_resource(&record.request.resource_type))
                    && filter.as_ref().is_none_or(|filter| {
                        record.request.url.to_lowercase().contains(filter)
                            || record.request.method.to_lowercase().contains(filter)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        let text = serde_json::to_string_pretty(&records).map_err(|error| {
            BrowserError::Message(format!("network serialization failed: {error}"))
        })?;
        self.output_or_file(filename, &text, "network requests")
    }

    async fn network_request(
        &mut self,
        index: u64,
        part: Option<NetworkPart>,
        filename: Option<&str>,
    ) -> BrowserResult {
        let record = self
            .network
            .lock()
            .unwrap()
            .iter()
            .find(|record| record.index == index)
            .cloned()
            .ok_or_else(|| BrowserError::Message(format!("network request {index} not found")))?;
        let value = match part {
            Some(NetworkPart::RequestHeaders) => json!(record.request.headers),
            Some(NetworkPart::RequestBody) => json!(record.request.post_data),
            Some(NetworkPart::ResponseHeaders) => {
                json!(record.response.map(|response| response.headers))
            }
            Some(NetworkPart::ResponseBody) => {
                let page = self
                    .pages
                    .iter()
                    .find(|slot| slot.page.target_id == record.target_id)
                    .map(|slot| Arc::clone(&slot.page))
                    .ok_or_else(|| {
                        BrowserError::Message("request page is no longer available".to_owned())
                    })?;
                let body = page
                    .response_body(&record.request.request_id)
                    .await
                    .map_err(|error| browser_error("response body fetch failed", error))?;
                Value::String(String::from_utf8_lossy(&body).into_owned())
            }
            None => serde_json::to_value(record).map_err(|error| {
                BrowserError::Message(format!("network serialization failed: {error}"))
            })?,
        };
        let text = serde_json::to_string_pretty(&value).map_err(|error| {
            BrowserError::Message(format!("network serialization failed: {error}"))
        })?;
        self.output_or_file(filename, &text, "network request")
    }

    async fn tabs(
        &mut self,
        action: TabAction,
        index: Option<usize>,
        url: Option<&str>,
    ) -> Result<String, BrowserError> {
        self.sync_attached_pages().await?;
        self.pages.retain(|slot| !slot.page.is_detached());
        match action {
            TabAction::List => self.tab_list().await,
            TabAction::New => {
                let page = self
                    .ensure_browser()
                    .await?
                    .new_page()
                    .await
                    .map_err(|error| browser_error("new tab failed", error))?;
                self.attach_page(Arc::clone(&page));
                self.pages.push(PageSlot {
                    page: Arc::clone(&page),
                });
                self.active = self.pages.len() - 1;
                if let Some(url) = url {
                    page.goto(url)
                        .await
                        .map_err(|error| browser_error("new tab navigation failed", error))?;
                }
                self.reset_refs();
                self.snapshot(None, None, false, None).await
            }
            TabAction::Close => {
                if self.pages.is_empty() {
                    return Ok("No tabs are open.".to_owned());
                }
                let index = index.unwrap_or(self.active);
                if index >= self.pages.len() {
                    return Err(BrowserError::Message(format!(
                        "tab index {index} is out of range"
                    )));
                }
                self.pages[index]
                    .page
                    .close()
                    .await
                    .map_err(|error| browser_error("tab close failed", error))?;
                self.pages.remove(index);
                self.active = self.active.min(self.pages.len().saturating_sub(1));
                self.reset_refs();
                self.tab_list().await
            }
            TabAction::Select => {
                let index = index
                    .ok_or_else(|| BrowserError::Message("tab index is required".to_owned()))?;
                let page = self
                    .pages
                    .get(index)
                    .ok_or_else(|| {
                        BrowserError::Message(format!("tab index {index} is out of range"))
                    })?
                    .page
                    .clone();
                page.bring_to_front()
                    .await
                    .map_err(|error| browser_error("tab selection failed", error))?;
                self.active = index;
                self.reset_refs();
                self.snapshot(None, None, false, None).await
            }
        }
    }

    async fn tab_list(&self) -> Result<String, BrowserError> {
        let mut lines = Vec::new();
        for (index, slot) in self.pages.iter().enumerate() {
            let title = slot
                .page
                .evaluate("document.title")
                .await
                .ok()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            let url = slot.page.url().unwrap_or_default();
            lines.push(format!(
                "{} {index}: {title} — {url}",
                if index == self.active { "*" } else { " " }
            ));
        }
        Ok(if lines.is_empty() {
            "No tabs are open.".to_owned()
        } else {
            lines.join("\n")
        })
    }

    async fn handle_dialog(
        &mut self,
        accept: bool,
        prompt_text: Option<&str>,
    ) -> Result<(), BrowserError> {
        let page = self.ensure_page().await?;
        let dialog = page
            .dialogs()
            .into_iter()
            .next()
            .ok_or_else(|| BrowserError::Message("no dialog is pending".to_owned()))?;
        page.handle_dialog(&dialog.id, accept, prompt_text)
            .await
            .map_err(|error| browser_error("dialog handling failed", error))
    }

    async fn file_upload(&mut self, paths: &[String]) -> Result<(), BrowserError> {
        if paths.len() > MAX_FILE_INPUTS {
            return Err(BrowserError::Message(format!(
                "file upload accepts at most {MAX_FILE_INPUTS} files"
            )));
        }
        let mut total_bytes = 0;
        let mut files = Vec::with_capacity(paths.len());
        for raw in paths {
            let path = self.resolve_input_path(raw)?;
            let size = fs::metadata(&path)
                .map_err(|error| {
                    BrowserError::Message(format!("cannot inspect {}: {error}", path.display()))
                })?
                .len();
            total_bytes = accumulate_upload_bytes(total_bytes, size)?;
            files.push(path);
        }
        let page = self.ensure_page().await?;
        page.set_intercept_file_chooser(true)
            .await
            .map_err(|error| browser_error("file chooser interception failed", error))?;
        let chooser = page
            .wait_for_file_chooser(Duration::from_secs(5))
            .await
            .map_err(|error| browser_error("no file chooser is pending", error))?;
        chooser
            .set_files(&files)
            .await
            .map_err(|error| browser_error("file upload failed", error))
    }

    async fn wait_for(
        &mut self,
        time_seconds: Option<f64>,
        text: Option<&str>,
        text_gone: Option<&str>,
        timeout_ms: f64,
    ) -> Result<String, BrowserError> {
        if let Some(seconds) = time_seconds {
            tokio::time::sleep(Duration::from_secs_f64(seconds.max(0.0))).await;
        }
        if text.is_some() || text_gone.is_some() {
            let deadline = tokio::time::Instant::now()
                + Duration::from_secs_f64((timeout_ms.max(0.0)) / 1000.0);
            loop {
                let content = self
                    .evaluate("document.body ? document.body.innerText : ''")
                    .await?
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let present = text.is_none_or(|needle| content.contains(needle));
                let gone = text_gone.is_none_or(|needle| !content.contains(needle));
                if present && gone {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(BrowserError::Message("wait condition timed out".to_owned()));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        self.snapshot(None, None, false, None).await
    }

    async fn get_text(
        &mut self,
        selector: &str,
        max_chars: usize,
        strict: bool,
    ) -> Result<String, BrowserError> {
        let value = self
            .evaluate(&format!(
                "(() => {{ const el = document.querySelector({}); return el ? (el.innerText ?? el.textContent ?? '') : null; }})()",
                json!(selector)
            ))
            .await?;
        if value.is_null() && strict {
            return Err(BrowserError::Message(format!(
                "selector matched no element: {selector}"
            )));
        }
        let mut text = value.as_str().unwrap_or_default().to_owned();
        if text.chars().count() > max_chars {
            text = text.chars().take(max_chars).collect();
            text.push('…');
        }
        Ok(text)
    }

    async fn evaluate_function(&mut self, function: &str, target: Option<&str>) -> BrowserResult {
        let expression = match target {
            Some(target) => {
                let selector = self.current_ref_selector(target)?;
                format!(
                    "(() => {{ const el = document.querySelector({}); if (!el) throw new Error('target not found'); return ({function})(el); }})()",
                    json!(selector)
                )
            }
            None => format!("({function})()"),
        };
        let value = self.evaluate(&expression).await?;
        self.reset_refs();
        Ok(BrowserOutput::Text(json_text(&value)))
    }

    async fn page_info(&mut self) -> Result<String, BrowserError> {
        let value = self
            .evaluate("({url: location.href, title: document.title, width: innerWidth, height: innerHeight, readyState: document.readyState})")
            .await?;
        Ok(json_text(&value))
    }

    async fn take_screenshot(
        &mut self,
        full_page: bool,
        image_type: ScreenshotType,
    ) -> BrowserResult {
        static NEXT_SCREENSHOT: AtomicUsize = AtomicUsize::new(1);
        let page = self.ensure_page().await?;
        let old_size = page
            .evaluate("({width: innerWidth, height: innerHeight})")
            .await
            .map_err(|error| browser_error("viewport inspection failed", error))?;
        if full_page {
            let dimensions = page
                .evaluate("({width: Math.min(16384, Math.max(document.documentElement.scrollWidth, document.body?.scrollWidth || 0)), height: Math.min(16384, Math.max(document.documentElement.scrollHeight, document.body?.scrollHeight || 0))})")
                .await
                .map_err(|error| browser_error("page dimensions failed", error))?;
            let width = dimensions
                .get("width")
                .and_then(Value::as_u64)
                .unwrap_or(1280) as u32;
            let height = dimensions
                .get("height")
                .and_then(Value::as_u64)
                .unwrap_or(720) as u32;
            page.set_viewport(width.max(1), height.max(1))
                .await
                .map_err(|error| browser_error("full-page viewport failed", error))?;
        }
        let extension = match image_type {
            ScreenshotType::Png => "png",
            ScreenshotType::Jpeg => "jpg",
        };
        let mime = match image_type {
            ScreenshotType::Png => "image/png",
            ScreenshotType::Jpeg => "image/jpeg",
        };
        let path = env::temp_dir().join(format!(
            "code-camoufox-screenshot-{}-{}.{}",
            std::process::id(),
            NEXT_SCREENSHOT.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        let screenshot_result = page.screenshot_with_mime(&path, mime).await;
        if full_page {
            let width = old_size
                .get("width")
                .and_then(Value::as_u64)
                .unwrap_or(1280) as u32;
            let height = old_size
                .get("height")
                .and_then(Value::as_u64)
                .unwrap_or(720) as u32;
            let _ = page.set_viewport(width.max(1), height.max(1)).await;
        }
        screenshot_result.map_err(|error| browser_error("screenshot failed", error))?;
        let bytes = fs::read(&path)
            .map_err(|error| BrowserError::Message(format!("screenshot read failed: {error}")))?;
        let _ = fs::remove_file(&path);
        validate_screenshot(&bytes).map_err(|error| BrowserError::Message(error.to_string()))?;
        Ok(BrowserOutput::Image {
            bytes,
            mime,
            extension,
        })
    }

    fn resolve_input_path(&self, raw: &str) -> Result<PathBuf, BrowserError> {
        let workspace = self.workspace.as_ref().ok_or_else(|| {
            BrowserError::Message("the configured workspace must be set for file inputs".to_owned())
        })?;
        if !workspace.is_absolute() {
            return Err(BrowserError::Message(
                "the configured workspace must be an absolute path".to_owned(),
            ));
        }
        let workspace = workspace.canonicalize().map_err(|error| {
            BrowserError::Message(format!("the configured workspace is unavailable: {error}"))
        })?;
        let path = PathBuf::from(raw);
        let resolved = if path.is_absolute() {
            path
        } else {
            workspace.join(path)
        };
        let resolved = resolved.canonicalize().map_err(|error| {
            BrowserError::Message(format!("file input is unavailable: {raw}: {error}"))
        })?;
        if !resolved.starts_with(&workspace) {
            return Err(BrowserError::Message(format!(
                "file inputs are confined to the configured workspace ({}); got {raw}",
                workspace.display()
            )));
        }
        let metadata = resolved.metadata().map_err(|error| {
            BrowserError::Message(format!("file input metadata failed: {raw}: {error}"))
        })?;
        if !metadata.is_file() {
            return Err(BrowserError::Message(format!(
                "file input is not a regular file: {raw}"
            )));
        }
        if metadata.len() > MAX_FILE_INPUT_BYTES {
            return Err(BrowserError::Message(format!(
                "file input exceeds the {MAX_FILE_INPUT_BYTES}-byte per-file cap: {raw}"
            )));
        }
        Ok(resolved)
    }

    fn output_or_file(&self, filename: Option<&str>, text: &str, label: &str) -> BrowserResult {
        let Some(filename) = filename else {
            return Ok(BrowserOutput::Text(text.to_owned()));
        };
        let path = self.resolve_output_path(filename)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                BrowserError::Message(format!("could not write {}: {error}", path.display()))
            })?;
        if let Err(error) = output.write_all(text.as_bytes()) {
            drop(output);
            let _ = fs::remove_file(&path);
            return Err(BrowserError::Message(format!(
                "could not write {}: {error}",
                path.display()
            )));
        }
        Ok(BrowserOutput::Text(format!(
            "Saved {label} to `{}`.",
            path.display()
        )))
    }

    fn resolve_output_path(&self, filename: &str) -> Result<PathBuf, BrowserError> {
        if filename.is_empty() {
            return Err(BrowserError::Message(
                "output filename must be non-empty".to_owned(),
            ));
        }
        let path = Path::new(filename);
        if path.is_absolute() {
            return Err(BrowserError::Message(
                "output filename must be relative to the configured workspace".to_owned(),
            ));
        }
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(BrowserError::Message(
                "output filename cannot contain parent-directory components".to_owned(),
            ));
        }
        let workspace = self.workspace.as_ref().ok_or_else(|| {
            BrowserError::Message("output filename requires RUSTWRIGHT_MCP_WORKSPACE".to_owned())
        })?;
        if !workspace.is_absolute() {
            return Err(BrowserError::Message(
                "the configured workspace must be an absolute path".to_owned(),
            ));
        }
        let workspace = workspace.canonicalize().map_err(|error| {
            BrowserError::Message(format!("the configured workspace is unavailable: {error}"))
        })?;
        let candidate = workspace.join(path);
        let file_name = candidate
            .file_name()
            .ok_or_else(|| BrowserError::Message("output filename must name a file".to_owned()))?;
        let parent = candidate.parent().ok_or_else(|| {
            BrowserError::Message("output filename must have a parent directory".to_owned())
        })?;
        let parent = parent.canonicalize().map_err(|error| {
            BrowserError::Message(format!("output directory is unavailable: {error}"))
        })?;
        if !parent.starts_with(&workspace) {
            return Err(BrowserError::Message(format!(
                "output is confined to the configured workspace ({})",
                workspace.display()
            )));
        }
        let resolved = parent.join(file_name);
        if resolved.symlink_metadata().is_ok() {
            return Err(BrowserError::Message(
                "output file already exists".to_owned(),
            ));
        }
        Ok(resolved)
    }

    async fn close(&mut self) {
        self.pages.clear();
        self.attached_targets = None;
        self.active = 0;
        self.reset_refs();
        if let Some(mut browser) = self.browser.take() {
            let _ = browser.close().await;
        }
    }
}

fn browser_operation_needs_runtime(op: &BrowserOp) -> bool {
    match op {
        BrowserOp::Status
        | BrowserOp::Close
        | BrowserOp::ConsoleMessages { .. }
        | BrowserOp::NetworkRequests { .. }
        | BrowserOp::NetworkRequest { .. } => false,
        BrowserOp::Tabs { action, .. } => matches!(action, TabAction::New),
        _ => true,
    }
}

fn record_network_event(
    records: &Arc<Mutex<Vec<StoredNetworkRecord>>>,
    target_id: &str,
    event: NetworkEvent,
) {
    let mut records = records.lock().unwrap();
    match event {
        NetworkEvent::RequestWillBeSent(request) => {
            let index = records.last().map_or(1, |record| record.index + 1);
            records.push(StoredNetworkRecord {
                index,
                target_id: target_id.to_owned(),
                request,
                response: None,
                finished: None,
                failed: None,
            });
        }
        NetworkEvent::ResponseReceived(response) => {
            if let Some(record) = records
                .iter_mut()
                .rev()
                .find(|record| record.request.request_id == response.request_id)
            {
                record.response = Some(response);
            }
        }
        NetworkEvent::RequestFinished(finished) => {
            if let Some(record) = records
                .iter_mut()
                .rev()
                .find(|record| record.request.request_id == finished.request_id)
            {
                record.finished = Some(finished);
            }
        }
        NetworkEvent::RequestFailed(failed) => {
            if let Some(record) = records
                .iter_mut()
                .rev()
                .find(|record| record.request.request_id == failed.request_id)
            {
                record.failed = Some(failed);
            }
        }
        _ => {}
    }
    if records.len() > 5_000 {
        records.drain(..1_000);
        for (index, record) in records.iter_mut().enumerate() {
            record.index = index as u64 + 1;
        }
    }
}

async fn element_center(page: &JugglerPage, selector: &str) -> Result<(f64, f64), BrowserError> {
    let quads = page
        .element_quads(selector)
        .await
        .map_err(|error| browser_error("element bounds failed", error))?
        .ok_or_else(|| BrowserError::Message(format!("selector matched no element: {selector}")))?;
    let center = quads
        .first()
        .map(camoufox_juggler::Quad::center)
        .ok_or_else(|| {
            BrowserError::Message(format!("element has no clickable bounds: {selector}"))
        })?;
    Ok((center.x, center.y))
}

async fn focus_selector(page: &JugglerPage, selector: &str) -> Result<(), BrowserError> {
    page.evaluate(&format!(
        "(() => {{ const el = document.querySelector({}); if (!el) throw new Error('target not found'); el.focus(); return true; }})()",
        json!(selector)
    ))
    .await
    .map_err(|error| browser_error("focus failed", error))?;
    Ok(())
}

async fn set_text_value(
    page: &JugglerPage,
    selector: &str,
    value: &str,
) -> Result<(), BrowserError> {
    page.evaluate(&format!(
        "(() => {{ const el = document.querySelector({selector}); if (!el || !('value' in el)) throw new Error('target is not editable'); const prototype = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value'); descriptor.set.call(el, {value}); el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()",
        selector = json!(selector),
        value = json!(value),
    ))
    .await
    .map_err(|error| browser_error("field update failed", error))?;
    Ok(())
}

async fn is_password(page: &JugglerPage, selector: &str) -> Result<bool, BrowserError> {
    page.evaluate(&format!(
        "(() => {{ const el = document.querySelector({}); return el instanceof HTMLInputElement && el.type === 'password'; }})()",
        json!(selector)
    ))
    .await
    .map_err(|error| browser_error("field inspection failed", error))?
    .as_bool()
    .ok_or_else(|| BrowserError::Message("field inspection returned no boolean".to_owned()))
}

fn ref_selector(target: &str) -> Result<String, BrowserError> {
    let Some(digits) = target.strip_prefix('e') else {
        return Err(BrowserError::Message(format!(
            "invalid browser ref: {target}"
        )));
    };
    if digits.is_empty()
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(BrowserError::Message(format!(
            "invalid browser ref: {target}"
        )));
    }
    Ok(format!(r#"[data-mcp-ref="{target}"]"#))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn render_find(value: &Value) -> Result<String, BrowserError> {
    let find = value
        .get("find")
        .ok_or_else(|| BrowserError::Message(format!("find returned no result: {value}")))?;
    let total = find
        .get("totalMatches")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let matches = find
        .get("matches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lines = vec![format!("Found {total} matching elements.")];
    for (index, item) in matches.iter().enumerate() {
        let path = item.get("path").and_then(Value::as_str).unwrap_or("(root)");
        let line = item.get("line").and_then(Value::as_str).unwrap_or_default();
        lines.push(format!("{}. {path}\n   {line}", index + 1));
    }
    if find.get("incomplete").and_then(Value::as_bool) == Some(true) {
        lines.push(format!(
            "Search was incomplete: {}",
            find.get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown limit")
        ));
    }
    Ok(lines.join("\n"))
}

fn console_level_matches(
    minimum: RequestedConsoleLevel,
    actual: camoufox_juggler::ConsoleLevel,
) -> bool {
    let requested = match minimum {
        RequestedConsoleLevel::Error => 4,
        RequestedConsoleLevel::Warning => 3,
        RequestedConsoleLevel::Info => 2,
        RequestedConsoleLevel::Debug => 1,
    };
    let actual = match actual {
        camoufox_juggler::ConsoleLevel::Error | camoufox_juggler::ConsoleLevel::Assert => 4,
        camoufox_juggler::ConsoleLevel::Warning => 3,
        camoufox_juggler::ConsoleLevel::Log
        | camoufox_juggler::ConsoleLevel::Info
        | camoufox_juggler::ConsoleLevel::TimeEnd
        | camoufox_juggler::ConsoleLevel::Count => 2,
        _ => 1,
    };
    actual >= requested
}

fn is_static_resource(resource_type: &str) -> bool {
    matches!(
        resource_type.to_ascii_lowercase().as_str(),
        "image" | "stylesheet" | "font" | "media"
    )
}

fn json_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn browser_error(context: &str, error: impl std::fmt::Display) -> BrowserError {
    BrowserError::Message(format!("{context}: {error}"))
}

fn attached_page(event: &camoufox_juggler::protocol::Event) -> Option<AttachedPage> {
    if event.method != "Browser.attachedToTarget"
        || event
            .params
            .pointer("/targetInfo/type")
            .and_then(Value::as_str)
            != Some("page")
    {
        return None;
    }
    let session_id = event.params.get("sessionId")?.as_str()?.to_owned();
    let target_id = event
        .params
        .pointer("/targetInfo/targetId")?
        .as_str()?
        .to_owned();
    let browser_context_id = event
        .params
        .pointer("/targetInfo/browserContextId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Some(AttachedPage {
        session_id,
        target_id,
        browser_context_id,
    })
}

fn accumulate_file_input_bytes(
    operation: &str,
    current: u64,
    next: u64,
) -> Result<u64, BrowserError> {
    let total = current.checked_add(next).ok_or_else(|| {
        BrowserError::Message(format!("{operation} file-size accounting overflow"))
    })?;
    if total > MAX_FILE_INPUT_TOTAL_BYTES {
        return Err(BrowserError::Message(format!(
            "{operation} files exceed {MAX_FILE_INPUT_TOTAL_BYTES} bytes in total"
        )));
    }
    Ok(total)
}

fn accumulate_drop_bytes(current: u64, next: u64) -> Result<u64, BrowserError> {
    accumulate_file_input_bytes("drop", current, next)
}

fn accumulate_upload_bytes(current: u64, next: u64) -> Result<u64, BrowserError> {
    accumulate_file_input_bytes("file upload", current, next)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn backend_is_camoufox_and_refs_use_snapshot_attribute() {
        assert_eq!(ref_selector("e42").unwrap(), r#"[data-mcp-ref="e42"]"#);
        assert!(ref_selector("e0").is_err());
        assert!(ref_selector("selector").is_err());
    }

    #[test]
    fn boolean_form_values_are_explicit() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("OFF"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }

    #[test]
    fn browser_operation_deadline_starts_after_runtime_setup() {
        let request_started = Instant::now();
        let setup_finished = request_started + Duration::from_secs(75);
        let timeout = Duration::from_secs(60);

        assert_eq!(
            browser_operation_deadline(setup_finished, timeout),
            setup_finished + timeout
        );
    }

    #[test]
    fn browser_runtime_setup_only_precedes_browser_starting_operations() {
        assert!(browser_operation_needs_runtime(&BrowserOp::Navigate(
            "https://example.test".to_owned()
        )));
        assert!(browser_operation_needs_runtime(&BrowserOp::Tabs {
            action: TabAction::New,
            index: None,
            url: None,
        }));
        assert!(!browser_operation_needs_runtime(&BrowserOp::Status));
        assert!(!browser_operation_needs_runtime(&BrowserOp::Tabs {
            action: TabAction::List,
            index: None,
            url: None,
        }));
    }

    #[test]
    fn snapshot_refs_are_current_and_one_shot_for_actions() {
        let mut state = BrowserState::new(None);
        state.current_refs.insert("e7".to_owned());

        assert_eq!(
            state.consume_ref_selector("e7").unwrap(),
            r#"[data-mcp-ref="e7"]"#
        );
        assert!(state.current_refs.is_empty());

        let error = state.consume_ref_selector("e7").unwrap_err().to_string();
        assert!(error.contains("unknown or stale ref e7"), "{error}");
    }

    #[test]
    fn file_inputs_are_canonicalized_and_confined_to_workspace() {
        let root = tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("inside.txt"), b"inside").unwrap();
        fs::write(root.path().join("outside.txt"), b"outside").unwrap();
        let state = BrowserState::new(Some(workspace.clone()));

        assert_eq!(
            state.resolve_input_path("inside.txt").unwrap(),
            workspace.join("inside.txt").canonicalize().unwrap()
        );
        let outside = root.path().join("outside.txt");
        let error = state
            .resolve_input_path(outside.to_str().unwrap())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("confined to the configured workspace"),
            "{error}"
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, workspace.join("escape.txt")).unwrap();
            let error = state
                .resolve_input_path("escape.txt")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("confined to the configured workspace"),
                "{error}"
            );
        }
    }

    #[test]
    fn file_inputs_enforce_the_per_file_size_limit() {
        let root = tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let oversized = fs::File::create(workspace.join("oversized.bin")).unwrap();
        oversized.set_len(MAX_FILE_INPUT_BYTES + 1).unwrap();
        let state = BrowserState::new(Some(workspace));

        let error = state
            .resolve_input_path("oversized.bin")
            .unwrap_err()
            .to_string();
        assert!(error.contains("per-file cap"), "{error}");
    }

    #[test]
    fn output_paths_reject_symlink_escape_and_existing_files() {
        let root = tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(workspace.join("existing.txt"), b"keep").unwrap();
        let state = BrowserState::new(Some(workspace.clone()));

        let error = state
            .resolve_output_path("existing.txt")
            .unwrap_err()
            .to_string();
        assert!(error.contains("already exists"), "{error}");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, workspace.join("escape")).unwrap();
            let error = state
                .resolve_output_path("escape/result.txt")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("confined to the configured workspace"),
                "{error}"
            );
        }
    }

    #[test]
    fn drop_inputs_enforce_an_aggregate_size_limit() {
        let first = accumulate_drop_bytes(0, MAX_FILE_INPUT_BYTES / 2 + 1).unwrap();
        let error = accumulate_drop_bytes(first, MAX_FILE_INPUT_BYTES / 2 + 1)
            .unwrap_err()
            .to_string();

        assert!(error.contains("drop files exceed"), "{error}");
        assert!(error.contains("in total"), "{error}");
    }

    #[test]
    fn aggregate_file_input_limit_accepts_the_exact_boundary_and_rejects_overflow() {
        assert_eq!(
            accumulate_drop_bytes(0, MAX_FILE_INPUT_BYTES).unwrap(),
            MAX_FILE_INPUT_BYTES
        );
        assert_eq!(
            accumulate_upload_bytes(0, MAX_FILE_INPUT_BYTES).unwrap(),
            MAX_FILE_INPUT_BYTES
        );

        let drop_error = accumulate_drop_bytes(u64::MAX, 1).unwrap_err().to_string();
        assert!(drop_error.contains("overflow"), "{drop_error}");
        let upload_error = accumulate_upload_bytes(u64::MAX, 1)
            .unwrap_err()
            .to_string();
        assert!(upload_error.contains("overflow"), "{upload_error}");
    }

    #[test]
    fn attached_page_events_expose_popup_sessions() {
        let event = camoufox_juggler::protocol::Event {
            method: "Browser.attachedToTarget".to_owned(),
            params: json!({
                "sessionId": "session-popup",
                "targetInfo": {
                    "targetId": "target-popup",
                    "type": "page",
                    "browserContextId": "context-1"
                }
            }),
            session_id: camoufox_juggler::protocol::ROOT_SESSION.to_owned(),
        };

        let attachment = attached_page(&event).expect("popup page attachment");

        assert_eq!(attachment.session_id, "session-popup");
        assert_eq!(attachment.target_id, "target-popup");
        assert_eq!(attachment.browser_context_id.as_deref(), Some("context-1"));
    }
}
