//! `/profile` popup: pick the agent profile (fermilink fork).
//!
//! Selecting a profile persists `agent_profile` in the user config and starts
//! a fresh thread, because base instructions are thread-scoped and switching
//! prompt families mid-thread would mix histories.

use super::*;
use codex_agent_profiles::BUILT_IN_AGENT_PROFILES;

impl ChatWidget {
    pub(crate) fn open_agent_profile_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Profile selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let current_profile = self.config.agent_profile.as_str();
        let items: Vec<SelectionItem> = BUILT_IN_AGENT_PROFILES
            .iter()
            .map(|profile| {
                let profile_id = profile.id.to_string();
                let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                    tx.send(AppEvent::SelectAgentProfile {
                        id: profile_id.clone(),
                    });
                })];
                SelectionItem {
                    name: profile.display_name.to_string(),
                    description: Some(profile.description.to_string()),
                    is_current: profile.id == current_profile,
                    is_default: profile.id == codex_agent_profiles::DEFAULT_AGENT_PROFILE_ID,
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Select Agent Profile".bold()));
        header.push(Line::from(
            "Sets the base instructions for new threads; changing profile starts a new chat.".dim(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }
}
