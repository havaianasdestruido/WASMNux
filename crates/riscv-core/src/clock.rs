//! Host clocks are injected at the WASM boundary; native tests use the OS clock.
#[cfg(target_arch = "wasm32")]
extern "C" {
    fn wnx_wall_time_ns() -> f64;
}
pub fn wall_time_nanos() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { wnx_wall_time_ns() }.max(0.0) as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}
