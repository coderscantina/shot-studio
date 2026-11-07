# Git worktrees for parallel agents

## Hook

- One checkout, three agents, three broken builds

## Setup

- Create a worktree per task branch
- Share one build cache outside the worktrees

## Cleanup

- Merge the task branches and remove the worktrees
