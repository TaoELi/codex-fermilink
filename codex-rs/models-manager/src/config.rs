use codex_protocol::config_types::Personality;
use codex_protocol::openai_models::ModelsResponse;

#[derive(Debug, Clone, Default)]
pub struct ModelsManagerConfig {
    pub model_context_window: Option<i64>,
    pub model_auto_compact_token_limit: Option<i64>,
    pub tool_output_token_limit: Option<usize>,
    pub base_instructions: Option<String>,
    /// Forces the standard Responses transport (fermilink fork). Set for
    /// agent-mode replacement instructions, which must be sent as top-level
    /// `instructions` rather than demoted to a developer message on the
    /// Responses Lite path. Internal flows that intentionally pair custom
    /// instructions with Responses Lite (for example Guardian review) leave
    /// this unset.
    pub force_standard_responses: bool,
    pub personality_enabled: bool,
    pub personality: Option<Personality>,
    pub model_catalog: Option<ModelsResponse>,
}
