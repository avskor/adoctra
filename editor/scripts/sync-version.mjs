// The npm packages carry the workspace version of Cargo.toml.
//   node scripts/sync-version.mjs          write it into every package.json
//   node scripts/sync-version.mjs --check  fail when one differs (CI)
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const editor = fileURLToPath(new URL("..", import.meta.url));
const cargo = readFileSync(new URL("../../Cargo.toml", import.meta.url), "utf8");
const version = /^version = "(.*)"/m.exec(cargo)[1];
const check = process.argv.includes("--check");
const root = JSON.parse(readFileSync(`${editor}package.json`, "utf8"));
let stale = false;

for (const workspace of root.workspaces) {
  const path = `${editor}${workspace}/package.json`;
  const before = readFileSync(path, "utf8");
  const manifest = JSON.parse(before);
  if (manifest.version !== undefined) manifest.version = version;
  for (const group of ["dependencies", "devDependencies"]) {
    for (const name of Object.keys(manifest[group] ?? {})) {
      if (name.startsWith("@adoctra/")) manifest[group][name] = version;
    }
  }
  const after = `${JSON.stringify(manifest, null, 2)}\n`;
  if (after === before) continue;
  if (check) {
    console.error(`${workspace}/package.json is not at version ${version}`);
    stale = true;
  } else {
    writeFileSync(path, after);
    console.log(`${workspace}/package.json -> ${version}`);
  }
}
if (stale) {
  console.error("run `npm run version:sync` in editor/ and commit the result");
  process.exit(1);
}
