//! Test-only host for the production preprocessing module, including its tests.
//! Run from the repository root so .cargo/config.toml/rustflags also agree.
#[path = "../../../../src/depth_preprocess.rs"]
mod depth_preprocess;

// Allocation and scratch-buffer release are timed, so match the application.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
