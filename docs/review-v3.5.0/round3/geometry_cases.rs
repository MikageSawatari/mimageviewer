fn main() {
    let mut counts = [[0_usize; 2]; 3];
    for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
        for physical_scale in [1.0_f32, 0.73, 1.125] {
            let scale = physical_scale / ppp;
            for fixture in 0..3 {
                for rotation in [rotation_db::Rotation::None, rotation_db::Rotation::Cw90,
                                 rotation_db::Rotation::Cw180, rotation_db::Rotation::Cw270] {
                    let sizes: Vec<_> = (0..5).map(|i| {
                        let mut pages = Vec::new();
                        for side in 0..(if fixture == 0 {1} else {2}) {
                            let source_height = if fixture == 0 {
                                if i % 2 == 0 {1000.0} else {1001.0}
                            } else if side == 0 {1001.0} else {1000.0};
                            let source_width = if side == 0 {501.0} else {401.0};
                            let mut page = ContinuousReadingPageSize::full(i*2+side,
                                source_width*scale, source_height*scale);
                            page.logical_scale = scale;
                            page.rotation = rotation;
                            if fixture == 2 {
                                page.content_bbox = Some(if side == 0 {
                                    egui::Rect::from_min_max(egui::pos2(0.0,0.0), egui::pos2(1.0,0.6))
                                } else {
                                    egui::Rect::from_min_max(egui::pos2(0.0,0.5), egui::pos2(1.0,1.0))
                                });
                            }
                            pages.push(page);
                        }
                        ContinuousReadingUnitSize {width: pages.iter().map(|p|p.width).sum(),
                            height: pages.iter().map(|p|p.height).fold(0.0,f32::max),
                            pages, page_gap: 0.0, logical_scale: scale}
                    }).collect();
                    let heights: Vec<_> = sizes.iter().map(|s| {
                        let span = continuous_unit_drawn_span(s,ppp);
                        span.y_max-span.y_min
                    }).collect();
                    for gap in [0.0_f32,1.0,20.0] {
                        let offsets = vertical_reading_offsets(&heights,gap,2,ppp);
                        for base_px in [-1000.5_f32,-0.5,0.0,0.5,987.25] {
                            let origin = quantize_points_to_physical_pixels(base_px/ppp,ppp);
                            let bands: Vec<_> = sizes.iter().zip(&offsets).map(|(size,offset)| {
                                let unit=egui::Rect::from_center_size(egui::pos2(0.0,origin+offset),
                                    egui::vec2(size.width,size.height));
                                let rects=continuous_reading_page_rects(unit,size,ppp);
                                let mut lo=f32::INFINITY;
                                let mut hi=f32::NEG_INFINITY;
                                for (page,(_,rect,_)) in size.pages.iter().zip(rects) {
                                    let (a,b)=page.drawn_band(ContinuousAxis::Y,ppp);
                                    lo=lo.min((rect.top()+a)*ppp);
                                    hi=hi.max((rect.top()+b)*ppp);
                                }
                                (lo,hi)
                            }).collect();
                            let expected=quantize_points_to_physical_pixels(gap,ppp)*ppp;
                            let actual:Vec<_>=bands.windows(2).map(|p|p[1].0-p[0].1).collect();
                            let fails=actual.iter().filter(|a|(**a-expected).abs()>0.01).count();
                            counts[fixture][0]+=actual.len();
                            counts[fixture][1]+=fails;
                            if base_px==0.0 && gap==0.0 && physical_scale==1.0 {
                                println!("fixture={fixture} ppp={ppp} rotation={rotation:?} heights={heights:?} gap_px={expected} actual={actual:?} painted={bands:?}");
                            }
                        }
                    }
                }
            }
        }
    }
    println!("fixture 0 = alternating odd/even single pages; 1 = untrimmed mixed spreads; 2 = opposite trims on mixed spreads");
    println!("[boundaries checked, incorrect gap >0.01px] by fixture: {counts:?}");
}
