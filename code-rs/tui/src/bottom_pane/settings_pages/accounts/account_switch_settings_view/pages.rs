use super::AccountSwitchSettingsView;

use crate::bottom_pane::settings_ui::hints::{hint_esc, KeyHint};
use crate::bottom_pane::settings_ui::line_runs::SelectableLineRun;
use crate::bottom_pane::settings_ui::menu_page::SettingsMenuPage;
use crate::bottom_pane::settings_ui::menu_rows::SettingsMenuRow;
use crate::bottom_pane::settings_ui::panel::SettingsPanelStyle;
use crate::bottom_pane::settings_ui::rows::StyledText;
use crate::bottom_pane::settings_ui::toggle;
use crate::colors;
use code_core::config_types::AuthCredentialsStoreMode;
use ratatui::layout::Margin;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};

impl AccountSwitchSettingsView {
    fn provider_auth_label(provider_id: &str) -> (&'static str, &'static str) {
        let providers = code_core::built_in_model_providers(None);
        let Some(provider) = providers.get(provider_id) else {
            return ("Unavailable", "Provider is not registered in this build.");
        };

        if provider_id == code_core::STABLEHORDE_PROVIDER_ID {
            if provider.uses_stablehorde_anonymous_auth() {
                return (
                    "Anonymous",
                    "Uses 0000000000 at lowest queue priority; set AI_HORDE_API_KEY for authenticated access.",
                );
            }
            return (
                "Authenticated",
                "AI_HORDE_API_KEY is available from the encrypted store or environment.",
            );
        }

        if provider.api_key().ok().flatten().is_some() {
            (
                "Configured",
                "OPENROUTER_API_KEY is available from the encrypted store or environment.",
            )
        } else {
            (
                "Required",
                "Set OPENROUTER_API_KEY in the encrypted store or environment before use.",
            )
        }
    }

    pub(super) fn main_page(&self) -> SettingsMenuPage<'static> {
        SettingsMenuPage::new(
            "Accounts",
            SettingsPanelStyle::bottom_pane().with_margin(Margin::new(0, 0)),
            Vec::new(),
            Vec::new(),
        )
        .with_shortcuts(
            crate::bottom_pane::settings_ui::hints::ShortcutPlacement::Bottom,
            vec![
                KeyHint::new(format!("{ud}/Tab", ud = crate::icons::nav_up_down()), " navigate"),
                KeyHint::new("Enter/Space", " activate"),
                hint_esc(" close"),
            ],
        )
    }

    pub(super) fn main_runs(
        &self,
        selected_id: Option<usize>,
    ) -> Vec<SelectableLineRun<'static, usize>> {
        let bool_value = |enabled: bool| toggle::checkbox_marker(enabled);

        let mut runs = Vec::new();

        let mut auto = SettingsMenuRow::new(0usize, "Auto-switch on rate/usage limit")
            .with_value(bool_value(self.auto_switch_enabled))
            .with_selected_hint("Enter to toggle")
            .into_run(selected_id);
        auto.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                "Switches to another connected account on 429/usage_limit.",
                Style::new().fg(colors::text_dim()),
            ),
        ]));
        runs.push(auto);

        let mut fallback = SettingsMenuRow::new(1usize, "API key fallback when all accounts limited")
            .with_value(bool_value(self.api_key_fallback_enabled))
            .with_selected_hint("Enter to toggle")
            .into_run(selected_id);
        fallback.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                "Only used if every connected ChatGPT account is limited.",
                Style::new().fg(colors::text_dim()),
            ),
        ]));
        runs.push(fallback);

        let store_mode = Self::auth_store_mode_label(self.auth_credentials_store_mode);
        let store_detail = match self.auth_credentials_store_mode {
            AuthCredentialsStoreMode::Ephemeral => {
                "In-memory only (will not persist across restarts)."
            }
            _ => "Where Code stores CLI auth credentials (auth.json payload).",
        };
        let mut store = SettingsMenuRow::new(2usize, "Credential Store")
            .with_value(StyledText::new(
                format!("[{store_mode}]"),
                Style::new().fg(colors::primary()).bold(),
            ))
            .with_selected_hint("Enter to change")
            .into_run(selected_id);
        store.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(store_detail, Style::new().fg(colors::text_dim())),
        ]));
        runs.push(store);

        runs.push(SelectableLineRun::plain(vec![Line::from("")]));

        let (openrouter_status, openrouter_detail) = Self::provider_auth_label(
            code_common::model_presets::OPENROUTER_PROVIDER_ID,
        );
        let mut openrouter = SettingsMenuRow::new(3usize, "OpenRouter authentication")
            .with_value(StyledText::new(
                format!("[{openrouter_status}]"),
                Style::new().fg(colors::primary()).bold(),
            ))
            .with_selected_hint("Enter to manage secrets")
            .into_run(selected_id);
        openrouter.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(openrouter_detail, Style::new().fg(colors::text_dim())),
        ]));
        runs.push(openrouter);

        let (stablehorde_status, stablehorde_detail) =
            Self::provider_auth_label(code_core::STABLEHORDE_PROVIDER_ID);
        let mut stablehorde = SettingsMenuRow::new(4usize, "Stable Horde authentication")
            .with_value(StyledText::new(
                format!("[{stablehorde_status}]"),
                Style::new().fg(colors::primary()).bold(),
            ))
            .with_selected_hint("Enter to manage secrets")
            .into_run(selected_id);
        stablehorde.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(stablehorde_detail, Style::new().fg(colors::text_dim())),
        ]));
        runs.push(stablehorde);

        runs.push(SelectableLineRun::plain(vec![Line::from("")]));

        let mut manage = SettingsMenuRow::new(5usize, "Manage connected accounts")
            .with_selected_hint("Enter to open")
            .into_run(selected_id);
        manage.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                "View, switch, and remove stored accounts.",
                Style::new().fg(colors::text_dim()),
            ),
        ]));
        runs.push(manage);

        let mut add = SettingsMenuRow::new(6usize, "Add account")
            .with_selected_hint("Enter to open")
            .into_run(selected_id);
        add.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                "Start ChatGPT or API-key account setup.",
                Style::new().fg(colors::text_dim()),
            ),
        ]));
        runs.push(add);

        runs.push(SelectableLineRun::plain(vec![Line::from("")]));

        runs.push(
            SettingsMenuRow::new(7usize, "Close")
                .with_selected_hint("Enter to close")
                .into_run(selected_id),
        );

        runs
    }

    pub(super) fn confirm_page(&self, target: AuthCredentialsStoreMode) -> SettingsMenuPage<'static> {
        let current = Self::auth_store_mode_label(self.auth_credentials_store_mode);
        let next = Self::auth_store_mode_label(target);
        let header_lines = vec![
            Line::from(vec![
                Span::styled("Current: ", Style::new().fg(colors::text_dim())),
                Span::styled(current, Style::new().fg(colors::text())),
                Span::styled("   New: ", Style::new().fg(colors::text_dim())),
                Span::styled(next, Style::new().fg(colors::primary()).bold()),
            ]),
            Line::from(""),
        ];
        let shortcuts = vec![
            KeyHint::new(format!("{ud}/Tab", ud = crate::icons::nav_up_down()), " select"),
            KeyHint::new("Enter/Space", " apply"),
            hint_esc(" back"),
        ];

        SettingsMenuPage::new(
            "Credential Store",
            SettingsPanelStyle::bottom_pane().with_margin(Margin::new(0, 0)),
            header_lines,
            Vec::new(),
        )
        .with_shortcuts(crate::bottom_pane::settings_ui::hints::ShortcutPlacement::Bottom, shortcuts)
    }

    pub(super) fn confirm_rows(&self) -> Vec<SettingsMenuRow<'static, usize>> {
        vec![
            SettingsMenuRow::new(0usize, "Apply + migrate existing credentials"),
            SettingsMenuRow::new(1usize, "Apply (do not migrate)  (may log you out)"),
            SettingsMenuRow::new(2usize, "Cancel"),
        ]
    }
}
