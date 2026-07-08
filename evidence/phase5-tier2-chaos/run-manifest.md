# Evidence run manifest
date: 2026-07-08T14:54:17Z
host: Linux infra-control 6.8.0-124-generic #124-Ubuntu SMP PREEMPT_DYNAMIC Tue May 26 13:00:45 UTC 2026 x86_64 x86_64 x86_64 GNU/Linux
toolchain: rustc 1.96.1 (31fca3adb 2026-06-26)
commit: f381805 (post-review fixes; the lane was stamped with the prior HEAD 39ce86e at start but ran the fixed code)
seeds: 5 (0x5EED + i*7)
lattice: full (all 11 CrashPoints) + torn wal-append + torn ckpt-put
random kills per seed: 3
kill quota asserted: >= 50
