//! Search scope is a read-only projection of saved favorites, in their order.
use super::*;

const MAX_SEARCH_SCOPE: usize = 100;

impl Handlers {
    pub(super) fn status(&self, ctx: &ReadContext) -> Result<Payload, AgentError> {
        ctx.checkpoint()?;
        let settings = crate::settings_db::with_db_result(|db| db.load_into_settings())
            .map_err(|_| internal())?;
        let status = StatusPayload {
            app_version: env!("CARGO_PKG_VERSION").into(),
            permission: ctx.granted.0,
            fts_ready: self.readers.fts.read().unwrap().is_some(),
            remote_active: self.session.as_ref().is_some_and(|session| {
                session.snapshot().phase != crate::remote_ipc::session::RemoteControlPhase::Local
            }),
            search_scope: Vec::new(),
            search_scope_truncated: false,
        };
        let status = with_search_scope(status, &settings.favorites)?;
        ctx.checkpoint()?;
        Ok(Payload::Status(status))
    }
}

fn with_search_scope(
    mut status: StatusPayload,
    favorites: &[crate::settings::FavoriteEntry],
) -> Result<StatusPayload, AgentError> {
    let mut scope = favorites
        .iter()
        .filter(|favorite| favorite.auto_index_metadata);
    status.search_scope = scope
        .by_ref()
        .take(MAX_SEARCH_SCOPE)
        .map(|favorite| favorite.path.to_string_lossy().into_owned())
        .collect();
    status.search_scope_truncated = scope.next().is_some();
    // Count the complete status, including the escaped text copy, and preserve
    // each path exactly. The flag also participates in the resulting budget.
    while mcp_content_chars(&status).map_err(|_| internal())? > MAX_PAGE_CHARS {
        if status.search_scope.pop().is_none() {
            return Err(internal());
        }
        status.search_scope_truncated = true;
    }
    Ok(status)
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
