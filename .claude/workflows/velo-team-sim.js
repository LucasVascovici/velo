export const meta = {
  name: 'velo-team-sim',
  description: 'Simulate a team of Haiku developers working at the same time on one shared project with velo as the only VCS; deterministic oracle checks after every round; triage of every suspected velo bug',
  whenToUse: 'Stress-testing velo end to end with realistic concurrent multi-developer use: sync, merges, conflicts, rebases, history surgery, bundles, HTTP remote',
  phases: [
    { title: 'Setup', detail: 'Build velo, seed the shared remote, one clone per developer, start serve-http', model: 'haiku' },
    { title: 'Rounds', detail: 'Each round: all developers work concurrently, then the oracle checks fsck, tests, lost work', model: 'haiku' },
    { title: 'Converge', detail: 'Everyone syncs to main; oracle verifies all clones equal the remote', model: 'haiku' },
    { title: 'Performance', detail: 'Idle-machine benchmark vs git (budgets), command-log statistics, then an analyst reviews them', model: 'haiku' },
    { title: 'Triage', detail: 'Reproduce each suspected bug in isolation and classify it', },
    { title: 'Teardown', detail: 'Stop the HTTP server', model: 'haiku' },
    { title: 'UX Review', detail: 'Per-developer review of every command they ran, a CLI help audit, then a synthesis with a UX scorecard' },
  ],
}

// ─── Arguments ──────────────────────────────────────────────────────────────
// {
//   sim?: "C:/…/velo/sim/runs/run1",   sandbox directory; must NOT exist yet / be empty (default <repoRoot>/../velo-sim/run1, OUTSIDE the repo: velo applies an enclosing .gitignore, which hides *.py)
//   repoRoot?: "C:/Users/lvi/Documents/velo",
//   rounds?: 4,                        1..4 (themes: feature sprint, collision, history surgery, release & chaos)
//   personas?: ["alice","bob",...],    default all 8. Pilot: ["alice","bob","carol","frank"] with rounds 2
//   build?: true,                      cargo build --release before the run
//   maxTriage?: 20,
//   maxConvergeRounds?: 3,
//   perf?: true,                       run the benchmark (sim/team/perf.py) and its analysis; false skips both
//   perfFiles?: 2000, perfHistory?: 300, perfBigMb?: 32     benchmark sizes (perf.py --files/--history/--big-mb)
//   uxModel?: "sonnet",                model for the UX reviewers/synthesis and the perf analyst ("opus" for the deepest read)
// }
// Outputs (in the sandbox): perf/results.json, perf/cmdstats.json, ux/<dev>.md, reports/perf-report.md, reports/ux-report.md
const A = args || {}
const ROOT = (A.repoRoot || 'C:/Users/lvi/Documents/velo').replace(/\\/g, '/').replace(/\/$/, '')
const SIM = (A.sim || `${ROOT}/../velo-sim/run1`).replace(/\\/g, '/').replace(/\/$/, '')
const ROUNDS = Math.max(1, Math.min(4, A.rounds ?? 4))
const ALL = ['alice', 'bob', 'carol', 'dan', 'erin', 'frank', 'grace', 'heidi']
const PERSONAS = (A.personas || ALL).filter(p => ALL.includes(p))
const BUILD = A.build !== false
const MAX_TRIAGE = A.maxTriage ?? 20
const MAX_CONVERGE = A.maxConvergeRounds ?? 3
const PERF = A.perf !== false
const PERF_FILES = A.perfFiles ?? 2000
const PERF_HISTORY = A.perfHistory ?? 300
const PERF_BIG_MB = A.perfBigMb ?? 32
const UX_MODEL = A.uxModel || 'sonnet'
const vpath = n => `${SIM}/bin/${n}/velo`
const cdir = n => `${SIM}/clones/${n}`

// What a "feature" is, for coverage accounting. Agents report from this closed list.
const FEATURES = [
  'save', 'save-amend', 'save-paths', 'status', 'diff-worktree', 'diff-range', 'show', 'blame', 'blame-at', 'blame-lines',
  'grep', 'grep-snapshot', 'history', 'history-graph', 'history-all', 'history-file', 'history-from', 'restore', 'restore-path',
  'squash', 'undo', 'redo', 'switch', 'switch-force', 'branches', 'branch-delete', 'tag', 'tag-force', 'tag-delete',
  'merge-clean', 'merge-conflict', 'merge-abort', 'resolve-take', 'resolve-all', 'cherry-pick', 'rebase', 'rebase-abort',
  'rebase-continue', 'rebase-conflict', 'stash-push', 'stash-pop', 'stash-list', 'stash-drop', 'stash-show', 'mv', 'rename-aware-merge',
  'gc', 'fsck', 'fsck-repair', 'bundle-create', 'bundle-apply', 'clone', 'fetch', 'push', 'push-branch', 'push-refused', 'pull',
  'pull-diverged', 'remote-add', 'remote-remove', 'remote-ref', 'http-remote', 'large-file', 'veloignore',
]

const PERSONA_ROLE = {
  alice: 'feature developer who likes feature branches and clean merges',
  bob: 'feature developer; works on the same areas as alice and often duplicates her tasks independently',
  carol: 'the conflict magnet: edits the same lines as everybody else and resolves the resulting conflicts',
  dan: 'refactorer: renames and moves files, makes many small snapshots, rewrites history on his own branches',
  erin: 'release manager: tags, release branches, cherry-picks, bundles, offline transfer',
  frank: 'chaos developer: constantly changes his mind (undo, redo, amend, stash, restore, abort) and leaves things half-finished',
  grace: 'auditor: reads history more than she writes; blame, grep, diff, show, fsck, gc; double-checks what other people claim landed',
  heidi: 'remote developer who is only ever connected over HTTP (her origin is the http URL, never the filesystem path)',
}

// ─── Shared project tasks ───────────────────────────────────────────────────
const TASKS = {
  T1: 'bulk discount: add bulk_discount(amount, qty) to shop/pricing.py (10% off from 10 items, 20% off from 50), with a test in tests/',
  T2: 'low-stock report: add low_stock(inv) to shop/reports.py returning SKUs under LOW_STOCK_THRESHOLD, with a test',
  T3: 'order cancel/refund: add Order.cancel() and Order.refund_total() to shop/orders.py, with a test',
  T4: 'currency formatting: add format_money(amount) to shop/pricing.py using config.CURRENCY, with a test',
  T5: 'warehouses: make Inventory take an optional warehouse name defaulting to config.DEFAULT_WAREHOUSE, with a test',
  T6: 'CSV export: add stock_csv(inv) to shop/reports.py, with a test',
  T7: 'product loader: add shop/catalog.py that loads data/products.json (keep that JSON file valid; add two products), with a test',
  T8: 'docs: extend docs/guide.md with a section per module, and keep README.md module list in sync',
}
const COMMON_TASK_RULES =
  'Every feature task also needs: one line registering it in shop/registry.py (append at the very end of the file, after the "registered features" comment: ' +
  'REGISTRY entries like register("<feature>", None)), and one line in CHANGELOG.md under "Unreleased". Everyone appends at the end of those files, ' +
  'which makes them conflict hot-spots on purpose. Put work markers on your lines as described in your instructions.'

// ─── Round themes and per-persona missions ──────────────────────────────────
const THEMES = [
  'ROUND 1: FEATURE SPRINT. Each developer builds a feature on their own branch, then integrates into main and pushes. Several of you are doing the same task independently.',
  'ROUND 2: COLLISION. Trunk-based: commit small changes straight to main and push, pull and merge constantly. Everyone edits the same hot files at the same time. Expect push refusals and conflicts; handle them the way a careful team member would.',
  'ROUND 3: HISTORY SURGERY. Renames, rebases, cherry-picks, squashes, blame through renames, graph inspection, all while the others keep pushing.',
  'ROUND 4: RELEASE AND CHAOS. Releases, bundles, tags, interrupted and aborted operations, large files, maintenance commands, while the others keep pushing.',
]
const M = {
  1: {
    alice: `Do ${TASKS.T1} on branch feature/bulk-discount (use at least 3 snapshots). Merge it into main (pull first), run the tests, push main. Then do ${TASKS.T8} on main in a separate small snapshot.`,
    bob: `Do ${TASKS.T1} on branch feature/bulk-pricing (you and alice are doing the same task independently, write it your own way, same function name bulk_discount). Then ${TASKS.T5} on branch feature/warehouses. Merge both into main (pull first; expect conflicts with alice's work), run the tests, push main.`,
    carol: `Do ${TASKS.T2} on branch feature/low-stock. Also change LOW_STOCK_THRESHOLD in shop/config.py to 8, and add a "reports" line to the README module list. Merge into main (pull first), run the tests, push main.`,
    dan: `Do ${TASKS.T3} on branch feature/cancel with at least 6 small snapshots (tiny steps). Push that branch to origin (push of a non-main branch). Fetch regularly; when the work is done, merge it into main (pull first), test, push main.`,
    erin: `Do ${TASKS.T6} on branch feature/csv. Merge into main (pull first), test, push main. Then tag main v0.1.0-rc1 and create a bundle of the whole repository at ${SIM}/bundles/erin-r1.bundle (mkdir -p first).`,
    frank: `Do ${TASKS.T4} on branch feature/money. Practice: make three snapshots, undo one, redo it, amend the last one with a better message, make a partial save of only one file while another file stays modified, stash the remaining change under a name, switch branch and come back and pop the stash. Then merge into main (pull first), test, push main.`,
    grace: `Do ${TASKS.T8} on branch docs/guide (docs only). Meanwhile, every few steps, fetch and audit what the others have pushed: run fsck, blame a file on origin/main, grep for "def " in a past snapshot, diff two tags/branches. Merge your docs into main (pull first), push main. Record in your final notes anything about attribution (author per line) that looks wrong.`,
    heidi: `You are connected only via HTTP (your origin is already the http URL; check "velo remote"). Do ${TASKS.T7} on branch feature/catalog and push that branch over HTTP. Merge into main (pull first), test, push main over HTTP. Keep data/products.json valid JSON through every merge.`,
  },
  2: {
    alice: 'Trunk-based, on main. Three small changes, each pulled/merged and pushed separately: (1) in shop/config.py set TAX_RATE = 0.19, (2) add a function to shop/pricing.py, (3) add a registry line and a CHANGELOG line. Pull before each push.',
    bob: 'Trunk-based, on main. Three small changes, each pushed separately: (1) in shop/config.py set TAX_RATE = 0.2 (you are deliberately colliding with alice), (2) append to shop/pricing.py, (3) add a registry line and a CHANGELOG line. When a push is refused or a merge conflicts, resolve by hand-editing the sensible combination, run tests, save, push.',
    carol: 'Trunk-based, on main. Change CURRENCY and MAX_ITEMS_PER_ORDER in shop/config.py (the same file everyone is editing), edit the first line of docs/guide.md, and reword the README intro. Use "velo resolve" non-interactively (--take ours / --take theirs, and --all once) at least twice on real conflicts, and verify the result is what you intended.',
    dan: 'Trunk-based, on main. Make 5 tiny snapshots in a row, each touching NOTES.txt (append a marker line) and pushing each time. NOTES.txt is an append-hotspot for everybody; after every push-refusal pull and merge.',
    erin: 'Trunk-based, on main. Edit CHANGELOG.md: add a "## 0.1.0 candidate" heading just under Unreleased and move lines under it as they appear. Also append to shop/registry.py. Pull/merge/push after each step.',
    frank: 'Trunk-based, on main, but unsure of yourself: make a change to shop/orders.py, save, then undo it and redo it; start a second change, stash it while pulling; pop it; save --amend; push. If a merge conflicts, practise "merge --abort", check the tree is exactly as before ("diff" must be empty), then redo the merge for real.',
    grace: 'Trunk-based, on main. Edit docs/guide.md (lines 2 and 4) and append to NOTES.txt. In between, audit: "history --all --graph" on your clone, "history --file shop/config.py" to see who touched it, blame the file after each pull. Report any line whose blame author or snapshot is not what you expect.',
    heidi: 'Trunk-based, on main, over HTTP only. Add two products to data/products.json (valid JSON always; the file is a conflict hot-spot), append to shop/registry.py, add a CHANGELOG line. Push after each; when refused, pull and merge.',
  },
  3: {
    alice: 'Rebase practice: create branch feature/rebase-demo off main with 3 snapshots editing shop/inventory.py; then others will have advanced main; rebase your branch onto origin/main. Then fetch, and cherry-pick one snapshot from someone else\'s pushed branch (see "branches" and origin/<name>) onto main if it applies cleanly. Squash your last 3 snapshots on the branch into one, merge it to main, test, push.',
    bob: 'Create branch release/0.1 from the v0.1.0-rc1 tag (erin made it; fetch it, it may not exist yet: tags travel with bundles or pushes, check; if it is missing say so and use a snapshot hash instead). Cherry-pick two fixes from main onto it, start a merge of main into it, "merge --abort" once, then merge for real, test, and push the branch.',
    carol: 'Conflict gauntlet: fetch, then merge each other developer\'s pushed branch (origin/<branch>) into a local branch merge/gauntlet in turn. For each conflict try resolving with --take theirs on one file and by hand-edit on another. Use "resolve --all --take ours" once. After every merge run the tests. Finally merge merge/gauntlet into main, test, push.',
    dan: 'RENAME: with "velo mv", rename shop/reports.py to shop/reporting.py and fix the imports and tests; save; push main. Others are editing shop/reports.py right now, so merges across your rename will happen to them and to you: after your rename, pull and make sure edits made to reports.py by others ended up in reporting.py (rename-aware merge). Then also velo mv docs/guide.md docs/handbook.md.',
    erin: 'History archaeology: "history --graph --all", "history --from <a snapshot of a merge>", "history --file shop/reporting.py" (and shop/reports.py: does the history follow the rename once dan has pushed it?), "diff <a>..<b>" between your tag and main, "show" of a merge snapshot, "restore <tag> -- shop/config.py" to a path then save, tag v0.1.0-rc2 on main and push main.',
    frank: 'Rebase with conflicts: create branch wip/frank editing shop/config.py and shop/orders.py (same lines others change), then "rebase origin/main". On conflict: first "rebase --abort" and verify you are back where you started, then rebase again, resolve, "rebase --continue" until done. Squash, merge to main, test, push.',
    grace: 'Blame through renames: when dan has pushed his velo mv of shop/reports.py to shop/reporting.py (fetch and check; if not yet, do the other checks first and retry), run blame on shop/reporting.py at origin/main and at the older snapshots with "blame --at", and with "--lines 1-10". The original authors of the surviving lines must still be shown. Also "grep --snapshot" in old snapshots. Report precisely what is lost or wrong.',
    heidi: 'Over HTTP: create three branches with small commits (feature/h1, feature/h2, feature/h3), push all of them, delete feature/h3 locally with "branches --delete", run gc, fetch, merge feature/h1 and feature/h2 into main, test, push main.',
  },
  4: {
    alice: 'Release crunch. Pull main, run the tests; if anything is broken fix it with a minimal change. Then add a final CHANGELOG line, and use stash to switch to another branch and back while you have unsaved work. Push main.',
    bob: 'Release crunch. On main, make 4 snapshots, then squash the last 3, then push. Then undo the squash (undo) and see exactly what state you are in; redo if sensible; make sure your local main equals what you intend, and push only if it is fast-forward.',
    carol: 'Release crunch. Make conflicting edits against the newest main on purpose (config.py lines 1-3), resolve by hand AND with --take theirs; complete the merge; test; push. Then run "velo fsck" and "velo gc".',
    dan: 'Release crunch. Make a snapshot with a file deleted and another file moved back with velo mv (e.g. NOTES.txt to docs/notes.txt), push. Then verify with "history --file docs/notes.txt" that history follows the move.',
    erin: `RELEASE: on main (pull first, tests green) tag v0.2.0 (tag --force it once to move it, then tag --delete a throwaway tag), push main. Create a full bundle at ${SIM}/bundles/erin-r4.bundle and a bundle limited to the v0.2.0 tag at ${SIM}/bundles/erin-r4-tag.bundle. Then in a new empty directory ${SIM}/bundle-check run "velo init", "velo bundle apply" with each bundle, run fsck and compare the history with the original. Apply the same bundle twice (it must be idempotent).`,
    frank: 'Chaos. Start a merge of a branch and leave it half-resolved, then try other commands (switch, save, status): what does velo say? Then recover properly ("merge --abort" or resolve), run "restore --force" on a snapshot and then come back to main, "switch --force" with dirty files, "gc --keep-days 0" and "fsck --repair". Finally make sure you are on main, clean, and pull.',
    grace: 'Final audit. Fetch. For every marker in the ledger directory (' + SIM + '/ledger/*.jsonl) with status merged_main, grep origin/main (grep --snapshot origin/main) and report any marker that cannot be found. Run fsck, gc on your clone, and fsck again. Read-only otherwise; push nothing except a final NOTES.txt line.',
    heidi: 'Large file over HTTP: create data/blob.bin (about 4 MB of pseudo-random bytes, e.g. python -c with random.Random(1).randbytes), save, push over HTTP, then modify 100 bytes in the middle, save again, push again. Then fetch and confirm "show" reports it sensibly. Others will fetch/pull it, so keep it on main. Finally verify with a fresh "velo clone" of the http URL into a scratch directory under the sim dir that the blob matches (compare sizes and a hash).',
  },
}

// ─── Schemas ────────────────────────────────────────────────────────────────
const BUG = {
  type: 'object',
  properties: {
    title: { type: 'string' },
    category: { type: 'string', enum: ['crash', 'data-loss', 'wrong-result', 'bad-refusal', 'unrecoverable-state', 'hang', 'ux', 'perf'] },
    command: { type: 'string' },
    expected: { type: 'string' },
    actual: { type: 'string' },
    repro: { type: 'string' },
    severity: { type: 'string', enum: ['high', 'medium', 'low'] },
  },
  required: ['title', 'category', 'command', 'expected', 'actual'],
}
const ROUND_RESULT = {
  type: 'object',
  properties: {
    summary: { type: 'string', description: 'three sentences: what you did and how it ended' },
    featuresUsed: { type: 'array', items: { type: 'string', enum: FEATURES } },
    commandsRun: { type: 'number' },
    stuck: { type: 'boolean', description: 'true if you could not complete your missions or had to re-clone' },
    easeRating: { type: 'number', description: '1 (painful) to 5 (effortless): how easy was velo to use this round' },
    uxNotes: {
      type: 'object',
      properties: {
        intuitive: { type: 'array', items: { type: 'string' } },
        confusing: { type: 'array', items: { type: 'string' }, description: 'each item names the command' },
        missing: { type: 'array', items: { type: 'string' } },
      },
    },
    suspectedBugs: { type: 'array', items: BUG },
  },
  required: ['summary', 'featuresUsed', 'stuck', 'suspectedBugs'],
}
const ORACLE_RESULT = {
  type: 'object',
  properties: {
    ok: { type: 'boolean' },
    problems: { type: 'array', items: { type: 'string' } },
    warnings: { type: 'array', items: { type: 'string' } },
    remoteBranches: { type: 'string' },
    ledgerClaims: { type: 'number' },
  },
  required: ['ok', 'problems', 'warnings'],
}
const TRIAGE_RESULT = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['velo-bug', 'ux-issue', 'agent-error', 'by-design', 'cannot-reproduce'] },
    confidence: { type: 'string', enum: ['high', 'medium', 'low'] },
    explanation: { type: 'string' },
    minimalRepro: { type: 'string', description: 'a bash script that reproduces it from an empty directory, using $VELO' },
  },
  required: ['verdict', 'confidence', 'explanation'],
}

// ─── Prompts ────────────────────────────────────────────────────────────────
function devPrompt(name, round, mission, theme) {
  return [
    `You are ${name}, ${PERSONA_ROLE[name]}.`,
    `Your clone: ${cdir(name)}`,
    `Your velo wrapper (V): ${vpath(name)}`,
    `Sim directory (SIM): ${SIM}   (your ledger: ${SIM}/ledger/${name}.jsonl; create the directory if missing)`,
    `This is round ${round}. Marker prefix for your work: [[${name}-r${round}-<n>]].`,
    `Teammates working right now, concurrently: ${PERSONAS.filter(p => p !== name).join(', ')}.`,
    '',
    theme,
    '',
    COMMON_TASK_RULES,
    '',
    'YOUR MISSIONS THIS ROUND:',
    mission,
    '',
    'Integration routine for main: "pull"; if it reports divergence, merge origin/main into your main, resolve conflicts, run the tests, save, then push. If the push is refused because someone pushed in the meantime, repeat. Give up after 6 attempts and report it.',
    'Leave your clone in a sensible state at the end (no half-finished merge/rebase unless a mission asked for it; if so, finish or abort it before you stop). Unpushed work on side branches is fine.',
    'Then return the structured result.',
  ].join('\n')
}

function convergePrompt(name, attempt) {
  return [
    `You are ${name}. Final synchronisation (attempt ${attempt}). Your clone: ${cdir(name)}. Your velo wrapper (V): ${vpath(name)}. Sim directory: ${SIM}.`,
    'Goal: your local main must be IDENTICAL to main on origin, with nothing in progress, and the project tests passing.',
    '1. Look at "status". If a merge or rebase is in progress, finish it sensibly or abort it. If you have unsaved changes, save them on a side branch ("switch <name>" creates it) or discard them with "restore --force" / "switch --force" if they are junk.',
    '2. "switch main"; "pull". If it reports divergence, merge origin/main, resolve, test, save, push. If your main is ahead and the push succeeds, fine. If the push is refused, pull again and retry (up to 6 times).',
    '3. Run the tests (python -m unittest discover -s tests). If main is broken, fix it with a minimal change and push.',
    '4. "fetch"; "status" must say you are up to date with origin/main. Run "fsck".',
    'Teammates are doing the same right now, so main may move: if status shows you behind after your push, pull once more.',
    'For anything where velo itself looked wrong, report it as a suspected bug. Return the structured result (featuresUsed may be empty).',
  ].join('\n')
}

function oraclePrompt(label, converge) {
  return [
    `Run this command exactly and return its JSON output as structured data:`,
    `python ${ROOT}/sim/team/oracle.py ${SIM} ${label}${converge ? ' --converge' : ''}`,
    'It takes up to two minutes; wait for it. Fill the structured result from the JSON: ok, problems (each string verbatim), warnings (each verbatim), remoteBranches (the remoteBranches object as a compact JSON string), ledgerClaims.',
    'If the command itself crashes (traceback) set ok=false and put the last 20 lines of its error output in problems. Do not try to fix anything.',
  ].join('\n')
}

// ─── Run ────────────────────────────────────────────────────────────────────
const bugs = []   // { round, agent, ...BUG }
const covered = new Set()
const stuck = []
const roundReports = []
const selfReports = {} // developer -> [{ round, easeRating, uxNotes }]
const noteSelf = (name, round, res) => {
  if (!res) return
  ;(selfReports[name] = selfReports[name] || []).push({ round, easeRating: res.easeRating ?? null, uxNotes: res.uxNotes || {}, summary: res.summary })
}

phase('Setup')
const setup = await agent(
  [
    `Run exactly this and report whether it worked:`,
    `python ${ROOT}/sim/team/setup.py ${SIM}${BUILD ? '' : ' --no-build'} --personas=${PERSONAS.join(',')}`,
    BUILD ? 'It runs cargo build --release first; that takes a few minutes, so use a long timeout (up to 10 minutes) or run it in the background and wait.' : '',
    'Success means it prints a JSON object with "clones". Return ok=true and that JSON object as a string in "details". On failure return ok=false and the last 30 lines of output; do not try to repair anything.',
  ].join('\n'),
  {
    label: 'setup', phase: 'Setup', model: 'haiku',
    schema: { type: 'object', properties: { ok: { type: 'boolean' }, details: { type: 'string' } }, required: ['ok', 'details'] },
  },
)
if (!setup || !setup.ok) throw new Error('setup failed: ' + (setup ? setup.details : 'agent died'))
log(`sandbox ready at ${SIM} with ${PERSONAS.length} developers: ${PERSONAS.join(', ')}`)

for (let r = 1; r <= ROUNDS; r++) {
  phase('Rounds')
  log(`round ${r}/${ROUNDS}: ${THEMES[r - 1].split('.')[0]}`)
  const results = await parallel(PERSONAS.map(name => () =>
    agent(devPrompt(name, r, M[r][name], THEMES[r - 1]), {
      label: `r${r}:${name}`, phase: 'Rounds', agentType: 'velo-sim-dev', schema: ROUND_RESULT,
    }).then(res => ({ name, res }))))
  for (const x of results) {
    if (!x || !x.res) { stuck.push(`${x ? x.name : '?'} r${r}: agent died or was skipped`); continue }
    x.res.featuresUsed.forEach(f => covered.add(f))
    noteSelf(x.name, `r${r}`, x.res)
    if (x.res.stuck) stuck.push(`${x.name} r${r}: ${x.res.summary}`)
    for (const b of x.res.suspectedBugs) bugs.push({ round: r, agent: x.name, ...b })
  }
  const oracle = await agent(oraclePrompt(`round${r}`, false), { label: `oracle:r${r}`, phase: 'Rounds', model: 'haiku', schema: ORACLE_RESULT })
  roundReports.push({ round: r, oracle })
  if (!oracle) log(`round ${r}: oracle agent died`)
  else {
    log(`round ${r} oracle: ${oracle.ok ? 'OK' : 'PROBLEMS'} (${oracle.problems.length} problems, ${oracle.warnings.length} warnings, ${oracle.ledgerClaims ?? '?'} ledger claims)`)
    for (const p of oracle.problems) bugs.push({ round: r, agent: 'oracle', title: p.slice(0, 120), category: 'data-loss', command: 'oracle', expected: 'invariant holds', actual: p, severity: 'high' })
  }
}

phase('Converge')
let finalOracle = null
for (let a = 1; a <= MAX_CONVERGE; a++) {
  const res = await parallel(PERSONAS.map(name => () =>
    agent(convergePrompt(name, a), { label: `sync${a}:${name}`, phase: 'Converge', agentType: 'velo-sim-dev', schema: ROUND_RESULT })
      .then(x => ({ name, x }))))
  for (const y of res) {
    if (!y || !y.x) continue
    y.x.featuresUsed.forEach(f => covered.add(f))
    noteSelf(y.name, `sync${a}`, y.x)
    if (y.x.stuck) stuck.push(`${y.name} sync${a}: ${y.x.summary}`)
    for (const b of y.x.suspectedBugs) bugs.push({ round: 'sync', agent: y.name, ...b })
  }
  finalOracle = await agent(oraclePrompt(`final${a}`, true), { label: `oracle:final${a}`, phase: 'Converge', model: 'haiku', schema: ORACLE_RESULT })
  if (finalOracle && finalOracle.ok) { log(`converged after ${a} sync round(s)`); break }
  log(`sync ${a}: not converged: ${finalOracle ? finalOracle.problems.slice(0, 3).join(' | ') : 'oracle died'}`)
}
if (finalOracle && !finalOracle.ok) {
  for (const p of finalOracle.problems) bugs.push({ round: 'final', agent: 'oracle', title: p.slice(0, 120), category: 'wrong-result', command: 'oracle --converge', expected: 'all clones equal remote main', actual: p, severity: 'high' })
}

// ─── Performance ────────────────────────────────────────────────────────────
// Runs on a quiet machine: no developer or triage agent is active during the benchmark.
const PERF_RESULT = {
  type: 'object',
  properties: {
    ok: { type: 'boolean' },
    summary: { type: 'string' },
  },
  required: ['ok', 'summary'],
}
const FINDING = {
  type: 'object',
  properties: {
    metric: { type: 'string' },
    observed: { type: 'string' },
    expectation: { type: 'string', description: 'budget, git baseline, or scaling expectation it is judged against' },
    severity: { type: 'string', enum: ['high', 'medium', 'low'] },
    likelyCause: { type: 'string' },
    suggestion: { type: 'string' },
    confidence: { type: 'string', enum: ['high', 'medium', 'low'] },
  },
  required: ['metric', 'observed', 'severity', 'suggestion'],
}
const PERF_ANALYSIS = {
  type: 'object',
  properties: {
    grade: { type: 'string', enum: ['A', 'B', 'C', 'D', 'F'], description: 'overall smoothness / speed for a tool that must feel instant' },
    verdict: { type: 'string' },
    findings: { type: 'array', items: FINDING },
    scaling: { type: 'array', items: { type: 'string' }, description: 'how cost grows with files / history / concurrency' },
    strengths: { type: 'array', items: { type: 'string' } },
    reportFile: { type: 'string' },
  },
  required: ['grade', 'verdict', 'findings', 'strengths'],
}

let perfRun = null
let perfAnalysis = null
const runPerfAnalysis = async () => {
  if (!PERF) return null
  return await agent(
    [
      'You are a performance engineer reviewing velo, a Rust snapshot-based version control tool (source: ' + ROOT + '/crates, docs: ' + ROOT + '/ARCHITECTURE.md). The goal of the project is a tool that feels smooth and ultra fast.',
      `Read these files from the simulation sandbox ${SIM}:`,
      `- perf/results.json: an idle-machine benchmark. Every metric has velo_ms, git_ms (git on the same data, when comparable), a budget_ms (an initial guess, so judge whether it is itself reasonable), and a verdict. "notes" has storage and concurrency observations. Sizes: ${PERF_FILES} files, ${PERF_HISTORY} snapshots, ${PERF_BIG_MB} MB big file.`,
      `- perf/cmdstats.json: statistics from every velo call the simulated developers made while all of them ran at once (so absolute times are inflated by contention; use it for relative costs, outliers, and lock-contention hits).`,
      '',
      'Judge as a performance engineer would:',
      '1. Everything flagged (over-budget, slow-vs-git, error). Real regression or noise? Metrics under ~50 ms are dominated by process start; do not over-read them.',
      '2. Scaling: does latency grow with history depth (see "save latency growth"), file count, file size? Which commands are O(history) that should not be?',
      '3. Concurrency: errors, lock contention, "status while a writer saves", the contention hits in cmdstats. Does a busy repo block readers or other writers?',
      '4. Storage efficiency (notes): .velo size vs .git, and big-file edit growth.',
      '5. For each real problem, read the relevant code path in the source to give a likely cause and a concrete suggestion (file and function if you can find it). Say when you did not verify. Do not modify any file in the repo or the sandbox clones.',
      '6. List strengths too: where velo is clearly faster than git.',
      `Write a readable markdown report to ${SIM}/reports/perf-report.md (mkdir -p first): a table of all metrics with verdicts, then findings ranked by severity. Return the structured result with reportFile set to that path.`,
    ].join('\n'),
    { label: 'perf-analysis', phase: 'Performance', agentType: 'general-purpose', model: UX_MODEL, schema: PERF_ANALYSIS },
  )
}

phase('Performance')
if (PERF) {
  perfRun = await agent(
    [
      'Run the velo benchmark and the command-log digest, and report whether they finished. Do not change anything.',
      `1. Start the benchmark (it takes 10-25 minutes; nothing else is running, so do not run anything heavy yourself):`,
      `   python ${ROOT}/sim/team/perf.py ${SIM} --detach --files ${PERF_FILES} --history ${PERF_HISTORY} --big-mb ${PERF_BIG_MB}`,
      '   (--detach makes it relaunch itself as an independent process and return at once; it logs to perf/run.log. Do NOT use nohup or "&", and do NOT end your turn before the run finishes: the benchmark must complete while you are still polling.)',
      `2. Poll every 60 seconds with: sleep 60; tail -n 5 ${SIM}/perf/run.log   until the log contains a line with "metrics," and "flagged" (that is the end), or a Python traceback (failure), or 25 minutes have passed (give up: ok=false).`,
      `3. Then run: python ${ROOT}/sim/team/digest.py ${SIM}`,
      'Return ok=true and a summary string made of the "N metrics, M flagged" line plus the digest.py output line. On failure return ok=false and the last 30 lines of the relevant output.',
    ].join('\n'),
    { label: 'perf-run', phase: 'Performance', model: 'haiku', schema: PERF_RESULT },
  )
  if (!perfRun || !perfRun.ok) log('performance run failed: ' + (perfRun ? perfRun.summary.slice(0, 300) : 'agent died'))
  else log('performance: ' + perfRun.summary.slice(0, 300))
} else {
  perfRun = await agent(
    `Run: python ${ROOT}/sim/team/digest.py ${SIM}  and return ok=true with its output line as summary (ok=false and the error otherwise).`,
    { label: 'digest', phase: 'Performance', model: 'haiku', schema: PERF_RESULT },
  )
}
const perfOk = !!(perfRun && perfRun.ok)

// ─── Triage ─────────────────────────────────────────────────────────────────
// (the perf analyst only reads files, so it runs alongside triage)
phase('Triage')
// dedupe by normalised title; keep the first reporter and count the rest
const byTitle = new Map()
for (const b of bugs) {
  const key = b.title.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim()
  if (byTitle.has(key)) byTitle.get(key).alsoReportedBy.push(`${b.agent}/r${b.round}`)
  else byTitle.set(key, { ...b, alsoReportedBy: [] })
}
const unique = [...byTitle.values()]
unique.sort((a, b) => ({ high: 0, medium: 1, low: 2 }[a.severity || 'low']) - ({ high: 0, medium: 1, low: 2 }[b.severity || 'low']))
const toTriage = unique.slice(0, MAX_TRIAGE)
if (unique.length > toTriage.length) log(`triage cap: ${unique.length - toTriage.length} lower-severity reports NOT triaged (maxTriage=${MAX_TRIAGE})`)
log(`${bugs.length} raw reports, ${unique.length} unique, triaging ${toTriage.length}`)

const [triaged, perfAnalysisResult] = await parallel([
  () => pipeline(
    toTriage,
    (b, _item, i) => agent(
      [
        'A simulated developer reported a suspected bug in velo (a snapshot-based VCS, Rust, source in ' + ROOT + '/crates). Decide whether it is real.',
        JSON.stringify(b, null, 2),
        '',
        `Evidence you may use: the sim sandbox at ${SIM} (read-only; do not modify clones or the remote), the ledger in ${SIM}/ledger, the full command log in ${SIM}/cmdlog/<developer>.jsonl (every call with its output), and the velo source.`,
        `To reproduce, make a scratch directory ${SIM}/triage/t${i} and use the real binary ${SIM}/bin/real/velo.exe (set VELO_AUTHOR_NAME=triage). Reproduce from an EMPTY directory with a minimal script. Try it twice.`,
        'Classify: velo-bug (reproduces, behaviour is wrong), ux-issue (correct but misleading/unclear), agent-error (the developer misused it), by-design (check docs in the README/help text), cannot-reproduce.',
        'Be sceptical: developers are small models and often misread output. A velo-bug needs a reproduction you ran yourself.',
      ].join('\n'),
      { label: `triage:${i}:${b.title.slice(0, 30)}`, phase: 'Triage', agentType: 'general-purpose', schema: TRIAGE_RESULT },
    ).then(t => (t ? { report: b, ...t } : null)),
  ),
  () => (perfOk ? runPerfAnalysis() : null),
])
perfAnalysis = perfAnalysisResult

phase('Teardown')
await agent(`Run: python ${ROOT}/sim/team/teardown.py ${SIM}  and report nothing else.`, { label: 'teardown', phase: 'Teardown', model: 'haiku' })

// ─── UX review ──────────────────────────────────────────────────────────────
const FRICTION = {
  type: 'object',
  properties: {
    command: { type: 'string', description: 'the velo command(s) involved' },
    whatHappened: { type: 'string' },
    whyItIsAProblem: { type: 'string' },
    category: { type: 'string', enum: ['discoverability', 'error-message', 'naming', 'flag-consistency', 'output-clarity', 'recovery', 'safety', 'workflow-gap', 'mental-model', 'help-text', 'speed-feel'] },
    severity: { type: 'string', enum: ['blocker', 'major', 'minor', 'nit'] },
    evidence: { type: 'string', description: 'quote the developer command/output from the digest' },
    gitComparison: { type: 'string', description: 'what a git user would expect or how git handles it; empty if not relevant' },
    suggestion: { type: 'string' },
  },
  required: ['command', 'whatHappened', 'whyItIsAProblem', 'category', 'severity', 'suggestion'],
}
const UX_PERSONA = {
  type: 'object',
  properties: {
    overallEase: { type: 'number', description: '1 (hostile) to 5 (effortless), your own judgement from the log, not the developer\'s self-rating' },
    summary: { type: 'string' },
    frictions: { type: 'array', items: FRICTION },
    delights: { type: 'array', items: { type: 'string' }, description: 'things that clearly worked well, with the command' },
    mentalModelGaps: { type: 'array', items: { type: 'string' }, description: 'concepts the developer repeatedly got wrong' },
    selfReportVsLog: { type: 'string', description: 'where the developer\'s own ratings/notes disagree with what the log shows' },
  },
  required: ['overallEase', 'summary', 'frictions', 'delights'],
}
const SURFACE = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    inconsistencies: { type: 'array', items: FRICTION },
    helpTextGaps: { type: 'array', items: FRICTION },
    strengths: { type: 'array', items: { type: 'string' } },
  },
  required: ['summary', 'inconsistencies', 'helpTextGaps', 'strengths'],
}
const SCORE = {
  type: 'object',
  properties: { score: { type: 'number' }, evidence: { type: 'string' } },
  required: ['score', 'evidence'],
}
const UX_REPORT = {
  type: 'object',
  properties: {
    verdict: { type: 'string', description: 'is velo intuitive and easy to use? three to five sentences, honest' },
    scorecard: {
      type: 'object',
      description: 'each 1 (poor) to 5 (excellent)',
      properties: {
        discoverability: SCORE, errorMessages: SCORE, consistency: SCORE, recoverability: SCORE, outputClarity: SCORE,
        learnabilityForGitUsers: SCORE, collaborationFlow: SCORE, helpText: SCORE, safety: SCORE,
      },
      required: ['discoverability', 'errorMessages', 'consistency', 'recoverability', 'outputClarity', 'learnabilityForGitUsers', 'collaborationFlow', 'helpText', 'safety'],
    },
    topImprovements: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          title: { type: 'string' }, impact: { type: 'string', enum: ['high', 'medium', 'low'] }, effort: { type: 'string', enum: ['small', 'medium', 'large'] },
          rationale: { type: 'string' }, evidence: { type: 'string' }, affectedCommands: { type: 'array', items: { type: 'string' } },
        },
        required: ['title', 'impact', 'effort', 'rationale'],
      },
    },
    quickWins: { type: 'array', items: { type: 'string' } },
    whatWorksWell: { type: 'array', items: { type: 'string' } },
    mostConfusingCommands: { type: 'array', items: { type: 'string' } },
    reportFile: { type: 'string' },
  },
  required: ['verdict', 'scorecard', 'topImprovements', 'quickWins', 'whatWorksWell'],
}

phase('UX Review')
const missionsOf = name => [1, 2, 3, 4].slice(0, ROUNDS).map(r => `round ${r}: ${M[r][name]}`).join('\n')
const personaPrompt = name => [
  `You are a UX researcher evaluating velo, a snapshot-based version control tool that wants to be more intuitive than git (no staging area, undo/redo, named stashes, explicit merges, refuse-rather-than-guess sync). You are reviewing ONE developer's real usage: ${name}, ${PERSONA_ROLE[name]}. This developer is a small model that had to learn velo from "velo help" and error messages alone.`,
  '',
  `Read in full: ${SIM}/ux/${name}.md. It is a digest of every velo command they ran (an automatic wrapper recorded them, so it is ground truth): stats, every failing command with the output and their next steps, their help lookups, and a sample of successes. For the exact raw log, ${SIM}/cmdlog/${name}.jsonl (JSON lines: ts, args, rc, ms, out) is available; grep it for context when the digest is not enough. Also skim ${SIM}/ux/clusters.json for how the same errors hit other developers.`,
  '',
  'What they were trying to do (their missions):',
  missionsOf(name),
  '',
  'Their own end-of-round self-reports (treat as testimony, check against the log):',
  JSON.stringify(selfReports[name] || [], null, 1).slice(0, 6000),
  '',
  'Evaluate velo, not the developer. A failure is a UX finding when a careful, intelligent user would plausibly have done the same: the command name or flag was guessable but wrong, an error did not say how to fix it, the output was ambiguous, a recovery path was missing or scary, the same mistake repeated across rounds, or help had to be consulted repeatedly for something that should be obvious. Failures that are plainly the developer\'s sloppiness are not findings. Exit codes matter: a command that fails when it merely had nothing to do (or the reverse) is a finding.',
  'Check explicitly: (1) discoverability: how did they find the right command; (2) error messages: did each one lead to the fix; (3) consistency of names/flags between commands; (4) recovery after mistakes (undo, abort, restore); (5) clarity of success output (could they tell what state they were in); (6) trust and safety (did anything risk their work); (7) how a git user would compare.',
  'Be concrete: every friction cites the exact command and output. Rank by how much time or work the user lost.',
].join('\n')

const surfacePrompt = [
  `You are a UX/CLI-design reviewer. Audit the command-line surface of velo with the real binary ${SIM}/bin/real/velo.exe (do not run it inside any developer clone; run "help" only, or work in a scratch directory under ${SIM}/ux/scratch).`,
  'Run "velo --help" and "velo help <command>" for EVERY subcommand (and nested subcommands such as stash, remote, bundle, tag). Then judge the surface as a whole:',
  '- naming: verbs consistent and guessable? (save/restore/switch/squash/undo/redo/resolve/...) collisions with git meanings that would mislead a git user?',
  '- flag consistency: the same concept spelled the same way everywhere (--force, --abort, --take, --limit, --all, targets as <hash|tag|branch>, "--" path separators)?',
  '- help quality: does every command have a one-line summary, at least one example, the failure modes, and a pointer to what to run next? Are errors written to guide the user?',
  '- exit codes and output channels: nothing-to-do vs error, machine-readability, colour/unicode assumptions on a plain terminal.',
  '- safety defaults: which commands can destroy unsaved work, and do they warn or require --force?',
  'Cite the exact help text you are criticising. Do not edit anything in the repo.',
].join('\n')

const uxResults = await parallel([
  ...PERSONAS.map(name => () =>
    agent(personaPrompt(name), { label: `ux:${name}`, phase: 'UX Review', agentType: 'general-purpose', model: UX_MODEL, schema: UX_PERSONA })
      .then(r => ({ name, r }))),
  () => agent(surfacePrompt, { label: 'ux:cli-surface', phase: 'UX Review', agentType: 'general-purpose', model: UX_MODEL, schema: SURFACE }).then(r => ({ name: 'surface', r })),
])
const personaReviews = uxResults.filter(x => x && x.r && x.name !== 'surface')
const surfaceReview = (uxResults.find(x => x && x.name === 'surface') || {}).r || null
if (personaReviews.length < PERSONAS.length) log(`UX: ${PERSONAS.length - personaReviews.length} developer review(s) missing (agent died or was skipped)`)

const uxReport = await agent(
  [
    'You are the lead UX researcher for velo, a snapshot-based version control tool that aims to be more intuitive and easier to use than git. Below are independent reviews of how eight (or fewer) simulated developers actually used it, built from a ground-truth log of every command they ran, plus an audit of the CLI surface and the developers\' own self-reports. Synthesize them into a verdict on whether velo is intuitive and easy to use.',
    '',
    'Work like this: find the frictions that recur across developers and rounds (those are real), weigh single-developer frictions by severity, discard ones that look like the developer\'s own fault, and merge duplicates. You may verify claims against the raw logs (' + SIM + '/cmdlog/*.jsonl) and the failure clusters (' + SIM + '/ux/clusters.json). Do not edit anything in the repo.',
    'Scorecard: score each dimension 1-5 with one sentence of evidence (a concrete command and what happened). Be calibrated: do not hand out 4s and 5s unless the logs show smooth use; do not be harsh about things that merely differ from git if velo\'s way is clear.',
    `Top improvements: ranked by (user pain x how many developers hit it) / effort, each with the commands it affects, concrete evidence and a concrete proposed change (new message text, a flag alias, a default, a hint line printed on failure...). quickWins: changes under an hour. whatWorksWell: be specific, it tells the maintainers what NOT to change.`,
    `Write a readable markdown report to ${SIM}/reports/ux-report.md (mkdir -p first): verdict, scorecard table, ranked improvements with evidence quotes, quick wins, what works, per-developer one-paragraph summaries, and a short appendix on any disagreement between self-reports and logs. Return the structured result with reportFile set.`,
    '',
    '=== DEVELOPER REVIEWS ===',
    JSON.stringify(personaReviews.map(x => ({ developer: x.name, role: PERSONA_ROLE[x.name], ...x.r })), null, 1),
    '',
    '=== CLI SURFACE AUDIT ===',
    JSON.stringify(surfaceReview, null, 1),
    '',
    '=== DEVELOPER SELF-REPORTS (testimony) ===',
    JSON.stringify(selfReports, null, 1).slice(0, 20000),
  ].join('\n'),
  { label: 'ux:synthesis', phase: 'UX Review', agentType: 'general-purpose', model: UX_MODEL, schema: UX_REPORT },
)

const verdicts = (triaged || []).filter(Boolean)
return {
  sim: SIM,
  developers: PERSONAS,
  rounds: roundReports.map(r => ({
    round: r.round,
    ok: r.oracle ? r.oracle.ok : null,
    problems: r.oracle ? r.oracle.problems : ['oracle died'],
    warnings: r.oracle ? r.oracle.warnings : [],
  })),
  converged: !!(finalOracle && finalOracle.ok),
  finalProblems: finalOracle ? finalOracle.problems : ['oracle died'],
  coverage: {
    used: [...covered].sort(),
    notExercised: FEATURES.filter(f => !covered.has(f)),
  },
  stuck,
  confirmedBugs: verdicts.filter(v => v.verdict === 'velo-bug'),
  uxIssues: verdicts.filter(v => v.verdict === 'ux-issue'),
  dismissed: verdicts.filter(v => !['velo-bug', 'ux-issue'].includes(v.verdict)).map(v => ({ title: v.report.title, verdict: v.verdict, why: v.explanation })),
  rawReports: bugs.length,
  untriaged: unique.length - toTriage.length,
  performance: perfAnalysis
    ? { grade: perfAnalysis.grade, verdict: perfAnalysis.verdict, findings: perfAnalysis.findings, scaling: perfAnalysis.scaling, strengths: perfAnalysis.strengths, report: perfAnalysis.reportFile, benchmark: perfRun && perfRun.summary }
    : { skipped: !PERF, failed: PERF && !perfAnalysis, benchmark: perfRun && perfRun.summary },
  ux: uxReport
    ? { verdict: uxReport.verdict, scorecard: uxReport.scorecard, topImprovements: uxReport.topImprovements, quickWins: uxReport.quickWins, whatWorksWell: uxReport.whatWorksWell, mostConfusingCommands: uxReport.mostConfusingCommands, report: uxReport.reportFile }
    : { failed: true },
}
