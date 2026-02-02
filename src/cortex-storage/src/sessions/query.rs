//! Session query and filtering system.
//!
//! Provides a flexible query builder for filtering and sorting sessions:
//! - `SessionSort` - Sort order options
//! - `SessionQuery` - Query/filter builder

use chrono::Utc;

use super::types::SessionSummary;

/// Sort order for session queries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionSort {
    /// Sort by updated timestamp, newest first (default).
    #[default]
    UpdatedDesc,
    /// Sort by updated timestamp, oldest first.
    UpdatedAsc,
    /// Sort by created timestamp, newest first.
    CreatedDesc,
    /// Sort by created timestamp, oldest first.
    CreatedAsc,
    /// Sort by title alphabetically.
    TitleAsc,
}

/// Query/filter for sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionQuery {
    /// Text search in title or id.
    pub search: Option<String>,
    /// Filter by favorites only.
    pub favorites_only: bool,
    /// Filter by specific tags (any match).
    pub tags: Vec<String>,
    /// Filter by sessions updated after this timestamp.
    pub from_timestamp: Option<i64>,
    /// Filter by sessions updated before this timestamp.
    pub to_timestamp: Option<i64>,
    /// Filter by working directory.
    pub cwd: Option<String>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: usize,
    /// Sort order.
    pub sort: SessionSort,
}

impl SessionQuery {
    /// Create a new empty query.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set text search filter.
    pub fn search(mut self, query: impl Into<String>) -> Self {
        self.search = Some(query.into());
        self
    }

    /// Filter by favorites only.
    pub fn favorites(mut self) -> Self {
        self.favorites_only = true;
        self
    }

    /// Add a tag filter.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Filter by sessions updated after this timestamp.
    pub fn from(mut self, timestamp: i64) -> Self {
        self.from_timestamp = Some(timestamp);
        self
    }

    /// Filter by sessions updated before this timestamp.
    pub fn to(mut self, timestamp: i64) -> Self {
        self.to_timestamp = Some(timestamp);
        self
    }

    /// Filter by sessions from today.
    pub fn today(self) -> Self {
        let now = Utc::now();
        let start_of_day = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        self.from(start_of_day)
    }

    /// Filter by sessions from this week.
    pub fn this_week(self) -> Self {
        let now = Utc::now();
        let week_ago = now - chrono::Duration::days(7);
        self.from(week_ago.timestamp())
    }

    /// Filter by sessions from this month.
    pub fn this_month(self) -> Self {
        let now = Utc::now();
        let month_ago = now - chrono::Duration::days(30);
        self.from(month_ago.timestamp())
    }

    /// Filter by sessions from the last N days.
    pub fn last_days(self, days: u32) -> Self {
        let now = Utc::now();
        let cutoff = now - chrono::Duration::days(days as i64);
        self.from(cutoff.timestamp())
    }

    /// Filter by working directory.
    pub fn in_directory(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set maximum results.
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    /// Set offset for pagination.
    pub fn offset(mut self, n: usize) -> Self {
        self.offset = n;
        self
    }

    /// Set sort order.
    pub fn sort_by(mut self, sort: SessionSort) -> Self {
        self.sort = sort;
        self
    }

    /// Check if a session matches this query.
    pub fn matches(&self, session: &SessionSummary) -> bool {
        // Favorites filter
        if self.favorites_only && !session.is_favorite {
            return false;
        }

        // Tags filter (any match)
        if !self.tags.is_empty() && !self.tags.iter().any(|t| session.tags.contains(t)) {
            return false;
        }

        // Date range filters
        if let Some(from) = self.from_timestamp {
            if session.updated_at < from {
                return false;
            }
        }

        if let Some(to) = self.to_timestamp {
            if session.updated_at > to {
                return false;
            }
        }

        // Working directory filter
        if let Some(ref cwd) = self.cwd {
            if &session.cwd != cwd {
                return false;
            }
        }

        // Text search
        if let Some(ref search) = self.search {
            let search_lower = search.to_lowercase();
            let title_matches = session
                .title
                .as_ref()
                .is_some_and(|t| t.to_lowercase().contains(&search_lower));
            let id_matches = session.id.to_lowercase().contains(&search_lower);

            if !title_matches && !id_matches {
                return false;
            }
        }

        true
    }

    /// Apply sorting to a list of sessions.
    pub fn apply_sort(&self, sessions: &mut [SessionSummary]) {
        match self.sort {
            SessionSort::UpdatedDesc => sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
            SessionSort::UpdatedAsc => sessions.sort_by(|a, b| a.updated_at.cmp(&b.updated_at)),
            SessionSort::CreatedDesc => sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            SessionSort::CreatedAsc => sessions.sort_by(|a, b| a.created_at.cmp(&b.created_at)),
            SessionSort::TitleAsc => sessions.sort_by(|a, b| {
                a.title
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.title.as_deref().unwrap_or(""))
            }),
        }
    }

    /// Apply offset and limit to a list of sessions.
    pub fn apply_pagination(&self, sessions: Vec<SessionSummary>) -> Vec<SessionSummary> {
        sessions
            .into_iter()
            .skip(self.offset)
            .take(self.limit.unwrap_or(usize::MAX))
            .collect()
    }
}
