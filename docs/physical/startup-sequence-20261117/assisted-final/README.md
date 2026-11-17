# Assisted sleep, wake and touch: scoped final evidence

Fresh batch `startup-sleep-20261117-06a7c018` retained the same Reader process and
page 18 through the operator's physical power-button sleep/wake cycle. A later
single physical touch advanced to page 19. Source is `530f1516c332931059c52161b52d64e86488b3f1`,
with ELF `04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59`.
This is the isolated offline demo on Scribe / firmware 5.19.5, Normal mode and Bayer8.

The before checkpoint contains 28 lifecycle records. New same-PID suspend and
resume records are exactly 29 and 30, before the resumed checkpoint's record 35.
Their monotonic ordering, unchanged process birth, 16 identity/settings fields
and seven source-anchor fields are preserved in [historical-cycle.json](historical-cycle.json).
No live suspended checkpoint was executed. The small raw JSONL retains exact
original bytes for records 29-31; its fixture titles are static demonstration
content. It is not the full lifecycle, run state or an account trace.

The actual assisted start used `--exit-after-ms 480000 --init-timeout 90
--session-timeout 600` and **did not enable performance tracing**. Lifecycle
logging supplied the evidence. Native resume reports pixels written and refresh
requested, while physical panel completion remains false. No per-frame epoch,
driver-ack timing, button latency, optical completion or deep-sleep duration is
inferred. The operator's requested five-second pause is an observation protocol,
not a duration measured by the application event interval.

The awake before/resumed PNG and visible-memory hashes are identical. Only one
copy of that retained page is included, plus the page-19 capture. These are
original application framebuffer-memory PNGs, not photographs or native Home
captures. Geometry and conversion sidecars preserve the original 1860x2480
geometry, framebuffer rotation 3, reader rotation 0 and verified Y8 mode.

| Capture | Original memory PNG | Safe sidecar |
| --- | --- | --- |
| Resumed page 18; identical to before sleep | [resumed.png](images/resumed.png) | [resumed.json](images/resumed.json) |
| Page 19 after the actual touch | [after-input.png](images/after-input.png) | [after-input.json](images/after-input.json) |

Stop was a planned `operator-stop`, launcher status 143. Saved native service
and process restoration passed; all 28 owned PID/birth identities were gone.
Transport closure passed at `2026-11-17T11:28:12.125464+00:00`; exact owner/token
checked release followed at **2026-11-17T11:28:12.165059+00:00**
(19:28:12.165059 Asia/Shanghai). See [finalization.json](finalization.json).

The two stored operator observations remain unchanged. The later user statement
that native interface and touch were restored is separately recorded as a
derived post-release observation with unknown exact event time, not a forged
harness confirmation. The earlier `a1067e98` result remains incomplete and
unchanged. See [operator-observations.json](operator-observations.json).

This export performs only stored host-file analysis after return. It contains
no full run.json, private connection data, token files, database, ELF, package,
archive, raw framebuffer or native Home content. The fixed allowlist manifest
preserves original source hashes and each projection/copy transformation.
It excludes itself and its detached SHA file to avoid a circular hash.
No additional device, lease, probe, build or test operation was performed.
