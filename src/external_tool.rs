use std::path::PathBuf;

pub const DEFAULT_PDF_RENDER_LONG_EDGE: u32 = 4096;
const ASSOCIATED_APP_DISPLAY_NAME: &str = "関連付けアプリ";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ExternalToolId(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PayloadPolicy {
    #[default]
    AsDisplayed,
    Original,
    Container,
    RealFileOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VideoPolicy {
    #[default]
    File,
    CurrentFrame,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SpreadPolicy {
    #[default]
    Merged,
    BothPages,
    MainPageOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SelectionPolicy {
    #[default]
    Single,
    Each,
    Batch,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalTool {
    pub id: ExternalToolId,
    pub name: String,
    pub executable: Option<PathBuf>,
    pub arguments: String,
    pub working_directory: Option<PathBuf>,
    pub payload: PayloadPolicy,
    pub video: VideoPolicy,
    pub spread: SpreadPolicy,
    pub selection: SelectionPolicy,
    pub pdf_render_long_edge: u32,
    pub for_editing: bool,
    pub show_in_context_menu: bool,
    pub keep_temp: bool,
}

impl ExternalTool {
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        if let Some(stem) = self
            .executable
            .as_deref()
            .and_then(|path| path.file_stem())
            .filter(|stem| !stem.is_empty())
        {
            return stem.to_string_lossy().into_owned();
        }
        ASSOCIATED_APP_DISPLAY_NAME.to_string()
    }

    pub fn defaults_for_viewing() -> Self {
        Self {
            id: ExternalToolId(1),
            name: String::new(),
            executable: None,
            arguments: "{file}".to_string(),
            working_directory: None,
            payload: PayloadPolicy::AsDisplayed,
            video: VideoPolicy::File,
            spread: SpreadPolicy::Merged,
            selection: SelectionPolicy::Single,
            pdf_render_long_edge: DEFAULT_PDF_RENDER_LONG_EDGE,
            for_editing: false,
            show_in_context_menu: true,
            keep_temp: false,
        }
    }

    pub fn defaults_for_editing() -> Self {
        Self {
            payload: PayloadPolicy::Original,
            spread: SpreadPolicy::MainPageOnly,
            for_editing: true,
            ..Self::defaults_for_viewing()
        }
    }
}

/// 既存の最大 ID + 1 を返す。並べ替えでは変わらない。
///
/// ID 空間の上端に達している場合 (DB を手で編集した等) は panic せず、前から空きを探す。
/// 登録数は現実的に小さいので、この経路でもすぐ見つかる。
pub fn next_id(existing: &[ExternalTool]) -> ExternalToolId {
    let max_id = existing.iter().map(|tool| tool.id.0).max().unwrap_or(0);
    if let Some(next) = max_id.checked_add(1) {
        return ExternalToolId(next);
    }
    let used: std::collections::HashSet<u32> = existing.iter().map(|tool| tool.id.0).collect();
    let free = (1..=u32::MAX)
        .find(|candidate| !used.contains(candidate))
        .unwrap_or(u32::MAX);
    ExternalToolId(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_uses_name_then_executable_stem_then_association_label() {
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.name = "画像編集".to_string();
        tool.executable = Some(PathBuf::from(r"C:\Tools\editor.exe"));
        assert_eq!(tool.display_name(), "画像編集");

        tool.name.clear();
        assert_eq!(tool.display_name(), "editor");

        tool.executable = None;
        assert_eq!(tool.display_name(), ASSOCIATED_APP_DISPLAY_NAME);
    }

    #[test]
    fn editing_defaults_preserve_original_main_page() {
        let tool = ExternalTool::defaults_for_editing();
        assert_eq!(tool.payload, PayloadPolicy::Original);
        assert_eq!(tool.spread, SpreadPolicy::MainPageOnly);
        assert!(tool.for_editing);
        assert_eq!(tool.arguments, "{file}");
        assert_eq!(tool.pdf_render_long_edge, DEFAULT_PDF_RENDER_LONG_EDGE);
    }

    #[test]
    fn next_id_uses_maximum_regardless_of_order() {
        let mut tools = vec![
            ExternalTool {
                id: ExternalToolId(9),
                ..ExternalTool::defaults_for_viewing()
            },
            ExternalTool {
                id: ExternalToolId(2),
                ..ExternalTool::defaults_for_viewing()
            },
        ];
        assert_eq!(next_id(&tools), ExternalToolId(10));
        tools.reverse();
        assert_eq!(next_id(&tools), ExternalToolId(10));
        assert_eq!(next_id(&[]), ExternalToolId(1));
    }

    #[test]
    fn next_id_falls_back_to_a_free_slot_instead_of_panicking_at_the_top() {
        let tools = vec![
            ExternalTool {
                id: ExternalToolId(u32::MAX),
                ..ExternalTool::defaults_for_viewing()
            },
            ExternalTool {
                id: ExternalToolId(1),
                ..ExternalTool::defaults_for_viewing()
            },
        ];
        // max + 1 が溢れるので、前から空いている ID を返す。panic しないことが要件。
        assert_eq!(next_id(&tools), ExternalToolId(2));
    }
}
