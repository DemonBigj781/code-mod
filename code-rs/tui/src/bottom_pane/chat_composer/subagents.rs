use super::*;

impl ChatComposer {
    pub(crate) fn set_subagent_commands(&mut self, mut names: Vec<String>) {
        names.retain(|n| !is_reserved_subagent_name(n));
        names.sort();
        self.subagent_commands = names;
        if let ActivePopup::Command(popup) = &mut self.active_popup {
            popup.set_subagent_commands(self.subagent_commands.clone());
        }
    }

    pub(crate) fn set_project_aliases(&mut self, mut names: Vec<String>) {
        names.sort();
        names.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        self.project_aliases = names;
        if let ActivePopup::Command(popup) = &mut self.active_popup {
            popup.set_project_aliases(self.project_aliases.clone());
        }
    }
}
