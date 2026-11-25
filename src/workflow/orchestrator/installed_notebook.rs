//! Reuse independently verified local notebook text for the installed Reader.
use super::Workflow;
use crate::{
    conversation::{ArchivedNormalizedSource, Ledger, Record, TextScope},
    retrieval::{
        notebook::{
            saved_render::{NotebookRaster, SavedPage},
            WRITER_VISUAL_REVISION,
        },
        Index, SourceFormat,
    },
    storage::{Envelope, Namespace, Uuid},
    workflow::saved_notebook::{
        acquire_saved_notebook, acquire_saved_notebook_with_pages, NotebookSourceAcquisition,
        SavedNotebookSource,
    },
};
use anyhow::{ensure, Context, Result};
use remarkable_open_sdk::{native_adapter::ReaderFacts, PageKey, Uuid as PageId};
use std::{collections::BTreeSet, sync::Arc, time::Instant};

pub(super) struct CachedNotebook {
    saved: SavedNotebookSource,
    archive: Envelope,
    candidates: BTreeSet<(Uuid, Uuid)>,
    index: Arc<Index>,
}

pub(super) struct InstalledNotebook {
    ledger: Ledger,
    facts: ReaderFacts,
    cached: CachedNotebook,
    deadline: Instant,
}

impl InstalledNotebook {
    pub(super) fn index(&self) -> &Index {
        &self.cached.index
    }

    pub(super) fn check(&self) -> Result<()> {
        ensure!(
            self.ledger
                .store()
                .value(Namespace::Source, self.cached.archive.record_id)?
                .as_ref()
                == Some(&self.cached.archive),
            "Local notebook transcription changed",
        );
        let current = saved_source(acquire_saved_notebook(
            &self.facts,
            &self.ledger,
            self.deadline,
            &mut || Ok(()),
        ))?;
        ensure!(
            current == self.cached.saved,
            "Saved notebook changed during Reader request"
        );
        Ok(())
    }

    pub(super) fn into_cache(self) -> CachedNotebook {
        self.cached
    }
}

fn saved_source(acquisition: NotebookSourceAcquisition) -> Result<SavedNotebookSource> {
    match acquisition {
        NotebookSourceAcquisition::SavedInputs(saved) => Ok(saved),
        NotebookSourceAcquisition::Unavailable(error) => Err(error),
    }
}

pub(super) fn admit(
    workflow: &mut Workflow,
    ledger: &Ledger,
    capture_id: Uuid,
    deadline: Instant,
    retained: Option<CachedNotebook>,
) -> Result<Option<InstalledNotebook>> {
    let Some(owner) = workflow.resident_source.as_ref() else {
        return Ok(None);
    };
    let Some(active) = owner.opened_source_order()? else {
        return Ok(None);
    };
    let stored = ledger
        .store()
        .value(Namespace::Source, capture_id)?
        .context("Installed notebook source capture absent")?;
    let Record::ResidentCapture(capture) = Ledger::decode(&stored)? else {
        anyhow::bail!("Installed notebook source is not a resident capture");
    };
    let document = capture.before.document;
    let page = capture.before.page;
    let original = PageId::parse(&page.to_string()).context("Invalid original notebook page")?;
    if active.page().document.as_bytes() != document.as_bytes()
        || !active.order().contains(&original)
    {
        return Ok(None);
    }
    let facts = ReaderFacts::new(
        PageKey {
            document: active.page().document,
            page: original,
        },
        active.order().to_vec(),
    )
    .map_err(|error| anyhow::anyhow!("Invalid original notebook order: {error:?}"))?;
    let order: Vec<_> = facts
        .order()
        .iter()
        .map(|page| Uuid::from_bytes(*page.as_bytes()))
        .collect();
    let owned = ledger
        .notebook_page_associations(document, &order, || workflow.check_request_progress())?;
    let buddy_pages: BTreeSet<_> = owned
        .iter()
        .map(|binding| binding.receipt().observed_page)
        .collect();
    if buddy_pages.contains(&page) || order.len() - buddy_pages.len() < 2 {
        return Ok(None);
    }

    let candidates = local_archives(ledger, document)?;
    if candidates.is_empty() {
        return Ok(None);
    }
    let candidate_heads: BTreeSet<_> = candidates
        .iter()
        .map(|(head, _)| (head.record_id, head.revision_id))
        .collect();
    let saved = match saved_source(acquire_saved_notebook(
        &facts,
        ledger,
        deadline,
        &mut || workflow.check_request_progress(),
    )) {
        Ok(saved) => saved,
        Err(error) => {
            workflow.check_request_progress()?;
            log::debug!("No eligible local notebook revision: {error:#}");
            return Ok(None);
        }
    };
    let cached = if let Some(cached) =
        retained.filter(|cached| cached.saved == saved && cached.candidates == candidate_heads)
    {
        cached
    } else {
        let mut raster = NotebookRaster::default();
        let rendered = saved_source(acquire_saved_notebook_with_pages(
            &facts,
            ledger,
            deadline,
            &mut || workflow.check_request_progress(),
            &mut |page| {
                raster.push(
                    SavedPage {
                        id: Uuid::from_bytes(*page.page.as_bytes()),
                        scene: page.scene,
                        template_svg: page.template_svg,
                        template_png: page.template_png,
                    },
                    &mut || {
                        ensure!(Instant::now() < deadline, "Notebook rendering expired");
                        Ok(())
                    },
                )
            },
        ));
        let rendered = match rendered {
            Ok(rendered) => rendered,
            Err(error) => {
                workflow.check_request_progress()?;
                let unchanged = saved_source(acquire_saved_notebook(
                    &facts,
                    ledger,
                    deadline,
                    &mut || workflow.check_request_progress(),
                ))?;
                ensure!(
                    unchanged == saved,
                    "Saved notebook changed during Reader admission"
                );
                log::debug!("Saved notebook images unavailable: {error:#}");
                return Ok(None);
            }
        };
        ensure!(
            rendered == saved,
            "Saved notebook changed during local index admission"
        );
        let mut matched: Option<CachedNotebook> = None;
        for (envelope, archive) in candidates {
            workflow.check_request_progress()?;
            let mut orders = vec![order.clone()];
            if capture.before.order != order {
                orders.push(capture.before.order.clone());
            }
            // Writer archives record the exact ordered raster digest. A later
            // Buddy insertion changes that order, while the original PNGs may
            // remain identical. Use only recorded orders from the Writer's
            // actual acquisition and current verified output ownership.
            let records = ledger.inspect(archive.conversation, false)?;
            let Some(exchange) = records.iter().find_map(|record| match record {
                Record::Turn(turn)
                    if turn.id == archive.turn
                        && turn.role == crate::conversation::Role::User
                        && turn.mode == crate::conversation::Mode::Writer
                        && turn.sources.contains(&archive.id) =>
                {
                    Some(turn.exchange)
                }
                _ => None,
            }) else {
                continue;
            };
            let acquisition = crate::writer::stage(exchange, "source-capture");
            if let Some(before) = records.iter().find_map(|record| match record {
                Record::ResidentCapture(capture)
                    if capture.turn == acquisition && capture.before.document == document =>
                {
                    Some(&capture.before)
                }
                _ => None,
            }) {
                orders.push(before.order.clone());
                orders.extend(
                    owned
                        .iter()
                        .map(|binding| binding.receipt())
                        .filter(|receipt| {
                            receipt.conversation == archive.conversation
                                && receipt.source.page == before.page
                                && receipt.expected_order == before.order
                        })
                        .map(|receipt| receipt.observed_order.clone()),
                );
            } else {
                continue;
            }

            let originals: Vec<_> = saved
                .original_pages()
                .iter()
                .map(|id| Uuid::from_bytes(*id.as_bytes()))
                .collect();
            let mut source_order = None;
            for candidate in orders {
                let excluded: BTreeSet<_> = candidate
                    .iter()
                    .filter(|page| buddy_pages.contains(page))
                    .copied()
                    .collect();
                if !candidate
                    .iter()
                    .filter(|page| !excluded.contains(page))
                    .copied()
                    .eq(originals.iter().copied())
                {
                    continue;
                }
                let matches = raster.images(document, &candidate, &excluded, |images| {
                    Ok(images.image_digest()? == archive.source_sha256)
                })?;
                if matches {
                    source_order = Some(candidate);
                    break;
                }
            }
            let Some(source_order) = source_order else {
                continue;
            };
            let material = archive.load(ledger.store())?;
            let mut normalized = material.document().clone();
            // The archived Writer units use the positions of the original
            // pages in its admitted native order. Convert to original notebook
            // ordinals and retain the actual stable page ID for each passage.
            for unit in &mut normalized.units {
                let position = unit.page.context("Notebook passage lacks a page")? as usize;
                let id = source_order
                    .get(
                        position
                            .checked_sub(1)
                            .context("Invalid notebook page ordinal")?,
                    )
                    .context("Notebook passage outside its archived order")?;
                let ordinal = originals
                    .iter()
                    .position(|page| page == id)
                    .context("Notebook passage points to a Buddy output")?;
                unit.page = Some((ordinal + 1).try_into()?);
                let label = unit.section.as_deref().unwrap_or("writing");
                unit.section = Some(format!("{label} (original page {id})"));
            }
            normalized.source_sha256 = saved.revision().to_owned();
            let index = Arc::new(Index::build(normalized)?);
            if let Some(previous) = &matched {
                if previous.index.key().normalized_sha256 != index.key().normalized_sha256 {
                    log::info!("Matching local notebook transcriptions disagree");
                    return Ok(None);
                }
            } else {
                matched = Some(CachedNotebook {
                    saved: saved.clone(),
                    archive: envelope,
                    candidates: candidate_heads.clone(),
                    index,
                });
            }
        }
        let Some(cached) = matched else {
            return Ok(None);
        };
        cached
    };
    let admitted = InstalledNotebook {
        ledger: Ledger::new(ledger.store().clone()),
        facts,
        cached,
        deadline,
    };
    admitted.check()?;
    Ok(Some(admitted))
}

fn local_archives(
    ledger: &Ledger,
    document: Uuid,
) -> Result<Vec<(Envelope, ArchivedNormalizedSource)>> {
    let document = document.to_string();
    let (unavailable, heads) = ledger.store().snapshot_heads_matching_with_status(
        &[Namespace::Source],
        crate::storage::MAX_ITEMS,
        |head| {
            let record = &head.payload["record"];
            head.payload["record_kind"].as_str() == Some("archived-normalized-source")
                && record["document"].as_str() == Some(document.as_str())
                && record["extractor_revision"]
                    .as_str()
                    .is_some_and(|revision| revision.starts_with(WRITER_VISUAL_REVISION))
        },
    )?;
    ensure!(
        unavailable == 0,
        "Local notebook source history is incomplete"
    );
    let mut candidates = Vec::new();
    for head in heads {
        let Record::ArchivedNormalizedSource(archive) = Ledger::decode(&head)? else {
            anyhow::bail!("Local notebook archive record changed");
        };
        ensure!(
            ledger
                .store()
                .value(Namespace::Source, head.record_id)?
                .as_ref()
                == Some(&head),
            "Local notebook archive is conflicted",
        );
        if archive.format == SourceFormat::Notebook
            && archive.selection == TextScope::SuppliedNotebook
        {
            candidates.push((head, *archive));
        }
    }
    Ok(candidates)
}
