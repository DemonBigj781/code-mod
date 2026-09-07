use super::*;

impl ChatWidget<'_> {
    pub(crate) fn show_subagent_editor_for_name(&mut self, name: String) {
        // Build available agents from enabled ones (or sensible defaults)
        let available_agents: Vec<String> = if self.config.agents.is_empty() {
            enabled_agent_model_specs()
                .into_iter()
                .map(|spec| spec.slug.to_owned())
                .collect()
        } else {
            self.config
                .agents
                .iter()
                .filter(|a| a.enabled)
                .map(|a| a.name.clone())
                .collect()
        };
        let existing = self.config.subagent_commands.clone();
        let app_event_tx = self.app_event_tx.clone();
        let build_editor = || {
            SubagentEditorView::new_with_data(
                name.clone(),
                available_agents.clone(),
                existing.clone(),
                false,
                app_event_tx.clone(),
            )
        };

        if self.try_set_agents_settings_editor(build_editor()) {
            self.request_redraw();
            return;
        }

        self.ensure_settings_overlay_section(SettingsSection::Agents);
        self.show_agents_overview_ui();
        let _ = self.try_set_agents_settings_editor(build_editor());
        self.request_redraw();
    }

    pub(crate) fn show_new_subagent_editor(&mut self) {
        let available_agents: Vec<String> = if self.config.agents.is_empty() {
            enabled_agent_model_specs()
                .into_iter()
                .map(|spec| spec.slug.to_owned())
                .collect()
        } else {
            self.config
                .agents
                .iter()
                .filter(|a| a.enabled)
                .map(|a| a.name.clone())
                .collect()
        };
        let existing = self.config.subagent_commands.clone();
        let app_event_tx = self.app_event_tx.clone();
        let build_editor = || {
            SubagentEditorView::new_with_data(
                String::new(),
                available_agents.clone(),
                existing.clone(),
                true,
                app_event_tx.clone(),
            )
        };

        if self.try_set_agents_settings_editor(build_editor()) {
            self.request_redraw();
            return;
        }

        self.ensure_settings_overlay_section(SettingsSection::Agents);
        self.show_agents_overview_ui();
        let _ = self.try_set_agents_settings_editor(build_editor());
        self.request_redraw();
    }

    pub(crate) fn show_agent_editor_ui(&mut self, name: String) {
        if let Some(cfg) = code_core::agent_defaults::agent_config_for_model(
            &self.config.agents,
            &name,
        )
        .cloned()
        {
            let ro = if let Some(ref v) = cfg.args_read_only {
                Some(v.clone())
            } else if !cfg.args.is_empty() {
                Some(cfg.args.clone())
            } else {
                let d = code_core::agent_defaults::default_params_for(
                    &cfg.name, true, /*read_only*/
                );
                if d.is_empty() { None } else { Some(d) }
            };
            let wr = if let Some(ref v) = cfg.args_write {
                Some(v.clone())
            } else if !cfg.args.is_empty() {
                Some(cfg.args.clone())
            } else {
                let d = code_core::agent_defaults::default_params_for(
                    &cfg.name, false, /*read_only*/
                );
                if d.is_empty() { None } else { Some(d) }
            };
            let app_event_tx = self.app_event_tx.clone();
            let cfg_name = cfg.name.clone();
            let cfg_enabled = cfg.enabled;
            let cfg_instructions = cfg.instructions.clone();
            let cfg_command = Self::resolve_agent_command(
                &cfg.name,
                Some(cfg.command.as_str()),
                Some(cfg.command.as_str()),
            );
            let builtin = Self::is_builtin_agent(&cfg.name, &cfg_command);
            let description = Self::agent_description_for(
                &cfg.name,
                Some(&cfg_command),
                cfg.description.as_deref(),
            );
            let build_editor = || {
                AgentEditorView::new(AgentEditorInit {
                    name: cfg_name.clone(),
                    enabled: cfg_enabled,
                    args_read_only: ro.clone(),
                    args_write: wr.clone(),
                    instructions: cfg_instructions.clone(),
                    description: description.clone(),
                    command: cfg_command.clone(),
                    builtin,
                    app_event_tx: app_event_tx.clone(),
                })
            };
            if self.try_set_agents_settings_agent_editor(build_editor()) {
                self.request_redraw();
                return;
            }

            self.ensure_settings_overlay_section(SettingsSection::Agents);
            self.show_agents_overview_ui();
            let _ = self.try_set_agents_settings_agent_editor(build_editor());
            self.request_redraw();
        } else {
            // Fallback: synthesize defaults
            let cmd = Self::resolve_agent_command(&name, None, None);
            let ro = code_core::agent_defaults::default_params_for(&name, true /*read_only*/);
            let wr =
                code_core::agent_defaults::default_params_for(&name, false /*read_only*/);
            let app_event_tx = self.app_event_tx.clone();
            let description = Self::agent_description_for(&name, Some(&cmd), None);
            let builtin = Self::is_builtin_agent(&name, &cmd);
            let build_editor = || {
                AgentEditorView::new(AgentEditorInit {
                    name: name.clone(),
                    enabled: builtin,
                    args_read_only: if ro.is_empty() { None } else { Some(ro.clone()) },
                    args_write: if wr.is_empty() { None } else { Some(wr.clone()) },
                    instructions: None,
                    description: description.clone(),
                    command: cmd.clone(),
                    builtin,
                    app_event_tx: app_event_tx.clone(),
                })
            };
            if self.try_set_agents_settings_agent_editor(build_editor()) {
                self.request_redraw();
                return;
            }

            self.ensure_settings_overlay_section(SettingsSection::Agents);
            self.show_agents_overview_ui();
            let _ = self.try_set_agents_settings_agent_editor(build_editor());
            self.request_redraw();
        }
    }

    pub(crate) fn show_agent_editor_new_ui(&mut self) {
        let app_event_tx = self.app_event_tx.clone();
        let build_editor = || {
            AgentEditorView::new(AgentEditorInit {
                name: String::new(),
                enabled: true,
                args_read_only: None,
                args_write: None,
                instructions: None,
                description: None,
                command: String::new(),
                builtin: false,
                app_event_tx: app_event_tx.clone(),
            })
        };

        if self.try_set_agents_settings_agent_editor(build_editor()) {
            self.request_redraw();
            return;
        }

        self.ensure_settings_overlay_section(SettingsSection::Agents);
        self.show_agents_overview_ui();
        let _ = self.try_set_agents_settings_agent_editor(build_editor());
        self.request_redraw();
    }

    pub(crate) fn apply_subagent_update(
        &mut self,
        cmd: code_core::config_types::SubagentCommandConfig,
    ) {
        if let Some(slot) = self
            .config
            .subagent_commands
            .iter_mut()
            .find(|c| c.name.eq_ignore_ascii_case(&cmd.name))
        {
            *slot = cmd;
        } else {
            self.config.subagent_commands.push(cmd);
        }

        self.bottom_pane.set_subagent_commands(
            self.config
                .subagent_commands
                .iter()
                .map(|c| c.name.clone())
                .collect(),
        );
        self.refresh_settings_overview_rows();
    }

    pub(crate) fn delete_subagent_by_name(&mut self, name: &str) {
        self.config
            .subagent_commands
            .retain(|c| !c.name.eq_ignore_ascii_case(name));
        self.bottom_pane.set_subagent_commands(
            self.config
                .subagent_commands
                .iter()
                .map(|c| c.name.clone())
                .collect(),
        );
        self.refresh_settings_overview_rows();
    }

    pub(crate) fn apply_agent_update(&mut self, update: AgentUpdateRequest) {
        let AgentUpdateRequest {
            intent,
            name,
            enabled,
            args_ro,
            args_wr,
            instructions,
            description,
            command,
        } = update;
        let provided_command = if command.trim().is_empty() { None } else { Some(command.as_str()) };
        let existing_index = self
            .config
            .agents
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(&name));

        if matches!(intent, crate::app_event::AgentUpdateIntent::CreateModel)
            && existing_index.is_some()
        {
            return;
        }

        let existing_command = existing_index
            .and_then(|idx| self.config.agents.get(idx))
            .map(|cfg| cfg.command.clone());
        let resolved = Self::resolve_agent_command(
            &name,
            provided_command,
            existing_command.as_deref(),
        );

        let mut candidate_cfg = if let Some(idx) = existing_index {
            self.config.agents.get(idx).cloned().unwrap_or_else(|| AgentConfig {
                name,
                command: resolved.clone(),
                args: Vec::new(),
                read_only: false,
                enabled,
                session_enabled: true,
                review_enabled: true,
                auto_drive_enabled: true,
                description: description.clone(),
                env: None,
                args_read_only: args_ro.clone(),
                args_write: args_wr.clone(),
                instructions: instructions.clone(),
            })
        } else {
            AgentConfig {
                name,
                command: resolved.clone(),
                args: Vec::new(),
                read_only: false,
                enabled,
                session_enabled: true,
                review_enabled: true,
                auto_drive_enabled: true,
                description: description.clone(),
                env: None,
                args_read_only: args_ro.clone(),
                args_write: args_wr.clone(),
                instructions: instructions.clone(),
            }
        };

        candidate_cfg.command = resolved;
        candidate_cfg.enabled = enabled;
        candidate_cfg.description = description;
        candidate_cfg.args_read_only = args_ro;
        candidate_cfg.args_write = args_wr;
        candidate_cfg.instructions = instructions;

        let pending = PendingAgentUpdate { id: Uuid::new_v4(), cfg: candidate_cfg };
        let requires_validation = !self.test_mode && existing_index.is_none();
        if requires_validation {
            self.start_agent_validation(pending);
            return;
        }

        self.commit_agent_update(pending);
    }

    pub(crate) fn apply_model_role_update(
        &mut self,
        name: String,
        role: code_core::config_types::ModelRole,
        enabled: bool,
        description: Option<String>,
        command: String,
    ) {
        let canonical = agent_model_spec(&name).map(|spec| spec.slug);
        let existing_index = self.config.agents.iter().position(|agent| {
            if let Some(canonical) = canonical {
                agent_model_spec(&agent.name)
                    .is_some_and(|spec| spec.slug.eq_ignore_ascii_case(canonical))
            } else {
                agent.name.eq_ignore_ascii_case(&name)
            }
        });

        let mut config = existing_index
            .and_then(|index| self.config.agents.get(index).cloned())
            .or_else(|| agent_model_spec(&name).map(code_core::agent_defaults::agent_config_from_spec))
            .unwrap_or_else(|| AgentConfig {
                name: name.clone(),
                command: Self::resolve_agent_command(&name, Some(&command), None),
                args: Vec::new(),
                read_only: false,
                enabled: false,
                session_enabled: true,
                review_enabled: true,
                auto_drive_enabled: true,
                description: description.clone(),
                env: None,
                args_read_only: None,
                args_write: None,
                instructions: None,
            });

        if config.command.trim().is_empty() {
            config.command = Self::resolve_agent_command(&name, Some(&command), None);
        }
        if config.description.is_none() {
            config.description = description;
        }
        match role {
            code_core::config_types::ModelRole::Session => config.session_enabled = enabled,
            code_core::config_types::ModelRole::Subagent => config.enabled = enabled,
            code_core::config_types::ModelRole::Review => config.review_enabled = enabled,
            code_core::config_types::ModelRole::AutoDrive => config.auto_drive_enabled = enabled,
        }

        let active_planning_model = if self.config.planning_use_chat_model {
            self.config.model.as_str()
        } else {
            self.config.planning_model.as_str()
        };
        let disables_active_planning_model = role == code_core::config_types::ModelRole::Review
            && !enabled
            && matches!(self.collaboration_mode, CollaborationModeKind::Plan)
            && code_core::agent_defaults::agent_config_for_model(
                std::slice::from_ref(&config),
                active_planning_model,
            )
            .is_some();

        self.commit_agent_update(PendingAgentUpdate {
            id: Uuid::new_v4(),
            cfg: config,
        });
        if disables_active_planning_model {
            self.set_collaboration_mode(CollaborationModeKind::Default, true);
        }
    }

    fn start_agent_validation(&mut self, pending: PendingAgentUpdate) {
        let name = pending.cfg.name.clone();
        self.push_background_tail(format!(
            "Testing agent `{name}` (expecting \"ok\")…"
        ));
        self.pending_agent_updates.retain(|_, existing| {
            !existing.cfg.name.eq_ignore_ascii_case(&name)
        });
        let key = pending.key();
        let attempt = pending.clone();
        self.pending_agent_updates.insert(key, pending);
        self.refresh_settings_overview_rows();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let cfg = attempt.cfg.clone();
            let agent_name = cfg.name.clone();
            let attempt_id = attempt.id;
            let result = task::spawn_blocking(move || smoke_test_agent_blocking(cfg))
                .await
                .map_err(|e| format!("validation task failed: {e}"))
                .and_then(|res| res);
            tx.send(AppEvent::AgentValidationFinished { name: agent_name, result, attempt_id });
        });
    }

    pub(crate) fn handle_agent_validation_finished(&mut self, name: &str, attempt_id: Uuid, result: Result<(), String>) {
        let key = format!("{}:{}", name.to_ascii_lowercase(), attempt_id);
        let Some(pending) = self.pending_agent_updates.remove(&key) else {
            return;
        };

        match result {
            Ok(()) => {
                self.push_background_tail(format!(
                    "Agent `{name}` responded with \"ok\"."
                ));
                self.commit_agent_update(pending);
            }
            Err(err) => {
                self.history_push_plain_state(history_cell::new_error_event(format!(
                    "Agent `{name}` validation failed: {err}"
                )));
                self.show_agent_editor_for_pending(&pending);
            }
        }
        self.request_redraw();
    }

    fn commit_agent_update(&mut self, pending: PendingAgentUpdate) {
        let name = pending.cfg.name.clone();
        if let Some(slot) = self
            .config
            .agents
            .iter_mut()
            .find(|a| a.name.eq_ignore_ascii_case(&name))
        {
            *slot = pending.cfg.clone();
        } else {
            self.config.agents.push(pending.cfg.clone());
        }

        self.persist_agent_config(&pending.cfg);
        self.submit_op(self.current_configure_session_op());
        self.refresh_settings_overview_rows();
        self.show_agents_overview_ui();
    }

    fn persist_agent_config(&self, cfg: &AgentConfig) {
        if let Ok(home) = code_core::config::find_code_home() {
            let name = cfg.name.clone();
            let enabled = cfg.enabled;
            let session_enabled = cfg.session_enabled;
            let review_enabled = cfg.review_enabled;
            let auto_drive_enabled = cfg.auto_drive_enabled;
            let ro = cfg.args_read_only.clone();
            let wr = cfg.args_write.clone();
            let instr = cfg.instructions.clone();
            let desc = cfg.description.clone();
            let command = cfg.command.clone();
            tokio::spawn(async move {
                let _ = code_core::config_edit::upsert_agent_config(
                    &home,
                    code_core::config_edit::AgentConfigPatch {
                        name: &name,
                        enabled: Some(enabled),
                        session_enabled: Some(session_enabled),
                        review_enabled: Some(review_enabled),
                        auto_drive_enabled: Some(auto_drive_enabled),
                        args: None,
                        args_read_only: ro.as_deref(),
                        args_write: wr.as_deref(),
                        instructions: instr.as_deref(),
                        description: desc.as_deref(),
                        command: Some(command.as_str()),
                    },
                )
                .await;
            });
        }
    }

    fn show_agent_editor_for_pending(&mut self, pending: &PendingAgentUpdate) {
        let cfg = pending.cfg.clone();
        let app_event_tx = self.app_event_tx.clone();
        let name_value = cfg.name.clone();
        let enabled_value = cfg.enabled;
        let ro = cfg.args_read_only.clone();
        let wr = cfg.args_write.clone();
        let instructions = cfg.instructions.clone();
        let description = cfg.description.clone();
        let command = cfg.command.clone();
        let builtin = Self::is_builtin_agent(&cfg.name, &command);
        let build_editor = || {
            AgentEditorView::new(AgentEditorInit {
                name: name_value.clone(),
                enabled: enabled_value,
                args_read_only: ro.clone(),
                args_write: wr.clone(),
                instructions: instructions.clone(),
                description: description.clone(),
                command: command.clone(),
                builtin,
                app_event_tx: app_event_tx.clone(),
            })
        };
        if self.try_set_agents_settings_agent_editor(build_editor()) {
            self.request_redraw();
            return;
        }
        self.ensure_settings_overlay_section(SettingsSection::Agents);
        self.show_agents_overview_ui();
        let _ = self.try_set_agents_settings_agent_editor(build_editor());
        self.request_redraw();
    }

    fn resolve_agent_command(
        name: &str,
        provided: Option<&str>,
        existing: Option<&str>,
    ) -> String {
        let spec = agent_model_spec(name);
        if let Some(cmd) = provided
            && let Some(resolved) = Self::normalize_agent_command(cmd, name, spec) {
                return resolved;
            }
        if let Some(cmd) = existing
            && let Some(resolved) = Self::normalize_agent_command(cmd, name, spec) {
                return resolved;
            }
        if let Some(spec) = spec {
            return spec.cli.to_owned();
        }
        if let Some((provider, model)) = name.split_once('/')
            && !provider.trim().is_empty()
            && !model.trim().is_empty()
            && !provider.chars().any(char::is_whitespace)
            && !model.chars().any(char::is_whitespace)
        {
            return format!(
                "coder --model {model} -c model_provider={provider}"
            );
        }
        name.to_owned()
    }

    fn normalize_agent_command(
        candidate: &str,
        name: &str,
        spec: Option<&code_core::agent_defaults::AgentModelSpec>,
    ) -> Option<String> {
        if candidate.trim().is_empty() {
            return None;
        }
        if let Some(spec) = spec {
            if candidate.eq_ignore_ascii_case(name) && !spec.cli.eq_ignore_ascii_case(name) {
                return Some(spec.cli.to_owned());
            }
            if candidate.eq_ignore_ascii_case(spec.slug) && !spec.cli.eq_ignore_ascii_case(spec.slug) {
                return Some(spec.cli.to_owned());
            }
        }
        Some(candidate.to_owned())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_model_name_builds_a_custom_code_agent_command() {
        assert_eq!(
            ChatWidget::resolve_agent_command("openrouter/vendor/model:free", None, None),
            "coder --model vendor/model:free -c model_provider=openrouter",
        );
    }
}
