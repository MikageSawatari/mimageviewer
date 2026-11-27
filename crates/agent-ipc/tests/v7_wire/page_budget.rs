// Frozen from 23d73f026349cadd258f295cd8ecf366194d1f3f:crates/agent-ipc/src/page_budget.rs; tests removed.
//! Stable page membership for settings and operation customization reads.
use super::{MAX_ITEM_CHARS, MAX_PAGE_CHARS, PageRow, mcp_content_chars};
use serde::Serialize;

pub fn bound_page_rows<T: Serialize + Clone, P: Serialize>(
    rows: Vec<T>,
    identity: impl Fn(&T) -> String,
    page: impl Fn(&[PageRow<T>]) -> P,
) -> Result<Vec<PageRow<T>>, String> {
    let failed = |_| "応答を作成できませんでした".to_owned();
    let mut items = rows
        .into_iter()
        .map(|row| {
            if mcp_content_chars(&row).map_err(failed)? > MAX_ITEM_CHARS {
                PageRow::too_long(&identity(&row)).map_err(failed)
            } else {
                Ok(PageRow::Value(row))
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    while mcp_content_chars(&page(&items)).map_err(failed)? > MAX_PAGE_CHARS {
        let index = items
            .iter()
            .enumerate()
            .filter_map(|(index, row)| match row {
                PageRow::Value(row) => Some((index, mcp_content_chars(row).unwrap_or(usize::MAX))),
                PageRow::TooLong(_) => None,
            })
            .max_by_key(|(_, size)| *size)
            .map(|(index, _)| index)
            .ok_or_else(|| "値が大きすぎるので、項目を分けてください".to_owned())?;
        if let PageRow::Value(row) = &items[index] {
            items[index] = PageRow::too_long(&identity(row)).map_err(failed)?;
        }
    }
    Ok(items)
}
