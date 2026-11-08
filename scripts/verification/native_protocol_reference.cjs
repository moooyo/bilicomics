// Run the official Go/WASM callback with Go's JavaScript host as an independent reference.
// Inputs are synthetic cases emitted by the Rust integration test, never account requests.
const fs = require('node:fs');
const crypto = require('node:crypto');
globalThis.crypto = crypto.webcrypto;

const [assetPath, shimPath, casesPath] = process.argv.slice(2);
if (!assetPath || !shimPath || !casesPath) {
    throw new Error('Supply the pinned module, Go wasm_exec.js, and synthetic cases');
}
require(shimPath);

(async () => {
    const cases = JSON.parse(fs.readFileSync(casesPath, 'utf8'));
    const results = [];
    const go = new Go();
    const {instance} = await WebAssembly.instantiate(fs.readFileSync(assetPath), go.importObject);
    const running = go.run(instance);
    running.catch(error => { throw error; });
    const deadline = Date.now() + 10000;
    while (typeof globalThis.y1_z2w2a3 !== 'function') {
        if (Date.now() >= deadline) throw new Error('The official callback was not installed');
        await new Promise(resolve => setTimeout(resolve, 10));
    }
    for (const testCase of cases) {
        let value = globalThis.y1_z2w2a3(testCase.query, testCase.body, testCase.timestamp_ms);
        if (typeof value === 'string') value = JSON.parse(value);
        if (!value || typeof value.sign !== 'string' || value.sign.length === 0 || value.error) {
            throw new Error('The official callback did not return a signature');
        }
        results.push({name:testCase.name, signature_bytes:Buffer.byteLength(value.sign),
            signature_sha256:crypto.createHash('sha256').update(value.sign).digest('hex')});
    }
    process.stdout.write(JSON.stringify({reference:'official-go-js-host', runtime_instances:1, cases:results}) + '\n');
    process.exit(0);
})().catch(error => { process.stderr.write(error.stack + '\n'); process.exit(1); });
