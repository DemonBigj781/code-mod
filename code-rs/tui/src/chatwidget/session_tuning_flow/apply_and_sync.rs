impl ChatWidget<'_> {
    fn is_configured_model_catalog_provider(&self, provider_id: &str) -> bool {
        self.config
            .model_providers
            .get(provider_id)
            .is_some_and(|provider| {
                crate::direct_provider::is_model_catalog_provider_definition(provider_id, provider)
            })
    }

    fn active_provider_has_own_catalog(&self) -> bool {
        crate::direct_provider::is_model_catalog_provider_definition(
            &self.config.model_provider_id,
            &self.config.model_provider,
        )
    }

    fn configured_non_catalog_provider(
        &self,
        provider_id: &str,
    ) -> Option<(String, ModelProviderInfo)> {
        let provider = self.config.model_providers.get(provider_id)?;
        (!crate::direct_provider::is_model_catalog_provider_definition(provider_id, provider))
            .then(|| (provider_id.to_owned(), provider.clone()))
    }

    fn fallback_non_catalog_provider(&self) -> Option<(String, ModelProviderInfo)> {
        self.configured_non_catalog_provider("openai").or_else(|| {
            self.config
                .model_providers
                .iter()
                .filter(|(provider_id, provider)| {
                    !crate::direct_provider::is_model_catalog_provider_definition(
                        provider_id,
                        provider,
                    )
                })
                .min_by(|(left_id, _), (right_id, _)| left_id.cmp(right_id))
                .map(|(provider_id, provider)| (provider_id.clone(), provider.clone()))
        })
    }

    fn restore_provider_before_direct(&mut self) -> Result<bool, String> {
        if !self.active_provider_has_own_catalog() {
            return Ok(false);
        }

        let previous_provider_id = self.model_provider_before_direct.as_deref();
        let (provider_id, provider) = previous_provider_id
            .and_then(|provider_id| self.configured_non_catalog_provider(provider_id))
            .or_else(|| self.fallback_non_catalog_provider())
            .ok_or_else(|| {
                "No primary model provider is available for this selection.".to_owned()
            })?;
        let changed =
            self.config.model_provider_id != provider_id || self.config.model_provider != provider;
        self.model_provider_before_direct = None;
        self.config.model_provider_id = provider_id;
        self.config.model_provider = provider;
        Ok(changed)
    }

    fn sync_provider_for_model_selection(
        &mut self,
        model: &str,
        requested_provider_id: Option<&str>,
    ) -> Result<bool, String> {
        if let Some(provider_id) = requested_provider_id {
            if !self.is_configured_model_catalog_provider(provider_id) {
                return Err(format!(
                    "The selected model provider '{provider_id}' is no longer configured."
                ));
            }
            let provider = self
                .config
                .model_providers
                .get(provider_id)
                .cloned()
                .ok_or_else(|| {
                    format!("The selected model provider '{provider_id}' is unavailable.")
                })?;

            if !self.active_provider_has_own_catalog()
                && self.config.model_provider_id != provider_id
            {
                self.model_provider_before_direct = Some(self.config.model_provider_id.clone());
            }

            let clear_openrouter_profile =
                self.config.active_profile.as_deref() == Some(OPENROUTER_FREE_PROFILE);
            let changed = self.config.model_provider_id != provider_id
                || self.config.model_provider != provider
                || clear_openrouter_profile;
            self.config.model_provider_id = provider_id.to_owned();
            self.config.model_provider = provider;
            if clear_openrouter_profile {
                self.config.active_profile = None;
                self.model_provider_before_openrouter = None;
            }
            return Ok(changed);
        }

        let direct_provider_changed = self.restore_provider_before_direct()?;

        if model.eq_ignore_ascii_case(OPENROUTER_FREE_MAX_MODEL) {
            let Some(openrouter) = self
                .config
                .model_providers
                .get(OPENROUTER_PROVIDER_ID)
                .cloned()
            else {
                tracing::error!("OpenRouter Free was selected without a registered OpenRouter provider");
                return Ok(false);
            };

            if self.model_provider_before_openrouter.is_none()
                && (self.config.model_provider_id != OPENROUTER_PROVIDER_ID
                    || self.config.active_profile.as_deref() != Some(OPENROUTER_FREE_PROFILE))
            {
                self.model_provider_before_openrouter = Some((
                    self.config.model_provider_id.clone(),
                    self.config.model_provider.clone(),
                    self.config.active_profile.clone(),
                ));
            }

            let changed = direct_provider_changed
                || self.config.model_provider_id != OPENROUTER_PROVIDER_ID
                || self.config.model_provider != openrouter
                || self.config.active_profile.as_deref() != Some(OPENROUTER_FREE_PROFILE);
            self.config.model_provider_id = OPENROUTER_PROVIDER_ID.to_owned();
            self.config.model_provider = openrouter;
            self.config.active_profile = Some(OPENROUTER_FREE_PROFILE.to_owned());
            return Ok(changed);
        }

        if self.config.model_provider_id != OPENROUTER_PROVIDER_ID {
            return Ok(direct_provider_changed);
        }

        // A previous session may have persisted the OpenRouter provider without
        // the temporary profile marker. Normal model presets are OpenAI presets,
        // so selecting one must heal that stranded provider state as well as the
        // in-memory OpenRouter Free profile transition.
        let (provider_id, provider, profile) = self
            .model_provider_before_openrouter
            .take()
            .or_else(|| {
                self.config
                    .model_providers
                    .get("openai")
                    .cloned()
                    .map(|provider| ("openai".to_owned(), provider, None))
            })
            .expect("built-in OpenAI provider must be available");
        let changed = direct_provider_changed
            || self.config.model_provider_id != provider_id
            || self.config.model_provider != provider
            || self.config.active_profile != profile;
        self.config.model_provider_id = provider_id;
        self.config.model_provider = provider;
        self.config.active_profile = profile;
        Ok(changed)
    }

    fn clamp_reasoning_for_matching_preset(
        model: &str,
        requested: ReasoningEffort,
        presets: &[ModelPreset],
    ) -> Option<ReasoningEffort> {
        fn rank(effort: ReasoningEffort) -> u8 {
            match effort {
                ReasoningEffort::Minimal => 0,
                ReasoningEffort::Low => 1,
                ReasoningEffort::Medium => 2,
                ReasoningEffort::High => 3,
                ReasoningEffort::XHigh => 4,
                ReasoningEffort::Max => 5,
                ReasoningEffort::None => 6,
            }
        }

        let model_lower = model.to_ascii_lowercase();
        let preset = presets.iter().find(|preset| {
            preset.model.eq_ignore_ascii_case(&model_lower)
                || preset.id.eq_ignore_ascii_case(&model_lower)
                || preset.display_name.eq_ignore_ascii_case(&model_lower)
        })?;

        let supported: Vec<ReasoningEffort> = preset
            .supported_reasoning_efforts
            .iter()
            .map(|opt| ReasoningEffort::from(opt.effort))
            .collect();
        if supported.contains(&requested) {
            return Some(requested);
        }

        let requested_rank = rank(requested);
        Some(
            supported
                .into_iter()
                .min_by_key(|effort| {
                    let effort_rank = rank(*effort);
                    (requested_rank.abs_diff(effort_rank), u8::MAX - effort_rank)
                })
                .unwrap_or(requested),
        )
    }

    fn clamp_reasoning_for_model_from_presets(
        model: &str,
        requested: ReasoningEffort,
        presets: &[ModelPreset],
    ) -> ReasoningEffort {
        Self::clamp_reasoning_for_matching_preset(model, requested, presets)
            .unwrap_or_else(|| Self::clamp_reasoning_for_model(model, requested))
    }

    fn apply_model_selection_inner(
        &mut self,
        model: String,
        effort: Option<ReasoningEffort>,
        model_provider_id: Option<String>,
        mark_explicit: bool,
        announce: bool,
    ) {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return;
        }

        let provider_changed =
            match self.sync_provider_for_model_selection(trimmed, model_provider_id.as_deref()) {
                Ok(changed) => changed,
                Err(error) => {
                    self.bottom_pane.flash_footer_notice(error);
                    self.request_redraw();
                    return;
                }
            };

        if mark_explicit {
            self.chat_model_selected_explicitly = true;
            self.config.model_explicit = true;
        }

        let model_changed = if !self.config.model.eq_ignore_ascii_case(trimmed) {
            trimmed.clone_into(&mut self.config.model);
            let family = find_family_for_model(&self.config.model)
                .unwrap_or_else(|| derive_default_model_family(&self.config.model));
            self.config.model_family = family;
            true
        } else {
            false
        };

        let effort_changed = if let Some(explicit) = effort
            && self.config.preferred_model_reasoning_effort != Some(explicit) {
                self.config.preferred_model_reasoning_effort = Some(explicit);
                true
            } else {
                false
            };

        let requested_effort = effort
            .or(self.config.preferred_model_reasoning_effort)
            .unwrap_or(self.config.model_reasoning_effort);
        let clamped_effort = if let Some(provider_id) = model_provider_id.as_deref() {
            self.direct_model_catalogs
                .get(provider_id)
                .map(|catalog| {
                    crate::remote_model_presets::direct_model_presets(catalog.models.clone())
                })
                .and_then(|presets| {
                    Self::clamp_reasoning_for_matching_preset(trimmed, requested_effort, &presets)
                })
                .unwrap_or(requested_effort)
        } else {
            let presets = self.available_session_model_presets();
            Self::clamp_reasoning_for_model_from_presets(trimmed, requested_effort, &presets)
        };

        let reasoning_changed = if self.config.model_reasoning_effort != clamped_effort {
            self.config.model_reasoning_effort = clamped_effort;
            true
        } else {
            false
        };

        let updated = provider_changed || model_changed || effort_changed || reasoning_changed;

        if updated {
            let op = self.current_configure_session_op();
            self.submit_op(op);

            self.sync_follow_chat_models();
            self.refresh_settings_overview_rows();
        }

        if announce {
            let placement = self.ui_placement_for_now();
            let state = history_cell::new_model_output(&self.config.model, self.config.model_reasoning_effort);
            let cell = crate::history_cell::PlainHistoryCell::from_state(state.clone());
            self.push_system_cell(
                Box::new(cell),
                placement,
                Some("ui:model".to_owned()),
                None,
                "system",
                Some(HistoryDomainRecord::Plain(state)),
            );
        }

        self.request_redraw();
    }

    fn sync_follow_chat_models(&mut self) {
        if self.config.review_use_chat_model {
            self.config.review_model = self.config.model.clone();
            self.config.review_model_reasoning_effort = self.config.model_reasoning_effort;
            self.update_review_settings_model_row();
        }

        if self.config.review_resolve_use_chat_model {
            self.config.review_resolve_model = self.config.model.clone();
            self.config.review_resolve_model_reasoning_effort = self.config.model_reasoning_effort;
            self.update_review_settings_model_row();
        }

        if self.config.planning_use_chat_model {
            self.config.planning_model = self.config.model.clone();
            self.config.planning_model_reasoning_effort = self.config.model_reasoning_effort;
            self.update_planning_settings_model_row();
        }

        if self.config.auto_drive_use_chat_model {
            self.config.auto_drive.model = self.config.model.clone();
            self.config.auto_drive.model_reasoning_effort = self.config.model_reasoning_effort;
            self.update_auto_drive_settings_model_row();
        }

        if self.config.auto_review_use_chat_model {
            self.config.auto_review_model = self.config.model.clone();
            self.config.auto_review_model_reasoning_effort = self.config.model_reasoning_effort;
            self.update_review_settings_model_row();
        }

        if self.config.auto_review_resolve_use_chat_model {
            self.config.auto_review_resolve_model = self.config.model.clone();
            self.config.auto_review_resolve_model_reasoning_effort = self.config.model_reasoning_effort;
            self.update_review_settings_model_row();
        }
    }

    pub(crate) fn apply_review_model_selection(
        &mut self,
        model: String,
        effort: ReasoningEffort,
    ) {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return;
        }

        self.config.review_use_chat_model = false;

        let clamped_effort = Self::clamp_reasoning_for_model(trimmed, effort);

        let model_changed = if !self.config.review_model.eq_ignore_ascii_case(trimmed) {
            trimmed.clone_into(&mut self.config.review_model);
            true
        } else {
            false
        };

        let effort_changed = if self.config.review_model_reasoning_effort != clamped_effort {
            self.config.review_model_reasoning_effort = clamped_effort;
            true
        } else {
            false
        };

        let updated = model_changed || effort_changed;

        if !updated {
            self.bottom_pane
                .flash_footer_notice("Review model unchanged.");
            return;
        }

        let message = if let Ok(home) = code_core::config::find_code_home() {
            match code_core::config::set_review_model(
                &home,
                &self.config.review_model,
                self.config.review_model_reasoning_effort,
                self.config.review_use_chat_model,
            ) {
                Ok(_) => format!(
                    "Review model set to {} ({} reasoning)",
                    self.config.review_model,
                    Self::format_reasoning_effort(self.config.review_model_reasoning_effort)
                ),
                Err(err) => {
                    tracing::warn!("Failed to persist review model: {err}");
                    format!(
                        "Review model set for this session (failed to persist): {}",
                        self.config.review_model
                    )
                }
            }
        } else {
            tracing::warn!("Could not locate Code home to persist review model");
            format!(
                "Review model set for this session: {}",
                self.config.review_model
            )
        };

        self.bottom_pane.flash_footer_notice(message);
        self.refresh_settings_overview_rows();
        self.update_review_settings_model_row();
        self.request_redraw();
    }

    pub(crate) fn apply_review_resolve_model_selection(
        &mut self,
        model: String,
        effort: ReasoningEffort,
    ) {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return;
        }

        self.config.review_resolve_use_chat_model = false;

        let clamped_effort = Self::clamp_reasoning_for_model(trimmed, effort);

        let model_changed = if !self
            .config
            .review_resolve_model
            .eq_ignore_ascii_case(trimmed)
        {
            trimmed.clone_into(&mut self.config.review_resolve_model);
            true
        } else {
            false
        };

        let effort_changed = if self.config.review_resolve_model_reasoning_effort != clamped_effort {
            self.config.review_resolve_model_reasoning_effort = clamped_effort;
            true
        } else {
            false
        };

        let updated = model_changed || effort_changed;

        if !updated {
            self.bottom_pane
                .flash_footer_notice("Resolve model unchanged.");
            return;
        }

        let message = if let Ok(home) = code_core::config::find_code_home() {
            match code_core::config::set_review_resolve_model(
                &home,
                &self.config.review_resolve_model,
                self.config.review_resolve_model_reasoning_effort,
                self.config.review_resolve_use_chat_model,
            ) {
                Ok(_) => format!(
                    "Resolve model set to {} ({} reasoning)",
                    self.config.review_resolve_model,
                    Self::format_reasoning_effort(self.config.review_resolve_model_reasoning_effort)
                ),
                Err(err) => {
                    tracing::warn!("Failed to persist resolve model: {err}");
                    format!(
                        "Resolve model set for this session (failed to persist): {}",
                        self.config.review_resolve_model
                    )
                }
            }
        } else {
            tracing::warn!("Could not locate Code home to persist resolve model");
            format!(
                "Resolve model set for this session: {}",
                self.config.review_resolve_model
            )
        };

        self.bottom_pane.flash_footer_notice(message);
        self.refresh_settings_overview_rows();
        self.update_review_settings_model_row();
        self.request_redraw();
    }

    pub(crate) fn set_review_use_chat_model(&mut self, use_chat: bool) {
        if self.config.review_use_chat_model == use_chat {
            return;
        }
        self.config.review_use_chat_model = use_chat;
        if use_chat {
            self.config.review_model = self.config.model.clone();
            self.config.review_model_reasoning_effort = self.config.model_reasoning_effort;
        }

        if let Ok(home) = code_core::config::find_code_home()
            && let Err(err) = code_core::config::set_review_model(
                &home,
                &self.config.review_model,
                self.config.review_model_reasoning_effort,
                use_chat,
            ) {
                tracing::warn!("Failed to persist review use-chat toggle: {err}");
            }

        let notice = if use_chat {
            "Review model now follows Chat model".to_owned()
        } else {
            format!(
                "Review model set to {} ({} reasoning)",
                self.config.review_model,
                Self::format_reasoning_effort(self.config.review_model_reasoning_effort)
            )
        };
        self.bottom_pane.flash_footer_notice(notice);
        self.update_review_settings_model_row();
        self.refresh_settings_overview_rows();
        self.request_redraw();
    }

    pub(crate) fn set_review_resolve_use_chat_model(&mut self, use_chat: bool) {
        if self.config.review_resolve_use_chat_model == use_chat {
            return;
        }
        self.config.review_resolve_use_chat_model = use_chat;
        if use_chat {
            self.config.review_resolve_model = self.config.model.clone();
            self.config.review_resolve_model_reasoning_effort = self.config.model_reasoning_effort;
        }

        if let Ok(home) = code_core::config::find_code_home()
            && let Err(err) = code_core::config::set_review_resolve_model(
                &home,
                &self.config.review_resolve_model,
                self.config.review_resolve_model_reasoning_effort,
                use_chat,
            ) {
                tracing::warn!("Failed to persist resolve use-chat toggle: {err}");
            }

        let notice = if use_chat {
            "Resolve model now follows Chat model".to_owned()
        } else {
            format!(
                "Resolve model set to {} ({} reasoning)",
                self.config.review_resolve_model,
                Self::format_reasoning_effort(self.config.review_resolve_model_reasoning_effort)
            )
        };
        self.bottom_pane.flash_footer_notice(notice);
        self.update_review_settings_model_row();
        self.refresh_settings_overview_rows();
        self.request_redraw();
    }

    pub(crate) fn apply_auto_review_model_selection(
        &mut self,
        model: String,
        effort: ReasoningEffort,
    ) {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return;
        }

        self.config.auto_review_use_chat_model = false;
        let clamped_effort = Self::clamp_reasoning_for_model(trimmed, effort);

        let model_changed = if !self
            .config
            .auto_review_model
            .eq_ignore_ascii_case(trimmed)
        {
            trimmed.clone_into(&mut self.config.auto_review_model);
            true
        } else {
            false
        };

        let effort_changed = if self.config.auto_review_model_reasoning_effort != clamped_effort {
            self.config.auto_review_model_reasoning_effort = clamped_effort;
            true
        } else {
            false
        };

        let updated = model_changed || effort_changed;

        if !updated {
            self.bottom_pane
                .flash_footer_notice("Auto Review model unchanged.");
            return;
        }

        let notice = if let Ok(home) = code_core::config::find_code_home() {
            match code_core::config::set_auto_review_model(
                &home,
                &self.config.auto_review_model,
                self.config.auto_review_model_reasoning_effort,
                self.config.auto_review_use_chat_model,
            ) {
                Ok(_) => format!(
                    "Auto Review model set to {} ({} reasoning)",
                    self.config.auto_review_model,
                    Self::format_reasoning_effort(self.config.auto_review_model_reasoning_effort)
                ),
                Err(err) => {
                    tracing::warn!("Failed to persist Auto Review model: {err}");
                    format!(
                        "Auto Review model set for this session (failed to persist): {}",
                        self.config.auto_review_model
                    )
                }
            }
        } else {
            tracing::warn!("Could not locate Code home to persist Auto Review model");
            format!(
                "Auto Review model set for this session: {}",
                self.config.auto_review_model
            )
        };

        self.bottom_pane.flash_footer_notice(notice);
        self.refresh_settings_overview_rows();
        self.update_review_settings_model_row();
        self.request_redraw();
    }

    pub(crate) fn set_auto_review_use_chat_model(&mut self, use_chat: bool) {
        if self.config.auto_review_use_chat_model == use_chat {
            return;
        }
        self.config.auto_review_use_chat_model = use_chat;
        if use_chat {
            self.config.auto_review_model = self.config.model.clone();
            self.config.auto_review_model_reasoning_effort = self.config.model_reasoning_effort;
        }

        if let Ok(home) = code_core::config::find_code_home()
            && let Err(err) = code_core::config::set_auto_review_model(
                &home,
                &self.config.auto_review_model,
                self.config.auto_review_model_reasoning_effort,
                use_chat,
            ) {
                tracing::warn!("Failed to persist Auto Review use-chat toggle: {err}");
            }

        let notice = if use_chat {
            "Auto Review model now follows Chat model".to_owned()
        } else {
            format!(
                "Auto Review model set to {} ({} reasoning)",
                self.config.auto_review_model,
                Self::format_reasoning_effort(self.config.auto_review_model_reasoning_effort)
            )
        };
        self.bottom_pane.flash_footer_notice(notice);
        self.update_review_settings_model_row();
        self.refresh_settings_overview_rows();
        self.request_redraw();
    }

    pub(crate) fn apply_auto_review_resolve_model_selection(
        &mut self,
        model: String,
        effort: ReasoningEffort,
    ) {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return;
        }

        self.config.auto_review_resolve_use_chat_model = false;
        let clamped_effort = Self::clamp_reasoning_for_model(trimmed, effort);

        let model_changed = if !self
            .config
            .auto_review_resolve_model
            .eq_ignore_ascii_case(trimmed)
        {
            trimmed.clone_into(&mut self.config.auto_review_resolve_model);
            true
        } else {
            false
        };

        let effort_changed = if self.config.auto_review_resolve_model_reasoning_effort != clamped_effort {
            self.config.auto_review_resolve_model_reasoning_effort = clamped_effort;
            true
        } else {
            false
        };

        let updated = model_changed || effort_changed;

        if !updated {
            self.bottom_pane
                .flash_footer_notice("Auto Review resolve model unchanged.");
            return;
        }

        let notice = if let Ok(home) = code_core::config::find_code_home() {
            match code_core::config::set_auto_review_resolve_model(
                &home,
                &self.config.auto_review_resolve_model,
                self.config.auto_review_resolve_model_reasoning_effort,
                self.config.auto_review_resolve_use_chat_model,
            ) {
                Ok(_) => format!(
                    "Auto Review resolve model set to {} ({} reasoning)",
                    self.config.auto_review_resolve_model,
                    Self::format_reasoning_effort(self.config.auto_review_resolve_model_reasoning_effort)
                ),
                Err(err) => {
                    tracing::warn!("Failed to persist Auto Review resolve model: {err}");
                    format!(
                        "Auto Review resolve model set for this session (failed to persist): {}",
                        self.config.auto_review_resolve_model
                    )
                }
            }
        } else {
            tracing::warn!("Could not locate Code home to persist Auto Review resolve model");
            format!(
                "Auto Review resolve model set for this session: {}",
                self.config.auto_review_resolve_model
            )
        };

        self.bottom_pane.flash_footer_notice(notice);
        self.refresh_settings_overview_rows();
        self.update_review_settings_model_row();
        self.request_redraw();
    }

    pub(crate) fn set_auto_review_resolve_use_chat_model(&mut self, use_chat: bool) {
        if self.config.auto_review_resolve_use_chat_model == use_chat {
            return;
        }
        self.config.auto_review_resolve_use_chat_model = use_chat;
        if use_chat {
            self.config.auto_review_resolve_model = self.config.model.clone();
            self.config.auto_review_resolve_model_reasoning_effort =
                self.config.model_reasoning_effort;
        }

        if let Ok(home) = code_core::config::find_code_home()
            && let Err(err) = code_core::config::set_auto_review_resolve_model(
                &home,
                &self.config.auto_review_resolve_model,
                self.config.auto_review_resolve_model_reasoning_effort,
                use_chat,
            ) {
                tracing::warn!("Failed to persist Auto Review resolve use-chat toggle: {err}");
            }

        let notice = if use_chat {
            "Auto Review resolve model now follows Chat model".to_owned()
        } else {
            format!(
                "Auto Review resolve model set to {} ({} reasoning)",
                self.config.auto_review_resolve_model,
                Self::format_reasoning_effort(self.config.auto_review_resolve_model_reasoning_effort)
            )
        };
        self.bottom_pane.flash_footer_notice(notice);
        self.update_review_settings_model_row();
        self.refresh_settings_overview_rows();
        self.request_redraw();
    }

    pub(crate) fn set_auto_drive_use_chat_model(&mut self, use_chat: bool) {
        if self.config.auto_drive_use_chat_model == use_chat {
            return;
        }
        self.config.auto_drive_use_chat_model = use_chat;
        if use_chat {
            self.config.auto_drive.model = self.config.model.clone();
            self.config.auto_drive.model_reasoning_effort = self.config.model_reasoning_effort;
        }

        self.restore_auto_resolve_attempts_if_lost();

        if let Ok(home) = code_core::config::find_code_home()
            && let Err(err) = code_core::config::set_auto_drive_settings(
                &home,
                &self.config.auto_drive,
                use_chat,
            ) {
                tracing::warn!("Failed to persist Auto Drive use-chat toggle: {err}");
            }

        let notice = if use_chat {
            "Auto Drive model now follows Chat model".to_owned()
        } else {
            format!(
                "Auto Drive model set to {} ({} reasoning)",
                self.config.auto_drive.model,
                Self::format_reasoning_effort(self.config.auto_drive.model_reasoning_effort)
            )
        };

        self.bottom_pane.flash_footer_notice(notice);
        self.refresh_settings_overview_rows();
        self.update_auto_drive_settings_model_row();
        self.request_redraw();
    }

}
