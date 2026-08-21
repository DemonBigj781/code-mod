impl ChatWidget<'_> {
    pub(super) fn available_model_presets(&self) -> Vec<ModelPreset> {
        let auth_mode = self
            .auth_manager
            .auth()
            .map(|auth| auth.mode)
            .or({
                if self.config.using_chatgpt_auth {
                    Some(AuthMode::ChatGPT)
                } else {
                    Some(AuthMode::ApiKey)
                }
            });
        let supports_pro_only_models = self.auth_manager.supports_pro_only_models();
        let mut presets = builtin_model_presets(auth_mode, supports_pro_only_models);
        if let Some(remote_presets) = self.remote_model_presets.as_ref() {
            for remote in remote_presets {
                if let Some(existing) = presets.iter_mut().find(|preset| preset.id == remote.id) {
                    *existing = remote.clone();
                } else {
                    presets.push(remote.clone());
                }
            }
        }
        presets
    }

    pub(super) fn available_session_model_presets(&self) -> Vec<ModelPreset> {
        let mut presets = self.available_model_presets();
        let openrouter_model = code_common::model_presets::OPENROUTER_FREE_PROFILE_MODEL;
        if self.config.model_providers.contains_key(
            code_common::model_presets::OPENROUTER_PROVIDER_ID,
        ) && !presets
            .iter()
            .any(|preset| preset.model.eq_ignore_ascii_case(openrouter_model))
        {
            presets.push(code_common::model_presets::openrouter_free_profile_preset());
        }
        presets
    }

    fn configured_direct_model_providers(&self) -> Vec<(String, ModelProviderInfo)> {
        let mut providers: Vec<(String, ModelProviderInfo)> = self
            .config
            .model_providers
            .iter()
            .filter_map(|(provider_id, provider)| {
                crate::direct_provider::is_direct_provider_definition(provider_id, provider)
                    .then(|| (provider_id.clone(), provider.clone()))
            })
            .collect();
        providers.sort_by(|(left_id, left), (right_id, right)| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left_id.cmp(right_id))
        });
        providers
    }

    pub(super) fn available_direct_provider_catalogs(&self) -> Vec<DirectProviderModelCatalog> {
        self.configured_direct_model_providers()
            .into_iter()
            .map(|(provider_id, provider)| {
                let catalog = self.direct_model_catalogs.get(&provider_id);
                DirectProviderModelCatalog {
                    provider_id,
                    display_name: provider.name,
                    status: catalog
                        .map(|catalog| catalog.status.clone())
                        .unwrap_or(code_core::remote_models::RemoteModelsStatus::Loading),
                    presets: catalog
                        .map(|catalog| {
                            crate::remote_model_presets::direct_model_presets(
                                catalog.models.clone(),
                            )
                        })
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    pub(super) fn refresh_direct_provider_catalogs(&mut self) {
        let providers = self.configured_direct_model_providers();
        let configured_ids: HashSet<String> = providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect();
        self.direct_model_catalogs
            .retain(|provider_id, _| configured_ids.contains(provider_id));
        for (provider_id, _) in &providers {
            self.direct_model_catalogs
                .entry(provider_id.clone())
                .and_modify(|catalog| {
                    catalog.status = code_core::remote_models::RemoteModelsStatus::Loading;
                })
                .or_insert_with(|| code_core::remote_models::RemoteModelsCatalog {
                    provider_id: provider_id.clone(),
                    models: Vec::new(),
                    status: code_core::remote_models::RemoteModelsStatus::Loading,
                });
        }

        if crate::chatwidget::is_test_mode() {
            return;
        }

        let auth_manager = self.auth_manager.clone();
        let code_home = self.config.code_home.clone();
        let app_event_tx = self.app_event_tx.clone();
        for (provider_id, provider) in providers {
            let auth_manager = auth_manager.clone();
            let code_home = code_home.clone();
            let app_event_tx = app_event_tx.clone();
            tokio::spawn(async move {
                let manager = code_core::remote_models::RemoteModelsManager::new_for_provider(
                    auth_manager,
                    provider_id,
                    provider,
                    code_home,
                );
                let snapshot = manager.catalog_snapshot().await;
                app_event_tx.send(AppEvent::DirectModelCatalogUpdated {
                    catalog: snapshot.clone(),
                });
                if matches!(
                    snapshot.status,
                    code_core::remote_models::RemoteModelsStatus::Fresh
                ) {
                    return;
                }

                let mut loading = snapshot;
                loading.status = code_core::remote_models::RemoteModelsStatus::Loading;
                app_event_tx.send(AppEvent::DirectModelCatalogUpdated { catalog: loading });
                let catalog = manager.refresh_remote_models_no_cache().await;
                app_event_tx.send(AppEvent::DirectModelCatalogUpdated { catalog });
            });
        }
    }

    pub(crate) fn update_direct_model_catalog(
        &mut self,
        catalog: code_core::remote_models::RemoteModelsCatalog,
    ) {
        self.direct_model_catalogs
            .insert(catalog.provider_id.clone(), catalog);
        let catalogs = self.available_direct_provider_catalogs();
        self.bottom_pane
            .update_direct_provider_catalogs(catalogs.clone());
        if let Some(overlay) = self.settings.overlay.as_mut() {
            overlay.update_direct_provider_catalogs(catalogs);
        }
        self.request_redraw();
    }

    pub(crate) fn update_model_presets(
        &mut self,
        presets: Vec<ModelPreset>,
        default_model: Option<String>,
    ) {
        if presets.is_empty() {
            return;
        }

        self.remote_model_presets = Some(presets);
        let available_presets = self.available_model_presets();
        self.bottom_pane
            .update_model_selection_presets(available_presets.clone());
        if let Some(overlay) = self.settings.overlay.as_mut() {
            overlay.update_model_presets(available_presets);
        }

        if let Some(default_model) = default_model {
            self.maybe_apply_remote_default_model(default_model);
        }

        self.request_redraw();
    }

    pub(crate) fn finish_direct_provider_add(
        &mut self,
        result: Result<crate::direct_provider::DirectProviderAddOutcome, String>,
    ) {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.bottom_pane
                    .finish_direct_provider_add(Err(error.clone()));
                if let Some(overlay) = self.settings.overlay.as_mut() {
                    overlay.finish_direct_provider_add(Err(error));
                }
                return;
            }
        };

        self.direct_model_catalogs.insert(
            outcome.provider_id.clone(),
            code_core::remote_models::RemoteModelsCatalog {
                provider_id: outcome.provider_id,
                models: outcome.models,
                status: code_core::remote_models::RemoteModelsStatus::Fresh,
            },
        );
        let catalogs = self.available_direct_provider_catalogs();
        self.bottom_pane
            .update_direct_provider_catalogs(catalogs.clone());
        if let Some(overlay) = self.settings.overlay.as_mut() {
            overlay.update_direct_provider_catalogs(catalogs);
        }
        self.bottom_pane.finish_direct_provider_add(Ok(()));
        if let Some(overlay) = self.settings.overlay.as_mut() {
            overlay.finish_direct_provider_add(Ok(()));
        }
        self.request_redraw();
    }

    fn maybe_apply_remote_default_model(&mut self, default_model: String) {
        if !self.allow_remote_default_at_startup {
            return;
        }
        if self.chat_model_selected_explicitly {
            return;
        }
        if self.config.model_explicit {
            return;
        }
        if self.config.model.eq_ignore_ascii_case(&default_model) {
            return;
        }

        self.apply_model_selection_inner(default_model, None, None, false, false);
    }

    fn preset_effort_for_model(preset: &ModelPreset) -> ReasoningEffort {
        preset.default_reasoning_effort.into()
    }

    fn clamp_reasoning_for_model(model: &str, requested: ReasoningEffort) -> ReasoningEffort {
        let protocol_effort: code_protocol::config_types::ReasoningEffort = requested.into();
        let clamped = clamp_reasoning_effort_for_model(model, protocol_effort);
        ReasoningEffort::from(clamped)
    }

    fn find_model_preset(&self, input: &str, presets: &[ModelPreset]) -> Option<ModelPreset> {
        if presets.is_empty() {
            return None;
        }

        let input_lower = input.to_ascii_lowercase();
        let collapsed_input: String = input_lower
            .chars()
            .filter(|c| !c.is_ascii_whitespace() && *c != '-')
            .collect();

        let mut fallback_medium: Option<ModelPreset> = None;
        let mut fallback_first: Option<ModelPreset> = None;

        for preset in presets {
            let preset_effort = Self::preset_effort_for_model(preset);

            let id_lower = preset.id.to_ascii_lowercase();
            if Self::candidate_matches(&input_lower, &collapsed_input, &id_lower) {
                return Some(preset.clone());
            }

            let display_name_lower = preset.display_name.to_ascii_lowercase();
            if Self::candidate_matches(&input_lower, &collapsed_input, &display_name_lower) {
                return Some(preset.clone());
            }

            let effort_lower = preset_effort.to_string().to_ascii_lowercase();
            let model_lower = preset.model.to_ascii_lowercase();
            let spaced = format!("{model_lower} {effort_lower}");
            if Self::candidate_matches(&input_lower, &collapsed_input, &spaced) {
                return Some(preset.clone());
            }
            let dashed = format!("{model_lower}-{effort_lower}");
            if Self::candidate_matches(&input_lower, &collapsed_input, &dashed) {
                return Some(preset.clone());
            }

            if model_lower == input_lower
                || Self::candidate_matches(&input_lower, &collapsed_input, &model_lower)
            {
                if fallback_medium.is_none() && preset_effort == ReasoningEffort::Medium {
                    fallback_medium = Some(preset.clone());
                }
                if fallback_first.is_none() {
                    fallback_first = Some(preset.clone());
                }
            }
        }

        fallback_medium.or(fallback_first)
    }

    fn candidate_matches(input: &str, collapsed_input: &str, candidate: &str) -> bool {
        let candidate_lower = candidate.to_ascii_lowercase();
        if candidate_lower == input {
            return true;
        }
        let candidate_collapsed: String = candidate_lower
            .chars()
            .filter(|c| !c.is_ascii_whitespace() && *c != '-')
            .collect();
        candidate_collapsed == collapsed_input
    }

    fn collaboration_mode_display_name(mode: CollaborationModeKind) -> &'static str {
        match mode {
            CollaborationModeKind::Default => "default",
            CollaborationModeKind::Plan => "plan",
        }
    }

    fn parse_collaboration_mode(value: &str) -> Option<CollaborationModeKind> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" | "normal" => Some(CollaborationModeKind::Default),
            "plan" | "planning" => Some(CollaborationModeKind::Plan),
            _ => None,
        }
    }

    pub(crate) fn handle_mode_command(&mut self, command_args: String) {
        if self.is_task_running() {
            let message = "'/mode' is disabled while a task is in progress.".to_owned();
            self.history_push_plain_state(history_cell::new_error_event(message));
            return;
        }

        let trimmed = command_args.trim();
        if trimmed.is_empty() {
            let mode = Self::collaboration_mode_display_name(self.current_collaboration_mode());
            self.push_background_tail(format!(
                "Collaboration mode: {mode} (use /mode <default|plan>)"
            ));
            return;
        }

        let Some(mode) = Self::parse_collaboration_mode(trimmed) else {
            let message = format!(
                "Invalid mode: '{trimmed}'. Use /mode <default|plan>."
            );
            self.history_push_plain_state(history_cell::new_error_event(message));
            return;
        };

        self.set_collaboration_mode(mode, true);
    }

    pub(crate) fn set_collaboration_mode(
        &mut self,
        mode: CollaborationModeKind,
        announce: bool,
    ) {
        let previous = self.collaboration_mode;
        if previous == mode {
            if announce {
                let label = Self::collaboration_mode_display_name(mode);
                self.bottom_pane
                    .flash_footer_notice(format!("Collaboration mode already set to {label}."));
            }
            return;
        }

        self.collaboration_mode = mode;
        if matches!(mode, CollaborationModeKind::Plan) {
            self.apply_planning_session_model();
        } else if matches!(previous, CollaborationModeKind::Plan) {
            self.restore_planning_session_model();
        }

        let op = self.current_configure_session_op();
        self.submit_op(op);
        self.refresh_settings_overview_rows();

        if announce {
            let label = Self::collaboration_mode_display_name(mode);
            self.push_background_tail(format!("Collaboration mode set to {label}."));
        }
        self.request_redraw();
    }

}
