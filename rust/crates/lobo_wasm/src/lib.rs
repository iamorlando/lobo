//! Adapter-driven browser feeds and GPU-only chart rendering.
#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(any(target_arch = "wasm32", test))]
mod camera;
#[cfg(any(target_arch = "wasm32", test))]
mod deferred;
#[cfg(target_arch = "wasm32")]
mod loading;
#[cfg(target_arch = "wasm32")]
mod palette;
pub mod presentation;
#[cfg(target_arch = "wasm32")]
mod renderer;

#[cfg(target_arch = "wasm32")]
mod bars;
