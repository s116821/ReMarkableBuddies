//! Immutable processing facts. Deserialization and equality never authorize a send.
use super::*;
use crate::writer::notebook::Chunk;

pub const MAP_PROMPT: &str = "notebook-map-v1";
pub const REDUCE_PROMPT: &str = "notebook-reduce-v1";
pub const MAX_INPUT: usize = 96 * 1024;
pub const MAX_MAP_REPLY: usize = 4096;
pub const MAX_FINAL_TEXT: usize = 24 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotebookPlan {
    pub id: Uuid,
    pub conversation: Uuid,
    pub operation: Uuid,
    pub archive_id: Uuid,
    /// Immutable encoded envelope SHA-256, as pinned by the literal recipes.
    /// Live expected-head UUIDs are held separately by the invocation cursor.
    pub source_revision: String,
    pub source_identity_digest: String,
    pub request: String,
    pub guidance: Option<String>,
    pub personalization_digest: String,
    pub history_digest: String,
    pub prior: Option<(Uuid, String, String)>,
    pub caller_max_model_calls: usize,
    pub chunks: Vec<Chunk>,
    pub core_digest: String,
    pub map_input_digests: Vec<String>,
    pub reducer_prefix_digest: String,
    pub reducer_reserved_bytes: usize,
}
impl NotebookPlan {
    pub fn identity(operation: Uuid) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:writer-notebook-plan:v1:{operation}").as_bytes(),
        )
    }
    pub fn core_identity(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(&serde_json::json!([
            "writer-notebook-plan-v1",
            self.operation,
            self.archive_id,
            self.source_identity_digest,
            self.request,
            self.guidance,
            self.personalization_digest,
            self.history_digest,
            self.prior,
            [
                8388608,
                4194304,
                262144,
                256,
                8,
                9,
                32768,
                512,
                98304,
                4096,
                24576,
                95,
                600,
                64,
                256,
                self.caller_max_model_calls
            ],
            self.chunks
                .iter()
                .map(Chunk::identity_recipe)
                .collect::<Vec<_>>(),
            "notebook-chunker-v1",
            MAP_PROMPT,
            REDUCE_PROMPT
        ]))?))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.operation.is_nil()
                && !self.conversation.is_nil()
                && !self.archive_id.is_nil()
                && valid_digest(&self.source_revision)
                && self.id == Self::identity(self.operation),
            "invalid notebook plan identity"
        );
        ensure!(
            !self.request.trim().is_empty()
                && self.request.len() <= 16 * 1024
                && !self.request.contains('\0'),
            "invalid notebook request"
        );
        ensure!(
            self.guidance
                .as_ref()
                .is_none_or(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0')),
            "invalid notebook guidance"
        );
        ensure!(
            [
                &self.source_identity_digest,
                &self.personalization_digest,
                &self.history_digest,
                &self.core_digest,
                &self.reducer_prefix_digest
            ]
            .into_iter()
            .all(|s| valid_digest(s)),
            "invalid notebook plan digest"
        );
        ensure!(
            self.prior
                .as_ref()
                .is_none_or(|(id, rev, hash)| !id.is_nil()
                    && valid_digest(rev)
                    && valid_digest(hash)),
            "invalid prior notebook draft"
        );
        ensure!(
            (2..=9).contains(&self.caller_max_model_calls)
                && !self.chunks.is_empty()
                && self.chunks.len() <= 8
                && self.chunks.len() < self.caller_max_model_calls
                && self.map_input_digests.len() == self.chunks.len()
                && self.map_input_digests.iter().all(|s| valid_digest(s))
                && self.reducer_reserved_bytes <= MAX_INPUT
                && self.reducer_reserved_bytes >= self.chunks.len() * MAX_MAP_REPLY,
            "invalid notebook work reservation"
        );
        for (ordinal, chunk) in self.chunks.iter().enumerate() {
            ensure!(
                chunk.ordinal() == ordinal && !chunk.authoritative().is_empty(),
                "invalid notebook chunk ordering"
            );
            for segments in [
                chunk.authoritative(),
                chunk.before_context(),
                chunk.after_context(),
            ] {
                ensure!(segments.len() <= 256, "notebook range count bound");
                for segment in segments {
                    let [start, end] = segment.range();
                    ensure!(
                        start <= end && end <= 4 * 1024 * 1024,
                        "invalid notebook range"
                    );
                }
            }
        }
        ensure!(
            self.core_digest == self.core_identity()?,
            "notebook plan digest mismatch"
        );
        Ok(())
    }
    /// Compare with complete archived bytes; structural validation alone cannot
    /// establish UTF-8 boundaries or complete selected-unit coverage.
    pub fn validate_source(
        &self,
        archive: &ArchivedNormalizedSource,
        source: &ValidatedNotebook,
    ) -> Result<()> {
        self.validate()?;
        ensure!(
            self.archive_id == archive.id
                && self.conversation == archive.conversation
                && self.source_identity_digest == archive.identity_digest()?
                && self.guidance == archive.guidance,
            "notebook plan source mismatch"
        );
        archive.validate_bytes(source.canonical_bytes())?;
        ensure!(
            self.chunks
                == source
                    .coverage(&archive.selection, self.caller_max_model_calls)?
                    .chunks(),
            "notebook plan coverage mismatch"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotebookStageKind {
    Map,
    Reduce,
}
impl NotebookStageKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Reduce => "reduce",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotebookStageOutcome {
    Generated,
    Failed,
    Canceled,
    Uncertain,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotebookFailure {
    Provider,
    Canceled,
    Deadline,
    OutputLimit,
    EmptyOutput,
    PublicationUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotebookStage {
    pub id: Uuid,
    pub conversation: Uuid,
    pub operation: Uuid,
    pub plan_id: Uuid,
    pub core_digest: String,
    pub plan_revision: String,
    pub kind: NotebookStageKind,
    pub ordinal: usize,
    pub archive_id: Uuid,
    pub source_revision: String,
    pub input_digest: String,
    pub predecessors: Vec<(Uuid, String, String)>,
    pub prepared_revision: Option<String>,
    pub outcome: Option<NotebookStageOutcome>,
    pub invoked: Option<bool>,
    pub output: Option<String>,
    pub observed_output_digest: Option<String>,
    pub observed_output_bytes: Option<u64>,
    pub failure: Option<NotebookFailure>,
}
impl NotebookStage {
    pub fn identity(operation: Uuid, kind: NotebookStageKind, ordinal: usize) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "urn:rmb:writer-notebook-stage:v1:{operation}:{}:{ordinal}",
                kind.name()
            )
            .as_bytes(),
        )
    }
    pub fn transition(&self, finish: bool) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "urn:rmb:writer-notebook-transition:v1:{}:{}",
                self.id,
                if finish { "finish" } else { "prepare" }
            )
            .as_bytes(),
        )
    }
    pub fn prepare_fingerprint(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(&serde_json::json!([
            "prepare-writer-notebook-stage-v1",
            self.plan_id,
            self.core_digest,
            self.id,
            self.kind,
            self.ordinal,
            self.archive_id,
            self.source_revision,
            self.input_digest,
            self.predecessors
        ]))?))
    }
    pub fn finish_fingerprint(&self) -> Result<String> {
        ensure!(self.outcome.is_some(), "unfinished notebook stage");
        Ok(digest(&serde_json::to_vec(&serde_json::json!([
            "finish-writer-notebook-stage-v1",
            self.plan_id,
            self.id,
            self.prepared_revision,
            self.outcome,
            self.invoked,
            self.output,
            self.observed_output_digest,
            self.observed_output_bytes,
            self.failure
        ]))?))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.conversation.is_nil()
                && !self.operation.is_nil()
                && valid_digest(&self.plan_revision)
                && valid_digest(&self.source_revision)
                && !self.archive_id.is_nil()
                && self.plan_id == NotebookPlan::identity(self.operation)
                && self.id == Self::identity(self.operation, self.kind, self.ordinal)
                && valid_digest(&self.core_digest)
                && valid_digest(&self.input_digest),
            "invalid notebook stage identity"
        );
        ensure!(
            match self.kind {
                NotebookStageKind::Map => self.ordinal < 8 && self.predecessors.is_empty(),
                NotebookStageKind::Reduce =>
                    self.ordinal == 0 && (1..=8).contains(&self.predecessors.len()),
            },
            "invalid notebook stage dependencies"
        );
        for (ordinal, (id, rev, hash)) in self.predecessors.iter().enumerate() {
            ensure!(
                *id == Self::identity(self.operation, NotebookStageKind::Map, ordinal)
                    && valid_digest(rev)
                    && valid_digest(hash),
                "invalid notebook predecessor"
            );
        }
        match self.outcome {
            None => ensure!(
                self.prepared_revision.is_none()
                    && self.invoked.is_none()
                    && self.output.is_none()
                    && self.observed_output_digest.is_none()
                    && self.observed_output_bytes.is_none()
                    && self.failure.is_none(),
                "invalid prepared notebook stage"
            ),
            Some(outcome) => {
                ensure!(
                    self.prepared_revision
                        .as_ref()
                        .is_some_and(|hash| valid_digest(hash))
                        && self.invoked.is_some()
                        && self.observed_output_digest.is_some()
                            == self.observed_output_bytes.is_some()
                        && self
                            .observed_output_digest
                            .as_ref()
                            .is_none_or(|s| valid_digest(s)),
                    "invalid notebook result observation"
                );
                if outcome == NotebookStageOutcome::Generated {
                    let text = self.output.as_ref().context("notebook result absent")?;
                    ensure!(
                        self.invoked == Some(true)
                            && self.failure.is_none()
                            && !text.trim().is_empty()
                            && !text.contains('\0')
                            && self.observed_output_digest == Some(digest(text.as_bytes()))
                            && self.observed_output_bytes == Some(text.len() as u64),
                        "invalid notebook generated result"
                    );
                    ensure!(
                        match self.kind {
                            NotebookStageKind::Map =>
                                serde_json::to_vec(text)?.len() <= MAX_MAP_REPLY,
                            NotebookStageKind::Reduce => text.len() <= MAX_FINAL_TEXT,
                        },
                        "notebook result bound"
                    );
                } else {
                    ensure!(
                        self.output.is_none() && self.failure.is_some(),
                        "invalid notebook failed result"
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vector() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/writer_notebook_identity_vectors.json"
        ))
        .unwrap()
    }
    fn plan() -> NotebookPlan {
        let v = vector();
        let source = ValidatedNotebook::from_bytes(
            v["raw_document_utf8"].as_str().unwrap().as_bytes(),
            v["raw_document_sha256"].as_str().unwrap(),
        )
        .unwrap();
        NotebookPlan {
            id: v["plan"]["plan_id"].as_str().unwrap().parse().unwrap(),
            conversation: v["conversation"].as_str().unwrap().parse().unwrap(),
            operation: v["operation"].as_str().unwrap().parse().unwrap(),
            archive_id: v["source"]["archive_id"].as_str().unwrap().parse().unwrap(),
            source_revision: "b".repeat(64),
            source_identity_digest: v["source"]["sha256"].as_str().unwrap().into(),
            request: "Explain the force.".into(),
            guidance: None,
            personalization_digest: v["personalization"]["sha256"].as_str().unwrap().into(),
            history_digest: v["history"]["sha256"].as_str().unwrap().into(),
            prior: None,
            caller_max_model_calls: 3,
            chunks: source
                .coverage(&TextScope::SuppliedNotebook, 3)
                .unwrap()
                .chunks()
                .to_vec(),
            core_digest: v["plan"]["sha256"].as_str().unwrap().into(),
            map_input_digests: vec![v["model_input"]["sha256"].as_str().unwrap().into()],
            reducer_prefix_digest: "d".repeat(64),
            reducer_reserved_bytes: 8192,
        }
    }
    fn stage(plan: &NotebookPlan) -> NotebookStage {
        NotebookStage {
            id: NotebookStage::identity(plan.operation, NotebookStageKind::Map, 0),
            conversation: plan.conversation,
            operation: plan.operation,
            plan_id: plan.id,
            core_digest: plan.core_digest.clone(),
            plan_revision: "e".repeat(64),
            kind: NotebookStageKind::Map,
            ordinal: 0,
            archive_id: plan.archive_id,
            source_revision: plan.source_revision.clone(),
            input_digest: plan.map_input_digests[0].clone(),
            predecessors: vec![],
            prepared_revision: None,
            outcome: None,
            invoked: None,
            output: None,
            observed_output_digest: None,
            observed_output_bytes: None,
            failure: None,
        }
    }
    #[test]
    fn literal_plan_stage_and_transition_fingerprints_match_independent_vectors() {
        let v = vector();
        let p = plan();
        p.validate().unwrap();
        assert_eq!(p.core_identity().unwrap(), v["plan"]["sha256"]);
        let mut s = stage(&p);
        s.validate().unwrap();
        assert_eq!(s.id.to_string(), v["stage_ids"]["map_0"]);
        assert_eq!(
            s.transition(false).to_string(),
            v["stage_ids"]["map_prepare"]
        );
        assert_eq!(s.transition(true).to_string(), v["stage_ids"]["map_finish"]);
        assert_eq!(
            s.prepare_fingerprint().unwrap(),
            v["prepare_fingerprint"]["sha256"]
        );
        s.prepared_revision = Some("c".repeat(64));
        s.outcome = Some(NotebookStageOutcome::Generated);
        s.invoked = Some(true);
        s.output = Some("F = 10 N.".into());
        s.observed_output_bytes = Some(9);
        s.observed_output_digest = Some(digest(b"F = 10 N."));
        s.validate().unwrap();
        assert_eq!(
            s.finish_fingerprint().unwrap(),
            v["finish_fingerprint"]["sha256"]
        );
        assert_eq!(
            NotebookStage::identity(p.operation, NotebookStageKind::Reduce, 0).to_string(),
            v["stage_ids"]["reduce_0"]
        );
        let mut changed = p.clone();
        changed.request.push('!');
        assert!(changed.validate().is_err());
        let mut changed = p.clone();
        changed.caller_max_model_calls = 2;
        assert!(changed.validate().is_err());
        s.observed_output_bytes = Some(10);
        assert!(s.validate().is_err());
    }
    #[test]
    fn parsed_ranges_and_failed_or_oversize_results_never_become_valid_generated_facts() {
        assert!(serde_json::from_str::<crate::writer::notebook::Segment>("[0,3,2]").is_err());
        let mut s = stage(&plan());
        s.prepared_revision = Some("c".repeat(64));
        s.outcome = Some(NotebookStageOutcome::Generated);
        s.invoked = Some(true);
        let output = "\\".repeat(2048); // UTF-8 bytes fit, encoded JSON exceeds the map bound.
        s.observed_output_digest = Some(digest(output.as_bytes()));
        s.observed_output_bytes = Some(output.len() as u64);
        s.output = Some(output);
        assert!(s.validate().is_err());
        s.outcome = Some(NotebookStageOutcome::Failed);
        s.failure = Some(NotebookFailure::OutputLimit);
        s.output = None;
        s.validate().unwrap();
        s.failure = None;
        assert!(s.validate().is_err());
    }
}
