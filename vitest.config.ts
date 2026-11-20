import { defineConfig } from 'vitest/config';

/**
 * `maxWorkers: 1` is deliberate, not a leftover.
 *
 * Every test in this suite instantiates a real RV64GC machine in WASM and boots a genuine guest
 * kernel, so a single file can hold a ~128 MiB guest RAM plus a full-snapshot staging buffer.
 * Peak RSS for the full suite is already ~2.2 GiB with one worker; letting vitest fan out to
 * `availableParallelism() - 1` workers multiplied that and, more importantly, made results
 * depend on the host: the throughput measurements in `benchmarks.test.ts` degrade by ~25% when
 * sibling workers compete for CPU (measured 24.8 MIPS alone vs 18.5 MIPS under contention,
 * which is what broke CI on a 4-core runner).
 *
 * Serial execution is also faster in wall-clock here, because the guest boots are CPU-bound
 * and the suite is dominated by two long boots (Debian ~80 s) rather than by parallelism.
 */
export default defineConfig({
  test: {
    include: ['tests/unit/**/*.test.ts', 'tests/integration/**/*.test.ts'],
    environment: 'node',
    testTimeout: 120000,
    hookTimeout: 120000,
    maxWorkers: 1,
  },
});
