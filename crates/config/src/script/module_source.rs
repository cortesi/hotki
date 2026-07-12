//! Checked, cached module-source boundary for one config load.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use ruau::source::{
    InstanceKey, ModuleId, ReadContext, SourceError, SourceFuture, SourceMetadata, SourceProvider,
    resolve_request,
};

use super::{config::SourceMap, util::lock_unpoisoned};

/// Requester and literal request bytes used to cache one resolution edge.
type ResolutionKey = (Option<ModuleId>, Vec<u8>);

/// Module source that freezes one checked graph into a runtime allowlist.
pub(super) struct ConfigModuleSource {
    /// Filesystem resolver used during graph preparation and activation.
    delegate: Arc<dyn SourceProvider>,
    /// Entry module identity used when a root request has no requester.
    graph_root: ModuleId,
    /// Canonical directory used to expand diagnostic display paths.
    root_dir: PathBuf,
    /// Resolution edges observed while checking and activating the graph.
    resolutions: Arc<Mutex<HashMap<ResolutionKey, ModuleId>>>,
    /// Source bytes retained from preparation instead of rereading files.
    bytes: Arc<Mutex<HashMap<ModuleId, Vec<u8>>>>,
    /// Checked module identities; absent only while the graph is being built.
    allowed: Arc<Mutex<Option<HashSet<ModuleId>>>>,
    /// Whether entry evaluation has completed and new source reads are forbidden.
    sealed: Arc<AtomicBool>,
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
            resolutions: Arc::new(Mutex::new(HashMap::new())),
            bytes: Arc::new(Mutex::new(HashMap::new())),
            allowed: Arc::new(Mutex::new(None)),
            sealed: Arc::new(AtomicBool::new(false)),
            sources,
        }
    }

    /// Restrict runtime resolution and reads to the checked graph.
    pub(super) fn allow_only(&self, modules: impl IntoIterator<Item = ModuleId>) {
        *lock_unpoisoned(&self.allowed) = Some(modules.into_iter().collect());
    }

    /// Prevent later entrypoints from loading any uncached module source.
    pub(super) fn seal(&self) {
        self.sealed.store(true, Ordering::Release);
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
        if self.sealed.load(Ordering::Acquire) {
            let cached = lock_unpoisoned(&self.resolutions).get(&key).cloned();
            return Box::pin(async move {
                cached.ok_or_else(|| {
                    SourceError::other("module source is sealed after config entry evaluation")
                })
            });
        }

        if let Some(allowed) = lock_unpoisoned(&self.allowed).as_ref() {
            let candidate = match resolve_request(requester, request) {
                Ok(candidate) => candidate,
                Err(error) => return Box::pin(async move { Err(error) }),
            };
            if !allowed.contains(&candidate) {
                return Box::pin(async move {
                    Err(SourceError::other(format!(
                        "module '{candidate}' is outside the checked config graph"
                    )))
                });
            }
        }

        let future = self.delegate.resolve(requester, request);
        let resolutions = Arc::clone(&self.resolutions);
        let allowed = Arc::clone(&self.allowed);
        Box::pin(async move {
            let id = future.await?;
            if lock_unpoisoned(&allowed)
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&id))
            {
                return Err(SourceError::other(format!(
                    "module '{id}' is outside the checked config graph"
                )));
            }
            lock_unpoisoned(&resolutions).insert(key, id.clone());
            Ok(id)
        })
    }

    fn read(&self, id: &ModuleId) -> SourceFuture<Vec<u8>> {
        if self.sealed.load(Ordering::Acquire) {
            return Box::pin(async {
                Err(SourceError::other(
                    "module source is sealed after config entry evaluation",
                ))
            });
        }
        if let Some(source) = lock_unpoisoned(&self.bytes).get(id).cloned() {
            return Box::pin(async move { Ok(source) });
        }
        if lock_unpoisoned(&self.allowed)
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(id))
        {
            let id = id.clone();
            return Box::pin(async move {
                Err(SourceError::other(format!(
                    "module '{id}' is outside the checked config graph"
                )))
            });
        }

        let future = self.delegate.read(id);
        let id = id.clone();
        let bytes = Arc::clone(&self.bytes);
        let sources = Arc::clone(&self.sources);
        let path = Self::source_path(&self.root_dir, &self.delegate.metadata(&id));
        Box::pin(async move {
            let source = future.await?;
            lock_unpoisoned(&bytes).insert(id, source.clone());
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
