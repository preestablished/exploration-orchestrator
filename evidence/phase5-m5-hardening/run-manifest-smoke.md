# M5 Run Manifest

updated_at_utc: 2026-07-10T02:49:29Z
commit: d38fce153bb9718e193956b68755547b57aeb7a3
host: infra-control
rustc: rustc 1.96.1 (31fca3adb 2026-06-26)
start_utc: 2026-07-10T02:49:17Z
end_utc: 2026-07-10T02:49:29Z
elapsed_wall_seconds: 12
config_hash: 92e96be9d3244b110e530e38a113c4132bcfb38fcfdaf4b4f60f128835eea584
gc_every_commits: 4
rss_tolerance_percent: 50
rss_warmup_samples: 2
rss_required: 0
rss_min_evaluated_samples: 4

| Lane | Duration seconds | K | Seed | Fault seed | GC every commits | Evidence |
|---|---:|---:|---:|---:|---:|---|
| smoke | 10 | 64 | 24069 | 1024369 | 4 | soak-smoke.txt |

Fault settings: deterministic latency base=1 jitter=3 charged by async adapters for hypervisor/scorer/store/synth; one-shot Unavailable on hypervisor:run, scorer:score_batch, store:put_metadata, synth:propose_bursts, observatory:emit.
Fake snapshot retention: post-commit every 4 commits; final retention asserts live refs equal committed refs.
Tier-2 persistence/kill hooks: not used in this lane.
