# Performance — kernel strategy

Rugus is **pure Rust, no C, no C++, no FFI to vendor blobs**. Performance is delivered by combining Rust's zero-cost abstractions with:

- `core::arch::asm!` for surgical inline assembly when the compiler cannot express the exact instruction sequence (e.g. PendSV context switch tail).
- `#[naked]` functions for ABI-controlled trampolines and interrupt frames.
- `#[link_section = ".some_section"]` for placing hot tables in ITCM / SRAM1 / non-cacheable regions as appropriate.
- Compile-time **LUTs** (`const` arrays computed in `build.rs` or `const fn`) instead of runtime initialization for cycle-critical paths.

This document is a forward-looking scaffold. Concrete benchmarks land per milestone alongside their feature.

## Boundaries

| Boundary | Strategy |
|---|---|
| C / C++ source | **forbidden** in the workspace |
| Pre-built C objects | **forbidden** |
| `bindgen` to vendor headers | **forbidden** |
| `cc` / `cmake` crate builds | **forbidden** |
| Inline assembly via `core::arch::asm!` | **allowed**, must compile under `#![no_std]` |
| `#[naked]` functions | **allowed**, must be `extern "C"` and `unsafe` |
| Custom linker sections | **allowed**, must be defined in the example's `memory.x` |
| Compile-time tables | **preferred** over runtime caches |

A `clippy` lint and a `cargo deny` rule should be added in a follow-up to enforce the "no FFI" boundary mechanically.

## Targets

The numeric targets in `docs/ROADMAP.md` (post-G2 metrics table) are the binding contract. They are restated here with the implementation strategy that gets us there:

| Metric | Cortex-M7 @ 216 MHz | Cortex-A53 @ 1.5 GHz | Strategy |
|---|---|---|---|
| Boot → `Arch::init` complete | < 200 ms | < 500 ms | PLL config in `build.rs` const; no dynamic clock tree walk. |
| Syscall avg latency | < 5 µs | < 1 µs | SVC handler in ITCM, dispatch via dense LUT keyed by syscall id. |
| IRQ → handler | < 2 µs | < 500 ns | Vectored interrupts; `#[interrupt]` body inlined when feasible. |
| Context switch | < 3 µs | < 1 µs | PendSV in `#[naked]` ASM, FP context lazy-saved. |

## Critical-path techniques

### PendSV in `#[naked]` ASM

Context switch is the only path where Rust's stack-frame setup cost is unacceptable. The existing implementation in `crates/rugus-arch-cortex-m::switch` is a `#[naked] extern "C"` function that handles the entire prolog/epilog manually, with `psp`/`msp` swap and optional FPU save. See `docs/HAL_TRAITS.md` for the ABI contract.

### Syscall dispatch with const LUT

The SVC handler reads the syscall id from `r0`, indexes into a const `[fn(...) -> Errno; N]` table generated from `rugus-core::syscall::ABI_V0_1`, and tail-calls. No `match` arm enumeration, no virtual dispatch, no allocation. The table lives in flash; the SVC handler lives in ITCM via `#[link_section = ".itcm.text"]`.

### Hot-path tables in non-cacheable SRAM

Ethernet DMA descriptors live in a 16 KiB region at `0x2007_8000` marked Normal-Non-Cacheable by an MPU region (see `crates/rugus-hal-stm32f7::cache::configure_eth_mpu`). This avoids `clean_invalidate` cycles per descriptor write and is mandatory for correctness on Cortex-M7 with D-cache enabled.

### LUT-driven CSPRNG / hash bootstrap

`rugus-crypto::software` will move bootstrap constants (SHA-256 K-table, AES S-box) into `const` arrays placed in flash. Runtime allocations are forbidden in `no_std` builds (`rugus-crypto::no-std` feature gate already enforces this).

## Anti-patterns to avoid

- `Box::new` in any hot path (heap allocator is a linked list; allocation cost is unbounded).
- `String` / `Vec` for log formatting (use `defmt`'s zero-cost format strings).
- Runtime trait objects for HAL drivers (use generic monomorphization).
- Calling `cortex_m::interrupt::free` from a hot loop (it disables all interrupts; use single-writer ownership instead).

## Next benchmarks

These have **not** been measured yet and are tracked as future work:

- Context switch latency under load (G2 deliverable, measurement pending).
- ETH RX → smoltcp → socket recv end-to-end latency (G4 follow-up).
- TLS 1.3 handshake CPU cycles on F769 software backend vs F769 CRYP HW (G4 / G5 stretch).

Each measurement when added should land with:

1. A reproducible script under `tools/bench-*.sh`.
2. Raw numbers and the commit hash they were taken on.
3. A note in this file explaining the technique used to reach (or miss) the target.
