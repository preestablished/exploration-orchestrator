# M5 Run Manifest

updated_at_utc: 2026-07-09T23:00:43Z
commit: af8b2dd1edf009295b33ddf4588724bc987269d7
host: infra-control
rustc: rustc 1.96.1 (31fca3adb 2026-06-26)
start_utc: 2026-07-08T23:00:27Z
end_utc: 2026-07-09T23:00:43Z
elapsed_wall_seconds: 86416
config_hash: a4e431d4e1a528ad60e06647e63bbf313904e112b8e954794f2608bf53ee71eb
gc_every_commits: 4
rss_tolerance_percent: 50
rss_warmup_samples: 120

| Lane | Duration seconds | K | Seed | Fault seed | GC every commits | Evidence |
|---|---:|---:|---:|---:|---:|---|
| 24h | 86400 | 64 | 24069 | 1024369 | 4 | soak-24h.txt |

Fault settings: deterministic latency base=1 jitter=3 plus one-shot Unavailable on hypervisor:run, scorer:score_batch, store:put_metadata, synth:propose_bursts, observatory:emit.
Fake snapshot retention: post-commit every 4 commits; final retention asserts live refs equal committed refs.
Tier-2 persistence/kill hooks: not used in this lane.
