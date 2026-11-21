// Put the license texts a package ships beside its package.json (npm's
// `prepack`): the project's two, plus the notices of the third-party code
// compiled into a WASM module.
import { copyFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const files = ["LICENSE-MIT", "LICENSE-APACHE"];
// `adoctra-ext-diagram` is in both modules, the Rouge port in the preview.
if (process.argv.includes("--diagram")) files.push("crates/adoctra-ext-diagram/LICENSE-third-party");
if (process.argv.includes("--rouge")) files.push("crates/adoctra-highlight/LICENSE-rouge");
for (const file of files) copyFileSync(`${root}${file}`, file.split("/").pop());
