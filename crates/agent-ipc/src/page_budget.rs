//! Stable page membership for settings and operation customization reads.
use crate::{MAX_ITEM_CHARS, MAX_PAGE_CHARS, PageRow, mcp_content_chars};
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn operations_page_budget_omits_one_oversized_row_and_keeps_membership() {
        let rows = vec![
            "short".to_owned(),
            "\"\\\0".repeat(MAX_ITEM_CHARS),
            "last".into(),
        ];
        let items = bound_page_rows(
            rows,
            Clone::clone,
            |items| json!({"items":items,"page":1,"pages":1}),
        )
        .unwrap();
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0], PageRow::Value(row) if row == "short"));
        assert!(matches!(&items[1], PageRow::TooLong(_)));
        assert!(matches!(&items[2], PageRow::Value(row) if row == "last"));
    }

    #[test]
    fn operations_page_budget_omits_largest_rows_when_only_page_sum_overflows() {
        let rows: Vec<_> = (0..crate::SETTINGS_PAGE_SIZE).map(|index| {
            json!({"action":format!("action{index}"),"label":"操作", "inputs":vec!["ring.image.up_right"; index + 20]})
        }).collect();
        assert!(
            rows.iter()
                .all(|row| mcp_content_chars(row).unwrap() <= MAX_ITEM_CHARS)
        );
        assert!(mcp_content_chars(&json!({"items":rows})).unwrap() > MAX_PAGE_CHARS);
        let items = bound_page_rows(
            rows.clone(),
            |row| row["action"].as_str().unwrap().to_owned(),
            |items| json!({"items":items,"page":1,"pages":1}),
        )
        .unwrap();
        assert_eq!(items.len(), rows.len());
        assert!(items.iter().any(|row| matches!(row, PageRow::TooLong(_))));
        for (index, item) in items.iter().enumerate() {
            match item {
                PageRow::Value(row) => assert_eq!(row, &rows[index]),
                PageRow::TooLong(marker) => assert_eq!(
                    marker.id,
                    crate::wire_digest(&format!("action{index}")).unwrap()
                ),
            }
        }
        assert!(
            mcp_content_chars(&json!({"items":items,"page":1,"pages":1})).unwrap()
                <= MAX_PAGE_CHARS
        );
    }
}
