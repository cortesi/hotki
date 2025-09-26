use std::sync::{Arc, Mutex};

use once_cell::sync::OnceCell;

use super::scenario::{MimicDiagnostic, Quirk};
use crate::PlaceOptions;

/// Internal registry mapping spawned mimic windows to diagnostic metadata so world reconciliation
/// can surface `{scenario_slug, window_label, quirks[]}`.
static REGISTRY: OnceCell<Mutex<MimicRegistry>> = OnceCell::new();

fn registry() -> &'static Mutex<MimicRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(MimicRegistry::default()))
}

#[derive(Default)]
struct MimicRegistry {
    entries: Vec<MimicRegistryEntry>,
}

impl MimicRegistry {
    fn register(
        &mut self,
        slug: Arc<str>,
        label: Arc<str>,
        quirks: Vec<Quirk>,
        place: PlaceOptions,
    ) {
        self.entries.push(MimicRegistryEntry {
            slug,
            label,
            quirks,
            place,
        });
    }

    fn purge_slug(&mut self, slug: &Arc<str>) {
        self.entries.retain(|entry| !Arc::ptr_eq(&entry.slug, slug));
    }

    fn snapshot(&self) -> Vec<MimicRegistryEntry> {
        self.entries.clone()
    }
}

#[derive(Clone)]
struct MimicRegistryEntry {
    slug: Arc<str>,
    label: Arc<str>,
    quirks: Vec<Quirk>,
    place: PlaceOptions,
}

/// Snapshot the active mimic registry for artifact generation.
#[must_use]
pub fn registry_snapshot() -> Vec<MimicDiagnostic> {
    registry()
        .lock()
        .unwrap()
        .snapshot()
        .into_iter()
        .map(|entry| MimicDiagnostic {
            scenario_slug: entry.slug,
            window_label: entry.label,
            quirks: entry.quirks,
            place: entry.place,
        })
        .collect()
}

pub(super) fn register_mimic(
    slug: Arc<str>,
    label: Arc<str>,
    quirks: Vec<Quirk>,
    place: PlaceOptions,
) {
    registry()
        .lock()
        .unwrap()
        .register(slug, label, quirks, place);
}

pub(super) fn purge_slug(slug: &Arc<str>) {
    registry().lock().unwrap().purge_slug(slug);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_snapshot_reports_registration() {
        let slug: Arc<str> = Arc::from("test-scenario");
        let label: Arc<str> = Arc::from("primary");
        register_mimic(
            slug.clone(),
            label.clone(),
            vec![Quirk::DelayApplyMove],
            PlaceOptions::default(),
        );
        let snapshot = registry_snapshot();
        assert!(
            snapshot
                .iter()
                .any(|entry| entry.scenario_slug == slug && entry.window_label == label)
        );
        purge_slug(&slug);
    }
}
