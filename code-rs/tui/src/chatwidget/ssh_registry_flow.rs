use super::*;
use crate::bottom_pane::panes::ssh_credentials::SshCredentialView;
use crate::ssh_registry::{describe, run_registry_job};
use code_core::ssh_registry::{HELP, Registry, RegistryCommand};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ssh_registry_results_do_not_use_the_assistant_stream_channel() {
        let (sender, receiver) = std::sync::mpsc::channel();
        run_registry_job(AppEventSender::new(sender), || {
            Ok(vec!["SSH PC registry: 0 saved connection(s).".into()])
        });
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(event) = receiver.try_recv() {
                    break event;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("registry job must return its result");
        assert!(
            !matches!(event, AppEvent::InsertHistory(_)),
            "registry output must be a notice, not an assistant stream update"
        );
    }
}

impl ChatWidget<'_> {
    pub(crate) fn on_ssh_registry_result(&mut self, result: Result<Vec<String>, String>) {
        if let Some(overlay) = self.settings.overlay.as_mut() {
            overlay.apply_ssh_registry_result(&result);
        }
        self.bottom_pane.apply_ssh_registry_result(&result);
        let lines = match result {
            Ok(lines) => lines,
            Err(error) => vec![format!("SSH registry: {error}")],
        };
        self.history_push_plain_paragraphs(crate::history::state::PlainMessageKind::Notice, lines);
        self.request_redraw();
    }

    pub(crate) fn handle_ssh_command(&mut self, args: String) {
        if matches!(args.trim(), "help" | "?") {
            self.debug_notice(HELP.to_owned());
            return;
        }
        let command = match RegistryCommand::parse(&args) {
            Ok(command) => command,
            Err(error) => {
                self.debug_notice(format!("SSH registry: {error}"));
                return;
            }
        };
        let registry = Registry::new(&self.config.code_home);
        let tx = self.app_event_tx.clone();
        match command {
            RegistryCommand::Add {
                name,
                endpoints,
                auth,
            } => {
                self.bottom_pane.show_settings_view(SshCredentialView::new(
                    name.clone(),
                    auth,
                    self.config.cwd.clone(),
                    move |credentials| {
                        run_registry_job(tx, move || {
                            let connection = registry.add(&name, endpoints, credentials)?;
                            Ok(vec![format!(
                                "Registered {}. No connection attempted.",
                                describe(&connection)
                            )])
                        });
                    },
                ));
                self.request_redraw();
            }
            RegistryCommand::Auth { name, auth } => {
                let expected = match registry.get(&name) {
                    Ok(connection) => connection,
                    Err(error) => {
                        self.debug_notice(format!("SSH registry: {error}"));
                        return;
                    }
                };
                self.bottom_pane.show_settings_view(SshCredentialView::new(
                    name.clone(),
                    auth,
                    self.config.cwd.clone(),
                    move |credentials| {
                        run_registry_job(tx, move || {
                            registry.set_auth(&expected, credentials)?;
                            Ok(vec![format!("Saved authentication for {name}.")])
                        });
                    },
                ));
                self.request_redraw();
            }
            command => run_registry_job(tx, move || match command {
                RegistryCommand::List => {
                    let connections = registry.list()?;
                    let mut lines = vec![
                        format!(
                            "SSH PC registry: {} saved connection(s).",
                            connections.len()
                        ),
                        "self / 127.0.0.1 / 0.0.0.0 are reserved local aliases.".into(),
                    ];
                    lines.extend(connections.iter().map(describe));
                    lines.push("Displayed endpoints are in attempt order; 100.64.0.0/10 is last. Registration does not verify host identity.".into());
                    lines.push(HELP.into());
                    Ok(lines)
                }
                RegistryCommand::Show(name) => {
                    let connection = registry.get(&name)?;
                    Ok(vec![
                        describe(&connection),
                        format!("Connection ID: {}", connection.id),
                        format!(
                            "Configured methods: {}{}",
                            if connection.password_id.is_some() {
                                "pwd "
                            } else {
                                ""
                            },
                            if connection.key.is_some() { "key" } else { "" }
                        ),
                    ])
                }
                RegistryCommand::Update { name, endpoints } => {
                    registry.update_endpoints(&name, endpoints)?;
                    Ok(vec![format!(
                        "Updated {name}; identity and credentials preserved."
                    )])
                }
                RegistryCommand::Rename { name, new_name } => {
                    registry.rename(&name, &new_name)?;
                    Ok(vec![format!(
                        "Renamed {name} to {new_name}; identity and credentials preserved."
                    )])
                }
                RegistryCommand::Remove(name) => {
                    registry.remove(&name)?;
                    Ok(vec![format!(
                        "Removed {name} and its saved SSH credentials."
                    )])
                }
                RegistryCommand::Add { .. } | RegistryCommand::Auth { .. } => {
                    unreachable!("credential entry handled in the TUI")
                }
            }),
        }
    }
}
