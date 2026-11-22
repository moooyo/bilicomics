# Launcher timing replay: six previously recorded Scribe starts

This read-only `test-env` replay used measured cases `b01` through `b06` of [the November 21 native Library comparison](../../physical/shelf-render-20261121/README.md#device-results). It excludes the warmups and the control arm. No new application, device or account was started. All six `launcher-stages.tsv` files and their six `worker.pid` files matched the corresponding members of `lib-p-b4a63eed-evidence.tar.gz` byte for byte.

## Measurements

All columns are milliseconds on a single launcher worker's `/proc/uptime` clock (10 ms resolution). The first four columns reproduce the [published coarse intervals](../../physical/shelf-render-20261121/decision.json). Each read column shows **go to observed ready / observed ready to reaped**. `U` equals sample begin to takeover begin minus the three go-to-ready intervals in that case. `Other` subtracts the three ready-to-reaped intervals from `U`. Medians are taken **after** calculating each case; the median row is not an additive timeline.

| Case | Logging ready to sample begin | Display sample | Sample ready to takeover begin | Takeover | Display read | Portrait publication | Cover capability | U | Other |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| b01 | 2000 | 470 | 370 | 100 | 410 / 10 | 110 / 10 | 90 / 30 | 230 | 180 |
| b02 | 1860 | 440 | 370 | 100 | 380 / 10 | 110 / 10 | 90 / 40 | 230 | 170 |
| b03 | 1930 | 490 | 330 | 110 | 410 / 30 | 120 / 10 | 80 / 10 | 210 | 160 |
| b04 | 1910 | 490 | 330 | 120 | 440 / 10 | 120 / 10 | 80 / 10 | 180 | 150 |
| b05 | 1950 | 410 | 340 | 100 | 350 / 10 | 110 / 10 | 80 / 10 | 210 | 180 |
| b06 | 1950 | 490 | 380 | 230 | 420 / 10 | 120 / 10 | 90 / 0 | 240 | 220 |
| Median | 1940 | 480 | 355 | 105 | 410 / 10 | 115 / 10 | 85 / 10 | 220 | 175 |

The display read is the longest named transaction. The 355 ms following display-sample readiness is **not** solely the cover query: the latter's go-to-ready median is 85 ms. `U` includes reaping; `Other` still mixes setup, publication, transitions and supervisor scheduling. Neither identifies removable work. A zero interval means the two marks fell within the clock's resolution. Do not mix this clock with button-release or Rust timestamps, credit the protected two-second observer settle as savings, or extrapolate from this instrumented offline comparison to ordinary account or optical timing.

## Reproduce from the closed batch

From the retained project directory `/var/tmp/bilicomics-shelf-pair-20261121.yUSt9pqR` on `test-env`, run the existing [launcher projection](../../physical/shelf-render-20261121/project_launcher_stages.py) for each case. Its inputs are limited to that case's two files named above; the full launcher log, device transport and lease are unnecessary.

```sh
for case in b01 b02 b03 b04 b05 b06; do
    python3 -B project_launcher_stages.py "lib-p-b4a63eed/$case"
done
```
