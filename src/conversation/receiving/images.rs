//! Model-facing historical derivatives. Acquisition parents remain provenance.
use crate::{
    conversation::{ContextView, Ledger, Record},
    storage::{Media, ObjectRef, Uuid},
};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const MAX_IMAGES: usize = 12;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RESIDENT_PROOF_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESIDENT_PROOF_PIXELS: u64 =
    MAX_IMAGES as u64 * remarkable_open_sdk::native_adapter::MAX_CAPTURE_PIXELS;

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
    let mut resident: BTreeMap<_, _> = context
        .resident_captures
        .iter()
        .map(|capture| (capture.id, capture))
        .collect();
    for capture in resident.values() {
        capture.validate()?;
        by_source.insert(
            capture.id,
            capture
                .images
                .iter()
                .map(|image| {
                    (
                        image.provider_ordinal,
                        image.media.clone(),
                        image.dimensions,
                    )
                })
                .collect(),
        );
    }

    // Target aliases name actual retained SourceTarget records. They preserve the
    // original capture metadata and associations without inventing another capture.
    for view in &context.targets {
        let images = match &view.source {
            Record::Source(v) => vec![(v.ordinal, v.image.clone(), [v.width, v.height])],
            Record::Capture(v) => v
                .facts
                .images
                .iter()
                .enumerate()
                .map(|(n, i)| (n as u32, i.media(), i.dimensions))
                .collect(),
            Record::LegacyCapture(v) => v
                .images
                .iter()
                .enumerate()
                .map(|(n, i)| (n as u32, i.media.clone(), i.dimensions))
                .collect(),
            Record::DevelopmentCapture(v) => v
                .images
                .iter()
                .enumerate()
                .map(|(n, i)| (n as u32, i.media.clone(), i.dimensions))
                .collect(),
            Record::ResidentCapture(v) => {
                v.validate()?;
                if let Some(prior) = resident.insert(view.target.id, v) {
                    ensure!(
                        serde_json::to_vec(prior)? == serde_json::to_vec(v)?,
                        "target alias has conflicting resident evidence"
                    );
                }
                v.images
                    .iter()
                    .map(|i| (i.provider_ordinal, i.media.clone(), i.dimensions))
                    .collect()
            }
            _ => anyhow::bail!("target historical source is not a capture"),
        };
        if let Some(prior) = by_source.insert(view.target.id, images.clone()) {
            ensure!(
                prior == images,
                "target alias has conflicting image descriptors"
            );
        }
    }
    let mut ordered_sources = Vec::new();
    for turn in &context.turns {
        ordered_sources.extend(turn.sources.iter().copied());
        for used in context
            .target_uses
            .iter()
            .filter(|used| used.turn == turn.id)
        {
            if let Some(reference) = &used.target {
                let view = context
                    .targets
                    .iter()
                    .find(|view| &view.reference == reference)
                    .context("inherited target evidence absent")?;
                ordered_sources.push(view.target.id);
            }
        }
    }
    let mut by_digest = BTreeMap::new();
    let mut total = 0;
    let mut admitted: Vec<(Media, [u32; 2])> = Vec::new();
    let mut associations = Vec::new();
    let mut resident_proofs = Vec::new();
    let mut proof_bytes = 0u64;
    let mut proof_pixels = 0u64;
    let mut proven_captures = BTreeSet::new();
    for source in &ordered_sources {
        if let Some(images) = by_source.remove(source) {
            let capture = resident.get(source).copied();
            let proof = capture.map(serde_json::to_vec).transpose()?;
            if let Some(capture) = capture.filter(|_| {
                // Target aliases of the same frozen capture share one proof.
                // Different capture metadata still consumes independent work.
                proven_captures.insert(proof.expect("capture descriptor encoded"))
            }) {
                ensure!(
                    resident_proofs.len() < MAX_IMAGES,
                    "historical resident proof count exceeded"
                );
                for media in std::iter::once(&capture.original_png)
                    .chain(capture.images.iter().map(|image| &image.media))
                {
                    proof_bytes = proof_bytes
                        .checked_add(media.bytes)
                        .context("historical resident proof byte overflow")?;
                    ensure!(
                        proof_bytes <= MAX_RESIDENT_PROOF_BYTES,
                        "historical resident proof byte bound exceeded"
                    );
                }
                let [width, height] = capture.original_dimensions;
                let pixels = u64::from(width)
                    .checked_mul(u64::from(height))
                    .context("historical resident pixel work overflow")?;
                proof_pixels = proof_pixels
                    .checked_add(pixels)
                    .context("historical resident pixel work overflow")?;
                ensure!(
                    proof_pixels <= MAX_RESIDENT_PROOF_PIXELS,
                    "historical resident pixel work exceeded"
                );
                resident_proofs.push(capture);
            }
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
    // Descriptor/count/byte/pixel admission above completes before any object
    // read or decode. This is one cumulative request allowance, counting repeat
    // proof work even when the final provider attachments deduplicate.
    let mut verified_resident = BTreeMap::new();
    for capture in resident_proofs {
        let verified = Ledger::resident_images_from_evidence(capture.clone(), |media| {
            ledger.store().read_object(&ObjectRef {
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            })
        })?;
        for (image, bytes) in capture.images.iter().zip(verified.images) {
            verified_resident
                .entry(image.media.sha256.clone())
                .or_insert(bytes);
        }
        // The validated original stays provenance only. Retain the actual
        // returned derivatives through attachment rather than rereading a head.
    }
    let attachments = admitted
        .into_iter()
        .map(|(media, [width, height])| {
            let bytes = match verified_resident.remove(&media.sha256) {
                Some(bytes) => bytes,
                None => ledger.store().read_object(&ObjectRef {
                    sha256: media.sha256.clone(),
                    bytes: media.bytes,
                })?,
            };
            Ledger::validate_image_bytes(&media, &bytes, width, height)?;
            Ok(STANDARD.encode(bytes))
        })
        .collect::<Result<_>>()?;
    Ok(Images {
        attachments,
        associations,
    })
}
