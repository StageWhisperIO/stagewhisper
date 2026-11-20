use crate::insight_feedback::InsightProducer;
use crate::store::{
    InsightNote, InsightSource, LiveSessionRecord, SessionRecord, SessionStore, StoreError,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewInsight {
    pub id: Option<String>,
    pub producer: Option<InsightProducer>,
    pub severity: String,
    pub message: String,
    pub segment_index: Option<usize>,
    pub source: Option<InsightSource>,
}

fn already_recorded(insights: &[InsightNote], note: &InsightNote) -> bool {
    let repeats_last = insights
        .last()
        .is_some_and(|last| last.message == note.message);
    let id_taken = note
        .id
        .as_deref()
        .is_some_and(|id| insights.iter().any(|existing| existing.has_id(id)));
    repeats_last || id_taken
}

impl SessionStore {
    pub fn live_append_insight(
        &self,
        session_id: &str,
        severity: &str,
        message: &str,
        segment_index: Option<usize>,
        source: Option<InsightSource>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let insight = NewInsight {
            severity: severity.to_string(),
            message: message.to_string(),
            segment_index,
            source,
            ..NewInsight::default()
        };
        self.live_append_new_insight(session_id, insight, now_ms)
            .map(|_| ())
    }

    pub fn live_append_new_insight(
        &self,
        session_id: &str,
        insight: NewInsight,
        now_ms: u64,
    ) -> Result<bool, StoreError> {
        let message = insight.message.trim();
        if message.is_empty() {
            return Ok(false);
        }
        let note = InsightNote {
            offset_ms: 0,
            severity: insight.severity,
            message: message.to_string(),
            segment_index: insight.segment_index,
            source: insight.source,
            id: insight.id.filter(|id| !id.is_empty()),
            producer: insight.producer,
            feedback: None,
            feedback_withdraw_pending: false,
        };
        let _guard = self.lock.lock().unwrap();
        let Some(record) = self.read_live_raw(session_id)? else {
            let Some(session) = self.read_raw(session_id)? else {
                return Ok(false);
            };
            return self.push_insight_session_locked(session, note, now_ms);
        };
        self.push_insight_locked(record, note, now_ms)
    }

    fn push_insight_locked(
        &self,
        mut record: LiveSessionRecord,
        note: InsightNote,
        now_ms: u64,
    ) -> Result<bool, StoreError> {
        if already_recorded(&record.insights, &note) {
            return Ok(false);
        }
        record.insights.push(InsightNote {
            offset_ms: now_ms.saturating_sub(record.started_at_ms),
            ..note
        });
        record.updated_at_ms = now_ms;
        self.write_live_raw(&record)?;
        Ok(true)
    }

    fn push_insight_session_locked(
        &self,
        mut record: SessionRecord,
        note: InsightNote,
        now_ms: u64,
    ) -> Result<bool, StoreError> {
        if already_recorded(&record.insights, &note) {
            return Ok(false);
        }
        let started_ms = crate::insights::rfc3339_epoch_ms(&record.started_at).unwrap_or(now_ms);
        record.insights.push(InsightNote {
            offset_ms: now_ms.saturating_sub(started_ms),
            ..note
        });
        Self::refresh_notes_markdown(&mut record);
        self.write_raw(&record)?;
        Ok(true)
    }
}
