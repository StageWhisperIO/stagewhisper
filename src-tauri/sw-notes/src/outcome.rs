use crate::store::{
    CallOutcome, OutcomeConverted, OutcomeFelt, OutcomeLabel, OutcomeProvenance, SessionStore,
    StoreError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutcomeWrite {
    pub felt: Option<OutcomeFelt>,
    pub converted: Option<OutcomeConverted>,
    pub decided_at: Option<String>,
    pub provenance: OutcomeProvenance,
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutcomeWriteResult {
    pub felt: LabelWriteResult,
    pub converted: LabelWriteResult,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelWriteResult {
    #[default]
    NotOffered,
    Recorded,
    Unchanged,
    RefusedOverHumanLabel,
}

impl OutcomeWriteResult {
    pub fn changed_anything(&self) -> bool {
        self.felt == LabelWriteResult::Recorded || self.converted == LabelWriteResult::Recorded
    }
}

impl OutcomeWrite {
    pub fn by_user(felt: Option<OutcomeFelt>, converted: Option<OutcomeConverted>) -> Self {
        Self {
            felt,
            converted,
            decided_at: None,
            provenance: OutcomeProvenance::User,
            source: None,
        }
    }

    fn label<T>(&self, value: T, now: &str, publish_pending: bool) -> OutcomeLabel<T> {
        OutcomeLabel {
            value,
            decided_at: self.decided_at.clone().unwrap_or_else(|| now.to_string()),
            provenance: self.provenance,
            source: self.source.clone(),
            publish_pending,
        }
    }

    fn from_label<T: Copy>(label: &OutcomeLabel<T>, into_write: impl FnOnce(&mut Self, T)) -> Self {
        let mut write = Self {
            felt: None,
            converted: None,
            decided_at: Some(label.decided_at.clone()),
            provenance: label.provenance,
            source: label.source.clone(),
        };
        into_write(&mut write, label.value);
        write
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnpublishedOutcome {
    pub session_id: String,
    pub backend_session_id: String,
    pub write: OutcomeWrite,
}

fn clear_if_published<T: PartialEq>(
    slot: &mut Option<OutcomeLabel<T>>,
    offered: Option<T>,
    published: &OutcomeWrite,
) -> bool {
    let (Some(label), Some(offered)) = (slot.as_mut(), offered) else {
        return false;
    };
    let matches = label.publish_pending
        && label.value == offered
        && label.provenance == published.provenance
        && label.source == published.source;
    if matches {
        label.publish_pending = false;
    }
    matches
}

fn apply_label<T: PartialEq>(
    slot: &mut Option<OutcomeLabel<T>>,
    incoming: Option<OutcomeLabel<T>>,
) -> LabelWriteResult {
    let Some(incoming) = incoming else {
        return LabelWriteResult::NotOffered;
    };
    if let Some(existing) = slot.as_ref() {
        if existing.provenance == OutcomeProvenance::User
            && incoming.provenance != OutcomeProvenance::User
        {
            return LabelWriteResult::RefusedOverHumanLabel;
        }
        if existing.value == incoming.value
            && existing.provenance == incoming.provenance
            && existing.source == incoming.source
        {
            return LabelWriteResult::Unchanged;
        }
    }
    *slot = Some(incoming);
    LabelWriteResult::Recorded
}

pub(crate) fn merge_outcomes(
    first: Option<&CallOutcome>,
    second: Option<&CallOutcome>,
) -> Option<CallOutcome> {
    if first.is_none() && second.is_none() {
        return None;
    }
    let merged = CallOutcome {
        felt: pick_label(
            first.and_then(|o| o.felt.as_ref()),
            second.and_then(|o| o.felt.as_ref()),
        ),
        converted: pick_label(
            first.and_then(|o| o.converted.as_ref()),
            second.and_then(|o| o.converted.as_ref()),
        ),
    };
    Some(merged)
}

fn pick_label<T: Clone>(
    first: Option<&OutcomeLabel<T>>,
    second: Option<&OutcomeLabel<T>>,
) -> Option<OutcomeLabel<T>> {
    match (first, second) {
        (Some(a), Some(b)) => {
            if b.provenance == OutcomeProvenance::User && a.provenance != OutcomeProvenance::User {
                Some(b.clone())
            } else {
                Some(a.clone())
            }
        }
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    }
}

impl SessionStore {
    pub fn set_outcome(
        &self,
        session_id: &str,
        write: &OutcomeWrite,
        now: &str,
    ) -> Result<Option<OutcomeWriteResult>, StoreError> {
        let _guard = self.lock.lock().unwrap();
        self.ensure_not_live(session_id)?;
        let Some(mut record) = self.read_raw(session_id)? else {
            return Ok(None);
        };
        let publish_pending = record.backend_session_id.is_some();
        let mut outcome = record.outcome.take().unwrap_or_default();
        let result = OutcomeWriteResult {
            felt: apply_label(
                &mut outcome.felt,
                write
                    .felt
                    .map(|value| write.label(value, now, publish_pending)),
            ),
            converted: apply_label(
                &mut outcome.converted,
                write
                    .converted
                    .map(|value| write.label(value, now, publish_pending)),
            ),
        };
        record.outcome = (outcome != CallOutcome::default()).then_some(outcome);
        if result.changed_anything() {
            self.write_raw(&record)?;
        }
        Ok(Some(result))
    }

    pub fn unpublished_outcomes(&self) -> Result<Vec<UnpublishedOutcome>, StoreError> {
        let mut unpublished = Vec::new();
        for summary in self.list()? {
            let Some(outcome) = summary.outcome else {
                continue;
            };
            let felt_pending = outcome.felt.as_ref().is_some_and(|l| l.publish_pending);
            let converted_pending = outcome
                .converted
                .as_ref()
                .is_some_and(|l| l.publish_pending);
            if !felt_pending && !converted_pending {
                continue;
            }
            let Some(backend_session_id) = self
                .load(&summary.session_id)?
                .and_then(|record| record.backend_session_id)
            else {
                continue;
            };
            let writes = [
                outcome
                    .felt
                    .filter(|l| l.publish_pending)
                    .map(|l| OutcomeWrite::from_label(&l, |w, v| w.felt = Some(v))),
                outcome
                    .converted
                    .filter(|l| l.publish_pending)
                    .map(|l| OutcomeWrite::from_label(&l, |w, v| w.converted = Some(v))),
            ];
            for write in writes.into_iter().flatten() {
                unpublished.push(UnpublishedOutcome {
                    session_id: summary.session_id.clone(),
                    backend_session_id: backend_session_id.clone(),
                    write,
                });
            }
        }
        Ok(unpublished)
    }

    pub fn mark_outcome_published(
        &self,
        session_id: &str,
        published: &OutcomeWrite,
    ) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        self.ensure_not_live(session_id)?;
        let Some(mut record) = self.read_raw(session_id)? else {
            return Ok(());
        };
        let Some(outcome) = record.outcome.as_mut() else {
            return Ok(());
        };
        let felt_cleared = clear_if_published(&mut outcome.felt, published.felt, published);
        let converted_cleared =
            clear_if_published(&mut outcome.converted, published.converted, published);
        if felt_cleared || converted_cleared {
            self.write_raw(&record)?;
        }
        Ok(())
    }

    pub fn outcome(&self, session_id: &str) -> Result<Option<CallOutcome>, StoreError> {
        let _guard = self.lock.lock().unwrap();
        Ok(self.read_raw(session_id)?.and_then(|r| r.outcome))
    }
}

#[cfg(test)]
#[path = "outcome_tests.rs"]
mod outcome_tests;
