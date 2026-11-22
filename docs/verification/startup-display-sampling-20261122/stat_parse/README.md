# Stat parsing: a measurable host improvement, not a startup result

Both arms parsed the same **310-byte**, **50-tail-field** snapshot of a live Linux process. Both ran in one `rustc 1.99.1` x86-64 release executable, using the application's locked `anyhow 1.0.104`. A collected the fields into a vector as the product does; B advanced an iterator to the last field the guard uses. No system call, credential read, driver operation or application launch was inside the timed loop. The proposed change is [only an excerpt](iterator.inc); it is not in the product.

| Elapsed time per parsed record | Nanoseconds |
| --- | ---: |
| A median | 215.594 |
| B median | 101.748 |
| Median paired B minus A | −113.369 |
| Pair range | −144.838 to −110.633 |
| AB / BA subgroup median B minus A | −113.757 / −113.369 |

The iterator won **12/12** pairs. The [predeclared schedule](PLAN.md) ran in full: two excluded warmups, then 12 pairs, each arm parsing the same bytes 262,144 times. The [individual block times](measured.txt) are in nanoseconds. This is a warm, no-I/O, single-record-shape result on the host. It does not measure the full time to read and validate a live identity; it is not evidence of the improvement, or a bound on the improvement, on a Kindle.

All five test groups passed on `test-env`. They compared accepted values and rejection behavior across an actual `/proc/self/stat` snapshot, odd `comm` delimiters, short rows, malformed IDs and birth values, dead states, PID mismatches, whole-record UTF-8 and the exact 64 KiB limit. An initial fixture build had a slice-type mismatch. After it was fixed, the synthetic groups passed but the first toolchain namespace run lacked a positive process group or session and failed the required live-identity check. Running in a fresh Linux session provided a valid live fixture; no guard or assertion was weakened. Both corrections preceded the only timed series.

The separate [stock host experiment](../HOST_PLAN.md) counted 288 `Writer::process` calls on each of its three Scribe automatic-profile captures and 216 on each of its three PW6 fixed-profile captures. Multiplying those counts by the 113.369 ns paired parser difference gives an arithmetic scale of **32.65 µs** and **24.49 µs**, respectively, on the host. This is neither an upper bound nor a measured whole-capture or Kindle saving. It says nothing about file access, helper startup, system scheduling or the ARM implementation. Do not subtract it from the historical display-sampling envelope or replace any live owner recheck with cached identity.
