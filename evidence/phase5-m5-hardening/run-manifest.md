# M5 Run Manifest

updated_at_utc: 2026-07-08T18:49:43Z
commit: 8d4536100761d5172b2311ebefaa06486d6da06c
host: infra-control
rustc: rustc 1.96.1 (31fca3adb 2026-06-26)
start_utc: 2026-07-08T18:49:41Z
end_utc: 2026-07-08T18:49:43Z
elapsed_wall_seconds: 2
config_hash: d763e3f2852681944519911777cc28ef22845fe0565e78f9f28f14c08c109581
gc_every_commits: 64
rss_tolerance_percent: 50
rss_warmup_samples: 2

| Lane | Duration seconds | K | Seed | Fault seed | GC every commits | Evidence |
|---|---:|---:|---:|---:|---:|---|
| smoke | 2 | 64 | 24069 | 1024369 | 64 | soak-smoke.txt |

Fault settings: deterministic latency base=1 jitter=3 plus one-shot Unavailable on hypervisor:run, scorer:score_batch, store:put_metadata, synth:propose_bursts, observatory:emit.
Fake snapshot retention: post-commit every 64 commits; final retention asserts live refs equal committed refs.
Tier-2 persistence/kill hooks: not used in this lane.
