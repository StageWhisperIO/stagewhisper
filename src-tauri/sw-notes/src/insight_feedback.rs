use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer, Serialize};

use crate::store::{InsightNote, SessionStore, StoreError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightVerdict {
    Helpful,
    NotHelpful,
    Dismissed,
}

impl InsightVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Helpful => "helpful",
            Self::NotHelpful => "not_helpful",
            Self::Dismissed => "dismissed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightProducer {
    OnDevice,
    Assistant,
    Signals,
}

impl InsightProducer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OnDevice => "on_device",
            Self::Assistant => "assistant",
            Self::Signals => "signals",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackSurface {
    Live,
    Library,
}

impl FeedbackSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Library => "library",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsightFeedback {
    pub verdict: InsightVerdict,
    pub rated_at: String,
    pub surface: FeedbackSurface,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub publish_pending: bool,
}

impl InsightFeedback {
    fn is_same_rating_as(&self, other: &InsightFeedback) -> bool {
        self.verdict == other.verdict
            && self.rated_at == other.rated_at
            && self.surface == other.surface
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnpublishedInsightFeedback {
    pub session_id: String,
    pub backend_session_id: String,
    pub insight_id: String,
    pub feedback: Option<InsightFeedback>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Recognized<T> {
    Known(T),
    Unknown(IgnoredAny),
}

pub(crate) fn unknown_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match Option::<Recognized<T>>::deserialize(deserializer)? {
        Some(Recognized::Known(value)) => Some(value),
        Some(Recognized::Unknown(_)) | None => None,
    })
}

impl InsightNote {
    pub fn has_id(&self, insight_id: &str) -> bool {
        !insight_id.is_empty() && self.id.as_deref() == Some(insight_id)
    }

    pub fn feedback_unpublished(&self) -> bool {
        if self.id.is_none() {
            return false;
        }
        match &self.feedback {
            Some(feedback) => feedback.publish_pending,
            None => self.feedback_withdraw_pending,
        }
    }
}

fn apply_feedback(
    insights: &mut [InsightNote],
    insight_id: &str,
    feedback: Option<&InsightFeedback>,
    backend_session_known: bool,
) -> Option<bool> {
    let note = insights.iter_mut().find(|note| note.has_id(insight_id))?;
    let publishes = backend_session_known && note.producer == Some(InsightProducer::Signals);
    let before = (note.feedback.clone(), note.feedback_withdraw_pending);
    match feedback {
        Some(feedback) => {
            note.feedback = Some(InsightFeedback {
                publish_pending: publishes,
                ..feedback.clone()
            });
            note.feedback_withdraw_pending = false;
        }
        None => {
            if note.feedback.take().is_some() {
                note.feedback_withdraw_pending = publishes;
            }
        }
    }
    Some((note.feedback.clone(), note.feedback_withdraw_pending) != before)
}

fn settle_published(
    insights: &mut [InsightNote],
    insight_id: &str,
    published: Option<&InsightFeedback>,
) -> bool {
    let Some(note) = insights.iter_mut().find(|note| note.has_id(insight_id)) else {
        return false;
    };
    match (note.feedback.as_mut(), published) {
        (Some(current), Some(published))
            if current.publish_pending && current.is_same_rating_as(published) =>
        {
            current.publish_pending = false;
            true
        }
        (None, None) if note.feedback_withdraw_pending => {
            note.feedback_withdraw_pending = false;
            true
        }
        _ => false,
    }
}

fn collect_unpublished(
    session_id: &str,
    backend_session_id: Option<&str>,
    insights: &[InsightNote],
    into: &mut Vec<UnpublishedInsightFeedback>,
) {
    let Some(backend_session_id) = backend_session_id else {
        return;
    };
    for note in insights.iter().filter(|note| note.feedback_unpublished()) {
        let Some(insight_id) = note.id.as_deref() else {
            continue;
        };
        let already_listed = into
            .iter()
            .any(|item| item.session_id == session_id && item.insight_id == insight_id);
        if already_listed {
            continue;
        }
        into.push(UnpublishedInsightFeedback {
            session_id: session_id.to_string(),
            backend_session_id: backend_session_id.to_string(),
            insight_id: insight_id.to_string(),
            feedback: note.feedback.clone(),
        });
    }
}

impl SessionStore {
    pub fn set_insight_feedback(
        &self,
        session_id: &str,
        insight_id: &str,
        feedback: Option<InsightFeedback>,
    ) -> Result<bool, StoreError> {
        if insight_id.is_empty() {
            return Ok(false);
        }
        let _guard = self.lock.lock().unwrap();
        let mut found = false;
        if let Some(mut live) = self.read_live_raw(session_id)? {
            let backend_known = live.backend_session_id.is_some();
            let applied = apply_feedback(
                &mut live.insights,
                insight_id,
                feedback.as_ref(),
                backend_known,
            );
            if applied == Some(true) {
                self.write_live_raw(&live)?;
            }
            found |= applied.is_some();
        }
        if let Some(mut record) = self.read_raw(session_id)? {
            let backend_known = record.backend_session_id.is_some();
            let applied = apply_feedback(
                &mut record.insights,
                insight_id,
                feedback.as_ref(),
                backend_known,
            );
            if applied == Some(true) {
                self.write_raw(&record)?;
            }
            found |= applied.is_some();
        }
        Ok(found)
    }

    pub fn unpublished_insight_feedback(
        &self,
    ) -> Result<Vec<UnpublishedInsightFeedback>, StoreError> {
        let mut unpublished = Vec::new();
        for live in self.live_list()? {
            collect_unpublished(
                &live.session_id,
                live.backend_session_id.as_deref(),
                &live.insights,
                &mut unpublished,
            );
        }
        let flagged = {
            let _guard = self.lock.lock().unwrap();
            self.sessions_with_unpublished_insight_feedback()
        };
        for session_id in flagged {
            let Ok(Some(record)) = self.load(&session_id) else {
                continue;
            };
            collect_unpublished(
                &record.session_id,
                record.backend_session_id.as_deref(),
                &record.insights,
                &mut unpublished,
            );
        }
        Ok(unpublished)
    }

    pub fn mark_insight_feedback_published(
        &self,
        session_id: &str,
        insight_id: &str,
        published: Option<&InsightFeedback>,
    ) -> Result<(), StoreError> {
        let _guard = self.lock.lock().unwrap();
        if let Some(mut live) = self.read_live_raw(session_id)? {
            if settle_published(&mut live.insights, insight_id, published) {
                self.write_live_raw(&live)?;
            }
        }
        if let Some(mut record) = self.read_raw(session_id)? {
            if settle_published(&mut record.insights, insight_id, published) {
                self.write_raw(&record)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "insight_feedback_tests.rs"]
mod insight_feedback_tests;
