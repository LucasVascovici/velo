export const meta = {
  name: 'implement-phase',
  description: 'Opus plans an ARCHITECTURE.md phase into tasks; Sonnet agents implement, test, commit and push each; Haiku confirms delivery; reviews are routed by risk; returns a summary table',
  whenToUse: 'Implementing a whole phase (e.g. "14") or one item (e.g. "14.3") of ARCHITECTURE.md end to end',
  phases: [
    { title: 'Plan', detail: 'Opus splits the phase into dependency-ordered tasks', model: 'opus' },
    { title: 'Refresh', detail: 'Sonnet reconciles a stale brief with the code that landed since', model: 'sonnet' },
    { title: 'Implement', detail: 'Sonnet implements, tests, commits and pushes each task', model: 'sonnet' },
    { title: 'Land', detail: 'Haiku confirms the commits are on the branch and measures them', model: 'haiku' },
    { title: 'Review', detail: 'Opus for high-risk tasks, Sonnet for low-risk tasks and re-reviews' },
    { title: 'Finalize', detail: 'Full check suite on the branch head; mark the phase done in the docs', model: 'sonnet' },
  ],
}

// ─── Arguments ──────────────────────────────────────────────────────────────
// {
//   phase: "14" | "14.3",          required
//   branch?: "phase-14",           default phase-<N>, dots → dashes
//   repoRoot?: "C:/…/velo",        absolute path; worktree lanes keep their build dirs at <repoRoot>/target-lane<N>
//   maxParallel?: 2,               worktree lanes; 0 = everything in the main checkout, one at a time
//   maxFixRounds?: 2,
//   planOnly?: bool,               plan, return, stop
//   plan?: object,                 an approved plan; skips the planner. Tasks may omit brief/acceptance/tests
//                                  when planFile is given — agents then read their own brief from it
//   planFile?: "C:/…/plan.json",   absolute path of the saved plan ({ plan } or a bare plan)
//   planBase?: sha,                commit the plan was written against; with refresh, briefs are reconciled with base..HEAD
//   refresh?: bool,                reconcile every brief with the code before implementing it
//   skip?: [taskId],              not run in this pass, and not counted as done
//   doneEarlier?: [taskId],       finished and approved in an earlier pass: not run, counted as done
//   reviewOnly?: { taskId: [sha] } already implemented and on the branch; land + review only
// }
const A = args || {}
const PHASE = String(A.phase || '').trim()
if (!PHASE) throw new Error('args.phase is required, e.g. {"phase": "14"} or {"phase": "14.3"}')
const BRANCH = A.branch || `phase-${PHASE.replace(/\./g, '-')}`
const ROOT = (A.repoRoot || '').replace(/\\/g, '/').replace(/\/$/, '')
const MAX_PAR = ROOT ? (A.maxParallel ?? 2) : 0 // lanes need an absolute root for their build dirs
const MAX_FIX = A.maxFixRounds ?? 2
const DONE_EARLIER = new Set(A.doneEarlier || [])
const SKIP = new Set([...(A.skip || []), ...DONE_EARLIER])
const REVIEW_ONLY = A.reviewOnly || {}
if (!ROOT && (A.maxParallel ?? 2) > 0) log('No repoRoot given — running every task in the main checkout')

// ─── Schemas ────────────────────────────────────────────────────────────────
const TASK_PROPS = {
  id: { type: 'string', description: 'Short stable id, e.g. "14.3-a"' },
  item: { type: 'string', description: 'ARCHITECTURE.md item number it belongs to, e.g. "14.3"' },
  title: { type: 'string' },
  depends_on: { type: 'array', items: { type: 'string' } },
  parallel_safe: { type: 'boolean' },
  risk: { type: 'string', enum: ['high', 'low'] },
  files: { type: 'array', items: { type: 'string' }, description: 'Files expected to be created or modified' },
  brief: { type: 'string', description: 'Self-contained implementation spec (markdown)' },
  acceptance: { type: 'array', items: { type: 'string' } },
  tests: { type: 'string', description: 'Tests to write and where' },
}
const PLAN_SCHEMA = {
  type: 'object',
  properties: {
    phase_title: { type: 'string' },
    summary: { type: 'string', description: 'What the phase delivers, 2-4 sentences' },
    skipped_items: { type: 'array', items: { type: 'string' }, description: 'Doc items not planned and why' },
    tasks: {
      type: 'array',
      items: { type: 'object', properties: TASK_PROPS, required: ['id', 'item', 'title', 'depends_on', 'parallel_safe', 'risk', 'files', 'brief', 'acceptance', 'tests'] },
    },
  },
  required: ['phase_title', 'summary', 'tasks'],
}

const REFRESH_SCHEMA = {
  type: 'object',
  properties: {
    changed: { type: 'boolean', description: 'false when the brief still matches the code as written' },
    brief: { type: 'string' },
    files: { type: 'array', items: { type: 'string' } },
    acceptance: { type: 'array', items: { type: 'string' } },
    tests: { type: 'string' },
    notes: { type: 'string', description: 'What changed and why, one or two sentences' },
  },
  required: ['changed', 'notes'],
}

const CHECK = { type: 'string', enum: ['pass', 'fail', 'skipped'] }
const CHECKS = { type: 'object', properties: { fmt: CHECK, clippy: CHECK, test: CHECK, workflow_sim: CHECK }, required: ['fmt', 'clippy', 'test', 'workflow_sim'] }
const COMMITS = { type: 'array', items: { type: 'object', properties: { sha: { type: 'string' }, title: { type: 'string' } }, required: ['sha', 'title'] } }
const IMPL_SCHEMA = {
  type: 'object',
  properties: {
    status: { type: 'string', enum: ['done', 'partial', 'failed'] },
    pushed: { type: 'boolean' },
    commits: COMMITS,
    files_changed: { type: 'array', items: { type: 'string' } },
    tests_added: { type: 'number' },
    checks: CHECKS,
    summary: { type: 'string' },
    notes: { type: 'string' },
    follow_ups: { type: 'array', items: { type: 'string' } },
  },
  required: ['status', 'pushed', 'commits', 'files_changed', 'tests_added', 'checks', 'summary'],
}

const LAND_SCHEMA = {
  type: 'object',
  properties: {
    landed: { type: 'boolean' },
    commits: { ...COMMITS, description: 'The task\'s commits as they are on origin (SHAs change after a rebase)' },
    files: { type: 'number' },
    insertions: { type: 'number' },
    deletions: { type: 'number' },
    error: { type: 'string' },
  },
  required: ['landed', 'commits', 'files', 'insertions', 'deletions'],
}

const REVIEW_SCHEMA = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['approve', 'changes_required'] },
    criteria: { type: 'array', items: { type: 'object', properties: { criterion: { type: 'string' }, met: { type: 'boolean' } }, required: ['criterion', 'met'] } },
    issues: { type: 'array', items: { type: 'string' } },
    nits: { type: 'array', items: { type: 'string' } },
    summary: { type: 'string' },
  },
  required: ['verdict', 'issues', 'summary'],
}

const FINAL_SCHEMA = {
  type: 'object',
  properties: {
    checks: CHECKS,
    test_count: { type: 'number', description: 'Total tests passed in the workspace' },
    docs_commit: { type: 'string', description: 'SHA of the docs/status commit, or "" if none' },
    head: { type: 'string' },
    notes: { type: 'string' },
  },
  required: ['checks', 'head'],
}

// ─── Plan ───────────────────────────────────────────────────────────────────
phase('Plan')
let plan = A.plan
if (!plan) {
  plan = await agent(
    `Plan ARCHITECTURE.md ${PHASE.includes('.') ? 'item' : 'phase'} **${PHASE}** of velo for implementation.

The repo is checked out on branch \`${BRANCH}\`. ${PHASE.includes('.')
      ? `Plan only item ${PHASE}; treat everything else in its phase as context.`
      : `Plan every item of Phase ${PHASE} that is not already ✅.`}${SKIP.size ? `\n\nDo not plan these (already handled): ${[...SKIP].join(', ')}.` : ''}

Return a task list per your instructions. Ids must be unique; depends_on may only reference ids in your list.`,
    { label: `plan:${PHASE}`, phase: 'Plan', agentType: 'phase-planner', model: 'opus', effort: 'high', schema: PLAN_SCHEMA },
  )
  if (!plan) throw new Error('Planner returned nothing (interrupted?) — rerun planOnly')
}

// Sanitize the graph: drop unknown deps and break cycles, so scheduling cannot hang.
const tasks = plan.tasks.filter((t) => !SKIP.has(t.id))
const byId = Object.fromEntries(tasks.map((t) => [t.id, t]))
for (const t of tasks) t.depends_on = (t.depends_on || []).filter((d) => byId[d] && d !== t.id)
const state = {}
const visit = (t) => {
  state[t.id] = 'visiting'
  t.depends_on = t.depends_on.filter((d) => {
    if (state[d] === 'visiting') { log(`cycle: dropped ${t.id} → ${d}`); return false }
    if (!state[d]) visit(byId[d])
    return true
  })
  state[t.id] = 'done'
}
tasks.forEach((t) => state[t.id] || visit(t))
if (DONE_EARLIER.size) log(`Done in an earlier pass: ${[...DONE_EARLIER].join(', ')}`)
if (A.skip && A.skip.length) log(`Skipping by request: ${A.skip.join(', ')}`)
log(`${plan.phase_title}: ${tasks.length} tasks on ${BRANCH} (${tasks.filter((t) => t.parallel_safe).length} parallel-safe, ${tasks.filter((t) => t.risk === 'high').length} high-risk)`)

if (A.planOnly) return { mode: 'plan', phase: PHASE, branch: BRANCH, plan: { ...plan, tasks } }

// ─── Prompts ────────────────────────────────────────────────────────────────
const others = (t) => tasks.filter((o) => o.id !== t.id).map((o) => `- ${o.id} ${o.title}`).join('\n')
const PLAN_FILE = (A.planFile || '').replace(/\\/g, '/')
const taskBlock = (t) => (!t.brief && PLAN_FILE ? `## Task ${t.id} — ${t.title}  (ARCHITECTURE.md ${t.item})

Your full brief — spec, files, acceptance criteria and tests — is in the saved plan. Print it first and treat it exactly as if it were written here:

\`\`\`bash
python .claude/scripts/brief.py "${PLAN_FILE}" ${t.id}
\`\`\`` : taskBlockInline(t))
const taskBlockInline = (t) => `## Task ${t.id} — ${t.title}  (ARCHITECTURE.md ${t.item})

${t.brief}

### Files expected
${t.files.map((f) => `- ${f}`).join('\n')}

### Acceptance criteria
${t.acceptance.map((c, i) => `${i + 1}. ${c}`).join('\n')}

### Tests
${t.tests}`

const header = (iso, lane) => `BRANCH=${BRANCH}
MODE=${iso ? 'worktree' : 'main'}${iso ? `\nTARGET_DIR=${ROOT}/target-lane${lane}` : ''}`

// Commits that dependencies landed in this run, so the implementer reads what changed under the brief.
const depCommits = (t) => {
  const lines = t.depends_on.flatMap((d) => (results[d] && results[d].commits ? results[d].commits.map((c) => `- ${c.sha.slice(0, 10)} ${c.title} (${d})`) : []))
  return lines.length ? `\n\n### Commits that landed after this brief was written\n${lines.join('\n')}` : ''
}

const implPrompt = (t, iso, lane) => `${header(iso, lane)}

Implement the task below, test it, commit it and push it, following the velo-implementation skill.

${taskBlock(t)}${depCommits(t)}

### Other tasks in this phase — NOT yours, do not implement them
${others(t) || '(none)'}`

const refreshPrompt = (t) => `Reconcile one task brief with the current code on \`origin/${BRANCH}\`. Read-only: do not edit, build or test.

The brief was written against commit ${A.planBase || '(unknown — use the merge base with origin/main)'}. Since then these commits landed:
run \`git fetch -q origin && git log --oneline ${A.planBase ? `${A.planBase}..` : ''}origin/${BRANCH}\` and \`git show --stat\` the ones that touch this task's files or the APIs it uses.

Read the code the brief names at \`origin/${BRANCH}\` (\`git show origin/${BRANCH}:<path>\`, \`git grep … origin/${BRANCH}\`). If every type, function, file and assumption still holds, return \`changed: false\`. Otherwise return the corrected brief, files, acceptance and tests — same scope and intent, updated names and locations. Do not widen the scope. New core tests go in \`crates/velo-core/src/tests/<feature>.rs\`.

${taskBlock(t)}`

const landPrompt = (commits, iso) => `BRANCH=${BRANCH}
MODE=${iso ? 'worktree' : 'main'}
COMMITS=${commits.map((c) => c.sha).join(' ')}

Confirm these commits are on origin/${BRANCH}, push them if not, and report their diff stats.`

const reviewPrompt = (t, commits, impl, prior) => `Review task ${t.id} on branch \`${BRANCH}\`.

Commits to review: ${commits.map((c) => c.sha).join(' ')}
${impl ? `Implementer-reported checks: ${JSON.stringify(impl.checks)}
Implementer summary: ${impl.summary}
Implementer notes: ${impl.notes || '(none)'}` : 'Implemented in an earlier pass; review the commits as they stand.'}
${prior ? `\n### This is a re-review. Earlier required changes — check each is resolved:\n${prior.issues.map((s, i) => `${i + 1}. ${s}`).join('\n')}\n` : ''}
${taskBlock(t)}`

const fixPrompt = (t, review, round, iso, lane) => `${header(iso, lane)}

Fix round ${round} for task ${t.id}. Your earlier commits are already on ${BRANCH}; the reviewer requires changes. Address every issue below in a NEW commit (never amend or force-push), run the full gate, push, and report only the new commits.

### Required changes
${review.issues.map((s, i) => `${i + 1}. ${s}`).join('\n')}

### For reference, the original task
${taskBlock(t)}`

// ─── Risk ───────────────────────────────────────────────────────────────────
// The plan's own judgement wins; older plans have none, so infer it from what the task touches and how big it came out.
const RISKY = /FORMAT\.md|storage|repo\.rs|db\.rs|sync|transport|serve|merge|bundle|lib\.rs|error\.rs/
const isRisky = (t, land) => (t.risk ? t.risk === 'high' : t.files.some((f) => RISKY.test(f))) || (land ? land.insertions + land.deletions > 300 : false)

// ─── Task pipeline ──────────────────────────────────────────────────────────
// A null agent result means the run was interrupted (quota, user skip, process exit) — not that the code failed.
// From then on no new task starts, and finalize is skipped, so a resume picks up cleanly.
let halted = false
const halt = (why) => { if (!halted) log(`Interrupted at ${why} — no new tasks will start; resume to continue`); halted = true }
const results = {}

async function runTask(t, iso, lane) {
  const r = { status: 'interrupted', impl: null, land: null, review: null, rounds: 0, commits: [], refresh: null, note: '' }
  const implOpts = (label) => ({
    label, phase: 'Implement', agentType: 'phase-implementer', model: 'sonnet', schema: IMPL_SCHEMA,
    ...(iso ? { isolation: 'worktree' } : {}),
  })
  const landIt = async (commits, label) => {
    const land = await agent(landPrompt(commits, iso), { label, phase: 'Land', agentType: 'phase-lander', model: 'haiku', effort: 'low', schema: LAND_SCHEMA })
    if (!land) halt(label)
    return land
  }

  // Refresh a brief written before the code under it changed.
  if (A.refresh && !REVIEW_ONLY[t.id]) {
    const fresh = await agent(refreshPrompt(t), { label: `refresh:${t.id}`, phase: 'Refresh', agentType: 'phase-planner', model: 'sonnet', effort: 'medium', schema: REFRESH_SCHEMA })
    if (!fresh) { halt(`refresh:${t.id}`); return r }
    r.refresh = fresh.changed ? fresh.notes : 'unchanged'
    if (fresh.changed) {
      for (const k of ['brief', 'files', 'acceptance', 'tests']) if (fresh[k] && fresh[k].length) t[k] = fresh[k]
    }
  }

  // Implement — or take commits an earlier pass already landed.
  let impl
  if (REVIEW_ONLY[t.id]) {
    r.commits = REVIEW_ONLY[t.id].map((sha) => ({ sha, title: '' }))
  } else {
    impl = await agent(implPrompt(t, iso, lane), implOpts(`impl:${t.id}`))
    if (!impl) { halt(`impl:${t.id}`); return r }
    r.impl = impl
    r.commits = [...impl.commits]
    if (impl.status === 'failed' || !impl.commits.length) { r.status = 'failed'; r.note = impl.notes || impl.summary; return r }
  }

  // Land: confirm delivery deterministically, so a push failure is never mistaken for a code problem.
  let land = await landIt(r.commits, `land:${t.id}`)
  if (!land) return r
  r.land = land
  if (!land.landed) { r.status = 'push-failed'; r.note = land.error || 'commits not on origin'; return r }
  r.commits = land.commits.length ? land.commits : r.commits

  // Review: Opus where a missed defect is expensive, Sonnet elsewhere and for every re-review.
  let review = await agent(reviewPrompt(t, r.commits, impl, null), {
    label: `review:${t.id}`, phase: 'Review', agentType: 'phase-reviewer', model: isRisky(t, land) ? 'opus' : 'sonnet', schema: REVIEW_SCHEMA,
  })
  if (!review) { halt(`review:${t.id}`); return r }
  r.review = review

  while (review.verdict !== 'approve' && r.rounds < MAX_FIX) {
    r.rounds++
    const fix = await agent(fixPrompt(t, review, r.rounds, iso, lane), implOpts(`fix${r.rounds}:${t.id}`))
    if (!fix) { halt(`fix${r.rounds}:${t.id}`); return r }
    if (fix.status === 'failed' || !fix.commits.length) { r.status = 'needs-attention'; r.note = fix.notes || fix.summary; return r }
    const fixLand = await landIt(fix.commits, `land:${t.id}#${r.rounds}`)
    if (!fixLand) return r
    if (!fixLand.landed) { r.status = 'push-failed'; r.note = fixLand.error || 'fix commits not on origin'; return r }
    const fixCommits = fixLand.commits.length ? fixLand.commits : fix.commits
    r.commits.push(...fixCommits)
    land = { ...land, files: land.files + fixLand.files, insertions: land.insertions + fixLand.insertions, deletions: land.deletions + fixLand.deletions }
    r.land = land
    impl = impl
      ? { ...fix, tests_added: (impl.tests_added || 0) + (fix.tests_added || 0), follow_ups: [...(impl.follow_ups || []), ...(fix.follow_ups || [])], notes: [impl.notes, fix.notes].filter(Boolean).join(' / ') }
      : fix
    r.impl = impl
    const prior = review
    review = await agent(reviewPrompt(t, fixCommits, fix, prior), {
      label: `review:${t.id}#${r.rounds + 1}`, phase: 'Review', agentType: 'phase-reviewer', model: 'sonnet', schema: REVIEW_SCHEMA,
    })
    if (!review) { halt(`review:${t.id}#${r.rounds + 1}`); return r }
    r.review = review
  }
  r.status = review.verdict === 'approve' ? 'done' : 'needs-attention'
  return r
}

// ─── Scheduling ─────────────────────────────────────────────────────────────
// Parallel-safe tasks run in their own worktree, one per lane; each lane keeps a persistent
// target dir, so only its first build is cold. Everything else runs one at a time in the
// main checkout. Each task starts as soon as all of its dependencies are approved.
const freeLanes = Array.from({ length: MAX_PAR }, (_, i) => i + 1)
const waiters = []
const acquire = () => (freeLanes.length ? Promise.resolve(freeLanes.shift()) : new Promise((res) => waiters.push(res)))
const release = (lane) => { const w = waiters.shift(); w ? w(lane) : freeLanes.push(lane) }
let mainLane = Promise.resolve()

const promises = {}
function schedule(t) {
  if (promises[t.id]) return promises[t.id]
  promises[t.id] = Promise.all(t.depends_on.map((d) => schedule(byId[d]))).then(async (deps) => {
    let r
    const bad = t.depends_on.filter((d, i) => deps[i].status !== 'done')
    const run = async (iso, lane) => (halted ? { status: 'not-started', commits: [], rounds: 0, note: 'run interrupted before it started' } : runTask(t, iso, lane))
    if (bad.length) {
      const waiting = bad.every((d) => ['interrupted', 'not-started'].includes(results[d].status))
      r = { status: waiting ? 'not-started' : 'blocked', commits: [], rounds: 0, note: `${waiting ? 'waiting on' : 'blocked by'} ${bad.join(', ')}` }
    } else if (t.parallel_safe && MAX_PAR > 0) {
      const lane = await acquire()
      try { r = await run(true, lane) } finally { release(lane) }
    } else {
      const p = mainLane.then(() => run(false, 0))
      mainLane = p.catch(() => {})
      r = await p
    }
    results[t.id] = r
    log(`${t.id} → ${r.status}${r.note ? ` (${r.note.slice(0, 80)})` : ''}`)
    return r
  })
  return promises[t.id]
}
await Promise.all(tasks.map(schedule))

// ─── Finalize ───────────────────────────────────────────────────────────────
phase('Finalize')
const doneTasks = tasks.filter((t) => results[t.id].status === 'done')
let final = null
if (halted) {
  log('Run was interrupted — finalize skipped; resume with the same args to continue')
} else if (doneTasks.length) {
  const items = [...new Set(doneTasks.map((t) => t.item))]
  // Judge against the whole plan, so an item with tasks skipped in this run is not marked done.
  const fullyDone = items.filter((it) => plan.tasks.filter((t) => t.item === it).every((t) => (results[t.id] && results[t.id].status === 'done') || DONE_EARLIER.has(t.id)))
  final = await agent(`BRANCH=${BRANCH}
MODE=main

Finalize ${PHASE} on branch ${BRANCH}. You are in the main checkout. Follow the velo-implementation skill for setup, checks, commit and push — with one exception: this step IS allowed to edit ARCHITECTURE.md and CHANGELOG.md.

1. \`git pull --rebase origin ${BRANCH}\`, then run the full suite on the branch head: \`cargo fmt --all -- --check\`, clippy, \`timeout 900 cargo test --workspace --locked\`, and \`timeout 900 ./workflow_sim.sh\` (filtered output, per the skill). Report each result and the total number of tests passed. If anything fails, do NOT edit docs; report it and stop.
2. If green: in ARCHITECTURE.md mark these items ✅ **DONE**: ${fullyDone.join(', ') || '(none fully done)'}. Items partially done (leave their marker; update or add the short note of what landed): ${items.filter((i) => !fullyDone.includes(i)).join(', ') || '(none)'}. Update the phase's "What landed" subsection in the doc's existing voice.
3. Add entries to the top section of CHANGELOG.md in its existing style for:
${doneTasks.map((t) => `- ${t.id} ${t.title}: ${results[t.id].impl ? results[t.id].impl.summary : results[t.id].review.summary}`).join('\n')}
4. Commit ("Mark ${PHASE} progress in the architecture doc and changelog" or similar) and push.`,
  { label: 'finalize', phase: 'Finalize', agentType: 'phase-implementer', model: 'sonnet', schema: FINAL_SCHEMA })
  if (!final) log('Finalize was interrupted — rerun it with the same args (completed tasks replay from cache)')
} else {
  log('No task was approved — skipping finalize')
}

// ─── Report ─────────────────────────────────────────────────────────────────
const ICON = { done: '✅', 'needs-attention': '⚠️', failed: '❌', 'push-failed': '📤', blocked: '⛔', interrupted: '⏸️', 'not-started': '⏳' }
const chk = (c) => (c ? ['fmt', 'clippy', 'test', 'workflow_sim'].map((k) => `${k === 'workflow_sim' ? 'sim' : k}:${c[k] === 'pass' ? '✓' : c[k] === 'fail' ? '✗' : '–'}`).join(' ') : '–')
const esc = (s) => String(s ?? '').replace(/\|/g, '\\|').replace(/\n+/g, ' ')
const rows = tasks.map((t) => {
  const r = results[t.id]
  const i = r.impl
  const l = r.land
  return {
    id: t.id, item: t.item, title: t.title, status: r.status,
    mode: t.parallel_safe && MAX_PAR > 0 ? 'worktree' : 'main',
    commits: r.commits.map((c) => c.sha.slice(0, 8)),
    files: l ? l.files : 0,
    lines: l ? `+${l.insertions}/-${l.deletions}` : '–',
    tests_added: i ? i.tests_added : 0,
    checks: i ? i.checks : null,
    review: r.review ? `${r.review.verdict}` : '–',
    fix_rounds: r.rounds,
    refresh: r.refresh,
    summary: i ? i.summary : r.review ? r.review.summary : r.note || '',
    open_issues: r.status === 'done' ? [] : (r.review ? r.review.issues : r.note ? [r.note] : []),
    follow_ups: i ? i.follow_ups || [] : [],
    nits: r.review ? r.review.nits || [] : [],
  }
})

const table = [
  '| Task | Item | Title | Status | Commits | Files | Lines | Tests+ | Checks | Review (fix rounds) | Summary |',
  '| :--- | :--- | :--- | :--- | :--- | ---: | ---: | ---: | :--- | :--- | :--- |',
  ...rows.map((r) => `| ${r.id} | ${r.item} | ${esc(r.title)} | ${ICON[r.status]} ${r.status} | ${r.commits.join(' ') || '–'} | ${r.files} | ${r.lines} | ${r.tests_added} | ${chk(r.checks)} | ${r.review} (${r.fix_rounds}) | ${esc(r.summary)} |`),
].join('\n')

return {
  mode: 'run',
  phase: PHASE,
  phase_title: plan.phase_title,
  branch: BRANCH,
  interrupted: halted,
  resume: halted ? rows.filter((r) => ['interrupted', 'not-started'].includes(r.status)).map((r) => r.id) : [],
  summary: plan.summary,
  skipped_items: plan.skipped_items || [],
  counts: Object.fromEntries(Object.keys(ICON).map((s) => [s, rows.filter((r) => r.status === s).length])),
  final,
  table,
  rows,
}
