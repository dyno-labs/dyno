// node --test ".github/coverage/*.test.ts"
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
  check,
  type Config,
  lcovFiles,
  main,
  markdown,
  notLoaded,
  type Part,
  parseLcov,
  percent,
  type Totals,
} from "./report.ts";

const LCOV = `TN:
SF:src/a.rs
FNF:4
FNH:3
LF:100
LH:90
BRF:10
BRH:5
end_of_record
SF:src/b.rs
FNF:1
FNH:1
LF:100
LH:100
BRF:0
BRH:0
end_of_record
`;

const PART: Part = { name: "Rust", lcov: "rust.lcov", floor: { lines: 90 } };

function totals(linesHit: number, linesFound = 100): Totals {
  return {
    lines: { hit: linesHit, found: linesFound },
    branches: { hit: 0, found: 0 },
    functions: { hit: 1, found: 1 },
  };
}

test("an lcov report's totals are the sums over its files", () => {
  assert.deepEqual(parseLcov(LCOV), {
    lines: { hit: 190, found: 200 },
    branches: { hit: 5, found: 10 },
    functions: { hit: 4, found: 5 },
  });
});

test("a count with nothing to cover has no percentage", () => {
  assert.equal(percent({ hit: 0, found: 0 }), null);
  assert.equal(percent({ hit: 1, found: 4 }), 25);
});

test("a part at its floor passes, and one under it fails", () => {
  assert.deepEqual(check(PART, totals(90), null, 0.1, false).problems, []);
  assert.deepEqual(check(PART, totals(89), null, 0.1, false).problems, [
    "Rust: lines 89.00% is under the floor of 90.00%",
  ]);
});

test("a branch floor applies only when the part sets one and has branches", () => {
  const withBranches = { ...PART, floor: { lines: 0, branches: 60 } };
  const some = { ...totals(100), branches: { hit: 5, found: 10 } };
  assert.deepEqual(check(withBranches, some, null, 0.1, false).problems, [
    "Rust: branches 50.00% is under the floor of 60.00%",
  ]);
  assert.deepEqual(check(withBranches, totals(100), null, 0.1, false).problems, []);
});

test("a source file that is in no report fails its part", () => {
  const part = { ...PART, sources: ["tool/*.ts"], unloaded: ["**/*_test.ts", "tool/main.ts"] };
  const files = ["tool/check.ts", "tool/check_test.ts", "tool/main.ts", "tool/zip.ts"];
  const missing = notLoaded(part, files, ["/home/ci/repo/tool/zip.ts"]);
  assert.deepEqual(missing, ["tool/check.ts"]);
  assert.deepEqual(check(part, totals(100), null, 0.1, false, missing).problems, [
    "Rust: no test loads tool/check.ts, so no report covers it",
  ]);
});

test("a report's files are its SF lines", () => {
  assert.deepEqual(lcovFiles(LCOV), ["src/a.rs", "src/b.rs"]);
});

test("a part with no report fails", () => {
  assert.deepEqual(check(PART, null, null, 0.1, false).problems, [
    "Rust: no coverage report at rust.lcov",
  ]);
});

test("a drop below main of more than the tolerance fails; a smaller one passes", () => {
  assert.deepEqual(check(PART, totals(9989, 10000), totals(9990, 10000), 0.1, false).problems, []);
  assert.deepEqual(check(PART, totals(9500, 10000), totals(9700, 10000), 0.1, false).problems, [
    "Rust: lines 95.00% is 2.00 points below main (97.00%)",
  ]);
});

test("a drop below main is only a note when it is allowed", () => {
  const { problems, hints } = check(PART, totals(95), totals(97), 0.1, true);
  assert.deepEqual(problems, []);
  assert.ok(
    hints.some((hint) => hint.endsWith("allowed by the coverage-drop-ok label")),
    hints.join("\n"),
  );
});

test("a part a point or more over its floor says how far the floor can go up", () => {
  assert.deepEqual(check(PART, totals(9254, 10000), null, 0.1, false).hints, [
    "Rust: lines at 92.54%, so its floor can go up to 92%",
  ]);
  assert.deepEqual(check(PART, totals(9099, 10000), null, 0.1, false).hints, []);
});

test("the comment starts with its marker and shows each part with its change from main", () => {
  const config: Config = { id: "engine", title: "Engine coverage", tolerance: 0.1, parts: [PART] };
  const text = markdown(
    config,
    [{ part: PART, totals: totals(95), base: totals(94), problems: [], hints: [] }],
    true,
  );
  assert.ok(text.startsWith("<!-- coverage: engine -->\n### Engine coverage\n"));
  assert.match(
    text,
    /\| Rust \| 95\.00% \(95\/100\) \| \+1\.00 \| – \| 100\.00% \(1\/1\) \| 90% \|/,
  );
});

test("the report fails, and still writes its files, when a part fails", () => {
  const dir = mkdtempSync(join(tmpdir(), "coverage-test-"));
  writeFileSync(join(dir, "rust.lcov"), LCOV);
  const config: Config = {
    id: "engine",
    title: "Engine coverage",
    tolerance: 0.1,
    parts: [
      { ...PART, lcov: join(dir, "rust.lcov"), floor: { lines: 96 } },
      { name: "Probe", lcov: join(dir, "none.lcov"), floor: { lines: 1 } },
    ],
  };
  writeFileSync(join(dir, "config.json"), JSON.stringify(config));
  const out = join(dir, "out");
  const log = console.log;
  console.log = () => {};
  try {
    assert.equal(main(["--config", join(dir, "config.json"), "--out", out]), 1);
  } finally {
    console.log = log;
  }
  const summary = JSON.parse(readFileSync(join(out, "summary.json"), "utf8"));
  assert.deepEqual(Object.keys(summary.parts), ["Rust"]);
  const comment = readFileSync(join(out, "comment.md"), "utf8");
  assert.match(comment, /Rust: lines 95\.00% is under the floor of 96\.00%/);
  assert.match(comment, /Probe: no coverage report at/);
  assert.match(comment, /No report from main at the base commit/);
});

test("the report passes against main's summary when nothing dropped", () => {
  const dir = mkdtempSync(join(tmpdir(), "coverage-test-"));
  writeFileSync(join(dir, "rust.lcov"), LCOV);
  const config: Config = {
    id: "engine",
    title: "Engine coverage",
    tolerance: 0.1,
    parts: [{ ...PART, lcov: join(dir, "rust.lcov") }],
  };
  writeFileSync(join(dir, "config.json"), JSON.stringify(config));
  writeFileSync(
    join(dir, "base.json"),
    JSON.stringify({ id: "engine", parts: { Rust: parseLcov(LCOV) } }),
  );
  const log = console.log;
  console.log = () => {};
  try {
    assert.equal(
      main([
        "--config",
        join(dir, "config.json"),
        "--out",
        join(dir, "out"),
        "--base",
        join(dir, "base.json"),
      ]),
      0,
    );
  } finally {
    console.log = log;
  }
  assert.match(readFileSync(join(dir, "out", "comment.md"), "utf8"), /\| \+0\.00 \|/);
});
