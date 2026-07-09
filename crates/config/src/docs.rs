//! Embedded Luau API documentation helpers.

/// Declarations for behavior-oriented `config.luau` files.
pub const LUAU_CONFIG_API: &str = include_str!("../luau/hotki_config.d.luau");

/// Declarations for standalone `style.luau` files.
pub const LUAU_STYLE_API: &str = include_str!("../luau/hotki_style.d.luau");

/// Luau API surfaces available to the checker and CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuauApiSurface {
    /// Behavior config declarations.
    Config,
    /// Standalone style declarations.
    Style,
    /// Combined declarations for tooling.
    All,
}

/// Return the checked-in Luau config API definitions.
pub fn luau_api() -> &'static str {
    LUAU_CONFIG_API
}

/// Return an owned declaration bundle for one surface.
pub fn luau_api_surface(surface: LuauApiSurface) -> String {
    match surface {
        LuauApiSurface::Config => join_api([LUAU_CONFIG_API]),
        LuauApiSurface::Style => join_api([LUAU_STYLE_API]),
        LuauApiSurface::All => join_api([LUAU_CONFIG_API, LUAU_STYLE_API]),
    }
}

/// Return the Luau API definitions, optionally filtered to matching definition blocks.
pub fn luau_api_text(surface: LuauApiSurface, filter: Option<&str>) -> String {
    let source = luau_api_surface(surface);
    match filter.map(str::trim).filter(|value| !value.is_empty()) {
        Some(filter) => filter_blocks(&source, filter),
        None => source,
    }
}

/// Render the Luau API in a markdown fence.
pub fn luau_api_markdown(surface: LuauApiSurface, filter: Option<&str>) -> String {
    format!("```luau\n{}```", luau_api_text(surface, filter))
}

/// Join non-empty API declaration fragments with one blank line.
fn join_api<const N: usize>(parts: [&str; N]) -> String {
    let mut out = parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    out.push('\n');
    out
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
    use super::{LuauApiSurface, luau_api, luau_api_markdown, luau_api_surface, luau_api_text};

    #[test]
    fn runtime_api_returns_config_file() {
        assert!(luau_api().contains("declare hotki: HotkiApi"));
        assert!(luau_api().contains("declare action: ActionApi"));
        assert!(!luau_api().contains("type Style = {"));
    }

    #[test]
    fn config_api_contains_only_config_declarations() {
        let api = luau_api_surface(LuauApiSurface::Config);
        assert!(api.contains("type Toggle ="));
        assert!(api.contains("type ActionApi = {"));
        assert!(!api.contains("type Color ="));
        assert!(!api.contains("type Style = {"));
    }

    #[test]
    fn style_api_contains_only_style_declarations() {
        let api = luau_api_surface(LuauApiSurface::Style);
        assert!(api.contains("type FontWeight ="));
        assert!(api.contains("type Style = {"));
        assert!(!api.contains("type SelectorItem"));
        assert!(!api.contains("type ActionApi = {"));
    }

    #[test]
    fn all_api_contains_config_and_style_declarations() {
        let api = luau_api_surface(LuauApiSurface::All);
        assert!(api.contains("type ActionApi = {"));
        assert!(api.contains("type Style = {"));
    }

    #[test]
    fn api_filter_returns_matching_blocks() {
        let filtered = luau_api_text(LuauApiSurface::Config, Some("ActionApi"));
        assert!(filtered.contains("type ActionApi"));
        assert!(filtered.contains("shell: (cmd: string"));
        assert!(filtered.contains("selector: <T>(spec: SelectorSpec<T>)"));
        assert!(filtered.contains("declare action: ActionApi"));
        assert!(!filtered.contains("type HotkiApi"));
    }

    #[test]
    fn api_filter_action_keeps_action_field_list() {
        let filtered = luau_api_text(LuauApiSurface::Config, Some("action"));
        assert!(filtered.contains("type ActionApi"));
        assert!(filtered.contains("shell: (cmd: string"));
        assert!(filtered.contains("reload_config: Action"));
        assert!(filtered.contains("declare action: ActionApi"));
    }

    #[test]
    fn api_filter_hotki_keeps_hotki_field_list() {
        let filtered = luau_api_text(LuauApiSurface::Config, Some("hotki"));
        assert!(filtered.contains("type HotkiApi"));
        assert!(filtered.contains("root: (render: ModeRenderer) -> ()"));
        assert!(filtered.contains("applications: SelectorItemProvider<ApplicationInfo>"));
        assert!(filtered.contains("declare hotki: HotkiApi"));
    }

    #[test]
    fn api_filter_hotki_api_keeps_hotki_field_list() {
        let filtered = luau_api_text(LuauApiSurface::Config, Some("HotkiApi"));
        assert!(filtered.contains("type HotkiApi"));
        assert!(filtered.contains("applications: SelectorItemProvider<ApplicationInfo>"));
        assert!(filtered.contains("declare hotki: HotkiApi"));
        assert!(!filtered.contains("type ActionApi"));
    }

    #[test]
    fn markdown_wraps_filtered_content() {
        let markdown = luau_api_markdown(LuauApiSurface::Style, Some("Style"));
        assert!(markdown.starts_with("```luau\n"));
        assert!(markdown.contains("type Style"));
        assert!(markdown.ends_with("```"));
    }
}
