# 0008 — The idle memory budget excludes GPU driver libraries

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** P8 (performance budgets before v1.0)

## Context
PLAN.md set a 200MB idle memory budget, summed over all processes. After
[ADR 0007](0007-webkit-dmabuf-renderer-off.md) the test machine (Intel Arc + NVIDIA
RTX 500, Ubuntu 26.04) measured 206–207MB PSS. 32MB of that is GPU driver code
(Mesa, NVIDIA and libglvnd libraries, and LLVM, which Mesa loads) mapped into the app
and web processes. Which drivers load, and how big they are, depends on the machine's
graphics hardware rather than on IronCall: forcing Mesa only saved 22MB.

## Options considered
1. **Measure only on the reference box** (4-core, 8GB, Ubuntu 24.04). Fair, but none
   is to hand, and every machine's number would still differ by driver.
2. **Exclude GPU driver libraries from the budget (chosen).** The budget then measures
   what IronCall controls, and the number is comparable across machines.

## Decision
Option 2. Idle memory is PSS summed over IronCall's process tree, minus mappings of
GPU driver libraries. `tests/perf/idle_memory.py` (`just idle-memory`) measures it and
lists what counts as a driver library. On the test machine it is 174–181MB.

## Consequences
- The budget no longer catches growth inside GPU drivers. That's acceptable: IronCall
  can't change it, and turning off the DMA-BUF renderer is the one setting that did.
- The other memory budget (under 400MB with a 100MB body) still counts everything.
