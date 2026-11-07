// Reads the lcov reports that a coverage config names, checks each part against its floor and
// against main, and writes the report: a table on stdout and in the GitHub job summary,
// summary.json for the next comparison, and comment.md for the pull request. Exits 1 when a part
// is under its floor, drops below main, has no report, or has a source file that no test loads.
//
//   node .github/coverage/report.ts --config <config.json> --out <dir> [--base <summary.json>]
//                                    [--allow-drop]
//
// --base is main's summary.json at the pull request's base commit. Without it there is no
// comparison, and only the floors apply. --allow-drop reports a drop below main without failing.
// Needs Node 24 or later, which runs TypeScript by stripping the types. It adds no dependency.
import {
  appendFileSync,
  existsSync,
  globSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join, matchesGlob } from "node:path";

/** Hit and found counts of one kind of coverage. */
export type Count = { hit: number; found: number };
export type Totals = { lines: Count; branches: Count; functions: Count };

/**
 * One part of a coverage config: its report, and the least coverage it may have. A report that
 * leaves out the files no test loads, as Deno's does, needs `sources`: the part's files, as globs
 * from the repo root. Each must be in the report, except those `unloaded` names.
 */
export type Part = {
  name: string;
  lcov: string;
  floor: { lines: number; branches?: number };
  sources?: string[];
  unloaded?: string[];
};

/**
 * A coverage config. `id` marks the pull request comment, so each config keeps one comment.
 * `tolerance` is how many percentage points of lines a part may lose against main.
 */
export type Config = { id: string; title: string; tolerance: number; parts: Part[] };

export type Summary = { id: string; parts: Record<string, Totals> };

export type Row = {
  part: Part;
  totals: Totals | null;
  base: Totals | null;
  problems: string[];
  hints: string[];
};

/** The totals of an lcov report: the sum of the LH/LF, BRH/BRF and FNH/FNF lines of every file. */
export function parseLcov(text: string): Totals {
  const totals: Totals = { lines: zero(), branches: zero(), functions: zero() };
  const fields: Record<string, [keyof Totals, keyof Count]> = {
    LH: ["lines", "hit"],
    LF: ["lines", "found"],
    BRH: ["branches", "hit"],
    BRF: ["branches", "found"],
    FNH: ["functions", "hit"],
    FNF: ["functions", "found"],
  };
  for (const line of text.split("\n")) {
    const [key = "", value] = line.trim().split(":", 2);
    const field = fields[key];
    if (field && value !== undefined) totals[field[0]][field[1]] += Number(value);
  }
  return totals;
}

/** The files an lcov report covers: its SF lines. */
export function lcovFiles(text: string): string[] {
  return text
    .split("\n")
    .filter((line) => line.startsWith("SF:"))
    .map((line) => line.slice(3).trim());
}

/** The part's source files that are in no report, so no test loads them. */
export function notLoaded(part: Part, files: string[], reported: string[]): string[] {
  return files.filter(
    (file) =>
      !(part.unloaded ?? []).some((pattern) => matchesGlob(file, pattern)) &&
      !reported.some((path) => path === file || path.endsWith(`/${file}`)),
  );
}

function zero(): Count {
  return { hit: 0, found: 0 };
}

/** The share covered, in percent. A count with nothing to cover has none. */
export function percent(count: Count | undefined): number | null {
  return count && count.found > 0 ? (100 * count.hit) / count.found : null;
}

/** What is wrong with one part, and how its floor can go up. */
export function check(
  part: Part,
  totals: Totals | null,
  base: Totals | null,
  tolerance: number,
  allowDrop: boolean,
  unloadedFiles: string[] = [],
) {
  const problems: string[] = [];
  const hints: string[] = [];
  if (!totals) {
    problems.push(`${part.name}: no coverage report at ${part.lcov}`);
    return { problems, hints };
  }
  for (const file of unloadedFiles)
    problems.push(`${part.name}: no test loads ${file}, so no report covers it`);
  for (const kind of ["lines", "branches"] as const) {
    const floor = part.floor[kind];
    const value = percent(totals[kind]);
    if (floor === undefined || value === null) continue;
    if (value < floor)
      problems.push(
        `${part.name}: ${kind} ${fixed(value)}% is under the floor of ${fixed(floor)}%`,
      );
    else if (value - floor >= 1)
      hints.push(
        `${part.name}: ${kind} at ${fixed(value)}%, so its floor can go up to ${Math.floor(value)}%`,
      );
  }
  const now = percent(totals.lines);
  const before = percent(base?.lines);
  if (now !== null && before !== null && now < before - tolerance) {
    const drop = `${part.name}: lines ${fixed(now)}% is ${fixed(before - now)} points below main (${fixed(before)}%)`;
    if (allowDrop) hints.push(`${drop}, allowed by the coverage-drop-ok label`);
    else problems.push(drop);
  }
  return { problems, hints };
}

function fixed(value: number): string {
  return value.toFixed(2);
}

function cell(count: Count): string {
  const value = percent(count);
  return value === null ? "–" : `${fixed(value)}% (${count.hit}/${count.found})`;
}

function change(totals: Totals | null, base: Totals | null): string {
  const now = percent(totals?.lines);
  const before = percent(base?.lines);
  if (now === null || before === null) return "–";
  const delta = now - before;
  return `${delta >= 0 ? "+" : "−"}${fixed(Math.abs(delta))}`;
}

/** The report as Markdown, with the comment marker first. */
export function markdown(config: Config, rows: Row[], hasBase: boolean): string {
  const lines = [
    `<!-- coverage: ${config.id} -->`,
    `### ${config.title}`,
    "",
    "| Part | Lines | Change in lines | Branches | Functions | Floor |",
    "| --- | --- | --- | --- | --- | --- |",
  ];
  for (const { part, totals, base, problems } of rows) {
    const empty = { hit: 0, found: 0 };
    const floor = `${part.floor.lines}%${part.floor.branches === undefined ? "" : `, branches ${part.floor.branches}%`}`;
    const mark = problems.length > 0 ? " ❌" : "";
    lines.push(
      `| ${part.name}${mark} | ${cell(totals?.lines ?? empty)} | ${change(totals, base)} | ${cell(totals?.branches ?? empty)} | ${cell(totals?.functions ?? empty)} | ${floor} |`,
    );
  }
  lines.push("");
  lines.push(
    hasBase
      ? "The change is against main at this pull request's base commit."
      : "No report from main at the base commit, so there is no change to show. The floors still apply.",
  );
  const problems = rows.flatMap((row) => row.problems);
  const hints = rows.flatMap((row) => row.hints);
  if (problems.length > 0)
    lines.push("", "**Coverage check failed:**", "", ...problems.map((problem) => `- ${problem}`));
  if (hints.length > 0) lines.push("", ...hints.map((hint) => `- ${hint}`));
  lines.push("", 'The floors and how to raise them: `CONTRIBUTING.md`, "Coverage".');
  return `${lines.join("\n")}\n`;
}

function flag(args: string[], name: string): string | undefined {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

export function main(args: string[]): number {
  const configPath = flag(args, "--config");
  const out = flag(args, "--out");
  if (!configPath || !out) {
    console.error("give --config <config.json> and --out <dir>");
    return 64;
  }
  const config: Config = JSON.parse(readFileSync(configPath, "utf8"));
  const basePath = flag(args, "--base");
  const base: Summary | null =
    basePath && existsSync(basePath) ? JSON.parse(readFileSync(basePath, "utf8")) : null;
  const allowDrop = args.includes("--allow-drop");

  const rows: Row[] = config.parts.map((part) => {
    const text = existsSync(part.lcov) ? readFileSync(part.lcov, "utf8") : null;
    const totals = text === null ? null : parseLcov(text);
    const before = base?.parts[part.name] ?? null;
    const sources = (part.sources ?? []).flatMap((pattern) => globSync(pattern)).sort();
    const unloaded = text === null ? [] : notLoaded(part, sources, lcovFiles(text));
    return {
      part,
      totals,
      base: before,
      ...check(part, totals, before, config.tolerance, allowDrop, unloaded),
    };
  });
  const report = markdown(config, rows, base !== null);

  mkdirSync(out, { recursive: true });
  const summary: Summary = { id: config.id, parts: {} };
  for (const { part, totals } of rows) if (totals) summary.parts[part.name] = totals;
  writeFileSync(join(out, "summary.json"), `${JSON.stringify(summary, null, 2)}\n`);
  writeFileSync(join(out, "comment.md"), report);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, report);
  console.log(report);
  return rows.some((row) => row.problems.length > 0) ? 1 : 0;
}

if (import.meta.main) process.exit(main(process.argv.slice(2)));
