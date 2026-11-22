# Library configuration admission: host result

## Final candidate

**The 80,710-byte descriptor-corrected candidate passed both fixed host gates.** The [final raw result](host-result.json) comes from a new, complete two-block comparison against the original 80,185-byte baseline. No timing sample, control result or warmup was carried forward from the earlier pilot. The shell and the host clock observed only Library admission through the detached worker's successful preflight; native card release, Shelf rendering and SEND were not measured here.

| Final host block | A median (ms) | B median (ms) | Median paired B−A (ms) | B faster | AB / BA median paired delta (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 9.689 | 6.638 | −3.186 | 6/6 | −2.895 / −4.365 |
| 2 | 9.659 | 6.438 | −2.838 | 6/6 | −3.264 / −2.437 |

All ten independent call-count controls passed. In the valid case the complete checked command sequence executed once per process in A, and only once in the worker in B. With a nonexecutable application, unknown profile, malformed firmware or unaccepted firmware the caller still received status 1 and no worker pre-lock success receipt; neither logs/fonts directories nor the display lock were created. All three harness tests passed. The Ubuntu host and networking isolation matched the pilot; `test-env` remained shared and no other jobs were paused. The one-, five- and fifteen-minute load averages at the start and end were 2.45 / 1.95 / 1.36. No extra blocks were added. Temporary fixtures were removed.

## Pre-review pilot (not the final candidate)

Both preregistered host blocks passed for the initial, 80,642-byte candidate only. A read the profile, parsed firmware, matched the allowlist and prepared the log/font directories in both the Library parent and its detached worker. B performed each operation only once, in the independent worker before its display lock. Four refused-configuration controls per arm returned status 1 without an admission receipt, new log/font directories, or a display lock. The subsequent descriptor-handling correction changes the candidate; this evidence cannot qualify that revision.

| Host block | A median (ms) | B median (ms) | Median paired B−A (ms) | B faster | AB / BA median paired delta (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 10.628 | 7.180 | −3.275 | 6/6 | −3.174 / −3.375 |
| 2 | 10.994 | 7.359 | −3.629 | 6/6 | −3.623 / −3.635 |

The clock ran from host launch of the real shell `--library` entry to the worker's observed pre-lock receipt. A used the retained 80,185-byte launcher matching the installed source; B used the 80,642-byte candidate. The `test-env` x86-64 Ubuntu runtime used `/usr/bin/dash`, Python 3.12.3 and isolated networking. The two excluded warmups per block, all 24 measured activations, all ten separate control visits and the original timing values are recorded in [host-prereview-result.json](host-prereview-result.json). The [protocol](HOST_PROTOCOL.md) was fixed before measuring. Three analysis/fence tests also passed. Temporary fixtures were removed.

Both order groups favor B in both blocks. These small samples describe the fixture, not Kindle latency: shared host load, warm files, shell executable and framebuffer substitute differ from a device. For an end-to-end decision, compare the normal native Library card's button release to the first complete normal Shelf software render, then its accepted SEND separately. Neither was observed here.
