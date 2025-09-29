use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use core_foundation::{base::TCFType, string::CFString};
use once_cell::sync::Lazy;
use tracing::debug;

use super::{
    apply::{AxAttrRefs, apply_and_wait, apply_size_only_and_wait, nudge_axis_pos_and_wait},
    common::{Axis, SettleTiming},
    fallback::{fallback_shrink_move_grow, preflight_safe_park},
};
use crate::{
    Error, Result,
    ax::{ax_element_pid, ax_settable_pos_size, warn_once_nonsettable},
    geom::Rect,
};

/// Shared handle for Accessibility adapters used by placement.
pub type AxAdapterHandle = Arc<dyn AxAdapter>;

/// Adapter interface that encapsulates Accessibility operations so they can be
/// exercised deterministically in tests.
pub trait AxAdapter: Send + Sync + 'static {
    fn settable_pos_size(&self, win: &crate::AXElem) -> (Option<bool>, Option<bool>);
    fn element_pid(&self, win: &crate::AXElem) -> Option<i32>;
    fn warn_once_nonsettable(&self, pid: i32, can_pos: Option<bool>, can_size: Option<bool>);
    #[allow(clippy::too_many_arguments)]
    fn apply_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        target: &Rect,
        pos_first: bool,
        eps: f64,
        timing: SettleTiming,
    ) -> Result<(Rect, u64)>;
    #[allow(clippy::too_many_arguments)]
    fn apply_size_only_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        target_size: (f64, f64),
        eps: f64,
        timing: SettleTiming,
    ) -> Result<(Rect, u64)>;
    #[allow(clippy::too_many_arguments)]
    fn nudge_axis_pos_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        target: &Rect,
        axis: Axis,
        eps: f64,
        timing: SettleTiming,
    ) -> Result<(Rect, u64)>;
    #[allow(clippy::too_many_arguments)]
    fn fallback_shrink_move_grow(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        target: &Rect,
        eps: f64,
        timing: SettleTiming,
    ) -> Result<(Rect, u64)>;
    #[allow(clippy::too_many_arguments)]
    fn preflight_safe_park(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        visible_frame: &Rect,
        target: &Rect,
        eps: f64,
        timing: SettleTiming,
    ) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct SystemAxAdapter;

impl AxAdapter for SystemAxAdapter {
    fn settable_pos_size(&self, win: &crate::AXElem) -> (Option<bool>, Option<bool>) {
        ax_settable_pos_size(win.as_ptr())
    }

    fn element_pid(&self, win: &crate::AXElem) -> Option<i32> {
        ax_element_pid(win.as_ptr())
    }

    fn warn_once_nonsettable(&self, pid: i32, can_pos: Option<bool>, can_size: Option<bool>) {
        warn_once_nonsettable(pid, can_pos, can_size);
    }

    fn apply_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        target: &Rect,
        pos_first: bool,
        eps: f64,
        timing: SettleTiming,
    ) -> Result<(Rect, u64)> {
        apply_and_wait(op_label, win, attrs, target, pos_first, eps, timing)
    }

    fn apply_size_only_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        target_size: (f64, f64),
        eps: f64,
        timing: SettleTiming,
    ) -> Result<(Rect, u64)> {
        apply_size_only_and_wait(op_label, win, attrs, target_size, eps, timing)
    }

    fn nudge_axis_pos_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        target: &Rect,
        axis: Axis,
        eps: f64,
        timing: SettleTiming,
    ) -> Result<(Rect, u64)> {
        nudge_axis_pos_and_wait(op_label, win, attrs, target, axis, eps, timing)
    }

    fn fallback_shrink_move_grow(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        target: &Rect,
        eps: f64,
        timing: SettleTiming,
    ) -> Result<(Rect, u64)> {
        fallback_shrink_move_grow(op_label, win, attrs, target, eps, timing)
    }

    fn preflight_safe_park(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        attrs: AxAttrRefs,
        visible_frame: &Rect,
        target: &Rect,
        eps: f64,
        timing: SettleTiming,
    ) -> Result<()> {
        preflight_safe_park(op_label, win, attrs, visible_frame, target, eps, timing)
    }
}

static SYSTEM_ADAPTER: Lazy<AxAdapterHandle> = Lazy::new(|| {
    let adapter: AxAdapterHandle = Arc::new(SystemAxAdapter);
    adapter
});

pub(super) fn system_adapter_handle() -> AxAdapterHandle {
    SYSTEM_ADAPTER.clone()
}

#[derive(Debug, Clone)]
pub struct FakeApplyResponse {
    pub rect: Rect,
    pub settle_ms: u64,
    pub persist: bool,
}

impl FakeApplyResponse {
    pub fn new(rect: Rect) -> Self {
        Self {
            rect,
            settle_ms: 10,
            persist: true,
        }
    }

    pub fn with_persist(mut self, persist: bool) -> Self {
        self.persist = persist;
        self
    }
}

#[derive(Debug, Clone)]
pub enum FakeOp {
    Apply { label: String, pos_first: bool },
    SizeOnly { label: String },
    Nudge { label: String, axis: Axis },
    Fallback { label: String },
    Preflight { label: String },
}

#[derive(Debug)]
pub struct FakeWindowConfig {
    pub initial_rect: Rect,
    pub can_set_pos: Option<bool>,
    pub can_set_size: Option<bool>,
    pub pid: Option<i32>,
    pub apply_script: Vec<FakeApplyResponse>,
    pub size_only_script: Vec<FakeApplyResponse>,
    pub nudge_script: Vec<FakeApplyResponse>,
    pub fallback_script: Vec<FakeApplyResponse>,
    pub preflight_script: Vec<Result<()>>,
}

impl Default for FakeWindowConfig {
    fn default() -> Self {
        Self {
            initial_rect: Rect::new(0.0, 0.0, 800.0, 600.0),
            can_set_pos: Some(true),
            can_set_size: Some(true),
            pid: Some(4242),
            apply_script: Vec::new(),
            size_only_script: Vec::new(),
            nudge_script: Vec::new(),
            fallback_script: Vec::new(),
            preflight_script: Vec::new(),
        }
    }
}

#[derive(Default)]
pub struct FakeAxAdapter {
    windows: Mutex<HashMap<usize, FakeWindow>>, // key by raw pointer value
}

struct FakeWindow {
    can_set_pos: Option<bool>,
    can_set_size: Option<bool>,
    pid: Option<i32>,
    rect: Rect,
    apply: VecDeque<FakeApplyResponse>,
    size_only: VecDeque<FakeApplyResponse>,
    nudge: VecDeque<FakeApplyResponse>,
    fallback: VecDeque<FakeApplyResponse>,
    preflight: VecDeque<Result<()>>,
    ops: Vec<FakeOp>,
}

impl FakeWindow {
    fn push_ops(&mut self, op: FakeOp) {
        self.ops.push(op);
    }

    fn pop_response(queue: &mut VecDeque<FakeApplyResponse>, target: &Rect) -> FakeApplyResponse {
        queue
            .pop_front()
            .unwrap_or_else(|| FakeApplyResponse::new(*target))
    }
}

impl FakeAxAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_window(&self, config: FakeWindowConfig) -> crate::AXElem {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!("fake-window-{}", id);
        let cf = CFString::new(&name);
        let ptr = cf.as_concrete_TypeRef() as *mut _;
        std::mem::forget(cf);
        let elem = crate::AXElem::from_create(ptr).expect("non-null");
        let win = FakeWindow {
            can_set_pos: config.can_set_pos,
            can_set_size: config.can_set_size,
            pid: config.pid,
            rect: config.initial_rect,
            apply: config.apply_script.into_iter().collect(),
            size_only: config.size_only_script.into_iter().collect(),
            nudge: config.nudge_script.into_iter().collect(),
            fallback: config.fallback_script.into_iter().collect(),
            preflight: config.preflight_script.into_iter().collect(),
            ops: Vec::new(),
        };
        self.windows.lock().unwrap().insert(ptr as usize, win);
        elem
    }

    pub fn push_apply<I>(&self, win: &crate::AXElem, responses: I)
    where
        I: IntoIterator<Item = FakeApplyResponse>,
    {
        self.push_queue(win, responses, |window| &mut window.apply);
    }

    pub fn push_size_only<I>(&self, win: &crate::AXElem, responses: I)
    where
        I: IntoIterator<Item = FakeApplyResponse>,
    {
        self.push_queue(win, responses, |window| &mut window.size_only);
    }

    pub fn push_nudge<I>(&self, win: &crate::AXElem, responses: I)
    where
        I: IntoIterator<Item = FakeApplyResponse>,
    {
        self.push_queue(win, responses, |window| &mut window.nudge);
    }

    pub fn push_fallback<I>(&self, win: &crate::AXElem, responses: I)
    where
        I: IntoIterator<Item = FakeApplyResponse>,
    {
        self.push_queue(win, responses, |window| &mut window.fallback);
    }

    pub fn push_preflight<I>(&self, win: &crate::AXElem, outcomes: I)
    where
        I: IntoIterator<Item = Result<()>>,
    {
        let mut guard = self.windows.lock().unwrap();
        if let Some(window) = guard.get_mut(&(win.as_ptr() as usize)) {
            window.preflight.extend(outcomes);
        }
    }

    pub fn operations(&self, win: &crate::AXElem) -> Vec<FakeOp> {
        self.windows
            .lock()
            .unwrap()
            .get(&(win.as_ptr() as usize))
            .map(|w| w.ops.clone())
            .unwrap_or_default()
    }

    pub fn current_rect(&self, win: &crate::AXElem) -> Option<Rect> {
        self.windows
            .lock()
            .unwrap()
            .get(&(win.as_ptr() as usize))
            .map(|w| w.rect)
    }

    fn push_queue<I, F>(&self, win: &crate::AXElem, responses: I, mut select: F)
    where
        I: IntoIterator<Item = FakeApplyResponse>,
        F: FnMut(&mut FakeWindow) -> &mut VecDeque<FakeApplyResponse>,
    {
        let mut guard = self.windows.lock().unwrap();
        if let Some(window) = guard.get_mut(&(win.as_ptr() as usize)) {
            select(window).extend(responses);
        }
    }

    fn with_window<R, F>(&self, win: &crate::AXElem, f: F) -> Result<R>
    where
        F: FnOnce(&mut FakeWindow) -> R,
    {
        let mut guard = self.windows.lock().unwrap();
        let id = win.as_ptr() as usize;
        let window = guard.get_mut(&id).ok_or(Error::WindowGone)?;
        Ok(f(window))
    }
}

impl AxAdapter for FakeAxAdapter {
    fn settable_pos_size(&self, win: &crate::AXElem) -> (Option<bool>, Option<bool>) {
        self.windows
            .lock()
            .unwrap()
            .get(&(win.as_ptr() as usize))
            .map(|w| (w.can_set_pos, w.can_set_size))
            .unwrap_or((None, None))
    }

    fn element_pid(&self, win: &crate::AXElem) -> Option<i32> {
        self.windows
            .lock()
            .unwrap()
            .get(&(win.as_ptr() as usize))
            .and_then(|w| w.pid)
    }

    fn warn_once_nonsettable(&self, pid: i32, can_pos: Option<bool>, can_size: Option<bool>) {
        debug!(
            "fake_warn_nonsettable pid={} can_pos={:?} can_size={:?}",
            pid, can_pos, can_size
        );
    }

    fn apply_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        _attrs: AxAttrRefs,
        target: &Rect,
        pos_first: bool,
        _eps: f64,
        _timing: SettleTiming,
    ) -> Result<(Rect, u64)> {
        self.with_window(win, |window| {
            window.push_ops(FakeOp::Apply {
                label: op_label.to_string(),
                pos_first,
            });
            let resp = FakeWindow::pop_response(&mut window.apply, target);
            if resp.persist {
                window.rect = resp.rect;
            }
            (resp.rect, resp.settle_ms)
        })
    }

    fn apply_size_only_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        _attrs: AxAttrRefs,
        target_size: (f64, f64),
        _eps: f64,
        _timing: SettleTiming,
    ) -> Result<(Rect, u64)> {
        self.with_window(win, |window| {
            window.push_ops(FakeOp::SizeOnly {
                label: op_label.to_string(),
            });
            let target_rect = Rect {
                w: target_size.0,
                h: target_size.1,
                ..window.rect
            };
            let resp = FakeWindow::pop_response(&mut window.size_only, &target_rect);
            if resp.persist {
                window.rect = Rect {
                    w: resp.rect.w,
                    h: resp.rect.h,
                    ..window.rect
                };
            }
            (resp.rect, resp.settle_ms)
        })
    }

    fn nudge_axis_pos_and_wait(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        _attrs: AxAttrRefs,
        target: &Rect,
        axis: Axis,
        _eps: f64,
        _timing: SettleTiming,
    ) -> Result<(Rect, u64)> {
        self.with_window(win, |window| {
            window.push_ops(FakeOp::Nudge {
                label: op_label.to_string(),
                axis,
            });
            let resp = FakeWindow::pop_response(&mut window.nudge, target);
            if resp.persist {
                window.rect = resp.rect;
            }
            (resp.rect, resp.settle_ms)
        })
    }

    fn fallback_shrink_move_grow(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        _attrs: AxAttrRefs,
        target: &Rect,
        _eps: f64,
        _timing: SettleTiming,
    ) -> Result<(Rect, u64)> {
        self.with_window(win, |window| {
            window.push_ops(FakeOp::Fallback {
                label: op_label.to_string(),
            });
            let resp = FakeWindow::pop_response(&mut window.fallback, target);
            if resp.persist {
                window.rect = resp.rect;
            }
            (resp.rect, resp.settle_ms)
        })
    }

    fn preflight_safe_park(
        &self,
        op_label: &str,
        win: &crate::AXElem,
        _attrs: AxAttrRefs,
        _visible_frame: &Rect,
        target: &Rect,
        _eps: f64,
        _timing: SettleTiming,
    ) -> Result<()> {
        self.with_window(win, |window| {
            window.push_ops(FakeOp::Preflight {
                label: op_label.to_string(),
            });
            let outcome = window.preflight.pop_front().unwrap_or(Ok(()));
            if outcome.is_ok() {
                window.rect = *target;
            }
            outcome
        })?
    }
}
