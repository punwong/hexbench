// Build the updater manifest (`latest.json`) from the signed NSIS installer.
//
// Used by the release workflow and handy locally:
//   RELEASE_TAG=v0.1.3 node scripts/latest-json.mjs
// Reads the version from tauri.conf.json so the manifest can never disagree
// with the binary (the updater rejects mismatched signatures outright).

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const version = config.version;
const tag = process.env.RELEASE_TAG ?? `v${version}`;
const repo = process.env.GITHUB_REPOSITORY ?? "hexbench/hexbench";
const bundleDir = "target/release/bundle/nsis";

const expected = `HexBench_${version}_x64-setup.exe`;
const installer = readdirSync(bundleDir).includes(expected)
  ? expected
  : readdirSync(bundleDir).find((name) => name.endsWith("_x64-setup.exe"));
if (!installer) {
  console.error(`在 ${bundleDir} 里找不到 *_x64-setup.exe，先跑 pnpm tauri build`);
  process.exit(1);
}

const signaturePath = join(bundleDir, `${installer}.sig`);
let signature;
try {
  signature = readFileSync(signaturePath, "utf8").trim();
} catch {
  console.error(`找不到签名文件 ${signaturePath}，构建时缺 TAURI_SIGNING_PRIVATE_KEY`);
  process.exit(1);
}

const manifest = {
  version,
  notes: process.env.RELEASE_NOTES ?? `HexBench ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": {
      signature,
      url: `https://github.com/${repo}/releases/download/${tag}/${installer}`,
    },
  },
};

writeFileSync("latest.json", `${JSON.stringify(manifest, null, 2)}\n`);
console.log(JSON.stringify(manifest, null, 2));
