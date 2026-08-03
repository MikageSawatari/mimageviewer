use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use image::GenericImageView as _;
use mimageviewer::gpu_lanczos_spike::{Lanczos3Resampler, LanczosJob, LanczosPlan};

const RATIOS: [f64; 3] = [0.63, 0.41, 0.25];
const WARMUP_ITERATIONS: usize = 5;
const MEASURE_ITERATIONS: usize = 50;

fn main() {
    if let Err(error) = run() {
        eprintln!("gpu_lanczos_spike: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let source_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\tmp\miv-downscale-compare\src_2480x3508.png"));
    let reference_root = source_path
        .parent()
        .ok_or("source path has no parent")?
        .to_path_buf();
    let output_dir = PathBuf::from("target").join("lanczos-spike");
    std::fs::create_dir_all(&output_dir)?;

    let source_image = image::open(&source_path)?;
    let (source_width, source_height) = source_image.dimensions();
    let source_rgba = source_image.to_rgba8();
    let source_size = [source_width, source_height];

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))?;
    let adapter_info = adapter.get_info();
    let adapter_features = adapter.features();
    if !adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY) {
        return Err("selected adapter does not support TIMESTAMP_QUERY".into());
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("mIV GPU Lanczos stage-3 spike"),
        required_features: wgpu::Features::TIMESTAMP_QUERY,
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))?;

    let source_texture =
        upload_source_with_mips(&device, &queue, source_size, source_rgba.as_raw())?;
    device.poll(wgpu::PollType::wait_indefinitely())?;

    let resampler = Lanczos3Resampler::new(&device);
    let mut report = vec![
        format!(
            "adapter={} backend={:?} device_type={:?}",
            adapter_info.name, adapter_info.backend, adapter_info.device_type
        ),
        format!(
            "source={}x{} warmup={} measured={}",
            source_width, source_height, WARMUP_ITERATIONS, MEASURE_ITERATIONS
        ),
        "ratio mode mip mip_source target taps_y taps_x fetches gpu_ms mae rmse max_abs gt1_pct gt2_pct"
            .to_string(),
    ];

    for ratio in RATIOS {
        let target_size = [
            (source_width as f64 * ratio) as u32,
            (source_height as f64 * ratio) as u32,
        ];
        let reference_path = reference_root
            .join("full")
            .join(format!("r{ratio:.2}_lanczos3.png"));
        for (mode, use_mip) in [("direct", false), ("mip", true)] {
            let plan = LanczosPlan::new(source_size, target_size, use_mip)?;
            let job = resampler.prepare_job(&device, &source_texture, plan)?;
            let gpu_ms = benchmark_job(&device, &queue, &resampler, &job)?;
            let pixels = read_output_texture(&device, &queue, &job)?;
            let output_path = output_dir.join(format!("r{ratio:.2}_gpu_{mode}.png"));
            image::save_buffer_with_format(
                &output_path,
                &pixels,
                target_size[0],
                target_size[1],
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )?;
            let difference = compare_with_reference(&pixels, target_size, &reference_path)?;
            let line = format!(
                "{ratio:.2} {mode} {} {}x{} {}x{} {} {} {} {:.4} {:.4} {:.4} {} {:.4} {:.4}",
                plan.mip_level,
                plan.mip_source_size[0],
                plan.mip_source_size[1],
                target_size[0],
                target_size[1],
                plan.vertical_max_taps,
                plan.horizontal_max_taps,
                plan.texture_fetches,
                gpu_ms,
                difference.mae,
                difference.rmse,
                difference.max_abs,
                difference.gt1_percent,
                difference.gt2_percent,
            );
            println!("{line}");
            report.push(line);
        }
    }

    let report_path = output_dir.join("report.txt");
    std::fs::write(&report_path, report.join("\n") + "\n")?;
    println!("report={}", report_path.display());
    Ok(())
}

fn upload_source_with_mips(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    rgba: &[u8],
) -> Result<wgpu::Texture, Box<dyn Error>> {
    let expected = u64::from(size[0])
        .saturating_mul(u64::from(size[1]))
        .saturating_mul(4);
    if rgba.len() as u64 != expected {
        return Err(format!("RGBA byte length {} != expected {expected}", rgba.len()).into());
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mIV Lanczos spike source"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: egui_wgpu::mip_level_count(size[0], size[1]),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 4),
            rows_per_image: Some(size[1]),
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    egui_wgpu::MipmapGenerator::new(device).generate(device, queue, &texture);
    Ok(texture)
}

fn benchmark_job(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    resampler: &Lanczos3Resampler,
    job: &LanczosJob,
) -> Result<f64, Box<dyn Error>> {
    let mut warmup = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mIV Lanczos spike warmup encoder"),
    });
    for _ in 0..WARMUP_ITERATIONS {
        resampler.encode(&mut warmup, job, None, None, None);
    }
    queue.submit(Some(warmup.finish()));
    device.poll(wgpu::PollType::wait_indefinitely())?;

    let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("mIV Lanczos spike timestamps"),
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mIV Lanczos spike timestamp resolve"),
        size: 16,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mIV Lanczos spike timestamp readback"),
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mIV Lanczos spike measured encoder"),
    });
    for iteration in 0..MEASURE_ITERATIONS {
        resampler.encode(
            &mut encoder,
            job,
            Some(&query_set),
            (iteration == 0).then_some(0),
            (iteration + 1 == MEASURE_ITERATIONS).then_some(1),
        );
    }
    encoder.resolve_query_set(&query_set, 0..2, &resolve_buffer, 0);
    encoder.copy_buffer_to_buffer(&resolve_buffer, 0, &read_buffer, 0, 16);
    queue.submit(Some(encoder.finish()));

    map_buffer(device, &read_buffer)?;
    let mapped = read_buffer.slice(..).get_mapped_range();
    let start = u64::from_ne_bytes(mapped[0..8].try_into()?);
    let end = u64::from_ne_bytes(mapped[8..16].try_into()?);
    drop(mapped);
    read_buffer.unmap();
    let elapsed_ns = end.saturating_sub(start) as f64 * f64::from(queue.get_timestamp_period());
    Ok(elapsed_ns / 1_000_000.0 / MEASURE_ITERATIONS as f64)
}

fn read_output_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    job: &LanczosJob,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let [width, height] = job.plan.target_size;
    let bytes_per_row = width * 4;
    let padded_bytes_per_row = bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer_size = u64::from(padded_bytes_per_row) * u64::from(height);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mIV Lanczos spike image readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mIV Lanczos spike image readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &job.output_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    map_buffer(device, &buffer)?;
    let mapped = buffer.slice(..).get_mapped_range();
    let mut pixels = Vec::with_capacity((u64::from(bytes_per_row) * u64::from(height)) as usize);
    for row in mapped.chunks_exact(padded_bytes_per_row as usize) {
        pixels.extend_from_slice(&row[..bytes_per_row as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}

fn map_buffer(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<(), Box<dyn Error>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv()??;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct Difference {
    mae: f64,
    rmse: f64,
    max_abs: u8,
    gt1_percent: f64,
    gt2_percent: f64,
}

fn compare_with_reference(
    gpu_rgba: &[u8],
    size: [u32; 2],
    reference_path: &Path,
) -> Result<Difference, Box<dyn Error>> {
    let reference = image::open(reference_path)?.to_luma8();
    if reference.dimensions() != (size[0], size[1]) {
        return Err(format!(
            "reference {} is {:?}, expected {:?}",
            reference_path.display(),
            reference.dimensions(),
            size
        )
        .into());
    }
    let count = u64::from(size[0]) * u64::from(size[1]);
    if gpu_rgba.len() as u64 != count * 4 {
        return Err("GPU readback length does not match target size".into());
    }

    let mut abs_sum = 0_u64;
    let mut squared_sum = 0_u64;
    let mut max_abs = 0_u8;
    let mut gt1 = 0_u64;
    let mut gt2 = 0_u64;
    for (rgba, reference) in gpu_rgba.chunks_exact(4).zip(reference.as_raw()) {
        let difference = rgba[0].abs_diff(*reference);
        abs_sum += u64::from(difference);
        squared_sum += u64::from(difference) * u64::from(difference);
        max_abs = max_abs.max(difference);
        gt1 += u64::from(difference > 1);
        gt2 += u64::from(difference > 2);
    }
    Ok(Difference {
        mae: abs_sum as f64 / count as f64,
        rmse: (squared_sum as f64 / count as f64).sqrt(),
        max_abs,
        gt1_percent: gt1 as f64 * 100.0 / count as f64,
        gt2_percent: gt2 as f64 * 100.0 / count as f64,
    })
}
