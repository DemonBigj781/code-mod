use super::*;
use std::future::Future;

mod developer_message;
mod review;
mod run;
pub(in crate::codex) use run::resume_queued_work;

pub(super) async fn spawn_review_thread(
    sess: Arc<Session>,
    config: Arc<Config>,
    sub_id: String,
    review_request: ReviewRequest,
) {
    review::spawn_review_thread(sess, config, sub_id, review_request).await;
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AgentTaskKind {
    Regular,
    Review,
    Compact,
    OperatorTool,
}

/// A series of Turns in response to user input.
pub(in crate::codex) struct AgentTask {
    sess: Arc<Session>,
    pub(in crate::codex) sub_id: String,
    handle: AbortHandle,
    start_gate: TaskStartGate,
    kind: AgentTaskKind,
    pub(in crate::codex) _origin: TaskOriginKind,
    pub(in crate::codex) _visible_to_user: bool,
}

struct TaskStartGate {
    sender: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TaskStartGate {
    fn start(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(());
        }
    }
}

fn spawn_gated_task<F>(task: F) -> (AbortHandle, TaskStartGate)
where
    F: Future<Output = ()> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        if receiver.await.is_ok() {
            task.await;
        }
    })
    .abort_handle();
    (
        handle,
        TaskStartGate {
            sender: Some(sender),
        },
    )
}

impl AgentTask {
    pub(in crate::codex) fn operator_tool(
        sess: Arc<Session>,
        sub_id: String,
        name: String,
        arguments: Option<serde_json::Value>,
    ) -> Self {
        let session = Arc::clone(&sess);
        let id = sub_id.clone();
        let (handle, start_gate) = spawn_gated_task(async move {
            super::super::operator_tools::run(session, id, name, arguments).await;
        });
        Self {
            sess,
            sub_id,
            handle,
            start_gate,
            kind: AgentTaskKind::OperatorTool,
            _origin: TaskOriginKind::OperatorTool,
            _visible_to_user: true,
        }
    }

    pub(in crate::codex) fn spawn(
        sess: Arc<Session>,
        turn_context: Arc<TurnContext>,
        sub_id: String,
        input: Vec<InputItem>,
        origin: TaskOriginKind,
        visible_to_user: bool,
    ) -> Self {
        let sess_clone = Arc::clone(&sess);
        let tc_clone = Arc::clone(&turn_context);
        let sub_clone = sub_id.clone();
        let (handle, start_gate) = spawn_gated_task(async move {
            run::run_agent(sess_clone, tc_clone, sub_clone, input).await;
        });
        Self {
            sess,
            sub_id,
            handle,
            start_gate,
            kind: AgentTaskKind::Regular,
            _origin: origin,
            _visible_to_user: visible_to_user,
        }
    }

    pub(in crate::codex) fn compact(
        sess: Arc<Session>,
        turn_context: Arc<TurnContext>,
        sub_id: String,
        input: Vec<InputItem>,
    ) -> Self {
        let sess_clone = Arc::clone(&sess);
        let tc_clone = Arc::clone(&turn_context);
        let sub_clone = sub_id.clone();
        let (handle, start_gate) = spawn_gated_task(async move {
            compact::run_compact_task(sess_clone, tc_clone, sub_clone, input).await;
        });
        Self {
            sess,
            sub_id,
            handle,
            start_gate,
            kind: AgentTaskKind::Compact,
            _origin: TaskOriginKind::ManualCompact,
            _visible_to_user: false,
        }
    }

    pub(in crate::codex) fn review(
        sess: Arc<Session>,
        turn_context: Arc<TurnContext>,
        sub_id: String,
        input: Vec<InputItem>,
    ) -> Self {
        let sess_clone = Arc::clone(&sess);
        let tc_clone = Arc::clone(&turn_context);
        let sub_clone = sub_id.clone();
        let (handle, start_gate) = spawn_gated_task(async move {
            run::run_agent(sess_clone, tc_clone, sub_clone, input).await;
        });
        Self {
            sess,
            sub_id,
            handle,
            start_gate,
            kind: AgentTaskKind::Review,
            _origin: TaskOriginKind::Review,
            _visible_to_user: true,
        }
    }

    pub(in crate::codex) fn start(&mut self) {
        self.start_gate.start();
    }

    pub(in crate::codex) fn abort(self, reason: TurnAbortReason) {
        if !self.handle.is_finished() {
            self.handle.abort();
            let event = self
                .sess
                .make_event(&self.sub_id, EventMsg::TurnAborted(TurnAbortedEvent { reason }));
            let sess = self.sess.clone();
            let sub_id = self.sub_id.clone();
            let kind = self.kind;
            tokio::spawn(async move {
                if kind == AgentTaskKind::Review {
                    review::exit_review_mode(sess.clone(), sub_id, None).await;
                }
                sess.send_event(event).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    #[tokio::test(flavor = "current_thread")]
    async fn spawned_task_waits_until_ownership_is_registered() {
        let (ran_tx, mut ran_rx) = mpsc::channel(1);
        let (_handle, mut start_gate) = spawn_gated_task(async move {
            ran_tx
                .send(())
                .await
                .expect("test receiver should remain open");
        });

        assert!(
            timeout(Duration::from_millis(25), ran_rx.recv())
                .await
                .is_err(),
            "a newly spawned task must not run before Session registers ownership"
        );

        start_gate.start();

        assert_eq!(
            timeout(Duration::from_secs(1), ran_rx.recv())
                .await
                .expect("started task should run promptly"),
            Some(())
        );
    }
}
