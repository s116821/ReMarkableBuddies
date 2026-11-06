//! Model-facing historical derivatives. Acquisition parents remain provenance.
use crate::{
    conversation::{ContextView, Ledger},
    storage::{Media, ObjectRef, Uuid},
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_IMAGES: usize = 12;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Default)]
pub(crate) struct Images {
    pub(crate) attachments: Vec<String>,
    pub(crate) associations: Vec<Association>,
}

#[derive(Clone, Serialize)]
pub(crate) struct Association {
    source: Uuid,
    ordinal: u32,
    dimensions: [u32; 2],
    attachment_index: usize,
    sha256: String,
}

pub(crate) fn resolve(ledger: &Ledger, context: &ContextView) -> Result<Images> {
    ensure!(
        context.missing_media.is_empty(),
        "historical evidence unavailable"
    );
    let mut by_source = BTreeMap::<Uuid, Vec<(u32, Media, [u32; 2])>>::new();
    for source in &context.sources {
        by_source.insert(
            source.id,
            vec![(
                source.ordinal,
                source.image.clone(),
                [source.width, source.height],
            )],
        );
    }
    for capture in &context.captures {
        by_source.insert(
            capture.id,
            capture
                .facts
                .images
                .iter()
                .enumerate()
                .map(|(index, image)| (index as u32, image.media(), image.dimensions))
                .collect(),
        );
    }
    for capture in &context.legacy_captures {
        by_source.insert(
            capture.id,
            capture
                .images
                .iter()
                .enumerate()
                .map(|(index, image)| (index as u32, image.media.clone(), image.dimensions))
                .collect(),
        );
    }
    for capture in &context.development_captures {
        by_source.insert(
            capture.id,
            capture
                .images
                .iter()
                .enumerate()
                .map(|(index, image)| (index as u32, image.media.clone(), image.dimensions))
                .collect(),
        );
    }

    let mut by_digest = BTreeMap::new();
    let mut total = 0;
    let mut admitted: Vec<(Media, [u32; 2])> = Vec::new();
    let mut associations = Vec::new();
    for source in context.turns.iter().flat_map(|turn| &turn.sources) {
        if let Some(images) = by_source.remove(source) {
            for (ordinal, media, dimensions) in images {
                ensure!(
                    media.media_type == "image/png",
                    "unsupported historical image type"
                );
                ensure!(
                    associations.len() < 256,
                    "historical image association budget exceeded"
                );
                let attachment_index = if let Some(&index) = by_digest.get(&media.sha256) {
                    ensure!(
                        admitted.get(index) == Some(&(media.clone(), dimensions)),
                        "inconsistent historical image descriptor"
                    );
                    index
                } else {
                    ensure!(
                        admitted.len() < MAX_IMAGES && media.bytes <= MAX_BYTES - total,
                        "historical image budget exceeded"
                    );
                    let index = admitted.len();
                    by_digest.insert(media.sha256.clone(), index);
                    total += media.bytes;
                    admitted.push((media.clone(), dimensions));
                    index
                };
                associations.push(Association {
                    source: *source,
                    ordinal,
                    dimensions,
                    attachment_index,
                    sha256: media.sha256,
                });
            }
        }
    }
    let attachments = admitted
        .into_iter()
        .map(|(media, [width, height])| {
            let bytes = ledger.store().read_object(&ObjectRef {
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            })?;
            Ledger::validate_image_bytes(&media, &bytes, width, height)?;
            Ok(STANDARD.encode(bytes))
        })
        .collect::<Result<_>>()?;
    Ok(Images {
        attachments,
        associations,
    })
}
