//! Embedded Luau API documentation helpers.

/// Checked-in Luau definitions that define the scripting API contract.
pub const LUAU_API: &str = include_str!("../luau/hotki.d.luau");

/// Return the checked-in Luau API definitions.
pub fn luau_api() -> &'static str {
    LUAU_API
}

/// Return the Luau API definitions, optionally filtered to matching definition blocks.
pub fn luau_api_text(filter: Option<&str>) -> String {
    match filter.map(str::trim).filter(|value| !value.is_empty()) {
        Some(filter) => filter_blocks(LUAU_API, filter),
        None => LUAU_API.to_string(),
    }
}

/// Render the Luau API in a markdown fence.
pub fn luau_api_markdown(filter: Option<&str>) -> String {
    format!("```luau\n{}```", luau_api_text(filter))
}

/// Return the definition blocks that mention `filter`, preserving source order.
fn filter_blocks(source: &str, filter: &str) -> String {
    let needle = filter.to_ascii_lowercase();
    let mut matches = Vec::new();

    for block in source.split("\n\n") {
        if block.to_ascii_lowercase().contains(&needle) {
            matches.push(block.trim_end());
        }
    }

    if matches.is_empty() {
        String::new()
    } else {
        format!("{}\n", matches.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::{luau_api, luau_api_markdown, luau_api_text};

    #[test]
    fn api_text_returns_checked_in_file() {
        assert!(luau_api().contains("declare hotki: HotkiApi"));
    }

    #[test]
    fn api_filter_returns_matching_blocks() {
        let filtered = luau_api_text(Some("ThemesApi"));
        assert!(filtered.contains("export type ThemesApi"));
        assert!(!filtered.contains("export type ActionApi"));
    }

    #[test]
    fn markdown_wraps_filtered_content() {
        let markdown = luau_api_markdown(Some("ActionApi"));
        assert!(markdown.starts_with("```luau\n"));
        assert!(markdown.contains("export type ActionApi"));
        assert!(markdown.ends_with("```"));
    }
}
