export const meta = {
  name: 'implement-phase',
  description: 'Opus plans an ARCHITECTURE.md phase into tasks; Sonnet agents implement, test, commit and push each; Opus reviews each; returns a summary table',
  whenToUse: 'Implementing a whole phase (e.g. "14") or one item (e.g. "14.3") of ARCHITECTURE.md end to end',
  phases: [
    { title: 'Plan', detail: 'Opus splits the phase into dependency-ordered tasks', model: 'opus' },
    { title: 'Implement', detail: 'Sonnet implements, tests, commits and pushes each task', model: 'sonnet' },
    { title: 'Review', detail: 'Opus checks each task against its acceptance criteria', model: 'opus' },
    { title: 'Finalize', detail: 'Full check suite on the branch head; mark the phase done in the docs', model: 'sonnet' },
  ],
}

// ─── Arguments ──────────────────────────────────────────────────────────────
// { phase: "14" | "14.3", branch?, maxParallel?, maxFixRounds?, planOnly?, plan?, skip?: [taskId] }
const A = args || {}
const PHASE = String(A.phase || '').trim()
if (!PHASE) throw new Error('args.phase is required, e.g. {"phase": "14"} or {"phase": "14.3"}')
const BRANCH = A.branch || `phase-${PHASE.replace(/\./g, '-')}`
const MAX_PAR = A.maxParallel ?? 2 // concurrent worktree lanes (each has a cold target/)
const MAX_FIX = A.maxFixRounds ?? 2
const SKIP = new Set(A.skip || [])

// ─── Schemas ────────────────────────────────────────────────────────────────
const PLAN_SCHEMA = {
  type: 'object',
  properties: {
    phase_title: { type: 'string' },
    summary: { type: 'string', description: 'What the phase delivers, 2-4 sentences' },
    skipped_items: { type: 'array', items: { type: 'string' }, description: 'Doc items not planned and why (already done, out of scope, decision-only)' },
    tasks: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          id: { type: 'string', description: 'Short stable id, e.g. "14.3-a"' },
          item: { type: 'string', description: 'ARCHITECTURE.md item number it belongs to, e.g. "14.3"' },
          title: { type: 'string' },
          depends_on: { type: 'array', items: { type: 'string' } },
          parallel_safe: { type: 'boolean' },
          files: { type: 'array', items: { type: 'string' }, description: 'Files expected to be created or modified' },
          brief: { type: 'string', description: 'Self-contained implementation spec (markdown), including quoted spec text' },
          acceptance: { type: 'array', items: { type: 'string' } },
          tests: { type: 'string', description: 'Tests to write and where' },
        },
        required: ['id', 'item', 'title', 'depends_on', 'parallel_safe', 'files', 'brief', 'acceptance', 'tests'],
      },
    },
  },
  required: ['phase_title', 'summary', 'tasks'],
}

const CHECK = { type: 'string', enum: ['pass', 'fail', 'skipped'] }
const IMPL_SCHEMA = {
  type: 'object',
  properties: {
    status: { type: 'string', enum: ['done', 'partial', 'failed'] },
    commits: { type: 'array', items: { type: 'object', properties: { sha: { type: 'string' }, title: { type: 'string' } }, required: ['sha', 'title'] } },
    files_changed: { type: 'array', items: { type: 'string' } },
    lines_added: { type: 'number' },
    lines_removed: { type: 'number' },
    tests_added: { type: 'number' },
    checks: {
      type: 'object',
      properties: { fmt: CHECK, clippy: CHECK, test: CHECK, workflow_sim: CHECK },
      required: ['fmt', 'clippy', 'test', 'workflow_sim'],
    },
    summary: { type: 'string' },
    notes: { type: 'string' },
    follow_ups: { type: 'array', items: { type: 'string' } },
  },
  required: ['status', 'commits', 'files_changed', 'tests_added', 'checks', 'summary'],
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
    checks: { type: 'object', properties: { fmt: CHECK, clippy: CHECK, test: CHECK, workflow_sim: CHECK }, required: ['fmt', 'clippy', 'test', 'workflow_sim'] },
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
      : `Plan every item of Phase ${PHASE} that is not already ✅.`}

Return a task list per your instructions. Ids must be unique; depends_on may only reference ids in your list.`,
    { label: `plan:${PHASE}`, phase: 'Plan', agentType: 'phase-planner', model: 'opus', effort: 'high', schema: PLAN_SCHEMA },
  )
  if (!plan) throw new Error('Planner returned nothing')
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
if (SKIP.size) log(`Skipping by request: ${[...SKIP].join(', ')}`)
log(`${plan.phase_title}: ${tasks.length} tasks on ${BRANCH} (${tasks.filter((t) => t.parallel_safe).length} parallel-safe)`)

if (A.planOnly) return { mode: 'plan', phase: PHASE, branch: BRANCH, plan: { ...plan, tasks } }

// ─── Prompts ────────────────────────────────────────────────────────────────
const others = (t) => tasks.filter((o) => o.id !== t.id).map((o) => `- ${o.id} ${o.title}`).join('\n')
const taskBlock = (t) => `## Task ${t.id} — ${t.title}  (ARCHITECTURE.md ${t.item})

${t.brief}

### Files expected
${t.files.map((f) => `- ${f}`).join('\n')}

### Acceptance criteria
${t.acceptance.map((c, i) => `${i + 1}. ${c}`).join('\n')}

### Tests
${t.tests}`

const implPrompt = (t, iso) => `BRANCH=${BRANCH}
MODE=${iso ? 'worktree' : 'main'}

Implement the task below, test it, commit it and push it, following the velo-implementation skill.

${taskBlock(t)}

### Other tasks in this phase — NOT yours, do not implement them
${others(t) || '(none)'}`

const reviewPrompt = (t, commits, impl) => `Review task ${t.id} on branch \`${BRANCH}\`.

Commits to review: ${commits.map((c) => c.sha).join(' ')}
Implementer-reported checks: ${JSON.stringify(impl.checks)}
Implementer summary: ${impl.summary}
Implementer notes: ${impl.notes || '(none)'}

${taskBlock(t)}`

const fixPrompt = (t, review, round, iso) => `BRANCH=${BRANCH}
MODE=${iso ? 'worktree' : 'main'}

Fix round ${round} for task ${t.id}. Your earlier commits are already pushed to ${BRANCH}; the reviewer requires changes. Address every issue below in a NEW commit (never amend or force-push), re-run all checks, push, and report only the new commits.

### Required changes
${review.issues.map((s, i) => `${i + 1}. ${s}`).join('\n')}

### For reference, the original task
${taskBlock(t)}`

// ─── Scheduling ─────────────────────────────────────────────────────────────
// Parallel-safe tasks run in their own worktree, capped at MAX_PAR lanes.
// Everything else runs one at a time in the main checkout. Each task starts
// as soon as all of its dependencies are approved — no global wave barrier.
let slots = MAX_PAR
const waiters = []
const acquire = () => (slots > 0 ? (slots--, Promise.resolve()) : new Promise((r) => waiters.push(r)))
const release = () => { const w = waiters.shift(); w ? w() : slots++ }
let mainLane = Promise.resolve()

const results = {}
async function runTask(t, iso) {
  const implOpts = (label) => ({
    label, phase: 'Implement', agentType: 'phase-implementer', model: 'sonnet', schema: IMPL_SCHEMA,
    ...(iso ? { isolation: 'worktree' } : {}),
  })
  let impl = await agent(implPrompt(t, iso), implOpts(`impl:${t.id}`))
  if (!impl) return { status: 'failed', impl: null, review: null, rounds: 0, commits: [], note: 'implementer died' }
  const commits = [...impl.commits]
  let review = null
  let rounds = 0
  while (impl.status !== 'failed' && commits.length) {
    review = await agent(reviewPrompt(t, commits, impl), {
      label: `review:${t.id}${rounds ? `#${rounds + 1}` : ''}`, phase: 'Review',
      agentType: 'phase-reviewer', model: 'opus', schema: REVIEW_SCHEMA,
    })
    if (!review || review.verdict === 'approve' || rounds >= MAX_FIX) break
    rounds++
    const fix = await agent(fixPrompt(t, review, rounds, iso), implOpts(`fix${rounds}:${t.id}`))
    if (!fix) break
    commits.push(...fix.commits)
    impl = {
      ...fix,
      files_changed: [...new Set([...impl.files_changed, ...fix.files_changed])],
      tests_added: (impl.tests_added || 0) + (fix.tests_added || 0),
      lines_added: (impl.lines_added || 0) + (fix.lines_added || 0),
      lines_removed: (impl.lines_removed || 0) + (fix.lines_removed || 0),
      follow_ups: [...(impl.follow_ups || []), ...(fix.follow_ups || [])],
      notes: [impl.notes, fix.notes].filter(Boolean).join(' / '),
    }
    if (fix.status === 'failed') break
  }
  const approved = review && review.verdict === 'approve' && impl.status !== 'failed'
  return { status: approved ? 'done' : impl.status === 'failed' ? 'failed' : 'needs-attention', impl, review, rounds, commits }
}

const promises = {}
function schedule(t) {
  if (promises[t.id]) return promises[t.id]
  promises[t.id] = Promise.all(t.depends_on.map((d) => schedule(byId[d]))).then(async (deps) => {
    let r
    const bad = t.depends_on.filter((d, i) => deps[i].status !== 'done')
    if (bad.length) {
      r = { status: 'blocked', impl: null, review: null, rounds: 0, commits: [], note: `blocked by ${bad.join(', ')}` }
      log(`${t.id} blocked by ${bad.join(', ')}`)
    } else if (t.parallel_safe && MAX_PAR > 0) {
      await acquire()
      try { r = await runTask(t, true) } finally { release() }
    } else {
      const p = mainLane.then(() => runTask(t, false))
      mainLane = p.catch(() => {})
      r = await p
    }
    results[t.id] = r
    log(`${t.id} → ${r.status}`)
    return r
  })
  return promises[t.id]
}
await Promise.all(tasks.map(schedule))

// ─── Finalize ───────────────────────────────────────────────────────────────
phase('Finalize')
const doneTasks = tasks.filter((t) => results[t.id].status === 'done')
let final = null
if (doneTasks.length) {
  const items = [...new Set(doneTasks.map((t) => t.item))]
  // Judge against the whole plan, so an item with skipped tasks is never marked done.
  const fullyDone = items.filter((it) => plan.tasks.filter((t) => t.item === it).every((t) => results[t.id] && results[t.id].status === 'done'))
  final = await agent(`BRANCH=${BRANCH}
MODE=main

Finalize ${PHASE} on branch ${BRANCH}. You are in the main checkout. Follow the velo-implementation skill for setup, checks, commit and push — with one exception: this step IS allowed to edit ARCHITECTURE.md and CHANGELOG.md.

1. \`git pull --rebase origin ${BRANCH}\`, then run the full suite on the branch head: fmt check (\`cargo fmt --all -- --check\`), clippy, \`cargo test --workspace --locked\`, and \`./workflow_sim.sh\`. Report each result and the total number of tests passed. If anything fails, do NOT edit docs; report it and stop.
2. If green: in ARCHITECTURE.md mark these items ✅ **DONE**: ${fullyDone.join(', ') || '(none fully done)'}. Items partially done (leave their marker, add a short note of what landed): ${items.filter((i) => !fullyDone.includes(i)).join(', ') || '(none)'}. Add a short "What landed" subsection under the phase in the doc's existing voice.
3. Add entries to the top section of CHANGELOG.md in its existing style (Added / Changed / Format sections as relevant) for:
${doneTasks.map((t) => `- ${t.id} ${t.title}: ${results[t.id].impl.summary}`).join('\n')}
4. Commit ("Mark ${PHASE} done in the architecture doc and changelog" or similar) and push.`,
  { label: 'finalize', phase: 'Finalize', agentType: 'phase-implementer', model: 'sonnet', schema: FINAL_SCHEMA })
} else {
  log('No task was approved — skipping finalize')
}

// ─── Report ─────────────────────────────────────────────────────────────────
const ICON = { done: '✅', 'needs-attention': '⚠️', failed: '❌', blocked: '⛔' }
const chk = (c) => (c ? ['fmt', 'clippy', 'test', 'workflow_sim'].map((k) => `${k === 'workflow_sim' ? 'sim' : k}:${c[k] === 'pass' ? '✓' : c[k] === 'fail' ? '✗' : '–'}`).join(' ') : '–')
const esc = (s) => String(s ?? '').replace(/\|/g, '\\|').replace(/\n+/g, ' ')
const rows = tasks.map((t) => {
  const r = results[t.id]
  const i = r.impl
  return {
    id: t.id, item: t.item, title: t.title, status: r.status,
    mode: t.parallel_safe ? 'worktree' : 'main',
    commits: r.commits.map((c) => c.sha.slice(0, 8)),
    files: i ? i.files_changed.length : 0,
    lines: i ? `+${i.lines_added ?? '?'}/-${i.lines_removed ?? '?'}` : '–',
    tests_added: i ? i.tests_added : 0,
    checks: i ? i.checks : null,
    review: r.review ? r.review.verdict : '–',
    fix_rounds: r.rounds,
    summary: i ? i.summary : r.note || '',
    open_issues: r.status === 'done' ? [] : (r.review ? r.review.issues : []),
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
  summary: plan.summary,
  skipped_items: plan.skipped_items || [],
  counts: Object.fromEntries(Object.keys(ICON).map((s) => [s, rows.filter((r) => r.status === s).length])),
  final,
  table,
  rows,
}
