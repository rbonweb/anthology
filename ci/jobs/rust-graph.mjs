// The workspace's dependency graph, read from `cargo metadata --no-deps --format-version 1`
// (ci/jobs/rust-scope.sh runs this). Plain node, no packages: the CI runner and the pinned Rust
// container both have it.
//
//   node ci/jobs/rust-graph.mjs METADATA.json dirs
//       one `<folder of the crate, from the workspace root><TAB><package name>` line per member
//   node ci/jobs/rust-graph.mjs METADATA.json affected NAME...
//       the named members and every member that depends on one of them, through a normal, build
//       or dev dependency, one name per line, sorted
//   node ci/jobs/rust-graph.mjs METADATA.json closure NAME...
//       the named members and every member a build of them needs (normal and build
//       dependencies, not dev dependencies), sorted
//
// A name that is not a workspace member is an error (exit 2): the caller then runs everything.
import { readFileSync } from "node:fs";
import { dirname, relative } from "node:path";

const [file, command, ...names] = process.argv.slice(2);
const metadata = JSON.parse(readFileSync(file, "utf8"));
const members = metadata.packages.filter((p) => metadata.workspace_members.includes(p.id));
const known = new Set(members.map((p) => p.name));

// member name -> the member names it depends on, with and without its dev dependencies
const members_used = (p, dev) =>
  new Set(p.dependencies.filter((d) => dev || d.kind !== "dev").map((d) => d.name).filter((n) => known.has(n)));
const uses = new Map(members.map((p) => [p.name, members_used(p, true)]));
const builds = new Map(members.map((p) => [p.name, members_used(p, false)]));

// `start` and everything `next` reaches from it, sorted.
function reach(start, next) {
  const seen = new Set(start);
  const todo = [...start];
  while (todo.length > 0) {
    for (const n of next(todo.pop())) {
      if (!seen.has(n)) {
        seen.add(n);
        todo.push(n);
      }
    }
  }
  return [...seen].sort();
}

function requireMembers() {
  for (const n of names) {
    if (!known.has(n)) {
      console.error(`rust-graph: ${n} is not a workspace member`);
      process.exit(2);
    }
  }
}

switch (command) {
  case "dirs":
    for (const p of members) {
      console.log(`${relative(metadata.workspace_root, dirname(p.manifest_path))}\t${p.name}`);
    }
    break;
  case "affected":
    requireMembers();
    for (const n of reach(names, (n) => members.filter((p) => uses.get(p.name).has(n)).map((p) => p.name))) {
      console.log(n);
    }
    break;
  case "closure":
    requireMembers();
    for (const n of reach(names, (n) => builds.get(n))) {
      console.log(n);
    }
    break;
  default:
    console.error("usage: rust-graph.mjs METADATA.json dirs | affected NAME... | closure NAME...");
    process.exit(2);
}
