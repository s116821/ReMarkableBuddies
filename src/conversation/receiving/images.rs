//! Model-facing historical derivatives. Acquisition parents remain provenance.
use crate::{
    conversation::{ContextView, Ledger},
    storage::{Media, ObjectRef, Uuid},
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::collections::{BTreeMap, BTreeSet};

const MAX_IMAGES: usize = 12;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn resolve(ledger: &Ledger, context: &ContextView) -> Result<Vec<String>> {
    ensure!(
        context.missing_media.is_empty(),
        "historical evidence unavailable"
    );
    let mut by_source = BTreeMap::<Uuid, Vec<(Media, [u32; 2])>>::new();
    for source in &context.sources {
        by_source.insert(
            source.id,
            vec![(source.image.clone(), [source.width, source.height])],
        );
    }
    for capture in &context.captures {
        by_source.insert(
            capture.id,
            capture
                .facts
                .images
                .iter()
                .map(|image| (image.media(), image.dimensions))
                .collect(),
        );
    }
    for capture in &context.legacy_captures {
        by_source.insert(
            capture.id,
            capture
                .images
                .iter()
                .map(|image| (image.media.clone(), image.dimensions))
                .collect(),
        );
    }
    for capture in &context.development_captures {
        by_source.insert(
            capture.id,
            capture
                .images
                .iter()
                .map(|image| (image.media.clone(), image.dimensions))
                .collect(),
        );
    }

    let mut seen = BTreeSet::new();
    let mut total = 0;
    let mut admitted = Vec::new();
    for source in context.turns.iter().flat_map(|turn| &turn.sources) {
        if let Some(images) = by_source.remove(source) {
            for (media, dimensions) in images {
                ensure!(
                    media.media_type == "image/png",
                    "unsupported historical image type"
                );
                if !seen.insert(media.sha256.clone()) {
                    continue;
                }
                ensure!(
                    admitted.len() < MAX_IMAGES && media.bytes <= MAX_BYTES - total,
                    "historical image budget exceeded"
                );
                total += media.bytes;
                admitted.push((media, dimensions));
            }
        }
    }
    admitted
        .into_iter()
        .map(|(media, [width, height])| {
            let bytes = ledger.store().read_object(&ObjectRef {
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            })?;
            Ledger::validate_image_bytes(&media, &bytes, width, height)?;
            Ok(STANDARD.encode(bytes))
        })
        .collect()
}
