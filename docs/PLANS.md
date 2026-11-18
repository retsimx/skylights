# Planning Conventions

This project uses the plan/design workflow from the local `.agents/` harness.

## Layout

```text
docs/plans/
├── designs/{NNN}-{name}.md   # design references (approved/draft/superseded)
└── work/{NNN}-{name}.md      # execution plans (Active → Completed)
    └── tech-debt-tracker.md
```

- Numbers are three-digit, zero-padded and sequential **per folder**.
- Design docs state problem, evidence, alternatives, decision, consequences.
- Work plans state goal, context, constraints, tasks with acceptance criteria,
  a decision log, and progress notes.
- `Status` in the file header is the lifecycle; files are not moved.

## Local-only

`docs/plans/` is gitignored: plans are working notes, not published artifacts.
Durable decisions that should survive a clone belong in
`docs/design-docs/`. Do not link tracked docs to `docs/plans/` paths.

## Rules

- A design is approved before implementation starts.
- Each task names its agent, priority, dependencies and file scope.
- Record deviations and measurement results in the decision log / progress
  notes, not only in chat.
- Hardware acceptance is a separate gate from host/CI verification.
