//! Public production runner, missing runtime, disposable data root; no AI/native app started.
use std::sync::{Arc,atomic::AtomicBool};
use mimageviewer::{books::*,settings::AiFeatureMode,ai::upscale::AiProcessSizeLimit};
fn main(){
    mimageviewer::data_dir::set_test_override(Some(std::path::PathBuf::from("target/review-v350-round4/ai-data")));
    for (label,dims,mode,upscale,expected_ok) in [
        ("outside",[4096,4],AiFeatureMode::HighQuality,true,true),
        ("boundary",[2048,4],AiFeatureMode::HighQuality,true,true),
        ("inside",[16,16],AiFeatureMode::HighQuality,true,false),
        ("disabled",[16,16],AiFeatureMode::Disabled,true,true),
        ("no-model",[16,16],AiFeatureMode::HighQuality,false,true),
    ] {
        let mut params=mimageviewer::adjustment::AdjustParams::default();
        if upscale {params.upscale_model=Some("auto".into());}
        let materials=BookAiMaterials {
            manager:Arc::new(mimageviewer::ai::model_manager::ModelManager::new()),
            policy:BookAiPolicy {feature_mode:mode,
                upscale_limit:AiProcessSizeLimit::square(2048),denoise_limit:AiProcessSizeLimit::square(2048),transparent_bg_mode:0},
        };
        let runner=book_ai_snapshot(materials,None,params);
        let image=egui::ColorImage::new(dims,vec![egui::Color32::from_rgb(40,90,150);dims[0]*dims[1]]);
        let result=(runner.run)(&image,&Arc::new(AtomicBool::new(false)));
        println!("{label}: dims={dims:?} runtime=None success={}",result.is_ok());
        assert_eq!(result.is_ok(),expected_ok);
        if let Ok(result)=result {
            assert_eq!(result.image.size,image.size);assert_eq!(result.image.pixels,image.pixels);assert!(!result.used_upscale);
        }
    }
}
