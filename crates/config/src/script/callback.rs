//! Retained callback ownership for the dynamic configuration runtime.

use std::{
    fmt, mem,
    sync::{Arc, Mutex, Weak},
};

use ruau::{
    session::{FunctionHandle, LifecycleError, Runtime},
    vm::{Function, RuntimeError, Scope, StashedClosure},
};

use super::util::lock_unpoisoned;

/// Registry shared by callback references created by one dynamic config.
pub(super) type SharedCallbackRegistry = Arc<Mutex<CallbackRegistry>>;

/// Borrowed host context installed for each config entrypoint.
pub struct CallbackContext {
    /// Registry made visible to callbacks during this entrypoint.
    registry: SharedCallbackRegistry,
}

impl CallbackContext {
    /// Build a call context for one registry.
    pub(super) fn new(registry: SharedCallbackRegistry) -> Self {
        Self { registry }
    }
}

/// Callback state before and after promotion into the retained runtime.
enum CallbackTarget {
    /// Stash created during the currently active VM invocation.
    Pending(StashedClosure),
    /// Generational handle owned by the retained runtime.
    Retained(FunctionHandle),
}

/// Shared callback body. Its final drop queues a retained handle for release.
struct CallbackInner {
    /// Pending stash or promoted retained handle.
    target: Mutex<Option<CallbackTarget>>,
    /// Registry that receives the handle after the final owner drops.
    registry: Weak<Mutex<CallbackRegistry>>,
}

impl Drop for CallbackInner {
    fn drop(&mut self) {
        let target = self
            .target
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(CallbackTarget::Retained(handle)) = target else {
            return;
        };
        if let Some(registry) = self.registry.upgrade() {
            lock_unpoisoned(&registry).released.push(handle);
        }
    }
}

/// Cloneable callback reference with automatic last-owner release tracking.
#[derive(Clone)]
pub struct CallbackRef(Arc<CallbackInner>);

impl fmt::Debug for CallbackRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallbackRef")
            .finish_non_exhaustive()
    }
}

impl CallbackRef {
    /// Stash a function under the registry supplied by the active call context.
    pub(super) fn from_function<'s>(
        scope: &Scope<'s>,
        function: Function<'s>,
    ) -> Result<Self, RuntimeError> {
        let registry = scope
            .context_mut::<CallbackContext>()
            .ok_or_else(|| RuntimeError::runtime("missing Hotki callback context"))?
            .registry
            .clone();
        let callback = Self(Arc::new(CallbackInner {
            target: Mutex::new(Some(CallbackTarget::Pending(
                scope.stash_function(function)?,
            ))),
            registry: Arc::downgrade(&registry),
        }));
        lock_unpoisoned(&registry)
            .pending
            .push(Arc::downgrade(&callback.0));
        Ok(callback)
    }

    /// Resolve this callback inside the currently active retained-runtime scope.
    pub(super) fn resolve<'s>(&self, scope: &Scope<'s>) -> Result<Function<'s>, RuntimeError> {
        let target = lock_unpoisoned(&self.0.target);
        match target
            .as_ref()
            .ok_or_else(|| RuntimeError::runtime("Hotki callback was released"))?
        {
            CallbackTarget::Pending(stash) => scope.fetch_function(stash),
            CallbackTarget::Retained(handle) => handle
                .resolve(scope)
                .map_err(|error| RuntimeError::runtime(error.to_string())),
        }
    }
}

/// Pending promotions and deferred releases for one retained runtime.
#[derive(Default)]
pub(super) struct CallbackRegistry {
    /// Callbacks awaiting promotion after their creating VM entry closes.
    pending: Vec<Weak<CallbackInner>>,
    /// Retained handles whose final domain owner has dropped.
    released: Vec<FunctionHandle>,
}

impl CallbackRegistry {
    /// Promote callbacks and release dead handles between VM invocations.
    pub(super) fn synchronize(
        registry: &SharedCallbackRegistry,
        runtime: &mut Runtime,
    ) -> Result<(), LifecycleError> {
        let (pending, released) = {
            let mut registry = lock_unpoisoned(registry);
            (
                mem::take(&mut registry.pending),
                mem::take(&mut registry.released),
            )
        };

        for handle in released {
            match runtime.release(&handle) {
                Ok(()) | Err(LifecycleError::StaleHandle { .. }) => {}
                Err(error) => return Err(error),
            }
        }

        for pending in pending {
            let Some(callback) = pending.upgrade() else {
                continue;
            };
            let mut target = lock_unpoisoned(&callback.target);
            if let Some(CallbackTarget::Pending(stash)) = target.as_ref() {
                *target = Some(CallbackTarget::Retained(runtime.retain(stash.clone())));
            }
        }
        Ok(())
    }
}
