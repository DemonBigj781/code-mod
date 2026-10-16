use super::*;
use crate::bottom_pane::panes::ssh_credentials::SshCredentialView;
use code_core::ssh_registry::{Connection, HELP, Registry, RegistryCommand};

fn describe(connection: &Connection) -> String {
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

fn run_registry_job(
    tx: AppEventSender,
    job: impl FnOnce() -> anyhow::Result<Vec<String>> + Send + 'static,
) {
    tokio::spawn(async move {
        let lines = match tokio::task::spawn_blocking(job).await {
            Ok(Ok(lines)) => lines,
            Ok(Err(error)) => vec![format!("SSH registry: {error:#}")],
            Err(_) => vec!["SSH registry worker failed; no connection was attempted.".into()],
        };
        tx.send(AppEvent::InsertHistory(
            lines.into_iter().map(Line::from).collect(),
        ));
    });
}

impl ChatWidget<'_> {
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
