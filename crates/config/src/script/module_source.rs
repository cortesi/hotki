//! Checked, cached module-source boundary for one config load.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ruau::source::{
    InstanceKey, ModuleId, ReadContext, SourceError, SourceFuture, SourceMetadata, SourceProvider,
    resolve_request,
};

use super::{config::SourceMap, util::lock_unpoisoned};

/// Requester and literal request bytes used to cache one resolution edge.
type ResolutionKey = (Option<ModuleId>, Vec<u8>);

/// Admission phase for one checked module graph.
#[derive(Default)]
enum GraphPhase {
    /// Resolution and source discovery are still building the graph.
    #[default]
    Building,
    /// Only modules in the checked graph may be resolved or read.
    Allowed(HashSet<ModuleId>),
    /// Entry evaluation completed; only cached resolutions remain available.
    Sealed,
}

/// Caches and admission policy advanced under one lock.
#[derive(Default)]
struct GraphState {
    /// Resolution edges retained for runtime cache hits.
    resolutions: HashMap<ResolutionKey, ModuleId>,
    /// Source bytes retained from graph preparation.
    bytes: HashMap<ModuleId, Vec<u8>>,
    /// Current graph admission phase.
    phase: GraphPhase,
}

impl GraphState {
    /// Install the checked graph allowlist.
    fn install_allowlist(&mut self, modules: impl IntoIterator<Item = ModuleId>) {
        assert!(
            !matches!(self.phase, GraphPhase::Sealed),
            "cannot install a module allowlist after sealing"
        );
        self.phase = GraphPhase::Allowed(modules.into_iter().collect());
    }

    /// End source loading while retaining cached resolution edges.
    fn seal(&mut self) {
        self.phase = GraphPhase::Sealed;
    }

    /// Admit a resolution or return its sealed cached result.
    fn resolve_admission(
        &self,
        key: &ResolutionKey,
        requester: Option<&ModuleId>,
        request: &[u8],
    ) -> Result<Option<ModuleId>, SourceError> {
        match &self.phase {
            GraphPhase::Building => Ok(None),
            GraphPhase::Allowed(allowed) => {
                let candidate = resolve_request(requester, request)?;
                if allowed.contains(&candidate) {
                    Ok(None)
                } else {
                    Err(outside_graph_error(&candidate))
                }
            }
            GraphPhase::Sealed => self
                .resolutions
                .get(key)
                .cloned()
                .map(Some)
                .ok_or_else(sealed_error),
        }
    }

    /// Revalidate and cache a delegated resolution.
    fn finish_resolution(
        &mut self,
        key: ResolutionKey,
        id: ModuleId,
    ) -> Result<ModuleId, SourceError> {
        match &self.phase {
            GraphPhase::Building => {}
            GraphPhase::Allowed(allowed) if allowed.contains(&id) => {}
            GraphPhase::Allowed(_) => return Err(outside_graph_error(&id)),
            GraphPhase::Sealed => return Err(sealed_error()),
        }
        self.resolutions.insert(key, id.clone());
        Ok(id)
    }

    /// Admit a source read or return its retained bytes.
    fn read_admission(&self, id: &ModuleId) -> Result<Option<Vec<u8>>, SourceError> {
        match &self.phase {
            GraphPhase::Sealed => Err(sealed_error()),
            GraphPhase::Allowed(allowed) if !allowed.contains(id) => Err(outside_graph_error(id)),
            GraphPhase::Building | GraphPhase::Allowed(_) => Ok(self.bytes.get(id).cloned()),
        }
    }

    /// Revalidate and cache delegated source bytes.
    fn finish_read(&mut self, id: ModuleId, source: Vec<u8>) -> Result<(), SourceError> {
        match &self.phase {
            GraphPhase::Building => {}
            GraphPhase::Allowed(allowed) if allowed.contains(&id) => {}
            GraphPhase::Allowed(_) => return Err(outside_graph_error(&id)),
            GraphPhase::Sealed => return Err(sealed_error()),
        }
        self.bytes.insert(id, source);
        Ok(())
    }
}

/// Stable error for work attempted after entry evaluation.
fn sealed_error() -> SourceError {
    SourceError::other("module source is sealed after config entry evaluation")
}

/// Stable error for a module outside the checked graph.
fn outside_graph_error(id: &ModuleId) -> SourceError {
    SourceError::other(format!("module '{id}' is outside the checked config graph"))
}

/// Module source that freezes one checked graph into a runtime allowlist.
pub(super) struct ConfigModuleSource {
    /// Filesystem resolver used during graph preparation and activation.
    delegate: Arc<dyn SourceProvider>,
    /// Entry module identity used when a root request has no requester.
    graph_root: ModuleId,
    /// Canonical directory used to expand diagnostic display paths.
    root_dir: PathBuf,
    /// Caches and admission phase observed atomically.
    state: Arc<Mutex<GraphState>>,
    /// Text retained by absolute path for runtime excerpts.
    sources: SourceMap,
}

impl ConfigModuleSource {
    /// Wrap a resolver and share loaded source text with runtime diagnostics.
    pub(super) fn new(
        delegate: Arc<dyn SourceProvider>,
        graph_root: ModuleId,
        root_dir: PathBuf,
        sources: SourceMap,
    ) -> Self {
        Self {
            delegate,
            graph_root,
            root_dir,
            state: Arc::new(Mutex::new(GraphState::default())),
            sources,
        }
    }

    /// Restrict runtime resolution and reads to the checked graph.
    pub(super) fn allow_only(&self, modules: impl IntoIterator<Item = ModuleId>) {
        lock_unpoisoned(&self.state).install_allowlist(modules);
    }

    /// Prevent later entrypoints from loading any uncached module source.
    pub(super) fn seal(&self) {
        lock_unpoisoned(&self.state).seal();
    }

    /// Return true for the only request spelling accepted by Hotki configs.
    fn request_is_relative(request: &[u8]) -> bool {
        request.starts_with(b"./") || request.starts_with(b"../")
    }

    /// Expand one source display name into its diagnostic filesystem path.
    fn source_path(root_dir: &Path, metadata: &SourceMetadata) -> PathBuf {
        let path = PathBuf::from(&metadata.display_name);
        if path.is_absolute() {
            path
        } else {
            root_dir.join(path)
        }
    }
}

impl SourceProvider for ConfigModuleSource {
    fn resolve(&self, requester: Option<&ModuleId>, request: &[u8]) -> SourceFuture<ModuleId> {
        if !Self::request_is_relative(request) {
            let request = String::from_utf8_lossy(request).into_owned();
            return Box::pin(async move {
                Err(SourceError::other(format!(
                    "module request '{request}' must begin with ./ or ../"
                )))
            });
        }

        let requester = requester.or(Some(&self.graph_root));
        let key = (requester.cloned(), request.to_vec());
        match lock_unpoisoned(&self.state).resolve_admission(&key, requester, request) {
            Ok(Some(cached)) => return Box::pin(async move { Ok(cached) }),
            Ok(None) => {}
            Err(error) => return Box::pin(async move { Err(error) }),
        }

        let future = self.delegate.resolve(requester, request);
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let id = future.await?;
            lock_unpoisoned(&state).finish_resolution(key, id)
        })
    }

    fn read(&self, id: &ModuleId) -> SourceFuture<Vec<u8>> {
        match lock_unpoisoned(&self.state).read_admission(id) {
            Ok(Some(source)) => return Box::pin(async move { Ok(source) }),
            Ok(None) => {}
            Err(error) => return Box::pin(async move { Err(error) }),
        }

        let future = self.delegate.read(id);
        let id = id.clone();
        let state = Arc::clone(&self.state);
        let sources = Arc::clone(&self.sources);
        let path = Self::source_path(&self.root_dir, &self.delegate.metadata(&id));
        Box::pin(async move {
            let source = future.await?;
            lock_unpoisoned(&state).finish_read(id, source.clone())?;
            if let Ok(text) = String::from_utf8(source.clone()) {
                lock_unpoisoned(&sources).insert(path, Arc::from(text.into_boxed_str()));
            }
            Ok(source)
        })
    }

    fn read_request(&self, request: ReadContext<'_>) -> SourceFuture<Vec<u8>> {
        self.read(request.id())
    }

    fn instance_key(&self, request: ReadContext<'_>) -> InstanceKey {
        self.delegate.instance_key(request)
    }

    fn metadata(&self, id: &ModuleId) -> SourceMetadata {
        self.delegate.metadata(id)
    }

    fn epoch(&self) -> u64 {
        self.delegate.epoch()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_state_revalidates_allowlist_and_preserves_only_sealed_resolutions() {
        let allowed = ModuleId::new("allowed");
        let denied = ModuleId::new("denied");
        let key = (None, b"./allowed".to_vec());
        let mut state = GraphState::default();

        state
            .finish_resolution(key.clone(), allowed.clone())
            .expect("building phase accepts resolution");
        state.install_allowlist([allowed.clone()]);
        assert!(
            state
                .finish_resolution((None, b"./denied".to_vec()), denied)
                .expect_err("allowlist rejects delegated result")
                .to_string()
                .contains("outside the checked config graph")
        );
        state
            .finish_read(allowed.clone(), b"return true".to_vec())
            .expect("allowed phase accepts source");

        state.seal();

        assert_eq!(
            state
                .resolve_admission(&key, None, b"./allowed")
                .expect("sealed cached resolution"),
            Some(allowed.clone())
        );
        assert!(
            state
                .read_admission(&allowed)
                .expect_err("sealed state rejects source reads")
                .to_string()
                .contains("module source is sealed")
        );
    }
}
