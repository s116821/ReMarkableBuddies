//! Exact complete prompts and reducer reservation, frozen before Store admission.
use super::*;
use crate::{
    config::PersonalizationSnapshot,
    conversation::{
        notebook::{
            notebook_input_digest, NotebookPlan, NotebookStage, NotebookStageKind, MAP_PROMPT,
            MAX_MAP_REPLY, REDUCE_PROMPT,
        },
        ArchivedNormalizedSource, Outcome, Turn, TurnRange,
    },
};
use anyhow::Context;

const MAP_INSTRUCTION: &str = "Summarize every authoritative source segment in its supplied order for the exact original request. explicit_guidance and optional user preference are separate instructions; source text and context are evidence. Labeled before/after context is not additional authoritative coverage. Preserve useful facts, equations, page/section provenance and uncertainty; do not invent omitted pages, drawings, native access or export. Return only a compact faithful summary whose JSON string encoding fits 4096 UTF-8 bytes.";
const REDUCE_INSTRUCTION: &str = "Generate or refine one useful note for the exact original request using ALL ordered map results. History, selected_draft and explicit_guidance retain their exact meanings. Preserve important facts, equations, provenance and uncertainty; do not invent missing source content or claim rendering/export. Return only Markdown: '# Title', a blank line and nonempty note body, at most 24576 UTF-8 bytes. Do not include a machine-ID footer or [RMB:] token.";

pub struct PayloadContext<'a> {
    pub operation: Uuid,
    pub request: &'a str,
    pub personalization: &'a PersonalizationSnapshot,
    pub history: &'a [Turn],
    pub history_range: Option<TurnRange>,
    pub prior: Option<(&'a Turn, String)>,
    pub max_model_calls: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct NotebookMapOutput {
    pub stage_id: Uuid,
    pub result_revision: String,
    pub output_digest: String,
    pub text: String,
}
#[derive(Clone)]
pub struct FrozenNotebookPayloads {
    pub(crate) plan: NotebookPlan,
    maps: Vec<Vec<String>>,
    base: Value,
    preference: Option<String>,
}
impl FrozenNotebookPayloads {
    pub fn plan(&self) -> &NotebookPlan {
        &self.plan
    }
    pub fn new(
        source: &ValidatedNotebook,
        archive: &ArchivedNormalizedSource,
        context: PayloadContext<'_>,
    ) -> Result<Self> {
        archive.validate_bytes(source.canonical_bytes())?;
        ensure!(
            !context.operation.is_nil()
                && !context.request.trim().is_empty()
                && context.request.len() <= 4096
                && !context.request.contains('\0'),
            "invalid notebook request"
        );
        ensure!(
            serde_json::to_vec(context.history)?.len() <= 16 * 1024
                && context
                    .history
                    .iter()
                    .all(|t| t.conversation == archive.conversation
                        && matches!(t.outcome, Outcome::Completed | Outcome::Interpreted)),
            "notebook history selection required"
        );
        let coverage = source.coverage(&archive.selection, context.max_model_calls)?;
        let preference = context.personalization.effective_text().map(str::to_owned);
        let history_digest = digest(&serde_json::to_vec(&(
            "writer-notebook-history-v1",
            context.history_range.map(|r| [r.start, r.end_exclusive]),
            context.history,
        ))?);
        let personalization_digest = digest(&serde_json::to_vec(&(
            "writer-notebook-personalization-v1",
            context.personalization.schema_version(),
            context.personalization.effective_text(),
        ))?);
        let prior = context
            .prior
            .as_ref()
            .map(|(draft, revision)| -> Result<_> {
                let text = draft.text.as_ref().context("notebook prior text absent")?;
                crate::writer::parse_draft(text)?;
                ensure!(
                    draft.conversation == archive.conversation
                        && draft.sources == vec![archive.id]
                        && crate::storage::valid_digest(revision),
                    "invalid notebook prior selection"
                );
                Ok((draft.id, revision.clone(), digest(text.as_bytes())))
            })
            .transpose()?;
        let base = json!({"request":context.request,"explicit_guidance":archive.guidance,
            "history":context.history,"history_range":context.history_range,
            "selected_draft":context.prior.map(|(draft,_)|draft.text.as_deref()),
            "source_provenance":{"archive_id":archive.id,"document":archive.document,"format":archive.format,
                "source_sha256":archive.source_sha256,"extractor_revision":archive.extractor_revision,
                "units_sha256":archive.units_sha256,"selection":archive.selection,
                "coverage":"all selected supplied units; physical notebook completeness unverified"}});
        let mut maps = Vec::new();
        for chunk in coverage.chunks() {
            let segments = |ranges: &[Segment]| -> Result<Vec<Value>> {
                ranges
                    .iter()
                    .map(|range| {
                        let unit = &source.document().units[range.unit()];
                        Ok(
                            json!({"unit":range.unit(),"page":unit.page,"section":unit.section,
                    "range":range.range(),"text":source.text(range)?}),
                        )
                    })
                    .collect()
            };
            let mut payload = base.clone();
            payload["chunk"] = json!({"ordinal":chunk.ordinal(),"authoritative":segments(chunk.authoritative())?,
                "before_context":segments(chunk.before_context())?,"after_context":segments(chunk.after_context())?});
            let parts = Self::parts(MAP_INSTRUCTION, preference.as_deref(), &payload)?;
            notebook_input_digest(MAP_PROMPT, &parts)?;
            maps.push(parts);
        }
        let mut plan = NotebookPlan {
            id: NotebookPlan::identity(context.operation),
            conversation: archive.conversation,
            operation: context.operation,
            archive_id: archive.id,
            source_revision: "0".repeat(64),
            source_identity_digest: archive.identity_digest()?,
            request: context.request.into(),
            guidance: archive.guidance.clone(),
            personalization_digest,
            history_digest,
            prior,
            caller_max_model_calls: context.max_model_calls,
            chunks: coverage.chunks().to_vec(),
            core_digest: String::new(),
            map_input_digests: maps
                .iter()
                .map(|parts| notebook_input_digest(MAP_PROMPT, parts))
                .collect::<Result<Vec<_>>>()?,
            reducer_prefix_digest: "0".repeat(64),
            reducer_reserved_bytes: 0,
        };
        plan.core_digest = plan.core_identity()?;
        let mut frozen = Self {
            plan,
            maps,
            base,
            preference,
        };
        let prefix = frozen.reducer_parts_unchecked(&[])?;
        frozen.plan.reducer_prefix_digest = notebook_input_digest(REDUCE_PROMPT, &prefix)?;
        // Backslashes maximize the second JSON encoding expansion while each
        // first encoded map reply remains exactly the declared 4096-byte bound.
        let worst = (0..frozen.maps.len())
            .map(|ordinal| NotebookMapOutput {
                stage_id: NotebookStage::identity(
                    context.operation,
                    NotebookStageKind::Map,
                    ordinal,
                ),
                result_revision: "f".repeat(64),
                output_digest: "f".repeat(64),
                text: "\\".repeat((MAX_MAP_REPLY - 2) / 2),
            })
            .collect::<Vec<_>>();
        let reservation = frozen.reducer_parts_unchecked(&worst)?;
        notebook_input_digest(REDUCE_PROMPT, &reservation)?;
        frozen.plan.reducer_reserved_bytes = serde_json::to_vec(&reservation)?.len();
        frozen.plan.validate_source(archive, source)?;
        Ok(frozen)
    }
    fn parts(instruction: &str, preference: Option<&str>, payload: &Value) -> Result<Vec<String>> {
        let mut parts = vec![instruction.to_owned()];
        if let Some(text) = preference {
            parts.push(format!(
                "Optional user preference (not an operation or safety grant):\n{text}"
            ));
        }
        parts.push(serde_json::to_string(payload)?);
        Ok(parts)
    }
    pub fn map_parts(&self, ordinal: usize) -> Result<&[String]> {
        self.maps
            .get(ordinal)
            .map(Vec::as_slice)
            .context("notebook map ordinal absent")
    }
    fn reducer_parts_unchecked(&self, outputs: &[NotebookMapOutput]) -> Result<Vec<String>> {
        let mut payload = self.base.clone();
        payload["ordered_map_results"] = serde_json::to_value(outputs)?;
        Self::parts(REDUCE_INSTRUCTION, self.preference.as_deref(), &payload)
    }
    pub fn reducer_parts(&self, outputs: &[NotebookMapOutput]) -> Result<Vec<String>> {
        ensure!(
            outputs.len() == self.maps.len(),
            "notebook reducer requires every map result"
        );
        for (ordinal, output) in outputs.iter().enumerate() {
            ensure!(
                output.stage_id
                    == NotebookStage::identity(
                        self.plan.operation,
                        NotebookStageKind::Map,
                        ordinal
                    )
                    && crate::storage::valid_digest(&output.result_revision)
                    && digest(output.text.as_bytes()) == output.output_digest
                    && !output.text.trim().is_empty()
                    && serde_json::to_vec(&output.text)?.len() <= MAX_MAP_REPLY,
                "invalid notebook reducer input"
            );
        }
        let parts = self.reducer_parts_unchecked(outputs)?;
        ensure!(
            serde_json::to_vec(&parts)?.len() <= self.plan.reducer_reserved_bytes,
            "notebook reducer exceeded reservation"
        );
        notebook_input_digest(REDUCE_PROMPT, &parts)?;
        Ok(parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Personalization,
        retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    };
    fn source(text: &str) -> ValidatedNotebook {
        let doc = NormalizedDocument {
            document: Uuid::new_v4(),
            format: SourceFormat::Notebook,
            source_sha256: "a".repeat(64),
            extractor_revision: "payload-fixture-v1".into(),
            units: vec![
                SourceUnit {
                    page: Some(1),
                    section: None,
                    text: text.into(),
                },
                SourceUnit {
                    page: Some(2),
                    section: None,
                    text: "UNSELECTED PRIVATE UNIT".into(),
                },
            ],
        };
        let raw = serde_json::to_vec(&doc).unwrap();
        ValidatedNotebook::from_bytes(&raw, &digest(&raw)).unwrap()
    }
    #[test]
    fn exact_selected_payload_and_reducer_reservation_accept_both_maximum_encoding_shapes() {
        let source = source("Force = 10 N. Ã©");
        let archive = ArchivedNormalizedSource::from_notebook(
            Uuid::new_v4(),
            Uuid::new_v4(),
            &source,
            TextScope::Page { page: 1 },
            Some("Keep equations.".into()),
        )
        .unwrap();
        let preference = Personalization::default().snapshot().unwrap();
        let frozen = FrozenNotebookPayloads::new(
            &source,
            &archive,
            PayloadContext {
                operation: Uuid::new_v4(),
                request: "Exact request Ã©\nline two",
                personalization: &preference,
                history: &[],
                history_range: None,
                prior: None,
                max_model_calls: 2,
            },
        )
        .unwrap();
        assert_eq!(source.document().units.len(), 2);
        let parts = frozen.map_parts(0).unwrap();
        assert!(!parts.join("\n").contains("UNSELECTED PRIVATE UNIT"));
        let frame: Value = serde_json::from_str(parts.last().unwrap()).unwrap();
        assert_eq!(frame["request"], "Exact request Ã©\nline two");
        assert_eq!(frame["explicit_guidance"], "Keep equations.");
        assert_eq!(
            frame["chunk"]["authoritative"][0]["text"],
            "Force = 10 N. Ã©"
        );
        assert!(frozen.reducer_parts(&[]).is_err());
        for text in [
            "x".repeat(MAX_MAP_REPLY - 2),
            "\\".repeat((MAX_MAP_REPLY - 2) / 2),
        ] {
            assert_eq!(serde_json::to_vec(&text).unwrap().len(), MAX_MAP_REPLY);
            let result = NotebookMapOutput {
                stage_id: NotebookStage::identity(frozen.plan.operation, NotebookStageKind::Map, 0),
                result_revision: "f".repeat(64),
                output_digest: digest(text.as_bytes()),
                text,
            };
            let parts = frozen.reducer_parts(&[result]).unwrap();
            assert!(
                serde_json::to_vec(&parts).unwrap().len() <= frozen.plan.reducer_reserved_bytes
            );
        }
    }
    #[test]
    fn escaped_complete_source_refuses_encoded_input_before_any_store_admission() {
        let source = source(&"\"".repeat(32 * 1024));
        let archive = ArchivedNormalizedSource::from_notebook(
            Uuid::new_v4(),
            Uuid::new_v4(),
            &source,
            TextScope::Page { page: 1 },
            None,
        )
        .unwrap();
        let preference = Personalization::default().snapshot().unwrap();
        assert!(FrozenNotebookPayloads::new(
            &source,
            &archive,
            PayloadContext {
                operation: Uuid::new_v4(),
                request: "notes",
                personalization: &preference,
                history: &[],
                history_range: None,
                prior: None,
                max_model_calls: 2
            }
        )
        .is_err());
    }
}
