Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Export only enumerated, already stored text evidence. Never start a verifier.
$repositoryRoot = 'D:\Code\bilicomics'
$storedRoot = Join-Path $repositoryRoot 'artifacts\verification\native-library-entry-20261117\software'
$bundleRoot = Join-Path $repositoryRoot 'docs\verification\native-library-entry-20261117'
$remoteRoot = '/root/bilicomics-native-library-entry-20261117-01a1d11a-r1'
$historicalRoot = '/root/bilicomics-startup-sequence-20261117-01a1d11a-r1/checks-r3'
$exporterPath = Join-Path $storedRoot 'export-software-r1.ps1'
$utf8 = [System.Text.UTF8Encoding]::new($false)
$rows = [System.Collections.Generic.List[object]]::new()

if (Test-Path -LiteralPath $bundleRoot) {
    throw 'The evidence destination must be new.'
}
if ([System.IO.Path]::GetFullPath($bundleRoot) -ne 'D:\Code\bilicomics\docs\verification\native-library-entry-20261117') {
    throw 'The evidence destination is outside the fixed export path.'
}

function Read-StoredJson([string]$relative) {
    return Get-Content -LiteralPath (Join-Path $storedRoot $relative) -Raw | ConvertFrom-Json -AsHashtable
}

function File-Identity([string]$path) {
    return [ordered]@{
        bytes = ([System.IO.FileInfo]::new($path)).Length
        sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

function Write-Utf8([string]$path, [string]$text) {
    [System.IO.File]::WriteAllText($path, $text.Replace("`r`n", "`n"), $utf8)
}

function Write-Json([string]$path, [object]$value) {
    Write-Utf8 $path (($value | ConvertTo-Json -Depth 100) + "`n")
}

function Add-Raw([string]$sourceRelative, [string]$destinationRelative, [string]$originalRecord) {
    $sourcePath = Join-Path $storedRoot $sourceRelative
    $destinationPath = Join-Path $bundleRoot $destinationRelative
    $parent = Split-Path -Parent $destinationPath
    if (-not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent | Out-Null
    }
    $sourceIdentity = File-Identity $sourcePath
    [System.IO.File]::Copy($sourcePath, $destinationPath, $false)
    $destinationIdentity = File-Identity $destinationPath
    if ($sourceIdentity.bytes -ne $destinationIdentity.bytes -or $sourceIdentity.sha256 -ne $destinationIdentity.sha256) {
        throw 'A copied record differs from its stored bytes.'
    }
    $transformation = 'none; exact original bytes; destination path relocation only'
    if ($sourceRelative.EndsWith('.log') -and $destinationRelative.EndsWith('.txt')) {
        $transformation = 'none to bytes; destination .log suffix renamed to .txt'
    }
    $rows.Add([ordered]@{
        path = $destinationRelative
        kind = 'raw'
        origin = [ordered]@{
            stored_local_file = $sourcePath
            original_record = $originalRecord
        }
        bytes = $destinationIdentity.bytes
        sha256 = $destinationIdentity.sha256
        transformation = $transformation
    })
}

function Add-Derived([string]$destinationRelative, [string[]]$origins, [string]$transformation) {
    $recordIdentity = File-Identity (Join-Path $bundleRoot $destinationRelative)
    $rows.Add([ordered]@{
        path = $destinationRelative
        kind = 'derived'
        origin = $origins
        bytes = $recordIdentity.bytes
        sha256 = $recordIdentity.sha256
        transformation = $transformation
    })
}

$r1 = Read-StoredJson 'checks-r1/summary.json'
$r2 = Read-StoredJson 'checks-r2/summary.json'
$diagnosis = Read-StoredJson 'manifest-order-diagnosis-r1.json'
$rebundle = Read-StoredJson 'checks-r2/rebundle-provenance.json'
$packageCheck = Read-StoredJson 'checks-r2/package-verification.json'
$packageAudit = Read-StoredJson 'checks-r2/package-entry-audit.json'
$baseBuild = Read-StoredJson 'checks-r2/base-build-summary.json'

New-Item -ItemType Directory -Path $bundleRoot | Out-Null

# These lists are the complete raw-file allowlist; no source directory is copied.
$firstAttempt = @(
    'summary.json',
    'failure.txt',
    'archive-source-inputs.sha256',
    'source-inputs.sha256'
)
foreach ($name in $firstAttempt) {
    Add-Raw "checks-r1/$name" "original/attempt-r1/$name" "$remoteRoot/checks-r1/$name"
}

$secondAttempt = @(
    'summary.json',
    'archive-source-inputs.sha256',
    'source-inputs.sha256',
    'source-inputs-run-suite.sha256',
    'binary-build-source.sha256',
    'base-build-summary.json',
    'base-compiler-profile.json',
    'rebundle-provenance.json',
    'package-entry-audit.json',
    'package-verification.json',
    'package-verification.readelf.txt',
    'package-verification.worker.readelf.txt',
    'selected-arm/bilicomics-kindle-0.1.25-dev.manifest.json'
)
foreach ($name in $secondAttempt) {
    Add-Raw "checks-r2/$name" "original/attempt-r2/$name" "$remoteRoot/checks-r2/$name"
}

$logs = @(
    'toolchain-python',
    'toolchain-readelf',
    'toolchain-setsid',
    'library-entry-lifecycle-17',
    'launcher-full-146',
    'library-entry-package-25',
    'test-startup-package',
    'test-spatial-dither-package',
    'test-native-media-package',
    'shell-syntax-launch',
    'shell-syntax-document-launcher',
    'rebundle-library-entry',
    'strict-package-verification'
)
foreach ($name in $logs) {
    Add-Raw "checks-r2/logs/$name.log" "original/attempt-r2/logs/$name.txt" "$remoteRoot/checks-r2/logs/$name.log"
}

$helpers = @('runner-r1.py', 'run-r1.sh', 'runner-r2.py', 'run-r2.sh')
foreach ($name in $helpers) {
    Add-Raw $name "original/helpers/$name" "$remoteRoot/$name"
}
Add-Raw 'diagnose-manifest-r1.py' 'original/helpers/diagnose-manifest-r1.py' 'Stored helper supplied through stdin to Python on the designated worker'
Add-Raw 'manifest-order-diagnosis-r1.json' 'original/manifest-order-diagnosis-r1.json' "$remoteRoot/manifest-order-diagnosis-r1.json"
Add-Raw 'export-software-r1.ps1' 'original/helpers/export-software-r1.ps1' $exporterPath

$suiteLabels = @(
    'library-entry-lifecycle-17', 'launcher-full-146', 'library-entry-package-25',
    'test-startup-package', 'test-spatial-dither-package', 'test-native-media-package'
)
$suites = foreach ($entry in $r2.steps) {
    if ($entry.label -in $suiteLabels) {
        [ordered]@{
            label = $entry.label
            status = $entry.status
            executed_tests = $entry.executed_tests
            returncode = $entry.returncode
            seconds = $entry.seconds
            actual_command = $entry.command
            log = "original/attempt-r2/logs/$($entry.label).txt"
            original_log_sha256 = $entry.log.sha256
        }
    }
}
$summary = [ordered]@{
    schema_version = 1
    evidence = 'native-library-entry-software'
    status = $r2.status
    passed = $r2.passed
    scope = 'Stored test-env Linux fixtures and static package checks only'
    current_frozen_source = [ordered]@{
        commit = $r2.candidate_commit
        purpose = 'Entry scripts, tests, package sources, and software verification candidate; not a new binary compile commit'
        archive_identity = $r2.source_archive
        archive_payload_exported = $false
        archive_file_count = $r2.archive_source_inputs.files
        git_archive_pax_commit = $r2.archive_source_inputs.pax_commit
        archive_extracted_bytes_identical = $r2.archive_source_inputs.all_extracted_bytes_identical
        archive_source_manifest = 'original/attempt-r2/archive-source-inputs.sha256'
        archive_source_manifest_sha256 = $r2.archive_source_inputs.sha256
    }
    binary_build_inputs = [ordered]@{
        files = $r2.binary_input_provenance.files
        canonical_manifest = 'original/attempt-r2/source-inputs.sha256'
        canonical_sha256 = $r2.binary_input_provenance.source_inputs.sha256
        canonical_serialization = $r2.binary_input_provenance.canonical_serialization
        historical_manifest_copy = 'original/attempt-r2/binary-build-source.sha256'
        canonical_raw_bytes_identical_to_historical_build = $r2.binary_input_provenance.byte_identical
        alternate_manifest = 'original/attempt-r2/source-inputs-run-suite.sha256'
        alternate_serialized_sha256 = $r2.binary_input_provenance.alternate_manifest.sha256
        path_digest_pairs_identical = $r2.binary_input_provenance.both_repository_manifest_path_digest_pairs_identical
        ordering_diagnosis = 'original/manifest-order-diagnosis-r1.json'
        missing_paths = $diagnosis.missing_paths
        additional_paths = $diagnosis.additional_paths
        changed_paths = $diagnosis.changed_paths
    }
    historical_binary_build = [ordered]@{
        source_commit = $baseBuild.candidate_commit
        raw_summary = 'original/attempt-r2/base-build-summary.json'
        summary_original_record = "$historicalRoot/summary.json"
        compiler_profile = 'original/attempt-r2/base-compiler-profile.json'
        compiler_profile_sha256 = $r2.base_build_toolchain_record.sha256
        compiler_profile_original_record = "$historicalRoot/compiler-profile.json"
        application_identity = $r2.selected_arm_binary
        jpeg_worker_identity = $r2.selected_jpeg_worker
        base_archive_identity = $r2.base_archive
        application_and_worker_reused_without_recompilation = $true
        historical_test_counts_included_in_current_totals = $false
        binary_payloads_exported = $false
    }
    current_results = [ordered]@{
        unique_lifecycle_cases = 146
        new_lifecycle_cases_within_full_suite = 17
        unique_package_cases = 51
        unique_cases_total = 197
        executions_total = $r2.executed_unittests_including_targeted_repeat
        additional_targeted_repeat_executions = 17
        repeated_cases_are_included_in_the_full_lifecycle_suite = $true
        suites = @($suites)
        production_shell_syntax_checks = 2
        strict_static_package_checks = $r2.package.strict_checker_checks
        exact_archive_entries = $r2.package.entry_count
        raw_summary = 'original/attempt-r2/summary.json'
        raw_summary_sha256 = (File-Identity (Join-Path $storedRoot 'checks-r2/summary.json')).sha256
    }
    current_package = [ordered]@{
        script_and_packaging_source_commit = $r2.candidate_commit
        archive_identity = $r2.package.archive
        sidecar_identity = $r2.package.sidecar
        sidecar_record = 'original/attempt-r2/selected-arm/bilicomics-kindle-0.1.25-dev.manifest.json'
        archive_payload_exported = $false
        base_entries = $rebundle.base_entry_count
        output_entries = $rebundle.output_entry_count
        added_entries = $rebundle.added_entries
        updated_entries = $rebundle.updated_entries
        retained_old_entries_byte_identical = $rebundle.retained_entries_byte_identical
        old_entry_modes_and_timestamps_preserved = $rebundle.existing_entry_modes_and_timestamps_preserved
        selected_packaging_sources = $rebundle.packaging_sources
        rebundle_current_manifest_serialization_sha256 = $rebundle.source_manifest_sha256
        rebundle_historical_binary_manifest_sha256 = $rebundle.binary_source_manifest_sha256
        provenance = 'original/attempt-r2/rebundle-provenance.json'
        entry_metadata_audit = 'original/attempt-r2/package-entry-audit.json'
        static_checker = 'original/attempt-r2/package-verification.json'
        static_checker_names = @($packageCheck.checks | ForEach-Object { $_.name })
        embedded_application_and_worker_bytes_identical = $packageAudit.embedded_arm_bytes_identical
    }
    preserved_attempts = @(
        [ordered]@{
            name = 'attempt-r1'
            raw_status = $r1.status
            raw_passed = $r1.passed
            raw_error = $r1.error
            executed_tests = $r1.executed_unittests_including_targeted_repeat
            raw_summary = 'original/attempt-r1/summary.json'
            raw_failure = 'original/attempt-r1/failure.txt'
            explanation = 'The verifier compared different manifest serialization orders before running tests; the stored diagnosis establishes 544 identical path/digest pairs. Raw failed fields remain unchanged.'
        },
        [ordered]@{
            name = 'attempt-r2'
            raw_status = $r2.status
            raw_passed = $r2.passed
            raw_summary = 'original/attempt-r2/summary.json'
            explanation = 'A new runner and new output directory used native serialization for the original build fingerprint and retained the alternate mapping comparison. Product sources were unchanged.'
        }
    )
    stored_final_integrity = $r2.final_integrity
    stored_fixture_temporary_entries_remaining = $r2.fixture_temporary_entries_remaining
    selected_verification_tool_identities = $r2.verification_tools
    qualification_limits = [ordered]@{
        new_rust_compilation_in_this_software_run = $r2.rust_compilation_performed
        arm_binary_executed_in_this_software_run = $r2.arm_binary_executed
        physical_device_accessed_in_this_software_run = $r2.physical_device_accessed
        device_verified = $r2.device_verified
        physical_results_included = $false
        performance_results_included = $false
        local_inventory_used_as_qualification = $false
        native_library_behavior_or_physical_acceptance_established = $false
    }
    export_operations = [ordered]@{
        stored_only = $true
        new_tests = $false
        new_builds = $false
        new_application_or_runtime_probes = $false
        new_remote_commands = $false
        new_device_or_lease_operations = $false
    }
}
Write-Json (Join-Path $bundleRoot 'software-summary.json') $summary
Add-Derived 'software-summary.json' @(
    'original/attempt-r1/summary.json', 'original/attempt-r2/summary.json',
    'original/manifest-order-diagnosis-r1.json', 'original/attempt-r2/base-build-summary.json',
    'original/attempt-r2/rebundle-provenance.json', 'original/attempt-r2/package-entry-audit.json',
    'original/attempt-r2/package-verification.json'
) 'Selected identities, scope, and counts derived only from the enumerated stored records; raw reports are not rewritten'

$readme = @'
# Native Library entry software evidence: 2026-11-17

This fixed-allowlist bundle preserves completed `test-env` Linux fixture and
static-package results. Exporting it ran no new tests, builds, application,
runtime probes, remote commands, device operations, or lease operations. It does
not establish native Library behavior, physical acceptance, hardware support,
startup speed, or reading performance. Physical and performance evidence remain
separate.

The entry-script and package candidate is
`39fac822a982868078201e605e50bdacf1dcf3ed`. Its archive SHA-256 is
`5c513d92e48cbcdcbf408dbc02a80a6d9678455e4e06c95a62dd63523736b4b5`.
The stored PAX commit matches; all 1,782 extracted files matched the archive.
[software-summary.json](software-summary.json) links the raw source identities.
The canonical manifest binds 544 binary inputs and is byte-identical to the old
build manifest, SHA-256
`8537123d6b1dc0c75a048b09d1204664988a6c1668ea35d721bfbac641ff49c6`.
The `run_suite` serialization has SHA-256
`5d7d60c1164ceffe2b6a7339990cbb49b408d134aa0c8afc62f1367fdfec461b`;
its path/digest mapping is identical. The retained
[ordering diagnosis](original/manifest-order-diagnosis-r1.json) records no added,
missing, or changed inputs.

The application and JPEG worker were reused from the passed R3 build of
`530f1516c332931059c52161b52d64e86488b3f1`; they were not compiled from the new
entry-script commit. Their SHA-256 values are
`04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59` and
`12c8988c6a93cade9923d54bef784b774e13cb5e3b042f2e72ea5ecb6b897beb`.
The historical compiler-profile digest is
`8dae7219954e57236434f834b51c6dd74256ad7e14e5b68f88cf91864f31c955`.
No Rust compilation or ARM execution occurred in this software run. Historical
build-summary test counts are not included in the following totals.

| Stored check | Result |
| --- | --- |
| Full launcher lifecycle suite | 146 passed, including 17 new entry cases |
| Earlier targeted execution of those same new cases | 17 passed; repeated coverage |
| Library package regressions | 25 passed |
| Existing startup / spatial-dither / native-media package regressions | 18 / 5 / 3 passed |
| Unique cases | 197: 146 lifecycle + 51 package |
| Test executions | 214: 197 unique + 17 targeted repeats |
| Production shell syntax | 2 passed |
| Strict static package checker | 20 checks passed; 49 exact entries |

The refreshed ZIP digest is
`c57d5fc3674f1a0de448b85beb2552ee0959b83756c3e8f62afb559bf62a4882`.
It adds the document entry and three notices, updates README, third-party
notices, and the canonical launcher, and retains the other 42 old entries byte
for byte. Every old entry retains its mode and timestamp. The seven selected
packaging inputs, all 49 entry identities, and ELF/worker/ZIP identities are in
the retained provenance, metadata audit, checker, and sidecar records. The
launcher and document source digests are respectively
`584ad6065565bf5529fd4ef17393a454a0db811824c685e104a360c82080dea4` and
`7ac07e2a4e2b790642ff88787deaf05988e1bb8786d418b45bfd092d64d023f4`.

[Attempt R1](original/attempt-r1/summary.json) remains failed, with its original
error and zero executed tests. Its runner compared differing manifest orders.
[Attempt R2](original/attempt-r2/summary.json) is the passed software report; it
used a fresh runner and output directory. Both attempts' source, archive, and
runner integrity checks remained true. All 13 R2 command logs are retained,
including the two empty successful shell-syntax logs.

[export-manifest.json](export-manifest.json) maps every payload file to its raw
or derived status, origin, bytes, SHA-256, and transformation. Raw records and
runner snapshots preserve exact bytes; `.log` to `.txt` changes only filenames.
The manifest excludes itself and its digest from its payload list, and its own
SHA-256 is in [export-manifest.sha256](export-manifest.sha256). No directory was
recursively exported. ELF, ZIP, tar, SQLite, account/authentication/session data,
token values, private device transport, and physical-run material are excluded.
'@
Write-Utf8 (Join-Path $bundleRoot 'README.md') ($readme + "`n")
Add-Derived 'README.md' @('software-summary.json', 'original/attempt-r1/summary.json', 'original/attempt-r2/summary.json') 'English explanation derived only from the exported stored records'

$payloadPaths = @($rows | ForEach-Object { $_.path } | Sort-Object)
$manifest = [ordered]@{
    schema_version = 1
    created_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
    export_scope = 'Fixed-allowlist export of already stored software text evidence; no new tests, builds, runtime, remote, device, or lease operations'
    exporter_origin = $exporterPath
    exporter_sha256 = (File-Identity $exporterPath).sha256
    script_and_package_source_commit = $r2.candidate_commit
    reused_binary_build_source_commit = $baseBuild.candidate_commit
    payload_file_count = $rows.Count
    raw_file_count = @($rows | Where-Object { $_.kind -eq 'raw' }).Count
    derived_file_count = @($rows | Where-Object { $_.kind -eq 'derived' }).Count
    total_file_count_including_manifest_and_digest = $rows.Count + 2
    fixed_exported_file_allowlist = @($payloadPaths + @('export-manifest.json', 'export-manifest.sha256') | Sort-Object)
    self_reference_policy = 'The manifest and its digest are excluded from files; export-manifest.sha256 binds the manifest bytes'
    files = @($rows | Sort-Object { $_.path })
}
$manifestPath = Join-Path $bundleRoot 'export-manifest.json'
Write-Json $manifestPath $manifest
$manifestIdentity = File-Identity $manifestPath
Write-Utf8 (Join-Path $bundleRoot 'export-manifest.sha256') ($manifestIdentity.sha256 + "  export-manifest.json`n")

[ordered]@{
    exported_payload_files = $rows.Count
    total_exported_files = $rows.Count + 2
    manifest_sha256 = $manifestIdentity.sha256
    destination = $bundleRoot
    stored_only = $true
} | ConvertTo-Json
