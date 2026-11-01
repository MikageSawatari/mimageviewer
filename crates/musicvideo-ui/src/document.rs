use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use musicvideo_core::Project;

static NEXT_DOCUMENT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

fn fresh_document_session_id() -> DocumentSessionId {
    DocumentSessionId(NEXT_DOCUMENT_SESSION_ID.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentSessionId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditGeneration(u64);

#[derive(Debug, Clone)]
pub struct SaveCheckpoint {
    session: DocumentSessionId,
    path: PathBuf,
    project: Project,
}

impl SaveCheckpoint {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn project(&self) -> &Project {
        &self.project
    }
}

#[derive(Debug, Clone)]
struct SavedDocument {
    path: PathBuf,
    project: Project,
}

#[derive(Debug, Clone)]
pub struct DocumentSession {
    id: DocumentSessionId,
    edit_generation: EditGeneration,
    next_edit_generation: u64,
    saved: Option<SavedDocument>,
}

impl Default for DocumentSession {
    fn default() -> Self {
        Self {
            id: fresh_document_session_id(),
            edit_generation: EditGeneration(1),
            next_edit_generation: 2,
            saved: None,
        }
    }
}

impl DocumentSession {
    pub fn path(&self) -> Option<&Path> {
        self.saved.as_ref().map(|saved| saved.path.as_path())
    }

    pub fn is_dirty(&self, project: &Project) -> bool {
        self.saved
            .as_ref()
            .is_none_or(|saved| saved.project != *project)
    }

    pub fn edit_generation(&self) -> EditGeneration {
        self.edit_generation
    }

    pub fn id(&self) -> DocumentSessionId {
        self.id
    }

    pub fn is_current(&self, id: DocumentSessionId) -> bool {
        self.id == id
    }

    /// Records a committed edit, including runtime resource changes whose
    /// serialized project happens to compare equal to the previous state.
    pub fn note_edit(&mut self) {
        self.edit_generation = EditGeneration(self.next_edit_generation);
        self.next_edit_generation = self.next_edit_generation.wrapping_add(1).max(1);
    }

    pub fn begin_save(&self, path: PathBuf, project: &Project) -> SaveCheckpoint {
        SaveCheckpoint {
            session: self.id,
            path,
            project: project.clone(),
        }
    }

    /// Adopts a completed save only when it still belongs to this document.
    ///
    /// Later edits in the same session remain dirty because the saved baseline
    /// is the exact project snapshot sent to the worker.
    pub fn complete_save(&mut self, checkpoint: SaveCheckpoint) -> bool {
        if checkpoint.session != self.id {
            return false;
        }
        self.saved = Some(SavedDocument {
            path: checkpoint.path,
            project: checkpoint.project,
        });
        true
    }

    pub fn replace_with_loaded(&mut self, path: PathBuf, project: &Project) {
        self.id = fresh_document_session_id();
        self.note_edit();
        self.saved = Some(SavedDocument {
            path,
            project: project.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicvideo_core::default_project;

    #[test]
    fn delayed_save_cannot_replace_a_loaded_documents_path() {
        let mut document = DocumentSession::default();
        let project_a = default_project(10.0);
        let save_a = document.begin_save(PathBuf::from("a.mivmv.json"), &project_a);

        let project_b = default_project(20.0);
        document.replace_with_loaded(PathBuf::from("b.mivmv.json"), &project_b);

        assert!(!document.complete_save(save_a));
        assert_eq!(document.path(), Some(Path::new("b.mivmv.json")));
        assert!(!document.is_dirty(&project_b));
    }

    #[test]
    fn save_completion_keeps_later_edits_dirty_and_adopts_the_path() {
        let mut document = DocumentSession::default();
        let saved = default_project(10.0);
        let checkpoint = document.begin_save(PathBuf::from("a.mivmv.json"), &saved);
        let mut edited = saved.clone();
        edited.display_gain_db = 6.0;

        assert!(document.complete_save(checkpoint));
        assert_eq!(document.path(), Some(Path::new("a.mivmv.json")));
        assert!(document.is_dirty(&edited));
        assert!(!document.is_dirty(&saved));
    }

    #[test]
    fn content_equality_recognizes_return_to_the_saved_state_after_a_branch() {
        let mut document = DocumentSession::default();
        let saved = default_project(10.0);
        let checkpoint = document.begin_save(PathBuf::from("a.mivmv.json"), &saved);
        assert!(document.complete_save(checkpoint));

        let mut edited = saved.clone();
        edited.display_gain_db = -3.0;
        assert!(document.is_dirty(&edited));
        assert!(!document.is_dirty(&saved));
    }

    #[test]
    fn edit_generation_changes_even_when_serialized_content_does_not() {
        let mut document = DocumentSession::default();
        let before = document.edit_generation();
        document.note_edit();
        assert_ne!(document.edit_generation(), before);
    }

    #[test]
    fn checkpoint_from_another_document_instance_is_rejected() {
        let first = DocumentSession::default();
        let mut second = DocumentSession::default();
        let project = default_project(10.0);
        let checkpoint = first.begin_save(PathBuf::from("first.mivmv.json"), &project);

        assert!(!second.complete_save(checkpoint));
        assert_eq!(second.path(), None);
    }
}
