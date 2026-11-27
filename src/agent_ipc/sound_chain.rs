//! Worker-side wire conversion; the UI receives an owned, prepared request and
//! submits it once. The sound service owns late completion, persistence/notices.
use mimageviewer_agent_ipc::{
    AgentError, ErrorCode, MAX_PAGE_CHARS, SoundChainError, SoundChainErrorCode, SoundChainRequest,
    SoundChainResponse, SoundChainResult, SoundPipeline, mcp_content_chars,
};

use crate::effetune::sound_document::{ApplyInput, EffectInput, GetInput, Pipeline, SelectInput};
use crate::effetune::sound_service::{SoundError, SoundRequest, SoundResponse};

fn pipeline(value: SoundPipeline) -> Pipeline {
    match value {
        SoundPipeline::A => Pipeline::A,
        SoundPipeline::B => Pipeline::B,
    }
}

/// Validate/convert on the agent worker, before the UI admission CAS. Parameter
/// values move into the request without cloning them in the UI pass.
pub(super) fn prepare_request(args: SoundChainRequest) -> Result<SoundRequest, AgentError> {
    args.validate()
        .map_err(|message| AgentError::new(ErrorCode::BadRequest, message))?;
    Ok(match args {
        SoundChainRequest::Lock {} => SoundRequest::Lock,
        SoundChainRequest::Unlock { lock } => SoundRequest::Unlock { lock },
        SoundChainRequest::Get {
            pipeline: selected,
            offset,
            ids,
        } => SoundRequest::Get(GetInput {
            pipeline: selected.map(pipeline),
            offset,
            ids,
        }),
        SoundChainRequest::Apply {
            lock,
            revision,
            pipeline: selected,
            effects,
        } => SoundRequest::Apply {
            lock,
            revision,
            input: ApplyInput {
                pipeline: pipeline(selected),
                effects: effects
                    .into_iter()
                    .map(|effect| EffectInput {
                        id: effect.id,
                        effect_type: effect.effect_type,
                        enabled: effect.enabled,
                        parameters: effect.parameters,
                        input_bus: effect.input_bus,
                        output_bus: effect.output_bus,
                        channel: effect.channel,
                    })
                    .collect(),
            },
        },
        SoundChainRequest::Select {
            lock,
            revision,
            use_pipeline,
            bypass,
        } => SoundRequest::Select {
            lock,
            revision,
            input: SelectInput {
                use_pipeline: use_pipeline.map(pipeline),
                bypass,
            },
        },
        SoundChainRequest::Undo { lock } => SoundRequest::Undo { lock },
        SoundChainRequest::SavePreset {
            lock,
            pipeline: selected,
            name,
            replace,
        } => SoundRequest::SavePreset {
            lock,
            pipeline: pipeline(selected),
            name,
            replace,
        },
    })
}

fn service_error(error: SoundError) -> SoundChainError {
    let code = match &error {
        SoundError::Busy => SoundChainErrorCode::Busy,
        SoundError::Unlocked => SoundChainErrorCode::Unlocked,
        SoundError::LockMismatch => SoundChainErrorCode::LockMismatch,
        SoundError::Changed => SoundChainErrorCode::Changed,
        SoundError::NothingToUndo => SoundChainErrorCode::NothingToUndo,
        SoundError::Aborted => SoundChainErrorCode::Aborted,
        SoundError::Invalid(_) => SoundChainErrorCode::Invalid,
        SoundError::Unavailable(_) => SoundChainErrorCode::Unavailable,
    };
    SoundChainError {
        code,
        message: error.to_string(),
    }
}

pub(super) fn wire_result(
    result: Result<SoundResponse, SoundError>,
) -> Result<SoundChainResult, AgentError> {
    let result = match result {
        Ok(response) => {
            // The service is app-private; deserialize its bounded projection
            // into the exported wire types here on the worker. This also
            // prevents unknown internal fields from leaking to the adapter.
            let value = serde_json::to_value(response).map_err(|_| super::internal())?;
            let response: SoundChainResponse =
                serde_json::from_value(value).map_err(|_| super::internal())?;
            Ok(response)
        }
        Err(error) => Err(service_error(error)),
    };
    if mcp_content_chars(&result).map_err(|_| super::internal())? > MAX_PAGE_CHARS {
        return Err(super::error(
            ErrorCode::Internal,
            "mIV Sound の応答が大きすぎます",
        ));
    }
    Ok(result)
}
