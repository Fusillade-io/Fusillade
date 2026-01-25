//! Memory monitoring and estimation utilities for Fusillade.
//!
//! Provides functions for:
//! - Estimating maximum worker capacity based on available RAM
//! - Monitoring current RSS (Resident Set Size)
//! - Pre-flight checks before test execution

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use sysinfo::System;

/// Per-worker memory overhead in bytes (based on benchmarks: ~12 KB)
pub const PER_WORKER_BYTES: usize = 12 * 1024;

/// Base engine overhead in bytes (based on benchmarks: ~30 MB)
pub const BASE_OVERHEAD_BYTES: usize = 30 * 1024 * 1024;

/// Memory usage threshold for warnings (85%)
pub const WARN_THRESHOLD_PERCENT: f64 = 0.85;

/// Memory usage threshold for graceful shutdown (95%)
pub const CRITICAL_THRESHOLD_PERCENT: f64 = 0.95;

/// Information about system memory state
#[derive(Debug, Clone)]
pub struct MemoryInfo {
    /// Available memory in bytes
    pub available_bytes: u64,
    /// Total system memory in bytes
    pub total_bytes: u64,
    /// Current process RSS in bytes
    pub current_rss: u64,
}

impl MemoryInfo {
    /// Get current memory information from the system
    pub fn current() -> Self {
        let mut sys = System::new();
        sys.refresh_memory();

        let total = sys.total_memory();
        let available = sys.available_memory();

        // Get current process RSS
        let pid = sysinfo::get_current_pid().ok();
        let current_rss = pid
            .and_then(|p| {
                // Refresh only current process
                sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[p]), true);
                sys.process(p).map(|proc| proc.memory())
            })
            .unwrap_or(0);

        Self {
            available_bytes: available,
            total_bytes: total,
            current_rss,
        }
    }

    /// Get memory usage as a percentage of total
    pub fn usage_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        let used = self.total_bytes.saturating_sub(self.available_bytes);
        (used as f64) / (self.total_bytes as f64)
    }
}

/// Estimate the maximum number of workers that can run given available memory.
///
/// Formula: (available_ram - base_overhead) / per_worker_overhead
pub fn estimate_max_workers(available_bytes: u64) -> usize {
    let available = available_bytes as usize;
    if available <= BASE_OVERHEAD_BYTES {
        return 0;
    }
    (available - BASE_OVERHEAD_BYTES) / PER_WORKER_BYTES
}

/// Estimate memory required for a given number of workers.
///
/// Formula: base_overhead + (workers * per_worker_overhead)
pub fn estimate_memory_for_workers(workers: usize) -> u64 {
    (BASE_OVERHEAD_BYTES + (workers * PER_WORKER_BYTES)) as u64
}

/// Result of pre-flight memory check
#[derive(Debug)]
pub struct PreflightResult {
    /// Whether the requested workers can likely fit in memory
    pub safe: bool,
    /// Estimated maximum workers for available memory
    pub estimated_max: usize,
    /// Requested worker count
    pub requested: usize,
    /// Available memory in bytes
    pub available_bytes: u64,
    /// Estimated memory needed in bytes
    pub estimated_needed: u64,
}

/// Perform a pre-flight memory check.
///
/// Returns information about whether the requested worker count
/// is likely to fit in available memory.
pub fn preflight_check(requested_workers: usize) -> PreflightResult {
    let info = MemoryInfo::current();
    let estimated_max = estimate_max_workers(info.available_bytes);
    let estimated_needed = estimate_memory_for_workers(requested_workers);

    PreflightResult {
        safe: requested_workers <= estimated_max,
        estimated_max,
        requested: requested_workers,
        available_bytes: info.available_bytes,
        estimated_needed,
    }
}

/// Format bytes as human-readable string (e.g., "1.5 GB")
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Memory monitor that runs in a background thread.
///
/// Checks memory usage periodically and calls the provided callback
/// when thresholds are exceeded.
pub struct MemoryMonitor {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MemoryMonitor {
    /// Start a new memory monitor.
    ///
    /// - `check_interval`: How often to check memory (recommended: 5 seconds)
    /// - `on_warning`: Called when memory exceeds WARN_THRESHOLD_PERCENT
    /// - `on_critical`: Called when memory exceeds CRITICAL_THRESHOLD_PERCENT
    pub fn start<W, C>(check_interval: Duration, on_warning: W, on_critical: C) -> Self
    where
        W: Fn(MemoryInfo) + Send + 'static,
        C: Fn(MemoryInfo) + Send + 'static,
    {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_flag.clone();

        let handle = thread::spawn(move || {
            let mut warned = false;

            while !stop_clone.load(Ordering::Relaxed) {
                thread::sleep(check_interval);

                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }

                let info = MemoryInfo::current();
                let usage = info.usage_percent();

                if usage >= CRITICAL_THRESHOLD_PERCENT {
                    on_critical(info);
                } else if usage >= WARN_THRESHOLD_PERCENT && !warned {
                    warned = true;
                    on_warning(info.clone());
                } else if usage < WARN_THRESHOLD_PERCENT {
                    warned = false; // Reset warning state if memory drops
                }
            }
        });

        Self {
            stop_flag,
            handle: Some(handle),
        }
    }

    /// Stop the memory monitor.
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for MemoryMonitor {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =================================================================
    // estimate_max_workers() tests
    // =================================================================

    #[test]
    fn test_estimate_max_workers_with_1gb_ram() {
        // With 1GB available, we should be able to run ~84K workers
        // Formula: (1GB - 30MB base) / 12KB per worker ≈ 84,650
        let one_gb: u64 = 1024 * 1024 * 1024;
        let max = estimate_max_workers(one_gb);

        // Verify the math: (1073741824 - 31457280) / 12288 = 84863
        let expected = (one_gb as usize - BASE_OVERHEAD_BYTES) / PER_WORKER_BYTES;
        assert_eq!(max, expected);
        assert!(max > 80_000, "Should support 80K+ workers with 1GB");
        assert!(max < 90_000, "Should not exceed 90K workers with 1GB");
    }

    #[test]
    fn test_estimate_max_workers_with_16gb_ram() {
        // Typical developer machine with 16GB
        let sixteen_gb: u64 = 16 * 1024 * 1024 * 1024;
        let max = estimate_max_workers(sixteen_gb);

        // Should be able to run ~1.3M workers
        assert!(max > 1_300_000, "16GB should support 1.3M+ workers");
        assert!(max < 1_400_000, "16GB should not exceed 1.4M workers");
    }

    #[test]
    fn test_estimate_max_workers_returns_zero_when_ram_below_base_overhead() {
        // If available RAM is less than base overhead (30MB), no workers can run
        let only_20mb = 20 * 1024 * 1024;
        assert_eq!(estimate_max_workers(only_20mb), 0);

        // Edge case: exact base overhead should also return 0
        assert_eq!(estimate_max_workers(BASE_OVERHEAD_BYTES as u64), 0);
    }

    #[test]
    fn test_estimate_max_workers_with_minimal_ram() {
        // Just enough for base + 1 worker
        let minimal = BASE_OVERHEAD_BYTES + PER_WORKER_BYTES;
        assert_eq!(estimate_max_workers(minimal as u64), 1);

        // Just enough for base + 10 workers
        let for_ten = BASE_OVERHEAD_BYTES + (10 * PER_WORKER_BYTES);
        assert_eq!(estimate_max_workers(for_ten as u64), 10);
    }

    // =================================================================
    // estimate_memory_for_workers() tests
    // =================================================================

    #[test]
    fn test_estimate_memory_for_workers_formula() {
        // Verify the formula: base + (workers * per_worker)
        let mem_0 = estimate_memory_for_workers(0);
        assert_eq!(mem_0, BASE_OVERHEAD_BYTES as u64, "0 workers = base only");

        let mem_1000 = estimate_memory_for_workers(1000);
        let expected_1000 = BASE_OVERHEAD_BYTES as u64 + (1000 * PER_WORKER_BYTES as u64);
        assert_eq!(mem_1000, expected_1000);

        // ~42MB for 1000 workers
        assert!(mem_1000 > 40 * 1024 * 1024, "1000 workers need 40MB+");
        assert!(mem_1000 < 50 * 1024 * 1024, "1000 workers need <50MB");
    }

    #[test]
    fn test_estimate_memory_for_workers_at_scale() {
        // 100K workers should need ~1.2GB
        let mem_100k = estimate_memory_for_workers(100_000);
        let gb = 1024 * 1024 * 1024;
        assert!(mem_100k > gb, "100K workers need >1GB");
        assert!(mem_100k < 2 * gb, "100K workers need <2GB");

        // 1M workers should need ~12GB
        let mem_1m = estimate_memory_for_workers(1_000_000);
        assert!(mem_1m > 11 * gb, "1M workers need >11GB");
        assert!(mem_1m < 13 * gb, "1M workers need <13GB");
    }

    // =================================================================
    // MemoryInfo tests
    // =================================================================

    #[test]
    fn test_memory_info_current_returns_valid_data() {
        let info = MemoryInfo::current();

        // Must have total memory (all systems have RAM)
        assert!(info.total_bytes > 0, "System must have total memory");

        // Available must be <= total (can't have more available than exists)
        assert!(
            info.available_bytes <= info.total_bytes,
            "Available ({}) cannot exceed total ({})",
            info.available_bytes,
            info.total_bytes
        );

        // Sanity check: at least 100MB total (any modern system)
        assert!(
            info.total_bytes > 100 * 1024 * 1024,
            "System should have >100MB RAM"
        );
    }

    #[test]
    fn test_memory_usage_percent_calculation() {
        // 80% used (200 available out of 1000)
        let info_80 = MemoryInfo {
            total_bytes: 1000,
            available_bytes: 200,
            current_rss: 0,
        };
        assert!((info_80.usage_percent() - 0.8).abs() < 0.001);

        // 0% used (all available)
        let info_0 = MemoryInfo {
            total_bytes: 1000,
            available_bytes: 1000,
            current_rss: 0,
        };
        assert!(info_0.usage_percent() < 0.001);

        // 100% used (nothing available)
        let info_100 = MemoryInfo {
            total_bytes: 1000,
            available_bytes: 0,
            current_rss: 0,
        };
        assert!((info_100.usage_percent() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_memory_usage_percent_handles_zero_total() {
        // Edge case: don't divide by zero
        let info = MemoryInfo {
            total_bytes: 0,
            available_bytes: 0,
            current_rss: 0,
        };
        assert_eq!(info.usage_percent(), 0.0);
    }

    // =================================================================
    // preflight_check() tests
    // =================================================================

    #[test]
    fn test_preflight_check_low_worker_count_is_safe() {
        // 100 workers should be safe on any system
        let result = preflight_check(100);

        assert!(result.safe, "100 workers must be safe");
        assert_eq!(result.requested, 100);
        assert!(result.estimated_max > 100, "Max should exceed 100");
        assert!(
            result.estimated_needed < result.available_bytes,
            "Needed memory should be less than available"
        );
    }

    #[test]
    fn test_preflight_check_extreme_worker_count_is_unsafe() {
        // 100M workers is unsafe on any current hardware
        let result = preflight_check(100_000_000);

        assert!(!result.safe, "100M workers must be unsafe");
        assert_eq!(result.requested, 100_000_000);
        assert!(
            result.estimated_needed > result.available_bytes,
            "Needed memory should exceed available"
        );
    }

    #[test]
    fn test_preflight_check_returns_consistent_estimates() {
        // Same input should give consistent outputs
        let result1 = preflight_check(10_000);
        let result2 = preflight_check(10_000);

        assert_eq!(result1.requested, result2.requested);
        assert_eq!(result1.estimated_needed, result2.estimated_needed);
        // Note: available_bytes may vary slightly between calls
    }

    // =================================================================
    // format_bytes() tests
    // =================================================================

    #[test]
    fn test_format_bytes_all_units() {
        // Bytes
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1023), "1023 B");

        // Kilobytes
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");

        // Megabytes
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1572864), "1.5 MB");

        // Gigabytes
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_bytes(1610612736), "1.5 GB");
    }

    #[test]
    fn test_format_bytes_large_values() {
        // 100 GB
        assert_eq!(format_bytes(100 * 1024 * 1024 * 1024), "100.0 GB");

        // Edge: boundary between units
        assert_eq!(format_bytes(1024 * 1024 - 1), "1024.0 KB"); // Just under 1MB
    }
}
