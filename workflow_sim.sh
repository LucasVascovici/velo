#!/usr/bin/env bash
#
# workflow_sim.sh — end-to-end workflow simulation for Velo.
#
# Unlike the unit tests (which check one function at a time), this drives the
# real `velo` binary through a long-lived, realistic project shared by two
# developers — "Alice" and "Bob" — and asserts the outcome of every step.
# It exercises every command and its important edge cases, including the tricky
# interactions (auto-merge, conflict resolution, rebase-with-conflict, undo/redo
# of a merge, stash context-switching, squash safety, single-file restore, …).
#
# Every action is printed; every outcome is checked. It is fail-fast: the first
# broken expectation stops the run with a clear diagnostic and a non-zero exit.
#
# Usage:  ./workflow_sim.sh        (builds the release binary first if needed)
#
set -u

# ── Colours ──────────────────────────────────────────────────────────────────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  BOLD=$'\e[1m'; DIM=$'\e[2m'; RST=$'\e[0m'
  RED=$'\e[31m'; GRN=$'\e[32m'; YEL=$'\e[33m'; BLU=$'\e[34m'; CYN=$'\e[36m'; MAG=$'\e[35m'
else
  BOLD=""; DIM=""; RST=""; RED=""; GRN=""; YEL=""; BLU=""; CYN=""; MAG=""
fi

PASS=0

# ── Locate / build the binary ────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Pick the binary that matches THIS shell's platform. On WSL / Linux / macOS the
# native binary is `velo`; only under a Windows shell (Git Bash / MSYS / Cygwin)
# do we want `velo.exe`. Never run the Windows .exe from WSL: SQLite's file
# locking breaks across the Windows↔WSL filesystem boundary and every command
# fails with "database is locked".
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) VELO="$SCRIPT_DIR/target/release/velo.exe" ;;
  *)                    VELO="$SCRIPT_DIR/target/release/velo" ;;
esac

if [ ! -f "$VELO" ]; then
  printf "${YEL}Native release binary not found — building it (cargo build --release)…${RST}\n"
  ( cd "$SCRIPT_DIR" && cargo build --release ) \
    || { printf "${RED}build failed — run 'cargo build --release' in this environment first.${RST}\n"; exit 1; }
fi
if [ ! -f "$VELO" ]; then
  printf "${RED}No native velo binary at %s — build it in this environment first.${RST}\n" "$VELO"
  exit 1
fi

# ── Sandbox ──────────────────────────────────────────────────────────────────
SANDBOX="$(mktemp -d)"
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT
mkdir -p "$SANDBOX/taskmanager"
cd "$SANDBOX/taskmanager" || exit 1

# ── Output helpers ───────────────────────────────────────────────────────────
act()  { printf "\n${BOLD}${BLU}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RST}\n"
         printf "${BOLD}${BLU}  %s${RST}\n" "$1"
         printf "${BOLD}${BLU}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RST}\n" ; }
say()  { printf "  ${DIM}%s${RST}\n" "$1"; }
who()  { printf "  ${MAG}%s${RST}\n" "$1"; }
file() { # write a file, announce it
  local p="$1"; shift
  printf '%b' "$1" > "$p"
  printf "  ${DIM}✎ wrote %s${RST}\n" "$p"
}

H() { tr -d ' \t\r\n' < .velo/PARENT; }   # current snapshot hash

# Convert a shell path to one the velo binary understands natively. Under
# MSYS/Git-Bash a "/tmp/..." path is only translated for bare arguments, so URLs
# that embed a path (e.g. "child:/tmp/x") must be converted explicitly.
native_path() {
  if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s' "$1"; fi
}

# Move to another repository, announcing which one we're operating in.
repo() {
  cd "$1" 2>/dev/null || bad "cannot enter repository '$1'"
  printf "  ${MAG}📁 %s${RST}\n" "${2:-$(basename "$1")}"
}

# ── Command runners ──────────────────────────────────────────────────────────
_disp() { local d=""; for a in "$@"; do case "$a" in *" "*) d="$d \"$a\"";; *) d="$d $a";; esac; done; printf '%s' "$d"; }

v() { # run, expect success
  printf "  ${CYN}\$ velo%s${RST}\n" "$(_disp "$@")"
  OUT="$("$VELO" "$@" 2>&1)"; RC=$?
  [ -n "$OUT" ] && printf '%s\n' "$OUT" | while IFS= read -r l; do printf "    ${DIM}│ %s${RST}\n" "$l"; done
  [ $RC -ne 0 ] && bad "expected success but 'velo$(_disp "$@")' exited $RC"
  return 0
}

vfail() { # run, expect non-zero exit
  printf "  ${CYN}\$ velo%s${RST} ${DIM}(expected to be rejected)${RST}\n" "$(_disp "$@")"
  OUT="$("$VELO" "$@" 2>&1)"; RC=$?
  [ -n "$OUT" ] && printf '%s\n' "$OUT" | while IFS= read -r l; do printf "    ${DIM}│ %s${RST}\n" "$l"; done
  if [ $RC -eq 0 ]; then bad "expected failure but 'velo$(_disp "$@")' succeeded"; else good "correctly rejected"; fi
}

# ── Assertions ───────────────────────────────────────────────────────────────
good() { printf "    ${GRN}✓${RST} %s\n" "$1"; PASS=$((PASS+1)); }
bad()  { printf "    ${RED}${BOLD}✗ FAILED: %s${RST}\n" "$1"; summary_fail; exit 1; }

has()      { case "$1" in *"$2"*) good "$3";; *) printf "      ${DIM}expected to contain: %s${RST}\n" "$2"; bad "$3";; esac; }
hasnt()    { case "$1" in *"$2"*) printf "      ${DIM}should NOT contain: %s${RST}\n" "$2"; bad "$3";; *) good "$3";; esac; }
file_is()  { local g; g="$(cat "$1" 2>/dev/null)"; if [ "$g" = "$2" ]; then good "$3"; else printf "      ${DIM}--- expected ---${RST}\n%s\n      ${DIM}--- got ---${RST}\n%s\n" "$2" "$g"; bad "$3"; fi; }
file_has() { local g; g="$(cat "$1" 2>/dev/null)"; has "$g" "$2" "$3"; }
exists()   { [ -e "$1" ] && good "$2" || bad "$2 (missing: $1)"; }
absent()   { [ ! -e "$1" ] && good "$2" || bad "$2 (should be absent: $1)"; }
clean()    { local s; s="$("$VELO" status 2>&1)"; has "$s" "clean" "${1:-working tree is clean}"; }
status_has(){ local s; s="$("$VELO" status 2>&1)"; has "$s" "$1" "$2"; }

summary_fail() {
  printf "\n${RED}${BOLD}════════════════════════════════════════════════════════════════════${RST}\n"
  printf "${RED}${BOLD}  SIMULATION FAILED after %d passing checks.${RST}\n" "$PASS"
  printf "${RED}${BOLD}════════════════════════════════════════════════════════════════════${RST}\n"
}

# =============================================================================
printf "${BOLD}${MAG}\n"
printf "        ╦  ╦╔═╗╦  ╔═╗   ┬ ┬┌─┐┬─┐┬┌─┌─┐┬  ┌─┐┬ ┬  ┌─┐┬┌┬┐\n"
printf "        ╚╗╔╝║╣ ║  ║ ║   ││││ │├┬┘├┴┐├┤ │  │ ││││  └─┐││││\n"
printf "         ╚╝ ╚═╝╩═╝╚═╝   └┴┘└─┘┴└─┴ ┴└  ┴─┘└─┘└┴┘  └─┘┴┴ ┴\n"
printf "${RST}${DIM}   A long-lived project, then a team: every command, every edge case.${RST}\n"
printf "${DIM}   Acts 1–16 solo workflow · Acts 17–22 clone/push/pull, ssh-path, bundles.${RST}\n"
printf "${DIM}   Sandbox: %s${RST}\n" "$SANDBOX/taskmanager"

# =============================================================================
act "ACT 1 · Project kickoff (Alice)"
who "👩 Alice starts the TaskManager project."
v init
has "$OUT" "Initialized" "init reports success"
exists ".velo" ".velo directory created"
file README.md   "# TaskManager\n\nA tiny task app.\n"
file app.py      'def main():\n    print("Task Manager")\n    return 0\n'
file config.py   'DEBUG = False\nHOST = "localhost"\nPORT = 8080\nTIMEOUT = 30\nRETRIES = 3\n'
v save "Initial project skeleton"
has "$OUT" "Saved" "first snapshot saved"
H1="$(H)"; say "→ first snapshot is $H1"
clean "clean immediately after save"

# =============================================================================
act "ACT 2 · Daily edits — diff, amend, pathspec"
who "👩 Alice adds a greeting, notices a typo, and amends."
file app.py 'def main():\n    print("Task Manager")\n    print("Hello!")\n    return 0\n'
status_has "app.py" "status shows app.py modified"
v diff
has "$OUT" "Hello" "diff shows the added line"
v save "Add gretting"      # deliberate typo in the message
v save "Add greeting" --amend
has "$OUT" "Amended" "amend produced a snapshot"
# amend keeps history length the same (skeleton + greeting == 2)
COUNT="$("$VELO" history --oneline --limit 999 2>&1 | grep -cE '[0-9a-f]{12}')"
[ "$COUNT" = "2" ] && good "amend did not add a commit (still 2)" || bad "expected 2 commits, got $COUNT"

who "👩 She then spots a forgotten file — amend needs no message to fold it in."
file greeting_helper.py 'def greet():\n    return "Hello!"\n'
v save --amend
has "$OUT" "Amended" "amend without a message works"
OUT="$("$VELO" history --oneline --limit 999 2>&1)"
has "$OUT" "Add greeting" "the existing message was kept"
COUNT="$(printf '%s' "$OUT" | grep -cE '[0-9a-f]{12}')"
[ "$COUNT" = "2" ] && good "still no extra commit" || bad "expected 2 commits, got $COUNT"
v save --amend
has "$OUT" "Nothing to amend" "amending with nothing to do is a graceful no-op"

who "👩 Alice adds two files but only commits one (pathspec)."
file utils.py 'def slug(s):\n    return s.lower()\n'
file notes.txt 'notes v1\n'
v save "Add slug helper" -- utils.py
status_has "notes.txt" "notes.txt left unsaved by pathspec save"
v save "Add notes"
clean "everything committed"

# =============================================================================
act "ACT 3 · Parallel branches (Alice: login, Bob: api)"
who "👩 Alice opens a login feature branch."
v switch feature/login
has "$OUT" "feature/login" "switched to feature/login"
file login.py 'def login(user):\n    return user is not None\n'
# Alice also tweaks the TOP of config.py
file config.py 'DEBUG = True\nHOST = "localhost"\nPORT = 8080\nTIMEOUT = 30\nRETRIES = 3\n'
v save "Add login + enable debug"

who "👨 Bob opens an API feature branch off main."
v switch main
v switch feature/api
file api.py 'def handler():\n    return 200\n'
v save "Add API handler"
v branches
has "$OUT" "feature/login" "branches lists feature/login"
has "$OUT" "feature/api" "branches lists feature/api"

# =============================================================================
act "ACT 4 · Fast-forward merge (Bob's API, main untouched)"
v switch main
v merge feature/api
has "$OUT" "Fast-forward" "API merge fast-forwards"
exists "api.py" "api.py present on main after FF"
clean "clean after fast-forward"

# =============================================================================
act "ACT 5 · True 3-way merge — non-overlapping edits AUTO-MERGE"
who "👩 Alice edited the top of config.py; main now edits the bottom."
file config.py 'DEBUG = False\nHOST = "localhost"\nPORT = 8080\nTIMEOUT = 30\nRETRIES = 5\n'
v save "Bump retries to 5"
v merge feature/login
has "$OUT" "Auto-merged" "config.py auto-merged (non-overlapping)"
has "$OUT" "Clean merge" "no conflicts reported"
exists "login.py" "login.py pulled in from feature/login"
# Both edits present: Alice's DEBUG=True (top) AND main's RETRIES=5 (bottom)
file_is config.py $'DEBUG = True\nHOST = "localhost"\nPORT = 8080\nTIMEOUT = 30\nRETRIES = 5' "both sides' changes merged, none lost"
v save "Merge feature/login"
clean "clean after finalising the merge"

# =============================================================================
act "ACT 6 · Conflicting merge + resolve"
who "👨 Bob hotfixes app.py; main changes the same line differently."
v switch feature/hotfix
file app.py 'def main():\n    print("Task Manager v1")\n    print("Hello!")\n    return 0\n'
v save "Hotfix: version string v1"
v switch main
file app.py 'def main():\n    print("Task Manager PRO")\n    print("Hello!")\n    return 0\n'
v save "Rename to PRO"
v merge feature/hotfix
has "$OUT" "Conflict" "merge reports a real conflict"
exists ".velo/MERGE_HEAD" "MERGE_HEAD written during conflict"
status_has "Conflict" "status shows the conflict"
v resolve app.py --take theirs
file_has app.py 'Task Manager v1' "resolve --take theirs took the incoming version"
hasnt "$(cat app.py)" "PRO" "our conflicting line was replaced"
v save "Merge hotfix"
H_MERGE="$(H)"; say "→ merge commit is $H_MERGE"
absent ".velo/MERGE_HEAD" "MERGE_HEAD cleared after save"

# =============================================================================
act "ACT 7 · Undo / redo must preserve merge topology AND tags"
v tag v1.0
v tag
has "$OUT" "v1.0" "tag v1.0 listed"
who "👩 Alice undoes the merge…"
v undo
file_has app.py "PRO" "working tree rewound to pre-merge state"
OUT="$("$VELO" tag 2>&1)"; hasnt "$OUT" "v1.0" "tag detached while merge is undone"
who "…then redoes it."
v redo
file_has app.py 'Task Manager v1' "merge restored by redo"
OUT="$("$VELO" tag 2>&1)"; has "$OUT" "v1.0" "tag restored by redo"
v history --graph
has "$OUT" "Merge hotfix" "merge still visible in the graph after undo→redo"

# =============================================================================
act "ACT 8 · merge --abort restores the pre-merge state"
v switch feature/exp
file config.py 'DEBUG = True\nHOST = "0.0.0.0"\nPORT = 8080\nTIMEOUT = 30\nRETRIES = 5\n'
v save "Experiment: bind all interfaces"
v switch main
file config.py 'DEBUG = True\nHOST = "127.0.0.1"\nPORT = 8080\nTIMEOUT = 30\nRETRIES = 5\n'
v save "Pin host to loopback"
BEFORE_ABORT="$(cat config.py)"
v merge feature/exp
has "$OUT" "Conflict" "experiment merge conflicts on config.py"
v merge --abort
absent ".velo/MERGE_HEAD" "MERGE_HEAD gone after abort"
file_is config.py "$BEFORE_ABORT" "config.py restored to exact pre-merge content"
clean "clean after merge --abort"

# =============================================================================
act "ACT 9 · Stash — context switch without committing"
who "👩 Alice is mid-refactor when an urgent fix comes in."
file app.py 'def main():\n    print("Task Manager v1")\n    print("Hello!")\n    # TODO: big refactor in progress\n    return 0\n'
status_has "app.py" "WIP shows as modified"
v stash push wip-refactor
clean "working tree clean after stashing"
hasnt "$(cat app.py)" "big refactor" "WIP removed from working tree"
v stash list
has "$OUT" "wip-refactor" "shelf listed by name"
v stash show wip-refactor
who "👩 Alice ships the urgent fix on a clean tree."
file config.py 'DEBUG = False\nHOST = "127.0.0.1"\nPORT = 9090\nTIMEOUT = 30\nRETRIES = 5\n'
v save "Urgent: move to port 9090"
who "…then pops her refactor back."
v stash pop wip-refactor
file_has app.py "big refactor" "WIP restored by stash pop"
v save "Finish refactor"
# drop (delete a shelf without restoring)
file junk.txt 'throwaway\n'
v stash push junk-shelf
absent junk.txt "junk stashed away"
v stash drop junk-shelf
OUT="$("$VELO" stash list 2>&1)"; hasnt "$OUT" "junk-shelf" "dropped shelf is gone"
absent junk.txt "drop did not restore the junk (as expected)"
clean "clean after stash workflow"

# =============================================================================
act "ACT 10 · Rebase — clean, conflict+continue, and abort"
who "👨 Bob rebases a clean feature onto an advanced main."
v switch feature/clean
file report.py 'def report():\n    return "ok"\n'
v save "Add report module"
v switch main
file metrics.py 'def metrics():\n    return {}\n'
v save "Add metrics module"
v switch feature/clean
v rebase main
absent ".velo/REBASE_STATE" "clean rebase leaves no state file"
exists report.py "rebased commit kept report.py"
exists metrics.py "rebase picked up main's metrics.py"

say ""
who "👨 Bob rebases a CONFLICTING feature, resolves, and continues."
v switch main
file rb.txt 'shared baseline\n'
v save "Add rb baseline"
v switch feature/conf
file rb.txt 'conf version\n'
v save "Conf edits rb"
v switch main
file rb.txt 'main version\n'
v save "Main edits rb"
v switch feature/conf
v rebase main
has "$OUT" "Conflict" "rebase pauses on the conflicting commit"
exists ".velo/REBASE_STATE" "rebase state persists while paused"
v resolve --all --take theirs
v save "Rebase: keep conf version"
v rebase --continue
has "$OUT" "complete" "rebase completes after continue"
absent ".velo/REBASE_STATE" "state cleared after finishing"
file_has rb.txt "conf version" "resolved content preserved; commit not re-applied"

say ""
who "👨 Bob starts another rebase but changes his mind (abort)."
v switch main
file rb.txt 'main version 2\n'
v save "Main edits rb again"
v switch feature/conf
AB_TIP="$(H)"
v rebase main
has "$OUT" "Conflict" "second rebase conflicts"
v rebase --abort
absent ".velo/REBASE_STATE" "abort clears rebase state"
absent ".velo/MERGE_HEAD" "abort clears merge state"
[ "$(H)" = "$AB_TIP" ] && good "branch tip restored to the exact pre-rebase commit" \
  || bad "expected tip $AB_TIP, got $(H)"

# =============================================================================
act "ACT 11 · Cherry-pick a fix across branches"
v switch main
file notes.txt 'notes v1\n'
v save "Reset notes"
v switch fixbranch
file notes.txt 'notes v1\nIMPORTANT FIX\n'
v save "Note the important fix"
H_FIX="$(H)"
v switch main
v cherry-pick "$H_FIX"
file_has notes.txt "IMPORTANT FIX" "cherry-pick applied the fix onto main"

# =============================================================================
act "ACT 12 · Squash — collapse WIP, and refuse to orphan history"
who "👨 Bob squashes three WIP commits into one."
v switch feature/squash
file sq.txt 'a\n';        v save "wip 1"
file sq.txt 'a\nb\n';     v save "wip 2"
file sq.txt 'a\nb\nc\n';  v save "wip 3"
v squash 3 "Squashed 3 WIP commits"
file_is sq.txt $'a\nb\nc' "squash keeps the final content"
OUT="$("$VELO" history --oneline --limit 999 2>&1)"
has "$OUT" "Squashed 3 WIP commits" "squashed commit present"
hasnt "$OUT" "wip 2" "intermediate WIP commits gone"

say ""
who "🛡️  Squash must refuse when another branch forks off the range."
v switch main
v switch og-base
file og.txt 'x\n';       v save "og s1"
file og.txt 'x\ny\n';    v save "og s2"
v switch og-child          # fork off og s2
file childfile.txt 'child\n'; v save "child work"
v switch og-base
file og.txt 'x\ny\nz\n';  v save "og s3"
vfail squash 3 "would orphan og-child"

# =============================================================================
act "ACT 13 · Inspection — history, show, blame, grep, diff"
v switch main
MAIN_TIP="$(H)"
v history --oneline
v history --all
v history --graph
v history --limit 3
v history --branch feature/login
v history --file app.py
has "$OUT" "Initial project skeleton" "history --file lists only snapshots touching app.py"
v show "$H_MERGE"
has "$OUT" "Merge hotfix" "show displays the snapshot"
v show "$H_MERGE" -- app.py
v show v1.0
has "$OUT" "Merge hotfix" "show resolves a tag"
v blame app.py
has "$OUT" "main" "blame annotates lines with a snapshot"
v blame app.py --at "$H_MERGE"
v grep "def"
has "$OUT" "def" "grep finds matches in the working tree"
v grep "DEF" -i
has "$OUT" "def" "grep -i is case-insensitive"
v grep "def" -l
v grep "def" -C 1
v grep "def" --snapshot "$H1"
say "one 'velo diff' covers every comparison:"
v diff "$H1" "$MAIN_TIP"
has "$OUT" "app.py" "diff <a> <b> shows changes between two snapshots"
v diff "$H1..$MAIN_TIP"
has "$OUT" "app.py" "diff <a>..<b> range syntax works too"
v diff "$H1" "$MAIN_TIP" -- config.py
file app.py 'def main():\n    print("Task Manager v1")\n    print("Hello!")\n    # working edit\n    return 0\n'
v diff "$MAIN_TIP"
has "$OUT" "working edit" "diff <a> compares a snapshot to the working tree"
v diff
has "$OUT" "working edit" "bare diff shows uncommitted changes"
v diff app.py
has "$OUT" "working edit" "a lone filename is treated as a file, not a ref"
v diff -- app.py
has "$OUT" "working edit" "-- forces path interpretation"
v restore "$MAIN_TIP" --force
clean "restored clean after diff demo"

# =============================================================================
act "ACT 14 · Restore a single file (leaving the rest alone)"
BEFORE="$(cat config.py)"
v restore "$H1" -- config.py
file_has config.py "DEBUG = False" "config.py reverted to the very first version"
status_has "config.py" "only config.py shows as changed"
[ "$(H)" = "$MAIN_TIP" ] && good "PARENT unchanged by single-file restore" || bad "PARENT moved unexpectedly"
v restore "$MAIN_TIP" --force -- config.py
file_is config.py "$BEFORE" "config.py restored to current version"
clean "clean again"

# =============================================================================
act "ACT 15 · Maintenance — tags & branches & gc"
v tag v2.0 "$H1"
v tag
has "$OUT" "v2.0" "tag at a specific snapshot created"
v tag --delete v1.0
OUT="$("$VELO" tag 2>&1)"; hasnt "$OUT" "v1.0" "tag deleted"
v tag v2.0 --force
good "tag --force overwrote existing tag"
v branches --delete feature/api
OUT="$("$VELO" branches 2>&1)"; hasnt "$OUT" "feature/api" "branch soft-deleted"
v gc
case "$OUT" in *"GC complete"*|*"clean"*) good "gc ran cleanly";; *) bad "gc output unexpected";; esac

# =============================================================================
act "ACT 16 · Solo-project integrity check"
v switch main
clean "final working tree is clean"
COUNT="$("$VELO" history --oneline --limit 999 2>&1 | grep -cE '[0-9a-f]{12}')"
[ "$COUNT" -ge 8 ] && good "main has a rich linear+merge history ($COUNT commits)" || bad "history too short ($COUNT)"
v status
has "$OUT" "main" "status reports the current branch"

# The whole simulated repo must pass integrity verification.
v fsck
has "$OUT" "healthy" "velo fsck reports the repository is healthy"

# =============================================================================
act "ACT 17 · Clone (a teammate joins the project)"
who "👨 Bob clones the whole TaskManager history from Alice's repo."
PROJECT_URL="$(native_path "$SANDBOX/taskmanager")"
cd "$SANDBOX" || exit 1
v clone "$PROJECT_URL" bob-clone
has "$OUT" "Cloned" "clone reported success"
repo "$SANDBOX/bob-clone" "bob-clone"
clean "a fresh clone starts with a clean working tree"
v history --all --oneline --limit 999
has "$OUT" "Initial project skeleton" "clone carried the full history, back to the first commit"
v branches
has "$OUT" "feature/login" "clone carried the branches"
v tag
has "$OUT" "v2.0" "clone carried the tags"
v remote
has "$OUT" "origin" "clone configured the 'origin' remote"
v fsck
has "$OUT" "healthy" "the clone passes integrity verification"

# =============================================================================
act "ACT 18 · Push · fetch · pull (the everyday loop)"
say "A dedicated origin keeps the next few acts easy to follow."
mkdir -p "$SANDBOX/origin"
repo "$SANDBOX/origin" "origin"
v init
file config.yml 'version: 1\nname: taskmanager\nowner: team\n'
v save "Origin: initial config"
ORIGIN_URL="$(native_path "$SANDBOX/origin")"

cd "$SANDBOX" || exit 1
v clone "$ORIGIN_URL" alice
v clone "$ORIGIN_URL" bob

repo "$SANDBOX/alice" "alice"
who "👩 Alice commits and publishes."
file feature_a.txt 'alice feature A\n'
v save "Alice: add feature A"
v status
has "$OUT" "1 ahead of origin/main" "status reports Alice is 1 ahead"
v push
has "$OUT" "Pushed" "push succeeded (fast-forward)"
v status
has "$OUT" "up to date with origin/main" "status reports up to date after push"

repo "$SANDBOX/bob" "bob"
who "👨 Bob fetches — this must NOT touch his working tree."
v fetch
has "$OUT" "Fetched" "fetch reported success"
absent feature_a.txt "fetch left Bob's working tree untouched"
v status
has "$OUT" "1 behind origin/main" "status reports Bob is 1 behind"
v show origin/main
has "$OUT" "Alice: add feature A" "origin/main resolves as a ref"
who "👨 Bob pulls to catch up."
v pull
has "$OUT" "Fast-forwarded" "pull fast-forwarded cleanly"
exists feature_a.txt "Bob now has Alice's file"
v status
has "$OUT" "up to date with origin/main" "Bob is up to date"

# =============================================================================
act "ACT 19 · Divergence — rejected push, then reconcile"
repo "$SANDBOX/alice" "alice"
file shared.txt 'alpha\nbravo\ncharlie\ndelta\necho\n'
v save "Alice: add shared.txt"
v push
has "$OUT" "Pushed" "Alice publishes shared.txt"

repo "$SANDBOX/bob" "bob"
who "👨 Bob commits without pulling first — origin has moved under him."
file bob_only.txt 'bob work\n'
v save "Bob: add bob_only.txt"
vfail push
has "$OUT" "non-fast-forward" "push refused rather than overwriting Alice's work"
who "👨 Bob pulls: Velo reports divergence instead of guessing."
v pull
has "$OUT" "diverged" "pull reported divergence"
v status
has "$OUT" "diverged" "status confirms the divergence"
who "👨 Bob reconciles with an ordinary merge."
v merge origin/main
has "$OUT" "Conflicts: 0" "non-overlapping work auto-merged"
v save "Bob: merge origin/main"
exists shared.txt  "Bob picked up Alice's file"
exists bob_only.txt "Bob kept his own file"
v push
has "$OUT" "Pushed" "push now succeeds (fast-forward)"
BOB_TIP="$(H)"

repo "$SANDBOX/alice" "alice"
v pull
has "$OUT" "Fast-forward" "Alice fast-forwards onto Bob's merge"
exists bob_only.txt "Alice now has Bob's file"
[ "$(H)" = "$BOB_TIP" ] && good "Alice and Bob converged on the same snapshot" \
  || bad "tips diverged: alice=$(H) bob=$BOB_TIP"

# =============================================================================
act "ACT 20 · Streaming transport (the SSH code path)"
say "child: spawns 'velo serve-*' as a subprocess — the same protocol ssh uses,"
say "minus the network hop. Exercises the real client↔server pack exchange."
CHILD_URL="child:$(native_path "$SANDBOX/origin")"
cd "$SANDBOX" || exit 1
v clone "$CHILD_URL" carol
has "$OUT" "Cloned" "clone over the streaming protocol"
repo "$SANDBOX/carol" "carol"
clean "streamed clone has a clean tree"
exists shared.txt "streamed clone carried the history"
who "👩 Carol pushes over the streaming protocol."
file carol.txt 'carol work\n'
v save "Carol: add carol.txt"
v push
has "$OUT" "Pushed" "push over the streaming protocol"
v fsck
has "$OUT" "healthy" "Carol's repo is healthy"

repo "$SANDBOX/alice" "alice"
who "👩 Alice (on the plain-path remote) picks up Carol's streamed commit."
v pull
has "$OUT" "Fast-forward" "commit pushed over streaming is pulled over a path"
exists carol.txt "Alice has Carol's file"

# =============================================================================
act "ACT 21 · Offline transfer with bundles"
repo "$SANDBOX/alice" "alice"
BUNDLE_FILE="$SANDBOX/project.velo"
BUNDLE_URL="$(native_path "$BUNDLE_FILE")"
v bundle create "$BUNDLE_URL"
has "$OUT" "Bundled" "bundle created"
exists "$BUNDLE_FILE" "bundle file written to disk"

say "Import it into a brand-new, network-less repository:"
mkdir -p "$SANDBOX/airgap"
repo "$SANDBOX/airgap" "airgap"
v init
v bundle apply "$BUNDLE_URL"
has "$OUT" "Imported" "bundle imported"
v history --all --oneline
has "$OUT" "Alice: add feature A" "history transferred with no network"
has "$OUT" "Carol: add carol.txt" "every contributor's work came along"
v fsck
has "$OUT" "healthy" "air-gapped repo passes integrity verification"
say "Re-applying the same bundle must change nothing:"
v bundle apply "$BUNDLE_URL"
has "$OUT" "up to date" "bundle apply is idempotent"

say "A ref-scoped bundle is still self-contained:"
repo "$SANDBOX/alice" "alice"
v bundle create "$(native_path "$SANDBOX/scoped.velo")" main
has "$OUT" "Bundled" "ref-scoped bundle created"

# =============================================================================
act "ACT 22 · Final integrity sweep across every repository"
repo "$SANDBOX/origin" "origin"
v history --all --oneline
has "$OUT" "Carol: add carol.txt" "origin received every contributor's work"

for r in taskmanager bob-clone origin alice bob carol airgap; do
  repo "$SANDBOX/$r" "$r"
  v fsck
  has "$OUT" "healthy" "$r passes fsck"
done

# =============================================================================
printf "\n${GRN}${BOLD}════════════════════════════════════════════════════════════════════${RST}\n"
printf "${GRN}${BOLD}  ✓ ALL %d CHECKS PASSED — solo workflow AND collaboration are flawless.${RST}\n" "$PASS"
printf "${GRN}${BOLD}════════════════════════════════════════════════════════════════════${RST}\n"
