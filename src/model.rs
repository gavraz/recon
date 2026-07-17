//! Model ID → display name and context-window mappings.
//!
//! ## Source of truth
//!
//! The context-window sizes below come from Anthropic's official models
//! overview. When a new model ships, refresh these constants from:
//!
//!   https://platform.claude.com/docs/en/about-claude/models/overview.md
//!
//! (the `.md` variant is plain markdown and easy to diff). The page lists every
//! current and legacy model with its exact API ID and context window. Last
//! synced 2026-07-17:
//!
//! | Model        | API ID                       | Context window |
//! |--------------|------------------------------|----------------|
//! | Fable 5      | claude-fable-5               | 1M             |
//! | Opus 4.8     | claude-opus-4-8              | 1M             |
//! | Opus 4.7     | claude-opus-4-7              | 1M             |
//! | Opus 4.6     | claude-opus-4-6              | 1M             |
//! | Opus 4.5     | claude-opus-4-5-20251101     | 200k           |
//! | Sonnet 5     | claude-sonnet-5              | 1M             |
//! | Sonnet 4.6   | claude-sonnet-4-6            | 1M             |
//! | Sonnet 4.5   | claude-sonnet-4-5-20250929   | 200k           |
//! | Haiku 4.5    | claude-haiku-4-5-20251001    | 200k           |
//!
//! Note: 1M-token context is not tied to a tier — Sonnet 4.6 and Sonnet 5 have
//! it, but Sonnet 4.5 does not — so each model must be listed explicitly rather
//! than inferred from the family name.

/// Map raw model IDs to human-friendly display names.
pub fn display_name(model_id: &str) -> &str {
    match model_id {
        "claude-fable-5" => "Fable 5",
        "claude-opus-4-8" => "Opus 4.8",
        "claude-opus-4-7" => "Opus 4.7",
        "claude-opus-4-6" => "Opus 4.6",
        "claude-opus-4-5-20251101" => "Opus 4.5",
        "claude-sonnet-5" => "Sonnet 5",
        "claude-sonnet-4-6" => "Sonnet 4.6",
        "claude-sonnet-4-5-20250929" => "Sonnet 4.5",
        "claude-sonnet-4-5-20250514" => "Sonnet 4.5",
        "claude-haiku-4-5-20251001" => "Haiku 4.5",
        "claude-opus-4-20250514" => "Opus 4",
        "claude-sonnet-4-20250514" => "Sonnet 4",
        _ => model_id,
    }
}

/// Context window size for a given model ID.
pub fn context_window(model_id: &str) -> u64 {
    match model_id {
        "claude-fable-5" => 1_000_000,
        "claude-opus-4-8" => 1_000_000,
        "claude-opus-4-7" => 1_000_000,
        "claude-opus-4-6" => 1_000_000,
        "claude-opus-4-5-20251101" => 200_000,
        "claude-sonnet-5" => 1_000_000,
        "claude-sonnet-4-6" => 1_000_000,
        "claude-sonnet-4-5-20250929" => 200_000,
        "claude-sonnet-4-5-20250514" => 200_000,
        "claude-haiku-4-5-20251001" => 200_000,
        "claude-opus-4-20250514" => 200_000,
        "claude-sonnet-4-20250514" => 200_000,
        // Unlisted Opus 4.6+ (e.g. future dated variants) default to 1M, not 200k.
        _ if is_opus_1m(model_id) => 1_000_000,
        _ => 200_000,
    }
}

/// Whether an unlisted model ID is an Opus minor version 4.6 or newer (1M
/// context). Legacy dated `claude-opus-4-*` has no minor version and returns false.
fn is_opus_1m(model_id: &str) -> bool {
    let Some(rest) = model_id.strip_prefix("claude-opus-4-") else {
        return false;
    };
    let minor: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    matches!(minor.parse::<u32>(), Ok(n) if n >= 6)
}

/// Reverse lookup: display name (from /model output) → model ID.
/// Returns None if the display name is not recognized.
pub fn id_from_display_name(display: &str) -> Option<&'static str> {
    match display {
        "Fable 5" | "Fable 5 (1M context)" => Some("claude-fable-5"),
        "Opus 4.8" | "Opus 4.8 (1M context)" => Some("claude-opus-4-8"),
        "Opus 4.7" | "Opus 4.7 (1M context)" => Some("claude-opus-4-7"),
        "Opus 4.6" | "Opus 4.6 (1M context)" => Some("claude-opus-4-6"),
        "Sonnet 5" | "Sonnet 5 (1M context)" => Some("claude-sonnet-5"),
        "Sonnet 4.6" => Some("claude-sonnet-4-6"),
        "Sonnet 4.5" => Some("claude-sonnet-4-5-20250929"),
        "Haiku 4.5" => Some("claude-haiku-4-5-20251001"),
        "Opus 4" => Some("claude-opus-4-20250514"),
        "Sonnet 4" => Some("claude-sonnet-4-20250514"),
        _ => None,
    }
}

/// Format model name with optional effort level.
pub fn format_with_effort(model_id: &str, effort: &str) -> String {
    let name = display_name(model_id);
    if effort.is_empty() || effort == "default" {
        name.to_string()
    } else {
        format!("{name} ({effort})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_million_context_models() {
        assert_eq!(context_window("claude-fable-5"), 1_000_000);
        assert_eq!(context_window("claude-opus-4-8"), 1_000_000);
        assert_eq!(context_window("claude-opus-4-7"), 1_000_000);
        assert_eq!(context_window("claude-opus-4-6"), 1_000_000);
        assert_eq!(context_window("claude-sonnet-5"), 1_000_000);
        assert_eq!(context_window("claude-sonnet-4-6"), 1_000_000);
    }

    #[test]
    fn future_dated_opus_variants_fall_back_to_1m() {
        assert_eq!(context_window("claude-opus-4-7-20260101"), 1_000_000);
        assert_eq!(context_window("claude-opus-4-9"), 1_000_000);
    }

    #[test]
    fn two_hundred_k_context_models() {
        assert_eq!(context_window("claude-opus-4-5-20251101"), 200_000);
        assert_eq!(context_window("claude-sonnet-4-5-20250929"), 200_000);
        assert_eq!(context_window("claude-haiku-4-5-20251001"), 200_000);
        assert_eq!(context_window("claude-opus-4-20250514"), 200_000);
        assert_eq!(context_window("something-unknown"), 200_000);
    }
}
