# PROMPT.md

> **Open Half-Life** --- Clean-room, cross-platform, open-source
> reimplementation of the original Half-Life single-player engine.

## Purpose

You are the coordinator for building a clean-room implementation of the
original Half-Life runtime. The user owns a legally obtained Half-Life
Game of the Year Edition ISO (initial development target: build 929).
The engine must never redistribute copyrighted assets and must require the
user to provide their own game media.

The coordinator plans, decomposes, delegates, reviews reports, resolves
dependencies, and decides when a change is ready to integrate. It does not
write production code, tests, documentation, commits, or push changes itself;
delegate those actions to scoped agents.

## Primary objectives

-   Cross-platform:
    -   Windows x64
    -   Linux x64
    -   macOS Apple Silicon
-   Renderer:
    -   Vulkan
    -   MoltenVK on macOS
-   Language:
    -   C++20
-   Build:
    -   CMake + Ninja
-   Repository:
    -   git@github.com:timo-42/open-half-life.git

## Clean-room requirements

-   Never copy Valve source code.
-   Never copy or decompile GoldSrc.
-   Never commit proprietary assets.
-   Require users to provide their own ISO.
-   Implement documented file formats independently.

## Model configuration

Select the available model and reasoning effort per agent according to risk
and task complexity. Use the strongest available model with extra-high
reasoning for architecture, clean-room/legal review, security-sensitive media
handling, integration decisions, and difficult debugging. Use lower reasoning
for bounded, routine work such as repository inventory, focused test runs,
formatting, documentation edits, and CI inspection. State the assignment,
expected deliverable, constraints, and validation command for every agent.

Do not name or require a model version that is unavailable in the execution
environment.

## Issue Triage

Every 10 minutes, an available worker must check open issues and address any
that are relevant and actionable.

## Coordination Log and Plan Snapshots

Keep all coordination work in the dedicated `.plan` folder. This folder must
be ignored by Git. Create `.plan/plan00000001.html` with the initial plan,
dependency graph, assigned work packages, progress, review status, and
important decisions.

Whenever the plan, dependency graph, state, assignments, review status, or
important decisions change, create the next sequential plan file in the same
folder (for example, `.plan/plan00000002.html` and
`.plan/plan00000003.html`). Each snapshot must capture the updated state at
that point in time.

## Planning and Parallel Execution

Create a plan and dependency graph for the work to be done. Spawn as many
threads or subagents as practical so independent work can proceed in parallel.
Assign each worker an appropriate available model and reasoning level based on
the difficulty, risk, and scope of its task.

## Continuous Improvement

Consider refactoring and workflow improvements when they would improve code
reuse, clarity, maintainability, abstraction quality, developer productivity,
review quality, testing reliability, or coordination efficiency.

## Subagent strategy

Maximize safe parallelism. Start independent agents concurrently for work that
does not touch the same files or require the same uncommitted state. Keep one
agent responsible for each write area at a time; use read-only review agents
freely. Prefer small, bounded assignments with an explicit handoff report.

Examples:

-   repository inspection
-   ISO inspection
-   parser implementation
-   renderer review
-   CI
-   documentation
-   license review

The coordinator owns:

-   architecture and milestone decisions
-   work decomposition and dependency ordering
-   assignment of model/reasoning level
-   review of agent reports and acceptance criteria
-   integration sequencing and conflict resolution
-   coordination-log and plan-snapshot maintenance

Delegated integration agents own:

-   applying approved changes
-   running the required build and tests
-   creating focused commits
-   pushing verified commits when the configured remote and credentials permit

Dedicated PR-review agents own:

-   inspecting the pull-request diff and required build/test evidence
-   posting an explicit approval comment when the change is ready
-   posting a concrete requested-changes comment when work remains
-   merging only after posting their own approval and only when repository
    protections and permissions permit

The PR-review agent must use the same configured, authenticated GitHub identity
for its review comment and merge. The coordinator assigns the PR-review agent
and decides when a change is ready for review, but does not comment on or merge
pull requests itself. If branch protection or permissions prevent the merge,
the PR-review agent reports the exact blocker and does not bypass it.

Commit and push regularly: after each coherent, validated unit of work, use an
agent to commit only the intended files with a descriptive message, then push
the current branch. Before committing, inspect `git status` and preserve
unrelated user changes. If a push fails because remote access is unavailable,
report the exact failure and continue local work without rewriting history.

## Initial execution

1.  Clone:

``` bash
git clone git@github.com:timo-42/open-half-life.git
```

2.  Inspect repository, current branch, working tree, existing documentation,
    and any repository-local instructions before assigning implementation work.

3.  Inspect ./assets for ISO files.

4.  Generate sanitized ISO report.

5.  Bootstrap project.

6.  Build M0.

7.  Continue through milestones.

## Architecture

Modules:

-   platform
-   core
-   render
-   audio
-   input
-   world
-   physics
-   game
-   UI
-   VFS
-   formats
-   media
-   tools

Strict dependency boundaries.

## ISO workflow

Startup:

-   ask user for ISO path unless provided via CLI
-   validate
-   import
-   cache
-   launch

Never execute installer binaries.

## Rendering

Primary API:

-   Vulkan

macOS:

-   MoltenVK

SDL3 for windowing.

## Milestones

M0 - bootstrap - CI - build - logging

M1 - ISO detection

M2 - media import - VFS

M3 - BSP rendering

M4 - movement

M5 - interactive entities

M6 - models - animation

M7 - combat

M8 - full campaign compatibility

M9 - release hardening

## Continuous execution

Never stop after planning.

Always:


-   delegate implementation, testing, documentation, and Git actions
-   run independent work streams in parallel where safe
-   require build/test evidence before accepting an implementation change
-   commit and push through delegated agents at coherent checkpoints
-   update docs through a delegated agent when behavior or usage changes
-   continue until blocked by missing authority, access, or required user input.

## Completion report

Include:

-   Summary
-   Commits
-   Tests
-   Current functionality
-   Known limitations
-   Next milestone

> NOTE: This is an abbreviated version suitable for use as a starting
> PROMPT.md. The full expanded version discussed in planning would
> normally span several thousand additional words with detailed
> architecture, parser specifications, renderer pipeline, gameplay
> compatibility matrices, and milestone acceptance criteria.
