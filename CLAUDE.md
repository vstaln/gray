<!-- BEGIN SLATE (managed) -->
## Slate

You are running inside Slate, an infinite canvas the user is watching live.
The `slate` command is already on your PATH and already authenticated — it talks
to the running app directly. There is no MCP server to configure.

**The other agents.** Every terminal on the canvas is addressable by its visible
name, and you can act on any of them:

```sh
slate whoami                                   # your own agent id, terminal & task
slate workers                                  # who else is open; * marks you
slate rename --to term-3 --name backend        # give one a name that means something
slate tell backend "run the tests and report"  # type into its terminal
slate worker-read backend                       # read its ordinary terminal answer
```

Names beat ids: rename a sibling once, then address it by name everywhere.
`slate tell` does not create inbox mail. After a quick question sent with `tell`,
read the answer with `slate worker-read <name>`. Use `slate check` only for messages
sent through runs/tasks (`worker_done`, `ask`, `escalation`, and similar).

**Good orchestration.** Before dispatching, turn the objective into a small, bounded
task graph. Every task spec should state its goal, owned files or responsibility,
acceptance criteria, verification command, and stop condition. Add dependencies
only for real blockers; dispatch independent ready tasks in parallel, with no two
workers owning the same files. The coordinator waits for reports, inspects the
diff and test evidence, then releases or retains each dispatch — never treating
a started process or a `tell` message as proof of completion.

Use this compact task-spec shape when creating work:

```text
Goal: one concrete outcome
Scope: files or responsibility owned by this worker
Acceptance: observable conditions that must be true
Verify: exact test/check to run
Stop when: the acceptance criteria are met or a blocker is reported
```

Keep credentials, tokens, and private environment values out of task specs,
mail, and reports. Use only agents currently available in the worker menu; do
not invent a model or silently substitute an unavailable route.

**Coordinating with other agents.** For work you intend to *wait on*, use runs,
tasks and dispatches rather than `tell` — that is what gives you a completion
report instead of a guess.

```sh
slate status                                   # what is running right now
slate run-create --objective "..."             # open a run; check mail from this same terminal
slate task-create --spec "..." [--deps '["otask-1"]']
slate task-list --ready                        # what can be dispatched now
slate task-show <id>                           # view full specification and status
slate worker-start --task <id> --agent opencode  # opens a terminal and briefs it
slate check --wait --types worker_done,escalation,ask,permission   # block until a worker reports
slate reply <askId> "..."                      # unblock a worker that asked
slate ask --type permission --question "..."     # request safety approval and wait
slate allow <permission-id> [--note "..."]      # approve a permission request
slate deny <permission-id> [--reason "..."]     # reject a permission request
slate gates                                    # check open decision gates
slate worker-release <dispatchId>              # account for a finished worker
```

If *you* were dispatched, your preamble named your task and dispatch ids. Report
exactly once when you finish, success or failure — a coordinator is blocked on it:

```sh
slate done --outcome succeeded --task-id <t> --dispatch-id <d> --body "what changed"
slate ask --question "..."     # blocks until the coordinator answers
slate escalate --body "..."    # you are stuck and need intervention
```

**Planner.** The day planner is the workspace task list.
Use it to track work and report progress:

```sh
slate plan list                                # see all planner tasks
slate plan create|update|toggle|delete [<id>]   # manage planner tasks
```

**The rest of the app** is the same CLI: `slate canvas`, `slate plan`,
`slate terminal`, `slate git`, `slate journal`. Add `--json` for parseable
output. Prefer putting results on the canvas or a task over loose files —
the user is looking at the canvas, not at your scrollback.
<!-- END SLATE (managed) -->
