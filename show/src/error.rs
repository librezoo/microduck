//! What is wrong with a package, as a list a person can act on.

use serde::Serialize;

/// One thing wrong. `code` is stable (an app maps it to a sentence); `detail` is for the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub code: &'static str,
    pub detail: String,
    /// The event it is about, when there is one — so the app can mark it on the timeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
}

impl Problem {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            event_id: None,
        }
    }

    pub fn at(mut self, event_id: &str) -> Self {
        self.event_id = Some(event_id.to_owned());
        self
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)?;
        if let Some(id) = &self.event_id {
            write!(f, " ({id})")?;
        }
        Ok(())
    }
}

/// Every problem found, not just the first: an app fixing a package wants the whole list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Problems(pub Vec<Problem>);

impl Problems {
    pub fn push(&mut self, p: Problem) {
        self.0.push(p);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn into_result(self) -> Result<(), Problems> {
        if self.0.is_empty() { Ok(()) } else { Err(self) }
    }
    pub fn has(&self, code: &str) -> bool {
        self.0.iter().any(|p| p.code == code)
    }
}

impl std::fmt::Display for Problems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lines: Vec<String> = self.0.iter().map(ToString::to_string).collect();
        write!(f, "{}", lines.join("; "))
    }
}

impl std::error::Error for Problems {}
