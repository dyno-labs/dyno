// Builds the two probes from probe.ts by stripping the types: probe.js, the lab build
// that dyno injects over CDP, and probe.field.js, the field build that a platform adds to a game.
// The field build leaves out each block between the lines `// lab: begin` and `// lab: end`.
// Needs Node 22.13 or later; it adds no dependency. Run it from any folder:
// node crates/dyno-cdp/probe/build.mjs
// With --check it writes nothing, and exits 1 when a probe is not what probe.ts gives.
import { readFileSync, writeFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";

const here = new URL(".", import.meta.url);
const LAB_BEGIN = "// lab: begin";
const LAB_END = "// lab: end";

/** probe.ts without its lab blocks. */
const withoutLabBlocks = (source) => {
  const kept = [];
  let inBlock = false;
  for (const line of source.split("\n")) {
    const marker = line.trim();
    if (marker === LAB_BEGIN || marker === LAB_END) {
      if (inBlock === (marker === LAB_BEGIN)) throw new Error(`probe.ts: ${marker} out of order`);
      inBlock = marker === LAB_BEGIN;
    } else if (!inBlock) {
      kept.push(line);
    }
  }
  if (inBlock) throw new Error(`probe.ts: ${LAB_BEGIN} with no ${LAB_END}`);
  return kept.join("\n");
};

/** The script that a TypeScript source gives. */
const build = (source) => {
  const stripped = stripTypeScriptTypes(source, { mode: "strip" });
  // Stripping keeps every position: a type becomes spaces in the same place. So a run of spaces
  // in the output that holds a position where the source has another character is a stripped type,
  // and the run becomes one space. A run inside a string or a regular expression holds no stripped
  // position, so space the source wrote there is kept as it is.
  if (stripped.length !== source.length) throw new Error("stripping moved the text");
  let compact = "";
  for (let start = 0; start < stripped.length; ) {
    if (stripped[start] !== " ") {
      compact += stripped[start++];
      continue;
    }
    let end = start;
    let holdsAType = false;
    while (end < stripped.length && stripped[end] === " ") {
      holdsAType ||= source[end] !== " ";
      end++;
    }
    compact += holdsAType ? " " : stripped.slice(start, end);
    start = end;
  }
  // Indentation is never inside a literal unless a template literal spans lines, so the build
  // refuses a backtick outside a comment, then trims each line and drops comments and blank lines.
  const lines = compact
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "" && !line.startsWith("//"));
  const withBacktick = lines.find((line) => line.includes("`"));
  if (withBacktick) throw new Error(`probe.ts: a template literal the build would change: ${withBacktick}`);
  return "// Generated from probe.ts by build.mjs. Do not edit.\n" + lines.join("\n") + "\n";
};

const source = readFileSync(new URL("probe.ts", here), "utf8");
const probes = { "probe.js": build(source), "probe.field.js": build(withoutLabBlocks(source)) };
let stale = false;
for (const [name, script] of Object.entries(probes)) {
  const target = new URL(name, here);
  if (!process.argv.includes("--check")) {
    writeFileSync(target, script);
  } else if (readFileSync(target, "utf8") !== script) {
    console.error(`${name} is out of date: run node crates/dyno-cdp/probe/build.mjs`);
    stale = true;
  }
}
if (stale) process.exit(1);
