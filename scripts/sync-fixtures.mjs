// Copies mumbo-sdk's fixtures (real serde output, which the SDK's own tests
// keep in sync with Rust) into mumbo, where the frontend's strict zod
// schemas parse them. Run via `npm run sync:fixtures`; CI fails if the
// checked-in copies are stale, so an SDK bump can't leave them behind.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(repoRoot, "fixtures", "mumbo-sdk");

// Ask cargo where the SDK pinned in Cargo.lock lives (a git checkout, or a
// local path while developing against a `[patch]`).
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--format-version", "1", "--locked"], {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  }),
);
const sdk = metadata.packages.find((p) => p.name === "mumbo-sdk");
if (!sdk) {
  throw new Error("mumbo-sdk not found in `cargo metadata`");
}
const sdkFixtures = join(dirname(sdk.manifest_path), "fixtures");

rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });
for (const file of readdirSync(sdkFixtures).filter((f) => f.endsWith(".json"))) {
  copyFileSync(join(sdkFixtures, file), join(outDir, file));
}
