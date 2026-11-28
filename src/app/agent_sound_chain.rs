//! Live settings permission and one UI-turn submission, with no host wait or I/O.
use super::*;
use crate::effetune::sound_service::{SoundRequest, SoundTicket};
use mimageviewer_agent_ipc::{AgentError, ErrorCode};

impl App {
    pub(super) fn admit_agent_sound_chain(
        &mut self,
        request: SoundRequest,
        client_name: String,
        permit: Option<Arc<crate::agent_ipc::Permit>>,
        ctx: &egui::Context,
    ) -> Result<SoundTicket, AgentError> {
        self.admit_agent_settings_write()?;
        #[cfg(windows)]
        {
            let changes = self
                .agent_changes
                .as_ref()
                .map(|ui| ui.controller.clone())
                .ok_or_else(|| AgentError::new(ErrorCode::NotReady, "mIV の起動処理中です"))?;
            // This existing getter reads in-memory latency metadata, without
            // sending host commands. The sound round trip stays on the FIFO.
            let user_latency_secs = self.dsp_bridge.total_latency_samples() as f64
                / self.dsp_bridge.sample_rate().max(1) as f64;
            self.effetune.set_repaint_context(ctx);
            Ok(self.effetune.submit_sound(
                request,
                &client_name,
                changes,
                user_latency_secs,
                permit,
            ))
        }
        #[cfg(not(windows))]
        {
            let _ = (request, client_name, permit, ctx);
            Err(AgentError::new(
                ErrorCode::NotReady,
                "この OS では音響調整を利用できません",
            ))
        }
    }
}
