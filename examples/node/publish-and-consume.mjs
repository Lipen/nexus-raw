// The publish-and-consume flow through the npm package: publish a version directory with a claim, name it through a channel, download it back, verify offline.
//
// Without arguments the demo spawns the repo's mock server on a free port (`--port 0`).
// Pass a base URL to run against a real repository instead:
//
//   node publish-and-consume.mjs https://nexus.example.com/repository/demo/

import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import pkg from "nexus-raw";

// The placeholder package resolves but exports nothing: the smoke runs only
// against the real scoped surface, which the next release ships.
if (!pkg || typeof pkg.up !== "function") {
  console.log("@nexus-raw/nxr placeholder: the functional smoke starts with the scoped release");
  process.exit(0);
}

const { up, down, verify, channelSet, channelGet } = pkg;

const mock = process.argv[2] ? { url: process.argv[2], child: null } : await spawnMock();
const repo = mock.url;

// The producer side: a version directory plus the manifest naming its files.
const dist = "demo-dist";
mkdirSync(`${dist}/bom`, { recursive: true });
writeFileSync(`${dist}/app.bin`, Buffer.alloc(4096, 7));
writeFileSync(`${dist}/bom/manifest.json`, JSON.stringify({ artifacts: ["app.bin"] }));
// The manifest sits at the version root and lists what consumers may fetch.
writeFileSync(`${dist}/manifest.json`, JSON.stringify({ artifacts: ["app.bin"] }));

const events = (ev) => console.log(`  ${JSON.stringify(ev)}`);

const upSummary = await up(dist, `${repo}1.0.0/`, { claimFirst: "manifest.json", onEvent: events });
console.log(`up: uploaded ${upSummary.uploaded}, skipped ${upSummary.skipped}`);

// Name the version so consumers never hard-code it.
await channelSet(`${repo}latest`, "1.0.0", true);
const version = await channelGet(`${repo}latest`);
console.log(`channel latest -> ${version}`);

// The consumer side: enumerate from the manifest, land the tree, verify offline.
const downSummary = await down(`${repo}${version}/`, "demo-vendor", {
  manifest: `${dist}/manifest.json`,
  onEvent: events,
});
console.log(`down: downloaded ${downSummary.downloaded}, skipped ${downSummary.skipped}`);

const check = await verify("demo-vendor");
console.log(`verify: ${check.skipped + downSummary.downloaded} names ok`);
mock.child?.kill();

/** Spawn the repo's mock server.
 * Leaked on purpose: it dies with this process. */
function spawnMock() {
  // The demo runs from the repo root (just) or from examples/node (npm): try both, and let the environment override.
  const candidates = process.env.MOCK_NEXUS_BIN
    ? [process.env.MOCK_NEXUS_BIN]
    : ["target/debug/mock-nexus", "../../target/debug/mock-nexus"];

  // Port 0 = an ephemeral port: a busy fixed port would collide with whatever else lives on this machine.
  // The real URL arrives on stdout.
  // A spawn error or an early exit falls through to the next candidate.
  const tryCandidate = (i) =>
    i >= candidates.length
      ? Promise.reject(new Error("cannot spawn mock-nexus (build it: cargo build -p mock-nexus)"))
      : new Promise((resolve, reject) => {
          const child = spawn(candidates[i], ["atomic", "--port", "0"], {
            stdio: ["ignore", "pipe", "ignore"],
          });
          let buf = "";
          child.stdout.on("data", (chunk) => {
            buf += chunk;
            const line = buf.split("\n").find((l) => l.startsWith("listening http://"));
            if (line) resolve({ url: line.slice("listening ".length).replace(/\/?$/, "/"), child });
          });
          child.once("error", () => tryCandidate(i + 1).then(resolve, reject));
          child.once("exit", (code) => {
            if (code !== null && code !== 0) tryCandidate(i + 1).then(resolve, reject);
          });
        });

  return tryCandidate(0);
}
