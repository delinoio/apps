// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import test from "node:test";
const revision = "a".repeat(40);
function fixture(t, refs = []) {
  const root = mkdtempSync(join(tmpdir(), "complete-release-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "bin"));
  mkdirSync(join(root, ".release"));
  writeFileSync(
    join(root, ".release/plan.json"),
    JSON.stringify({
      components: [
        { component: "lane", tag: "lane@v1.0.0", binaries: true },
        { component: "devbase", tag: "devbase@v1.0.0", binaries: false },
      ],
    }),
  );
  writeFileSync(
    join(root, "state.json"),
    JSON.stringify({ refs, calls: [], draft: true }),
  );
  writeFileSync(
    join(root, "bin/git"),
    `#!/usr/bin/env node\nconsole.log('${revision}');\n`,
  );
  writeFileSync(
    join(root, "bin/gh"),
    `#!/usr/bin/env node
const fs = require('node:fs');
const state = JSON.parse(fs.readFileSync('state.json'));
const args = process.argv.slice(2); state.calls.push(args);
if (args[0] === 'api' && args[1].includes('matching-refs')) console.log(JSON.stringify(state.refs));
else if (args[0] === 'api') state.refs.push({ref:args[args.indexOf('-f')+1].slice(4),object:{sha:'${revision}'}});
else if (args[1] === 'view') console.log(JSON.stringify({isDraft: state.draft}));
else if (args[1] === 'edit') state.draft = false;
else process.exit(2);
fs.writeFileSync('state.json', JSON.stringify(state));
`,
  );
  for (const command of ["git", "gh"])
    chmodSync(join(root, "bin", command), 0o755);
  return {
    invoke: (mode) =>
      spawnSync(
        process.execPath,
        [
          new URL("./complete.mjs", import.meta.url).pathname,
          ...(mode ? [mode] : []),
        ],
        {
          cwd: root,
          encoding: "utf8",
          env: {
            ...process.env,
            PATH: `${join(root, "bin")}${delimiter}${process.env.PATH}`,
            RELEASE_REVISION: revision,
            RELEASE_COMPONENT: "lane",
            GITHUB_OUTPUT: join(root, "output"),
          },
        },
      ),
    state: () => JSON.parse(readFileSync(join(root, "state.json"))),
    output: () => readFileSync(join(root, "output"), "utf8"),
  };
}
test("completion tags only its component at the release SHA and completes a staged draft", (t) => {
  const f = fixture(t);
  const result = f.invoke();
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(f.state().refs, [
    { ref: "refs/tags/lane@v1.0.0", object: { sha: revision } },
  ]);
  assert.equal(f.state().draft, false);
  assert.equal(f.invoke().status, 0);
  assert.equal(f.state().refs.length, 1);
});
test("status is read-only and conflicting tags cannot be moved or published", (t) => {
  const f = fixture(t, [
    { ref: "refs/tags/lane@v1.0.0", object: { sha: revision } },
  ]);
  assert.equal(f.invoke("status").status, 0);
  assert.equal(f.output(), "tagged=true\n");
  assert.equal(f.state().draft, true);
  assert.equal(f.state().calls.length, 1);
  const conflict = fixture(t, [
    { ref: "refs/tags/lane@v1.0.0", object: { sha: "b".repeat(40) } },
  ]);
  assert.notEqual(conflict.invoke().status, 0);
  assert.equal(conflict.state().calls.length, 1);
  assert.equal(conflict.state().draft, true);
});
