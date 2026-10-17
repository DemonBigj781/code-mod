use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use code_core::ssh_registry::Connection;

pub(crate) fn describe(connection: &Connection) -> String {
    let endpoints = connection
        .ordered_endpoints()
        .iter()
        .map(|e| e.display())
        .collect::<Vec<_>>()
        .join(" → ");
    format!(
        "{} [{}] — {}",
        connection.name,
        connection.default_auth.label(),
        endpoints
    )
}

pub(crate) fn run_registry_job(
    tx: AppEventSender,
    job: impl FnOnce() -> anyhow::Result<Vec<String>> + Send + 'static,
) {
    tokio::spawn(async move {
        let result = match tokio::task::spawn_blocking(job).await {
            Ok(result) => result.map_err(|error| format!("{error:#}")),
            Err(_) => Err("Registry worker failed; no network connection was attempted.".into()),
        };
        tx.send(AppEvent::SshRegistryResult { result });
    });
}
