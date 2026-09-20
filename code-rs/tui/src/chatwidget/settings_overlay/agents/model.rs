use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::settings_pages::agents::{AgentEditorView, SubagentEditorView};
use code_core::config_types::ModelRole;
use unicode_width::UnicodeWidthStr;

const ROLE_COLUMNS: [(ModelRole, &str, u16); 4] = [
    (ModelRole::Session, "Session", 7),
    (ModelRole::Subagent, "Sub-agent", 10),
    (ModelRole::Review, "Review", 8),
    (ModelRole::AutoDrive, "Auto Drive", 11),
];

#[derive(Clone, Copy, Debug)]
pub(super) struct AgentsGridLayout {
    pub(super) name_width: u16,
    pub(super) role_starts: [u16; 4],
}

impl AgentsGridLayout {
    pub(super) fn new(rows: &[AgentOverviewRow], available_width: u16) -> Self {
        let max_name_width = rows
            .iter()
            .map(|row| UnicodeWidthStr::width(row.name.as_str()) as u16)
            .max()
            .unwrap_or(12)
            .max(12);
        let fixed_width = 2 + 2 + ROLE_COLUMNS.iter().map(|(_, _, width)| *width).sum::<u16>();
        let role_gaps = (ROLE_COLUMNS.len() - 1) as u16;
        let available_name_width = available_width.saturating_sub(fixed_width + role_gaps);
        let name_width = max_name_width.min(available_name_width.max(12)).min(30);
        let mut role_starts = [0; 4];
        let mut x = 2 + name_width + 2;
        for (index, (_, _, width)) in ROLE_COLUMNS.iter().enumerate() {
            role_starts[index] = x;
            x = x.saturating_add(*width).saturating_add(1);
        }
        Self {
            name_width,
            role_starts,
        }
    }

    pub(super) fn role_at(self, column: u16) -> Option<ModelRole> {
        ROLE_COLUMNS
            .iter()
            .enumerate()
            .find_map(|(index, (role, _, width))| {
                let start = self.role_starts[index];
                (column >= start && column < start.saturating_add(*width)).then_some(*role)
            })
    }

}

pub(super) fn model_roles() -> impl Iterator<Item = (ModelRole, &'static str, u16)> {
    ROLE_COLUMNS.into_iter()
}

pub(super) fn next_model_role(role: ModelRole) -> ModelRole {
    match role {
        ModelRole::Session => ModelRole::Subagent,
        ModelRole::Subagent => ModelRole::Review,
        ModelRole::Review => ModelRole::AutoDrive,
        ModelRole::AutoDrive => ModelRole::Session,
    }
}

pub(super) fn previous_model_role(role: ModelRole) -> ModelRole {
    match role {
        ModelRole::Session => ModelRole::AutoDrive,
        ModelRole::Subagent => ModelRole::Session,
        ModelRole::Review => ModelRole::Subagent,
        ModelRole::AutoDrive => ModelRole::Review,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AgentOverviewRow {
    pub(crate) name: String,
    pub(crate) session_enabled: bool,
    pub(crate) subagent_enabled: bool,
    pub(crate) review_enabled: bool,
    pub(crate) auto_drive_enabled: bool,
    pub(crate) installed: bool,
    pub(crate) description: Option<String>,
    pub(crate) command: String,
}

impl AgentOverviewRow {
    pub(super) fn role_enabled(&self, role: ModelRole) -> bool {
        match role {
            ModelRole::Session => self.session_enabled,
            ModelRole::Subagent => self.subagent_enabled,
            ModelRole::Review => self.review_enabled,
            ModelRole::AutoDrive => self.auto_drive_enabled,
        }
    }
}

#[derive(Default)]
pub(super) struct AgentsOverviewState {
    pub(super) rows: Vec<AgentOverviewRow>,
    pub(super) commands: Vec<String>,
    pub(super) selected: usize,
    pub(super) selected_role: ModelRole,
}

impl AgentsOverviewState {
    pub(super) fn total_rows(&self) -> usize {
        self.rows
            .len()
            .saturating_add(self.commands.len())
            .saturating_add(2)
    }

    pub(super) fn clamp_selection(&mut self) {
        let total = self.total_rows();
        if total == 0 {
            self.selected = 0;
        } else if self.selected >= total {
            self.selected = total - 1;
        }
    }
}

pub(super) enum AgentsPane {
    Overview(AgentsOverviewState),
    Subagent(Box<SubagentEditorView>),
    Agent(Box<AgentEditorView>),
}

pub(crate) struct AgentsSettingsContent {
    pub(super) pane: AgentsPane,
    pub(super) app_event_tx: AppEventSender,
}

impl AgentsSettingsContent {
    pub(crate) fn new_overview(
        rows: Vec<AgentOverviewRow>,
        commands: Vec<String>,
        selected: usize,
        app_event_tx: AppEventSender,
    ) -> Self {
        let mut overview = AgentsOverviewState {
            rows,
            commands,
            selected,
            selected_role: ModelRole::Session,
        };
        overview.clamp_selection();
        Self {
            pane: AgentsPane::Overview(overview),
            app_event_tx,
        }
    }

    pub(crate) fn set_overview(
        &mut self,
        rows: Vec<AgentOverviewRow>,
        commands: Vec<String>,
        selected: usize,
    ) {
        let selected_role = match &self.pane {
            AgentsPane::Overview(state) => state.selected_role,
            AgentsPane::Subagent(_) | AgentsPane::Agent(_) => ModelRole::Session,
        };
        let mut overview = AgentsOverviewState {
            rows,
            commands,
            selected,
            selected_role,
        };
        overview.clamp_selection();
        self.pane = AgentsPane::Overview(overview);
    }

    pub(crate) fn set_editor(&mut self, editor: SubagentEditorView) {
        self.pane = AgentsPane::Subagent(Box::new(editor));
    }

    pub(crate) fn set_overview_selection(&mut self, selected: usize) {
        if let AgentsPane::Overview(state) = &mut self.pane {
            state.selected = selected;
            state.clamp_selection();
        }
    }

    pub(crate) fn set_agent_editor(&mut self, editor: AgentEditorView) {
        self.pane = AgentsPane::Agent(Box::new(editor));
    }

    #[cfg(any(test, feature = "test-helpers"))]
    pub(crate) fn is_agent_editor_active(&self) -> bool {
        matches!(self.pane, AgentsPane::Agent(_))
    }

    #[cfg(test)]
    pub(crate) fn overview_agent_names(&self) -> Vec<&str> {
        match &self.pane {
            AgentsPane::Overview(state) => state.rows.iter().map(|row| row.name.as_str()).collect(),
            AgentsPane::Subagent(_) | AgentsPane::Agent(_) => Vec::new(),
        }
    }
}
