//! Narrow engine-facing facade for the retained Hotki configuration runtime.

use std::path::Path;

use mac_keycode::Chord;
use script::{config::LoadedConfig, types::ModeFrame};
pub use script::{
    handler::HandlerResult,
    selector::{SelectorConfig, SelectorItem},
    types::{
        ActionRepeatPermission, Binding, BindingKind, Effect, HandlerRef, ModeCtx, ModeId, ModeRef,
        NavRequest, RenderedState, RepeatSpec,
    },
};

use crate::{Error, Style, script};

/// Loaded, retained configuration runtime used by the Hotki engine.
pub struct ConfigRuntime(LoadedConfig);

/// Opaque stack of active configuration modes.
#[derive(Clone, Debug, Default)]
pub struct ModeStack(Vec<ModeFrame>);

impl ConfigRuntime {
    /// Load and validate a filesystem-backed Hotki configuration.
    pub fn load(path: &Path) -> Result<Self, Error> {
        script::loader::load_dynamic_config(path).map(Self)
    }

    /// Return the resolved base style owned by this runtime candidate.
    pub fn style(&self) -> Style {
        self.0.base_style.clone()
    }

    /// Ensure this runtime's root mode is installed in an empty stack.
    pub fn ensure_stack(&self, stack: &mut ModeStack) {
        if stack.0.is_empty() {
            stack.0.push(root_frame(self.0.root.clone()));
        }
    }

    /// Reset a stack to this runtime's root mode.
    pub fn reset_stack(&self, stack: &mut ModeStack) {
        stack.0 = vec![root_frame(self.0.root.clone())];
    }

    /// Render the active mode stack into engine-consumable state and warnings.
    pub fn render(&mut self, stack: &mut ModeStack, ctx: &ModeCtx) -> Result<RuntimeRender, Error> {
        let base_style = self.0.base_style.clone();
        let output = script::render::render_stack(&mut self.0, &mut stack.0, ctx, &base_style)?;
        Ok(RuntimeRender {
            state: output.rendered,
            warnings: output.warnings,
        })
    }

    /// Execute a handler using normal held-key repeat permissions.
    pub fn execute_handler(
        &mut self,
        handler: &HandlerRef,
        ctx: &ModeCtx,
    ) -> Result<HandlerResult, Error> {
        script::handler::execute_handler(&mut self.0, handler, ctx)
    }

    /// Execute a handler with an explicit repeat permission.
    pub fn execute_handler_with_permission(
        &mut self,
        handler: &HandlerRef,
        ctx: &ModeCtx,
        repeat: ActionRepeatPermission,
    ) -> Result<HandlerResult, Error> {
        script::handler::execute_handler_with_permission(&mut self.0, handler, ctx, repeat)
    }

    /// Execute a selector's selection callback with the chosen item and final query.
    pub fn execute_selector_selection(
        &mut self,
        selector: &SelectorConfig,
        ctx: &ModeCtx,
        item: &SelectorItem,
        query: &str,
    ) -> Result<HandlerResult, Error> {
        script::handler::execute_selector_handler(
            &mut self.0,
            &selector.on_select,
            ctx,
            item,
            query,
        )
    }

    /// Execute a selector's cancel callback when one is configured.
    pub fn execute_selector_cancel(
        &mut self,
        selector: &SelectorConfig,
        ctx: &ModeCtx,
    ) -> Result<Option<HandlerResult>, Error> {
        selector
            .on_cancel
            .as_ref()
            .map(|handler| script::handler::execute_handler(&mut self.0, handler, ctx))
            .transpose()
    }

    /// Resolve the items for a selector, evaluating its provider when configured.
    pub fn resolve_selector_items(
        &mut self,
        selector: &SelectorConfig,
        ctx: &ModeCtx,
    ) -> Result<Vec<SelectorItem>, Error> {
        selector.resolve_items(&mut self.0, ctx)
    }

    /// Resolve a chord against a rendered runtime snapshot.
    pub fn resolve_binding<'a>(rendered: &'a RenderedState, chord: &Chord) -> Option<&'a Binding> {
        script::render::resolve_binding(rendered, chord)
    }
}

impl ModeStack {
    /// Remove every active mode.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Number of child modes above the root.
    pub fn depth(&self) -> usize {
        self.0.len().saturating_sub(1)
    }

    /// Titles of active child modes in root-to-leaf order.
    pub fn breadcrumbs(&self) -> Vec<String> {
        self.0
            .iter()
            .skip(1)
            .map(|frame| frame.title.clone())
            .collect()
    }

    /// Push a child mode and retain how it was entered.
    pub fn push(
        &mut self,
        title: String,
        closure: ModeRef,
        entered_via: Option<(Chord, ModeId)>,
        capture: bool,
    ) {
        self.0.push(ModeFrame {
            title,
            closure,
            entered_via,
            rendered: Vec::new(),
            capture,
        });
    }

    /// Pop one child mode, leaving the root installed.
    pub fn pop(&mut self) -> bool {
        if self.depth() == 0 {
            return false;
        }
        self.0.pop();
        true
    }

    /// Remove every child mode, leaving the root installed.
    pub fn reset_to_root(&mut self) -> bool {
        let changed = self.depth() > 0;
        self.0.truncate(1);
        changed
    }
}

/// Result of rendering the active configuration stack.
#[derive(Debug, Clone)]
pub struct RuntimeRender {
    /// Fully rendered runtime state.
    pub state: RenderedState,
    /// User-visible warnings emitted during rendering.
    pub warnings: Vec<Effect>,
}

/// Construct the root frame for a loaded runtime.
fn root_frame(closure: ModeRef) -> ModeFrame {
    ModeFrame {
        title: "root".to_string(),
        closure,
        entered_via: None,
        rendered: Vec::new(),
        capture: false,
    }
}
