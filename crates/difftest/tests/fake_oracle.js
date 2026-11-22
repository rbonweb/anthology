// A fake difftest oracle for tests/cli.rs (no Python needed): run as
// `node fake_oracle.js -I <root>/port/difftest/oracle/_driver.py`, it answers each request from
// the committed golden corpora of core.lock and core.state (the cases the surfaces seed from),
// and answers {"error": ...} for anything else, as the real driver does for a crash.
"use strict";
const fs = require("fs");
const path = require("path");
const readline = require("readline");

const driver = process.argv[process.argv.indexOf("-I") + 1];
const root = path.resolve(path.dirname(driver), "../../..");
const goldens = {
  core_lock: "crates/anthology-core/tests/golden/lock.jsonl",
  core_state: "crates/anthology-core/tests/golden/state.jsonl",
};
const answers = new Map();
for (const [module, rel] of Object.entries(goldens)) {
  for (const line of fs.readFileSync(path.join(root, rel), "utf8").split("\n")) {
    if (!line.trim()) continue;
    const row = JSON.parse(line);
    answers.set(module + " " + JSON.stringify(row.case), row.result);
  }
}
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const req = JSON.parse(line);
  const got = answers.get(req.module + " " + JSON.stringify(req.case));
  const answer = got || { error: "Traceback: fake oracle: no golden answer" };
  process.stdout.write(JSON.stringify(answer) + "\n");
});
