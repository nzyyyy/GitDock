# GitDock Domain Glossary

## Working Tree

The checked-out repository state that GitDock reads and safely mutates. Its module owns Snapshot, Diff, and Conflict coordination, including stale-view validation.

## Snapshot

A backend-owned view of one Working Tree at a point in time. Snapshot IDs scope cached Diff hunks and Conflict documents to their repository and HEAD.

## Diff

A staged or unstaged file change derived from a Snapshot. Backend-owned hunk IDs prevent callers from submitting arbitrary patches.

## Conflict

A three-stage text merge state opened from a Snapshot. Resolution revalidates HEAD, index stages, file content, and file mode before replacing and staging the file.
