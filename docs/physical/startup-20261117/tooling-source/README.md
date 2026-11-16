# Frozen controller source supplement

These five original source files supplement the frozen 22-file physical
evidence export. They preserve the actual controller, native recovery fragment,
host fixtures, and placeholder configuration needed to review the R2 protocol.
No device operation or fixture test was repeated when these files were copied.

Their byte identities are already pinned in
[`sources/frozen-tools-r2.json`](../sources/frozen-tools-r2.json). The original
physical export manifest remains unchanged; the final repository artifact
check verifies this supplement against those five pinned entries.

The protocol, analyzer, and clock source remain under
[`tools/`](../tools/MEASUREMENT_PLAN.md). `config.example.json` contains
placeholders rather than the private executed configuration. It is not a
ready-to-run deployment. Reproduction requires separately built and selected
packages, a closed offline demo fixture, authenticated private transport, and
a new exact shared-device lease. Do not reuse the completed run or its tokens.

The 49 passing host fixtures and their source identities are recorded in the
frozen R2 manifest; this supplement adds no new execution or hardware claim.
