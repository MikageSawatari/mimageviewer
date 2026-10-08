//! Public production predicates only. No app, runtime/model, or profile initialization.
fn main() {
    use mimageviewer::{books::stage_requests_ai, bake_stage::BakeStage, settings::AiFeatureMode};
    use mimageviewer::ai::upscale::{AiProcessSizeLimit, should_process_rect};
    let mut params=mimageviewer::adjustment::AdjustParams::default();
    params.upscale_model=Some("auto".into());
    let limit=AiProcessSizeLimit::square(2048);
    for dims in [[1024,1024],[2048,2048],[4096,2160]] {
        let wants=stage_requests_ai(BakeStage::Ai,&params,AiFeatureMode::HighQuality);
        let in_range=should_process_rect(dims[0],dims[1],limit);
        println!("dims={dims:?} request_requires_runtime={wants} actual_ai_size_eligible={in_range}");
        assert!(wants);
        assert_eq!(in_range,dims[0]<2048);
    }
    assert!(!stage_requests_ai(BakeStage::Edits,&params,AiFeatureMode::HighQuality));
    assert!(!stage_requests_ai(BakeStage::Ai,&params,AiFeatureMode::Disabled));
}
