//! What the Windows app knows about a newer version, for the orange update
//! button in the window's top bar.
//!
//! Plain Rust with no Tauri in it, so its tests run on their own (CI runs
//! `rustc --test` on this file in the server job).

/// Release notes longer than this are cut short (the button's popover only
/// has room for a few lines).
pub const NOTES_MAX: usize = 600;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateState {
    /// The newer version that's out, if any.
    pub version: Option<String>,
    pub notes: Option<String>,
    pub date: Option<String>,
    /// Downloading it now (after "Update now").
    pub downloading: bool,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Downloaded and checked; installing and restarting.
    pub ready: bool,
    /// Why the last download or install failed.
    pub error: Option<String>,
}

impl UpdateState {
    /// A check found `version`. Returns true when it's news (a version we
    /// hadn't told you about yet), so the caller can show a notification.
    pub fn found(&mut self, version: &str, notes: Option<&str>, date: Option<&str>) -> bool {
        let version = version.trim();
        if version.is_empty() {
            return false;
        }
        // Mid-download or installing: leave it be.
        if self.downloading || self.ready {
            return false;
        }
        let new = self.version.as_deref() != Some(version);
        self.version = Some(version.to_string());
        self.notes = notes.map(clean_notes).filter(|n| !n.is_empty());
        self.date = date
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(String::from);
        if new {
            self.error = None;
            self.downloaded = 0;
            self.total = None;
        }
        new
    }

    /// A check found nothing newer (e.g. the update was pulled): forget it,
    /// unless it's already downloading.
    pub fn none_found(&mut self) {
        if !self.downloading && !self.ready {
            *self = UpdateState::default();
        }
    }

    pub fn available(&self) -> bool {
        self.version.is_some()
    }

    /// "Update now". False when there's nothing to download or it's
    /// already under way.
    pub fn start_download(&mut self) -> bool {
        if !self.available() || self.downloading || self.ready {
            return false;
        }
        self.downloading = true;
        self.downloaded = 0;
        self.total = None;
        self.error = None;
        true
    }

    pub fn chunk(&mut self, len: usize, total: Option<u64>) {
        if !self.downloading {
            return;
        }
        self.downloaded = self.downloaded.saturating_add(len as u64);
        if let Some(t) = total.filter(|t| *t > 0) {
            self.total = Some(t);
        }
    }

    pub fn finished(&mut self) {
        if self.downloading {
            self.downloading = false;
            self.ready = true;
        }
    }

    pub fn failed(&mut self, why: &str) {
        self.downloading = false;
        self.ready = false;
        let why = why.trim();
        self.error = Some(if why.is_empty() {
            "the update didn't work".to_string()
        } else {
            why.chars().take(200).collect()
        });
    }

    /// Download progress from 0 to 1, when the size is known.
    pub fn progress(&self) -> Option<f64> {
        if self.ready {
            return Some(1.0);
        }
        let total = self.total.filter(|t| *t > 0)?;
        Some((self.downloaded as f64 / total as f64).clamp(0.0, 1.0))
    }
}

/// Trims release notes and cuts them at `NOTES_MAX` characters (on a
/// character boundary, with an ellipsis).
pub fn clean_notes(notes: &str) -> String {
    let notes = notes.trim();
    if notes.chars().count() <= NOTES_MAX {
        return notes.to_string();
    }
    let mut cut: String = notes.chars().take(NOTES_MAX - 1).collect();
    cut = cut.trim_end().to_string();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_an_update_is_news_once() {
        let mut s = UpdateState::default();
        assert!(!s.available());
        assert!(s.found("0.2.50", Some("  Faster voice  "), Some("2026-11-09")));
        assert!(s.available());
        assert_eq!(s.notes.as_deref(), Some("Faster voice"));
        assert_eq!(s.date.as_deref(), Some("2026-11-09"));
        // The hourly check finds the same version again: no second notification.
        assert!(!s.found("0.2.50", Some("Faster voice"), None));
        // A newer one still is.
        assert!(s.found("0.2.51", None, None));
        assert_eq!(s.notes, None);
        assert!(!s.found("  ", None, None));
        assert_eq!(s.version.as_deref(), Some("0.2.51"));
    }

    #[test]
    fn download_progress_and_ready() {
        let mut s = UpdateState::default();
        assert!(!s.start_download(), "nothing to download yet");
        s.found("0.2.50", None, None);
        assert!(s.start_download());
        assert!(!s.start_download(), "already downloading");
        assert_eq!(s.progress(), None);
        s.chunk(250, Some(1000));
        assert_eq!(s.progress(), Some(0.25));
        s.chunk(250, None);
        assert_eq!(s.progress(), Some(0.5));
        s.chunk(5000, Some(1000));
        assert_eq!(s.progress(), Some(1.0));
        // A check during the download changes nothing.
        assert!(!s.found("0.2.52", None, None));
        s.none_found();
        assert!(s.downloading);
        assert_eq!(s.version.as_deref(), Some("0.2.50"));
        s.finished();
        assert!(s.ready && !s.downloading);
        assert_eq!(s.progress(), Some(1.0));
        assert!(!s.start_download());
    }

    #[test]
    fn a_failed_download_can_be_retried() {
        let mut s = UpdateState::default();
        s.found("0.2.50", None, None);
        s.start_download();
        s.chunk(10, Some(100));
        s.failed("signature didn't match");
        assert!(!s.downloading && !s.ready);
        assert_eq!(s.error.as_deref(), Some("signature didn't match"));
        assert!(s.available());
        assert!(s.start_download());
        assert_eq!(s.error, None);
        assert_eq!(s.downloaded, 0);
        s.failed("");
        assert_eq!(s.error.as_deref(), Some("the update didn't work"));
    }

    #[test]
    fn a_pulled_update_is_forgotten() {
        let mut s = UpdateState::default();
        s.found("0.2.50", None, None);
        s.none_found();
        assert_eq!(s, UpdateState::default());
    }

    #[test]
    fn chunks_before_start_are_ignored() {
        let mut s = UpdateState::default();
        s.found("0.2.50", None, None);
        s.chunk(100, Some(200));
        assert_eq!(s.downloaded, 0);
        s.finished();
        assert!(!s.ready);
    }

    #[test]
    fn long_notes_are_cut() {
        let long = "ü".repeat(NOTES_MAX + 50);
        let cut = clean_notes(&long);
        assert_eq!(cut.chars().count(), NOTES_MAX);
        assert!(cut.ends_with('…'));
        assert_eq!(clean_notes(" short "), "short");
    }
}
